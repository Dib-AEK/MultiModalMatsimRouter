//! Prepares sorted physical transfers and an optimistic reverse cost graph shared by weighted searches.
use crate::{cost::SearchOptions, model::*, time::Time};
use std::{
    cell::Cell,
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
    pub local: crate::local_search::LocalIndex,
    pub regional: Option<crate::regional_bounds::RegionalBounds>,
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
    memo: Vec<Cell<(u32, f64)>>,
    generation: u32,
    values: Vec<f64>,
    settled: Vec<bool>,
    touched: Vec<usize>,
    limit: f64,
    regional_targets: Vec<(usize, f64)>,
    heap: BinaryHeap<QueueEntry>,
}
impl BoundWorkspace {
    fn reset(&mut self, n: usize) {
        self.memo.resize_with(n, || Cell::new((0, 0.)));
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.memo.iter().for_each(|v| v.set((0, 0.)));
            self.generation = 1;
        }
        for i in self.touched.drain(..) {
            self.values[i] = f64::INFINITY;
            self.settled[i] = false;
        }
        self.values.resize(n, f64::INFINITY);
        self.settled.resize(n, false);
        self.heap.clear();
        self.regional_targets.clear();
        self.limit = f64::INFINITY;
    }
    pub fn disable(&mut self, n: usize) {
        self.reset(n);
        self.limit = 0.0;
    }
    pub fn use_regional(
        &mut self,
        n: usize,
        regional: &crate::regional_bounds::RegionalBounds,
        targets: &[crate::weighted::Endpoint],
        walk: f64,
    ) {
        debug_assert_eq!(self.values.len(), n);
        for e in targets {
            let cell = regional.cell(e.stop);
            let cost = down(e.duration as f64 * walk);
            if let Some((_, previous)) =
                self.regional_targets.iter_mut().find(|(id, _)| *id == cell)
            {
                *previous = previous.min(cost);
            } else {
                self.regional_targets.push((cell, cost));
            }
        }
    }
    pub fn transfer_suffix(
        &self,
        regional: &crate::regional_bounds::RegionalBounds,
        stop: usize,
        position: usize,
        mode: Option<usize>,
    ) -> f64 {
        regional.suffix(stop, position, &self.regional_targets)
            + regional.change_suffix(stop, position, mode)
    }
    pub fn get(&self, stop: StopId) -> f64 {
        if self.settled[stop as usize] {
            self.values[stop as usize]
        } else {
            self.limit
        }
    }
    pub fn get_regional(
        &self,
        stop: StopId,
        regional: Option<&crate::regional_bounds::RegionalBounds>,
    ) -> f64 {
        if self.settled[stop as usize] {
            return self.values[stop as usize];
        }
        let cached = self.memo[stop as usize].get();
        if cached.0 == self.generation {
            return cached.1;
        }
        let value = regional.map_or(self.limit, |r| {
            self.limit.max(r.lower(stop, &self.regional_targets))
        });
        self.memo[stop as usize].set((self.generation, value));
        value
    }
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
        let mut result = Self {
            regional: None,
            local: crate::local_search::LocalIndex::new(t),
            transfer_offsets,
            transfers,
            reverse_offsets,
            reverse,
            boarding_offsets,
            boarding_times,
            minimum_changes,
            modes: rates.len(),
        };
        if options.destination_bounds && options.regional_bound_cells > 0 {
            result.regional = Some(crate::regional_bounds::RegionalBounds::build(
                t,
                &result,
                options.regional_bound_cells,
                options.network_partition,
                options.transfer_blocks,
                walk,
            ));
        }
        result
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
    pub fn mode_count(&self) -> usize {
        self.modes
    }
    pub fn minimum_change(&self, stop: StopId, from: usize) -> f64 {
        if self.minimum_changes.is_empty() {
            return 0.0;
        }
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
        sources: &[crate::weighted::Endpoint],
        local: Option<usize>,
        w: &mut BoundWorkspace,
    ) -> usize {
        w.reset(n);
        let mut cutoff = None;
        let mut source_ids: Vec<_> = sources.iter().map(|s| s.stop).collect();
        source_ids.sort_unstable();
        source_ids.dedup();
        for e in targets {
            let cost = down(e.duration as f64 * walk);
            if cost < w.values[e.stop as usize] {
                if !w.values[e.stop as usize].is_finite() {
                    w.touched.push(e.stop as usize);
                }
                w.values[e.stop as usize] = cost;
                w.heap.push(QueueEntry { cost, stop: e.stop });
            }
        }
        let mut settled = 0;
        while let Some(QueueEntry { cost, stop }) = w.heap.pop() {
            if cost != w.values[stop as usize] {
                continue;
            }
            if cutoff.is_some_and(|limit| cost > limit) {
                // Every unsettled vertex is at least this far from the destination.
                // Capping its bound weakens pruning but cannot remove a real optimum.
                w.limit = cutoff.unwrap();
                break;
            }
            w.settled[stop as usize] = true;
            settled += 1;
            if local.is_some_and(|budget| budget > 0 && settled >= budget) {
                // Dijkstra's unsettled distances cannot be below the last settled cost.
                // Stop only the heuristic computation; the actual route search remains unrestricted.
                w.limit = cost;
                break;
            }
            if local.is_some() && cutoff.is_none() && source_ids.binary_search(&stop).is_ok() {
                cutoff = Some(cost * 4.0);
            }
            for e in &self.reverse
                [self.reverse_offsets[stop as usize]..self.reverse_offsets[stop as usize + 1]]
            {
                let next = cost.max(down(cost + e.cost));
                if next < w.values[e.to as usize] {
                    if !w.values[e.to as usize].is_finite() {
                        w.touched.push(e.to as usize);
                    }
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
            allocated_bytes: self.regional.as_ref().map_or(0, |r| r.allocated_bytes())
                + self.local.allocated_bytes()
                + self.transfer_offsets.capacity() * size_of::<usize>()
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
