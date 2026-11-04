//! Keeps a bounded advisory FIFO of query fingerprints and execution costs, never routing results.
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
};

pub(crate) struct WorkHistory {
    capacity: usize,
    costs: HashMap<u64, u64>,
    insertion_order: VecDeque<u64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct HistorySummary {
    pub entries: usize,
    pub capacity: usize,
}
pub(crate) fn fingerprints<Q: Hash>(queries: &[Q]) -> Vec<u64> {
    queries
        .iter()
        .map(|q| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            q.hash(&mut hash);
            hash.finish()
        })
        .collect()
}
impl WorkHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            costs: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }
    pub fn summary(&self) -> HistorySummary {
        HistorySummary {
            entries: self.costs.len(),
            capacity: self.capacity,
        }
    }
    pub fn snapshot(&self, keys: &[u64]) -> (Vec<u64>, usize) {
        let hints: Vec<_> = keys
            .iter()
            .map(|key| self.costs.get(key).copied())
            .collect();
        let hits = hints.iter().flatten().count();
        let unknown = hints.iter().flatten().copied().max().unwrap_or(0);
        (
            hints
                .into_iter()
                .map(|cost| cost.unwrap_or(unknown))
                .collect(),
            hits,
        )
    }
    pub fn observe(&mut self, keys: &[u64], costs: &[u64]) {
        if self.capacity == 0 {
            return;
        }
        for (&key, &cost) in keys.iter().zip(costs) {
            if let Some(previous) = self.costs.get_mut(&key) {
                *previous = ((3 * u128::from(*previous) + u128::from(cost)) / 4) as u64;
                continue;
            }
            while self.costs.len() >= self.capacity {
                if let Some(old) = self.insertion_order.pop_front() {
                    self.costs.remove(&old);
                } else {
                    self.costs.clear();
                }
            }
            self.costs.insert(key, cost);
            self.insertion_order.push_back(key);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_hints_are_advisory_and_handle_extreme_costs() {
        let mut history = WorkHistory::new(2);
        history.observe(&[1, 2], &[u64::MAX, 10]);
        history.observe(&[1], &[u64::MAX]);
        assert_eq!(history.snapshot(&[1, 2]).0, vec![u64::MAX, 10]);
        history.observe(&[3], &[20]);
        assert_eq!(history.summary().entries, 2);
        let (hints, hits) = history.snapshot(&[1, 2, 3]);
        assert_eq!(hits, 2);
        assert_eq!(hints, vec![20, 10, 20]);
        let mut disabled = WorkHistory::new(0);
        disabled.observe(&[1], &[5]);
        assert_eq!(disabled.summary().entries, 0);
    }
}
