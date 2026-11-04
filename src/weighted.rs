//! Searches time/cost frontiers by boarding round; weighted costs never reuse arrival-only dominance.
use crate::{
    cost::CostModel,
    model::*,
    routing::{Journey, Leg, Statistics},
    time::{NEVER, Time},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Access or egress walking time to one candidate stop, excluding initial waiting.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub stop: StopId,
    pub duration: Time,
}
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct Query {
    pub departure: Time,
    pub max_boardings: usize,
    pub max_duration: Time,
    pub access: Vec<Endpoint>,
    pub egress: Vec<Endpoint>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultJourney {
    #[serde(flatten)]
    pub journey: Journey,
    pub access: Endpoint,
    pub egress: Endpoint,
}
#[derive(Clone, Copy)]
struct Label {
    time: Time,
    cost: f64, // Includes fixed penalties; dynamic penalties evaluated from first boarding.
    first: Time,
    mode: usize,
    path: Option<usize>,
    access: usize,
}
#[derive(Clone, Copy)]
struct Ride {
    label: Label,
    from: StopId,
    departure: Time,
    prior: Option<usize>,
}
struct Path {
    prior: Option<usize>,
    leg: Leg,
}
#[derive(Clone, Copy)]
struct History {
    label: Label,
    boardings: usize,
}
#[derive(Default)]
pub struct Workspace {
    previous: Vec<Vec<Label>>,
    next: Vec<Vec<Label>>,
    ready: Vec<Vec<Label>>,
    rides: Vec<Vec<Ride>>,
    arena: Vec<Path>,
    targets: Vec<Vec<(usize, Time)>>,
    history: Vec<Vec<History>>,
    bounds: crate::search_index::BoundWorkspace,
}
impl Workspace {
    fn reset(&mut self, n: usize, trips: usize) {
        for frontiers in [&mut self.previous, &mut self.next, &mut self.ready] {
            frontiers.resize_with(n, Vec::new);
            for f in frontiers.iter_mut() {
                f.clear();
            }
        }
        self.rides.resize_with(trips, Vec::new);
        for f in &mut self.rides {
            f.clear();
        }
        self.arena.clear();
        self.history.resize_with(n, Vec::new);
        for f in &mut self.history {
            f.clear();
        }
        self.targets.resize_with(n, Vec::new);
        for f in &mut self.targets {
            f.clear();
        }
    }
    fn path(&mut self, prior: Option<usize>, leg: Leg) -> usize {
        let index = self.arena.len();
        self.arena.push(Path { prior, leg });
        index
    }
}
/// Sufficient dominance accounts for waiting AND boarding while a vehicle dwells.
/// Keeping separate last modes preserves every mode-to-mode transfer decision.
fn dominates(a: &Label, b: &Label, m: &CostModel) -> bool {
    a.mode == b.mode
        && a.time <= b.time
        && (!m.dynamic || a.first >= b.first)
        && a.cost + m.max_time_rate * (b.time - a.time) as f64 <= b.cost
}
fn insert(frontier: &mut Vec<Label>, label: Label, m: &CostModel) -> bool {
    if frontier.iter().any(|a| dominates(a, &label, m)) {
        return false;
    }
    frontier.retain(|b| !dominates(&label, b, m));
    frontier.push(label);
    true
}
/// Bounds are rounded down; a small relative guard also protects comparisons across cost-sum orders.
fn exceeds_bound(cost: f64, incumbent: f64) -> bool {
    cost > incumbent + 1e-10 * (1.0 + incumbent.abs())
}
fn cannot_finish(cost: f64, stop: StopId, best: Option<f64>, bounds: &[f64]) -> bool {
    let lower = bounds[stop as usize];
    !lower.is_finite() || best.is_some_and(|b| exceeds_bound(cost + lower, b))
}

/// Exact bounded-boarding additive disutility search over the prepared transfer graph.
/// Transfers are one direct footpath (or same-stop buffer), not arbitrary walking chains.
pub fn route(
    t: &Timetable,
    q: &Query,
    m: &CostModel,
    w: &mut Workspace,
) -> Result<Option<ResultJourney>> {
    ensure!(
        m.trip_modes.len() == t.trips.len(),
        "cost model belongs to another timetable"
    );
    ensure!(
        (1..=64).contains(&q.max_boardings) && q.departure < NEVER && q.max_duration < NEVER,
        "invalid weighted query bounds"
    );
    for e in q.access.iter().chain(&q.egress) {
        ensure!(
            (e.stop as usize) < t.stops.len() && e.duration < NEVER,
            "invalid endpoint"
        );
    }
    if q.access.is_empty() || q.egress.is_empty() {
        return Ok(None);
    }
    let horizon = q.departure.saturating_add(q.max_duration).min(NEVER - 1);
    w.reset(t.stops.len(), t.trips.len());
    let mut stats = Statistics::default();
    if m.search.destination_bounds {
        stats.bound_settled_stops = m
            .index
            .bounds(t.stops.len(), m.walk, &q.egress, &mut w.bounds);
    } else {
        w.bounds.values.resize(t.stops.len(), 0.0);
        w.bounds.values.fill(0.0);
    }
    // Raptor can finish via one final transfer walk; it is egress, not another boarding.
    let mut direct = vec![Vec::new(); t.stops.len()];
    for (i, e) in q.egress.iter().enumerate() {
        direct[e.stop as usize].push(i);
    }
    for (stop, es) in direct.iter().enumerate() {
        w.targets[stop].extend(es.iter().map(|&i| (i, 0)));
        for edge in t.walking_from(stop as u32) {
            w.targets[stop].extend(direct[edge.to as usize].iter().map(|&i| (i, edge.duration)));
        }
    }
    for (i, e) in q.access.iter().enumerate() {
        let time = q.departure.saturating_add(e.duration);
        if time <= horizon
            && w.bounds.values[e.stop as usize].is_finite()
            && (!m.search.boarding_index || m.index.next_boarding(e.stop, time, horizon).is_some())
        {
            insert(
                &mut w.previous[e.stop as usize],
                Label {
                    time,
                    cost: e.duration as f64 * m.walk,
                    first: 0,
                    mode: usize::MAX,
                    path: None,
                    access: i,
                },
                m,
            );
        }
    }
    let start = t.connections.partition_point(|c| c.departure < q.departure);
    let mut best: Option<(f64, Time, usize, Label, usize)> = None;
    for round in 1..=q.max_boardings {
        stats.boarding_rounds = round;
        for f in &mut w.ready {
            f.clear();
        }
        for f in &mut w.next {
            f.clear();
        }
        for f in &mut w.rides {
            f.clear();
        }
        // A transfer buffer consumes walking time/cost in SwissRailRaptor, even at the same stop.
        for stop in 0..t.stops.len() {
            for j in 0..w.previous[stop].len() {
                let label = w.previous[stop][j];
                if round == 1 {
                    insert(&mut w.ready[stop], label, m);
                    continue;
                }
                let transfers = m.index.transfers(stop);
                for (position, edge) in transfers.iter().enumerate() {
                    let (to, duration) = (edge.to, edge.duration);
                    stats.transfers_considered += 1;
                    let time = label.time.saturating_add(duration);
                    let cost = label.cost + duration as f64 * m.walk;
                    if time > horizon || best.as_ref().is_some_and(|b| exceeds_bound(cost, b.0)) {
                        if m.search.sorted_transfers {
                            stats.transfers_pruned += transfers.len() - position;
                            break;
                        }
                        stats.transfers_pruned += 1;
                        continue;
                    }
                    if cannot_finish(cost, to, best.as_ref().map(|b| b.0), &w.bounds.values) {
                        stats.transfers_pruned += 1;
                        continue;
                    }
                    if m.search.boarding_index {
                        let Some(departure) = m.index.next_boarding(to, time, horizon) else {
                            stats.transfers_pruned += 1;
                            continue;
                        };
                        // Boarding can start during dwell. A minimum time rate, not waiting rate,
                        // safely bounds all cost until departure. Dynamic penalties remain deferred.
                        let change = if m.dynamic {
                            0.0
                        } else {
                            m.index.minimum_change(to, label.mode)
                        };
                        let lower = cost + (departure - time) as f64 * m.min_time_rate + change;
                        if cannot_finish(lower, to, best.as_ref().map(|b| b.0), &w.bounds.values) {
                            stats.transfers_pruned += 1;
                            continue;
                        }
                    }
                    let mut ready = label;
                    ready.time = time;
                    ready.cost = cost;
                    // Allocate reconstruction only after the candidate survives dominance.
                    if w.ready[to as usize].iter().any(|a| dominates(a, &ready, m)) {
                        continue;
                    }
                    ready.path = Some(w.path(
                        label.path,
                        Leg::Walk {
                            from: stop as u32,
                            to,
                            departure: label.time,
                            arrival: time,
                        },
                    ));
                    insert(&mut w.ready[to as usize], ready, m);
                }
            }
        }
        let mut any = false;
        for c in &t.connections[start..] {
            if c.departure > horizon {
                break;
            }
            // Every elapsed millisecond has at least this nonnegative cost, irrespective of mode.
            if best
                .as_ref()
                .is_some_and(|b| m.min_time_rate * (c.departure - q.departure) as f64 > b.0)
            {
                break;
            }
            stats.connections_scanned += 1;
            if c.arrival > horizon {
                continue;
            }
            if !w.bounds.values[c.to as usize].is_finite() {
                stats.connections_pruned += 1;
                w.rides[c.trip as usize].clear();
                continue;
            }
            let mode = m.trip_modes[c.trip as usize];
            let rate = m.rates[mode];
            let rides = &mut w.rides[c.trip as usize];
            // Advance existing riders across this connection, charging intervening dwell as in-vehicle time.
            for ride in rides.iter_mut() {
                ride.label.cost += (c.arrival - ride.label.time) as f64 * rate;
                ride.label.time = c.arrival;
            }
            let before = rides.len();
            rides.retain(|r| {
                !cannot_finish(
                    r.label.cost,
                    c.to,
                    best.as_ref().map(|b| b.0),
                    &w.bounds.values,
                )
            });
            stats.labels_pruned_by_bound += before - rides.len();
            if c.pickup {
                for label in &w.ready[c.from as usize] {
                    if label.time > c.departure {
                        continue;
                    }
                    let boarding = label.time.max(c.boarding_arrival);
                    let transfer = if round == 1 || m.dynamic {
                        0.0
                    } else {
                        m.transfer(label.mode, mode)
                    };
                    let cost = label.cost
                        + (boarding - label.time) as f64 * m.wait
                        + (c.arrival - boarding) as f64 * rate
                        + transfer;
                    if cannot_finish(cost, c.to, best.as_ref().map(|b| b.0), &w.bounds.values) {
                        stats.labels_pruned_by_bound += 1;
                        continue;
                    }
                    let first = if m.dynamic && round == 1 {
                        boarding
                    } else {
                        label.first
                    };
                    let ride = Ride {
                        label: Label {
                            time: c.arrival,
                            cost,
                            first,
                            mode,
                            path: None,
                            access: label.access,
                        },
                        from: c.from,
                        departure: c.departure,
                        prior: label.path,
                    };
                    if rides
                        .iter()
                        .any(|r| (!m.dynamic || r.label.first >= first) && r.label.cost <= cost)
                    {
                        continue;
                    }
                    rides.retain(|r| (m.dynamic && first < r.label.first) || cost > r.label.cost);
                    rides.push(ride);
                }
            }
            if !c.dropoff {
                continue;
            }
            for j in 0..w.rides[c.trip as usize].len() {
                let ride = w.rides[c.trip as usize][j];
                let mut label = ride.label;
                if best.as_ref().is_some_and(|b| label.cost > b.0) {
                    continue;
                }
                if m.search.cross_round_dominance
                    && w.history[c.to as usize]
                        .iter()
                        .any(|a| a.boardings < round && dominates(&a.label, &label, m))
                {
                    stats.labels_pruned_by_history += 1;
                    continue;
                }
                // No arrival-only pruning: a later, cheaper path can still be optimal.
                if w.next[c.to as usize]
                    .iter()
                    .any(|a| dominates(a, &label, m))
                {
                    continue;
                }
                label.path = Some(w.path(
                    ride.prior,
                    Leg::Transit {
                        trip: c.trip,
                        from: ride.from,
                        to: c.to,
                        departure: ride.departure,
                        arrival: c.arrival,
                    },
                ));
                if insert(&mut w.next[c.to as usize], label, m) {
                    any = true;
                    stats.labels_improved += 1;
                    for j in 0..w.targets[c.to as usize].len() {
                        let (i, walking) = w.targets[c.to as usize][j];
                        let e = &q.egress[i];
                        let arrival = c.arrival.saturating_add(walking).saturating_add(e.duration);
                        let cost = label.cost
                            + (walking as f64 + e.duration as f64) * m.walk
                            + m.total_dynamic_penalty(round - 1, label.first, c.arrival);
                        if arrival <= horizon
                            && best.as_ref().is_none_or(|b| {
                                cost < b.0 || (cost == b.0 && (round, arrival) < (b.2, b.1))
                            })
                        {
                            let mut destination = label;
                            if walking > 0 || e.stop != c.to {
                                destination.path = Some(w.path(
                                    label.path,
                                    Leg::Walk {
                                        from: c.to,
                                        to: e.stop,
                                        departure: c.arrival,
                                        arrival: c.arrival.saturating_add(walking),
                                    },
                                ));
                            }
                            best = Some((cost, arrival, round, destination, i));
                        }
                    }
                }
            }
        }
        if !any {
            break;
        }
        if m.search.cross_round_dominance {
            // Only completed earlier rounds may dominate the next round. Never remove a history
            // entry using more boardings: its smaller boarding budget remains relevant.
            for stop in 0..t.stops.len() {
                for label in &w.next[stop] {
                    let history = &mut w.history[stop];
                    history.retain(|b| b.boardings < round || !dominates(label, &b.label, m));
                    history.push(History {
                        label: *label,
                        boardings: round,
                    });
                }
            }
        }
        std::mem::swap(&mut w.previous, &mut w.next);
    }
    let Some((cost, arrival, boardings, label, egress)) = best else {
        return Ok(None);
    };
    let mut legs = Vec::new();
    let mut cursor = label.path;
    while let Some(i) = cursor {
        legs.push(w.arena[i].leg.clone());
        cursor = w.arena[i].prior;
    }
    legs.reverse();
    stats.path_nodes = w.arena.len();
    Ok(Some(ResultJourney {
        journey: Journey {
            departure: q.departure,
            arrival,
            boardings,
            legs,
            pareto_arrivals: Vec::new(),
            statistics: stats,
            generalized_cost: Some(cost),
        },
        access: q.access[label.access].clone(),
        egress: q.egress[egress].clone(),
    }))
}
