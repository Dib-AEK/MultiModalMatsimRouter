//! Prepares sorted physical transfers and an optimistic reverse cost graph shared by weighted searches.
use crate::{cost::SearchOptions, model::*, time::Time};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap},
};

#[derive(Clone, Copy)]
pub(crate) struct Transfer {
    pub to: StopId,
    pub duration: Time,
}
#[derive(Clone, Copy)]
struct Edge {
    to: StopId,
    cost: f64,
}
pub(crate) struct SearchIndex {
    transfer_offsets: Vec<usize>,
    transfers: Vec<Transfer>,
    reverse_offsets: Vec<usize>,
    reverse: Vec<Edge>,
    boarding_offsets: Vec<usize>,
    boarding_times: Vec<Time>,
    minimum_changes: Vec<f64>,
    modes: usize,
}
#[derive(Clone, Copy)]
struct QueueEntry {
    cost: f64,
    stop: StopId,
}
impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost && self.stop == other.stop
    }
}
impl Eq for QueueEntry {}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.stop.cmp(&self.stop))
    }
}
/// Reused request-local Dijkstra state; computation is included in query timing, never cached by destination.
#[derive(Default)]
pub(crate) struct BoundWorkspace {
    pub values: Vec<f64>,
    heap: BinaryHeap<QueueEntry>,
}
/// Round each nonnegative relaxed operation down, retaining an optimistic floating-point estimate.
fn down(value: f64) -> f64 {
    if value > 0.0 { value.next_down() } else { 0.0 }
}
impl SearchIndex {
    pub fn new(
        t: &Timetable,
        walk: f64,
        trip_modes: &[usize],
        rates: &[f64],
        change_costs: &[f64],
        options: &SearchOptions,
    ) -> Self {
        let n = t.stops.len();
        // Factor station alternatives by physical platform and mode, never materializing
        // route-stop pairs. Boarding events retain actual departure times and pickup rules.
        let mut times = vec![Vec::new(); n];
        let mut modes = vec![Vec::new(); n];
        if options.boarding_index {
            for c in t.connections.iter().filter(|c| c.pickup) {
                times[c.from as usize].push(c.departure);
                modes[c.from as usize].push(trip_modes[c.trip as usize]);
            }
        }
        let mut boarding_offsets = Vec::with_capacity(n + 1);
        let mut boarding_times = Vec::new();
        let mut minimum_changes = if options.boarding_index {
            vec![f64::INFINITY; n * rates.len()]
        } else {
            Vec::new()
        };
        for stop in 0..n {
            boarding_offsets.push(boarding_times.len());
            times[stop].dedup(); // Connections were already sorted by departure time.
            boarding_times.extend_from_slice(&times[stop]);
            modes[stop].sort_unstable();
            modes[stop].dedup();
            if options.boarding_index {
                for from in 0..rates.len() {
                    minimum_changes[stop * rates.len() + from] = modes[stop]
                        .iter()
                        .map(|&to| change_costs[from * rates.len() + to])
                        .fold(f64::INFINITY, f64::min);
                }
            }
        }
        boarding_offsets.push(boarding_times.len());
        let mut transfers = Vec::with_capacity(n + t.walks.len());
        let mut transfer_offsets = Vec::with_capacity(n + 1);
        for (stop, s) in t.stops.iter().enumerate() {
            transfer_offsets.push(transfers.len());
            transfers.push(Transfer {
                to: stop as u32,
                duration: s.transfer_buffer,
            });
            transfers.extend(t.walking_from(stop as u32).iter().map(|e| Transfer {
                to: e.to,
                duration: e.duration,
            }));
            if options.sorted_transfers {
                transfers[transfer_offsets[stop]..].sort_by_key(|e| (e.duration, e.to));
            }
        }
        transfer_offsets.push(transfers.len());
        // Free waiting, free mode changes, no restrictions and minimum scheduled segment costs:
        // every real continuation has a path in this graph costing no more than that continuation.
        let mut edges = BTreeMap::<(StopId, StopId), f64>::new();
        if options.destination_bounds {
            let mut add = |from, to, cost: f64| {
                edges
                    .entry((to, from))
                    .and_modify(|c| *c = c.min(cost))
                    .or_insert(cost);
            };
            for c in &t.connections {
                add(
                    c.from,
                    c.to,
                    down((c.arrival - c.departure) as f64 * rates[trip_modes[c.trip as usize]]),
                );
            }
            for from in 0..n {
                for e in t.walking_from(from as u32) {
                    add(from as u32, e.to, down(e.duration as f64 * walk));
                }
            }
        }
        let mut reverse_offsets = vec![0; n + 1];
        for &(from, _) in edges.keys() {
            reverse_offsets[from as usize + 1] += 1;
        }
        for i in 0..n {
            reverse_offsets[i + 1] += reverse_offsets[i];
        }
        let reverse = edges
            .into_iter()
            .map(|((_, to), cost)| Edge { to, cost })
            .collect();
        Self {
            transfer_offsets,
            transfers,
            reverse_offsets,
            reverse,
            boarding_offsets,
            boarding_times,
            minimum_changes,
            modes: rates.len(),
        }
    }
    /// Earliest feasible departure only supplies a lower bound; later/express alternatives remain searchable.
    pub fn next_boarding(&self, stop: StopId, ready: Time, horizon: Time) -> Option<Time> {
        let times = &self.boarding_times
            [self.boarding_offsets[stop as usize]..self.boarding_offsets[stop as usize + 1]];
        times
            .get(times.partition_point(|&time| time < ready))
            .copied()
            .filter(|&time| time <= horizon)
    }
    pub fn minimum_change(&self, stop: StopId, from: usize) -> f64 {
        self.minimum_changes[stop as usize * self.modes + from]
    }
    pub fn transfers(&self, stop: usize) -> &[Transfer] {
        &self.transfers[self.transfer_offsets[stop]..self.transfer_offsets[stop + 1]]
    }
    /// Exact shortest paths in the relaxed graph. Multiple target candidates include their own egress costs.
    pub fn bounds(
        &self,
        n: usize,
        walk: f64,
        targets: &[crate::weighted::Endpoint],
        w: &mut BoundWorkspace,
    ) -> usize {
        w.values.resize(n, f64::INFINITY);
        w.values.fill(f64::INFINITY);
        w.heap.clear();
        for e in targets {
            let cost = down(e.duration as f64 * walk);
            if cost < w.values[e.stop as usize] {
                w.values[e.stop as usize] = cost;
                w.heap.push(QueueEntry { cost, stop: e.stop });
            }
        }
        let mut settled = 0;
        while let Some(QueueEntry { cost, stop }) = w.heap.pop() {
            if cost != w.values[stop as usize] {
                continue;
            }
            settled += 1;
            for e in &self.reverse
                [self.reverse_offsets[stop as usize]..self.reverse_offsets[stop as usize + 1]]
            {
                let next = cost.max(down(cost + e.cost));
                if next < w.values[e.to as usize] {
                    w.values[e.to as usize] = next;
                    w.heap.push(QueueEntry {
                        cost: next,
                        stop: e.to,
                    });
                }
            }
        }
        settled
    }
    pub fn summary(&self) -> IndexSummary {
        IndexSummary {
            relaxed_edges: self.reverse.len(),
            transfer_entries: self.transfers.len(),
            boarding_events: self.boarding_times.len(),
            allocated_bytes: self.transfer_offsets.capacity() * size_of::<usize>()
                + self.transfers.capacity() * size_of::<Transfer>()
                + self.reverse_offsets.capacity() * size_of::<usize>()
                + self.reverse.capacity() * size_of::<Edge>()
                + self.boarding_offsets.capacity() * size_of::<usize>()
                + self.boarding_times.capacity() * size_of::<Time>()
                + self.minimum_changes.capacity() * size_of::<f64>(),
        }
    }
}
/// Memory excludes the base timetable and thread-local labels/heaps, and is allocated only once per engine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexSummary {
    pub relaxed_edges: usize,
    pub transfer_entries: usize,
    pub boarding_events: usize,
    pub allocated_bytes: usize,
}
