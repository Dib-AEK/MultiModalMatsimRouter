//! Runs exact bounded-boarding connection scans with walking closure and reusable request-local labels.
use crate::{
    model::*,
    time::{NEVER, Time},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, collections::BinaryHeap};
const NO_PATH: u32 = u32::MAX;
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct Query {
    pub from: StopId,
    pub to: StopId,
    pub departure: Time,
    pub max_boardings: usize,
    pub max_duration: Time,
    pub return_pareto: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Leg {
    Walk {
        from: StopId,
        to: StopId,
        departure: Time,
        arrival: Time,
    },
    Transit {
        trip: TripId,
        from: StopId,
        to: StopId,
        departure: Time,
        arrival: Time,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generalized_cost: Option<f64>,
    pub departure: Time,
    pub arrival: Time,
    pub boardings: usize,
    pub legs: Vec<Leg>,
    pub pareto_arrivals: Vec<(usize, Time)>,
    pub statistics: Statistics,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Statistics {
    pub connections_scanned: usize,
    pub connections_pruned: usize,
    pub labels_improved: usize,
    pub boarding_rounds: usize,
    pub labels_pruned_by_history: usize,
    pub labels_pruned_by_bound: usize,
    pub transfers_considered: usize,
    pub transfers_pruned: usize,
    pub path_nodes: usize,
    pub bound_settled_stops: usize,
}
#[derive(Clone)]
struct PathNode {
    prior: u32,
    leg: Leg,
}

#[derive(Default)]
pub struct Workspace {
    highest_layer: usize,
    arrivals: Vec<Time>,
    paths: Vec<u32>,
    reached: Vec<u32>,
    boarding_connections: Vec<u32>,
    touched: Vec<usize>,
    touched_trips: Vec<usize>,
    arena: Vec<PathNode>,
    heap: BinaryHeap<Reverse<(Time, StopId, usize)>>,
}
impl Workspace {
    fn reset(&mut self, stops: usize, trips: usize, layers: usize) {
        for i in self.touched.drain(..) {
            self.arrivals[i] = NEVER;
            self.paths[i] = NO_PATH;
        }
        for i in self.touched_trips.drain(..) {
            self.reached[i] = NO_PATH;
        }
        self.arrivals.resize(stops * layers, NEVER);
        self.paths.resize(stops * layers, NO_PATH);
        self.reached.resize(trips * layers, NO_PATH);
        self.boarding_connections.resize(trips * layers, 0);
        self.highest_layer = 0;
        self.arena.clear();
        self.heap.clear();
    }
    fn append(&mut self, prior: u32, leg: Leg) -> Result<u32> {
        ensure!(
            self.arena.len() < NO_PATH as usize,
            "journey label arena exceeded u32 limit"
        );
        let index = self.arena.len() as u32;
        self.arena.push(PathNode { prior, leg });
        Ok(index)
    }
    /// An earlier/equal arrival with fewer boardings can make every future off-vehicle decision.
    fn dominated(&self, stops: usize, layer: usize, stop: StopId, time: Time) -> bool {
        (0..layer).any(|prior| self.arrivals[prior * stops + stop as usize] <= time)
    }
    fn improve(&mut self, index: usize, time: Time, path: u32, stats: &mut Statistics) {
        if self.arrivals[index] == NEVER {
            self.touched.push(index);
        }
        self.arrivals[index] = time;
        self.paths[index] = path;
        stats.labels_improved += 1;
    }
    fn walk(
        &mut self,
        t: &Timetable,
        layer: usize,
        seed: (StopId, Time, u32),
        horizon: Time,
        stats: &mut Statistics,
    ) -> Result<bool> {
        let (stop, time, path) = seed;
        let n = t.stops.len();
        let index = layer * n + stop as usize;
        if time >= self.arrivals[index] || time > horizon || self.dominated(n, layer, stop, time) {
            return Ok(false);
        }
        self.highest_layer = self.highest_layer.max(layer);
        self.improve(index, time, path, stats);
        self.heap.push(Reverse((time, stop, layer)));
        while let Some(Reverse((now, from, k))) = self.heap.pop() {
            let prior_index = k * n + from as usize;
            if now != self.arrivals[prior_index] {
                continue;
            }
            for edge in t.walking_from(from) {
                let next = now.saturating_add(edge.duration);
                let index = k * n + edge.to as usize;
                if next <= horizon
                    && next < self.arrivals[index]
                    && !self.dominated(n, k, edge.to, next)
                {
                    let path = self.append(
                        self.paths[prior_index],
                        Leg::Walk {
                            from,
                            to: edge.to,
                            departure: now,
                            arrival: next,
                        },
                    )?;
                    self.improve(index, next, path, stats);
                    self.heap.push(Reverse((next, edge.to, k)));
                }
            }
        }
        Ok(true)
    }
}

pub fn route(t: &Timetable, q: &Query, w: &mut Workspace) -> Result<Option<Journey>> {
    let n = t.stops.len();
    let trips = t.trips.len();
    ensure!(
        (q.from as usize) < n && (q.to as usize) < n,
        "query refers to unknown stop"
    );
    ensure!(
        (1..=64).contains(&q.max_boardings) && q.departure < NEVER && q.max_duration < NEVER,
        "invalid query bounds"
    );
    // Disconnected components and conservative destination flags prove impossibility before scanning.
    if t.components[q.from as usize] != t.components[q.to as usize] || !t.may_reach(q.from, q.to) {
        return Ok(None);
    }
    let horizon = q.departure.saturating_add(q.max_duration).min(NEVER - 1);
    w.reset(n, trips, q.max_boardings + 1);
    let mut stats = Statistics::default();
    w.walk(t, 0, (q.from, q.departure, NO_PATH), horizon, &mut stats)?;
    let mut cursor = t.connections.partition_point(|c| c.departure < q.departure);
    while cursor < t.connections.len() && t.connections[cursor].departure <= horizon {
        let departure = t.connections[cursor].departure;
        // Later departures cannot improve the incumbent arrival (including boarding-count ties).
        if !q.return_pareto
            && departure
                > (0..=q.max_boardings)
                    .map(|k| w.arrivals[k * n + q.to as usize])
                    .min()
                    .unwrap_or(NEVER)
        {
            break;
        }

        let end = cursor + t.connections[cursor..].partition_point(|c| c.departure == departure);
        let has_zero = t.connections[cursor..end]
            .iter()
            .any(|c| c.arrival == c.departure);
        loop {
            let mut changed = false;
            for (offset, c) in t.connections[cursor..end].iter().enumerate() {
                let connection_index = (cursor + offset) as u32;
                stats.connections_scanned += 1;
                if !t.may_reach(c.to, q.to) {
                    stats.connections_pruned += 1;
                    continue;
                }
                for k in 1..=q.max_boardings.min(w.highest_layer + 1) {
                    let trip_index = k * trips + c.trip as usize;
                    let source = (k - 1) * n + c.from as usize;
                    let buffer = if k == 1 {
                        0
                    } else {
                        t.stops[c.from as usize].transfer_buffer
                    };
                    if (w.reached[trip_index] == NO_PATH
                        || connection_index < w.boarding_connections[trip_index])
                        && c.pickup
                        && w.arrivals[source] < NEVER
                        && w.arrivals[source].saturating_add(buffer) <= c.departure
                        && !(1..k).any(|prior| {
                            w.reached[prior * trips + c.trip as usize] != NO_PATH
                                && w.boarding_connections[prior * trips + c.trip as usize]
                                    <= connection_index
                        })
                    {
                        let path = w.append(
                            w.paths[source],
                            Leg::Transit {
                                trip: c.trip,
                                from: c.from,
                                to: c.from,
                                departure: c.departure,
                                arrival: c.departure,
                            },
                        )?;
                        if w.reached[trip_index] == NO_PATH {
                            w.touched_trips.push(trip_index);
                        }
                        w.reached[trip_index] = path;
                        w.boarding_connections[trip_index] = connection_index;
                        changed = true;
                    }
                    let boarding = w.reached[trip_index];
                    if boarding != NO_PATH
                        && connection_index >= w.boarding_connections[trip_index]
                        && c.dropoff
                        && c.arrival <= horizon
                        && c.arrival < w.arrivals[k * n + c.to as usize]
                        && !w.dominated(n, k, c.to, c.arrival)
                    {
                        let seed = &w.arena[boarding as usize];
                        let (from, departure) = match seed.leg {
                            Leg::Transit {
                                from, departure, ..
                            } => (from, departure),
                            _ => unreachable!("trip boarding is transit"),
                        };
                        let path = w.append(
                            seed.prior,
                            Leg::Transit {
                                trip: c.trip,
                                from,
                                to: c.to,
                                departure,
                                arrival: c.arrival,
                            },
                        )?;
                        changed |= w.walk(t, k, (c.to, c.arrival, path), horizon, &mut stats)?;
                    }
                }
            }
            if !has_zero || !changed {
                break;
            }
        }
        cursor = end;
    }
    let mut pareto = Vec::new();
    let mut best = NEVER;
    for k in 0..=q.max_boardings {
        let arrival = w.arrivals[k * n + q.to as usize];
        if arrival < best {
            pareto.push((k, arrival));
            best = arrival;
        }
    }
    let Some(&(boardings, arrival)) = pareto.iter().min_by_key(|&&(k, a)| (a, k)) else {
        return Ok(None);
    };
    let mut cursor = w.paths[boardings * n + q.to as usize];
    let mut legs = Vec::new();
    while cursor != NO_PATH {
        let node = &w.arena[cursor as usize];
        legs.push(node.leg.clone());
        cursor = node.prior;
    }
    legs.reverse();
    Ok(Some(Journey {
        generalized_cost: None,
        departure: q.departure,
        arrival,
        boardings,
        legs,
        pareto_arrivals: if q.return_pareto { pareto } else { Vec::new() },
        statistics: stats,
    }))
}
