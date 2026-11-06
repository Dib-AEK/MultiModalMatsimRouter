//! Refines geographic bisections using service frequency and walking connectivity, preserving balance.
use crate::model::Timetable;
pub(crate) struct NetworkPartition {
    edges: Vec<Vec<(usize, u64)>>,
    membership: Vec<u8>,
}
impl NetworkPartition {
    pub fn new(t: &Timetable) -> Self {
        let mut edges = vec![Vec::new(); t.stops.len()];
        for c in &t.connections {
            edges[c.from as usize].push((c.to as usize, 1));
            edges[c.to as usize].push((c.from as usize, 1));
        }
        for from in 0..t.stops.len() {
            for e in t.walking_from(from as u32) {
                edges[from].push((e.to as usize, 32));
                edges[e.to as usize].push((from, 32));
            }
        }
        for row in &mut edges {
            row.sort_unstable_by_key(|e| e.0);
            let mut out = Vec::<(usize, u64)>::new();
            for &(to, w) in row.iter() {
                if let Some(last) = out.last_mut()
                    && last.0 == to
                {
                    last.1 += w;
                } else {
                    out.push((to, w));
                }
            }
            *row = out;
        }
        Self {
            membership: vec![2; t.stops.len()],
            edges,
        }
    }
    pub fn refine(
        &mut self,
        ids: &mut [usize],
        split: usize,
        left_cells: usize,
        right_cells: usize,
    ) -> usize {
        for (i, &id) in ids.iter().enumerate() {
            self.membership[id] = u8::from(i >= split);
        }
        let tolerance = (ids.len() / 10).max(left_cells);
        let min = split.saturating_sub(tolerance).max(left_cells);
        let max = (split + tolerance).min(ids.len() - right_cells);
        let mut left = split;
        for _ in 0..4 {
            let mut changed = false;
            for &id in ids.iter() {
                let side = self.membership[id];
                let mut own = 0u64;
                let mut other = 0u64;
                for &(to, w) in &self.edges[id] {
                    let s = self.membership[to];
                    if s == side {
                        own += w
                    } else if s < 2 {
                        other += w
                    }
                }
                if other > own && ((side == 0 && left > min) || (side == 1 && left < max)) {
                    self.membership[id] = 1 - side;
                    if side == 0 {
                        left -= 1
                    } else {
                        left += 1
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        ids.sort_unstable_by_key(|&id| (self.membership[id], id));
        for &id in ids.iter() {
            self.membership[id] = 2;
        }
        left
    }
}
