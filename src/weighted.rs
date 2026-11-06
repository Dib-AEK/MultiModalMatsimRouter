//! Searches time/cost frontiers by boarding round; weighted costs never reuse arrival-only dominance.
use crate::{
    cost::CostModel,
    model::*,
    routing::{Journey, Leg, Statistics},
    time::{NEVER, Time},
};
use crate::{
    local_search::ConnectionScan,
    sparse_rows::{Row, SparseRows},
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
    /// True only when a coordinate-routing work budget stopped the selected search early.
    #[serde(default, skip_serializing_if = "is_false")]
    pub search_truncated: bool,
    #[serde(flatten)]
    pub journey: Journey,
    pub access: Endpoint,
    pub egress: Endpoint,
}
fn is_false(value: &bool) -> bool {
    !*value
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
    previous: SparseRows<Label>,
    next: SparseRows<Label>,
    ready: SparseRows<Label>,
    rides: SparseRows<Ride>,
    arena: Vec<Path>,
    targets: SparseRows<(usize, Time, u32)>,
    scan: ConnectionScan,
    temporal: crate::temporal_reachability::Workspace,
    history: SparseRows<History>,
    bounds: crate::search_index::BoundWorkspace,
}
impl Workspace {
    fn reset(&mut self, n: usize, trips: usize) {
        self.previous.reset(n);
        self.next.reset(n);
        self.ready.reset(n);
        self.rides.reset(trips);
        self.arena.clear();
        self.history.reset(n);
        self.targets.reset(n);
        self.scan.reset(trips);
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
fn insert(frontier: &mut Row<Label>, label: Label, m: &CostModel) -> bool {
    if frontier.iter().any(|a| dominates(a, &label, m)) {
        return false;
    }
    insert_non_dominated(frontier, label, m);
    true
}
/// Callers already checked dominance before allocating a reconstruction node.
fn insert_non_dominated(frontier: &mut Row<Label>, label: Label, m: &CostModel) {
    debug_assert!(!frontier.iter().any(|a| dominates(a, &label, m)));
    frontier.retain(|b| !dominates(&label, b, m));
    frontier.push(label);
}
/// Bounds are rounded down; a small relative guard also protects comparisons across cost-sum orders.
fn exceeds_bound(cost: f64, incumbent: f64) -> bool {
    cost > incumbent + 1e-10 * (1.0 + incumbent.abs())
}
/// A boarding cannot improve the incumbent once even its elapsed-time lower bound is too costly.
/// Round outwards and retain two extra milliseconds to protect division and time conversion.
fn departure_limit(
    time: Time,
    cost: f64,
    best: Option<f64>,
    rate: f64,
    horizon: Time,
) -> Option<Time> {
    let Some(best) = best else {
        return Some(horizon);
    };
    let remaining = best + 1e-10 * (1.0 + best.abs()) - cost;
    if remaining < 0.0 {
        return None;
    }
    if rate == 0.0 {
        return Some(horizon);
    }
    let extra = (remaining / rate).ceil() + 2.0;
    Some(
        time.saturating_add(extra.min(horizon as f64) as Time)
            .min(horizon),
    )
}
fn cannot_finish(
    cost: f64,
    stop: StopId,
    best: Option<f64>,
    bounds: &crate::search_index::BoundWorkspace,
    regional: Option<&crate::regional_bounds::RegionalBounds>,
) -> bool {
    let lower = bounds.get_regional(stop, regional);
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
    Ok(route_with_budget(t, q, m, w, 0)?.journey)
}
/// An incomplete search without a journey is unresolved, never proof that no path exists.
pub struct SearchOutcome {
    pub journey: Option<ResultJourney>,
    pub budget_reached: bool,
}
/// Work units count scanned connections and considered transfers. Zero keeps the exact search.
pub fn route_with_budget(
    t: &Timetable,
    q: &Query,
    m: &CostModel,
    w: &mut Workspace,
    budget: usize,
) -> Result<SearchOutcome> {
    let mut reached = false;
    let mut journey = search(t, q, m, w, budget, &mut reached)?;
    if let Some(result) = &mut journey {
        result.search_truncated = reached;
    }
    Ok(SearchOutcome {
        journey,
        budget_reached: reached,
    })
}
fn search(
    t: &Timetable,
    q: &Query,
    m: &CostModel,
    w: &mut Workspace,
    budget: usize,
    reached: &mut bool,
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
    if !q.access.iter().any(|a| {
        q.egress
            .iter()
            .any(|e| t.components[a.stop as usize] == t.components[e.stop as usize])
    }) {
        return Ok(None);
    }
    if q.access.is_empty() || q.egress.is_empty() {
        return Ok(None);
    }
    let horizon = q.departure.saturating_add(q.max_duration).min(NEVER - 1);
    w.reset(t.stops.len(), t.trips.len());
    let mut stats = Statistics::default();
    if m.search.destination_bounds {
        if m.index.regional.is_none() || m.search.local_bounds {
            stats.bound_settled_stops = m.index.bounds(
                t.stops.len(),
                m.walk,
                &q.egress,
                &q.access,
                m.search
                    .local_bounds
                    .then_some(m.search.local_bound_settlements),
                &mut w.bounds,
            );
        } else {
            w.bounds.disable(t.stops.len());
        }
        if let Some(regional) = &m.index.regional {
            w.bounds
                .use_regional(t.stops.len(), regional, &q.egress, m.walk);
        }
    } else {
        w.bounds.disable(t.stops.len());
    }
    // Raptor can finish via one final transfer walk; it is egress, not another boarding.
    for (i, e) in q.egress.iter().enumerate() {
        w.targets[e.stop as usize].push((i, 0, 0));
        for edge in m.index.local.incoming(e.stop as usize) {
            w.targets[edge.from].push((i, edge.duration, edge.order));
        }
    }
    // Preserve the reference scan's direct/footpath/endpoint order for deterministic ties.
    for position in 0..w.targets.active.len() {
        let stop = w.targets.active[position];
        w.targets[stop].sort_by_key(|&(i, _, order)| (order, i));
    }
    for (i, e) in q.access.iter().enumerate() {
        let time = q.departure.saturating_add(e.duration);
        if time <= horizon
            && w.bounds
                .get_regional(e.stop, m.index.regional.as_ref())
                .is_finite()
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
    let mut checked_temporal = false;
    'rounds: for round in 1..=q.max_boardings {
        stats.boarding_rounds = round;
        w.ready.clear();
        w.next.clear();
        w.rides.clear();
        // A transfer buffer consumes walking time/cost in SwissRailRaptor, even at the same stop.
        for position in 0..w.previous.active.len() {
            let stop = w.previous.active[position];
            for j in 0..w.previous[stop].len() {
                let label = w.previous[stop][j];
                if round == 1 {
                    insert(&mut w.ready[stop], label, m);
                    continue;
                }
                let transfers = m.index.transfers(stop);
                for (position, edge) in transfers.iter().enumerate() {
                    if budget > 0
                        && stats
                            .connections_scanned
                            .saturating_add(stats.transfers_considered)
                            >= budget
                    {
                        *reached = true;
                        break 'rounds;
                    }

                    if m.search.transfer_blocks
                        && position % 16 == 0
                        && let (Some(incumbent), Some(regional)) = (&best, &m.index.regional)
                        && exceeds_bound(
                            label.cost
                                + w.bounds.transfer_suffix(
                                    regional,
                                    stop,
                                    position,
                                    if m.dynamic { None } else { Some(label.mode) },
                                ),
                            incumbent.0,
                        )
                    {
                        stats.transfers_pruned += transfers.len() - position;
                        break;
                    }
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
                    if cannot_finish(
                        cost,
                        to,
                        best.as_ref().map(|b| b.0),
                        &w.bounds,
                        m.index.regional.as_ref(),
                    ) {
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
                        if cannot_finish(
                            lower,
                            to,
                            best.as_ref().map(|b| b.0),
                            &w.bounds,
                            m.index.regional.as_ref(),
                        ) {
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
                    insert_non_dominated(&mut w.ready[to as usize], ready, m);
                }
            }
        }
        w.scan.reset(t.trips.len());
        if m.search.local_connections {
            for position in 0..w.ready.active.len() {
                let stop = w.ready.active[position];
                if let Some(earliest) = w.ready[stop].iter().map(|label| label.time).min() {
                    // Do not populate the heap with all later services in the two-day horizon.
                    // Include every label: later, cheaper alternatives can board later services.
                    let latest = w.ready[stop]
                        .iter()
                        .filter_map(|label| {
                            departure_limit(
                                label.time,
                                label.cost,
                                best.as_ref().map(|b| b.0),
                                m.min_time_rate,
                                horizon,
                            )
                        })
                        .max();
                    let Some(latest) = latest else {
                        continue;
                    };
                    for &id in m.index.local.boardings(t, stop, earliest, latest) {
                        w.scan
                            .activate(t.connections[id as usize].trip as usize, id);
                    }
                }
            }
            w.scan.start();
        } else {
            w.scan.start_dense(start);
        }
        let mut any = false;
        while let Some(id) = w.scan.next(&m.index.local, t.connections.len()) {
            if budget > 0
                && stats
                    .connections_scanned
                    .saturating_add(stats.transfers_considered)
                    >= budget
            {
                *reached = true;
                break 'rounds;
            }
            let c = &t.connections[id];
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
            // Difficult unreachable requests otherwise build large cost frontiers before radius expansion.
            // Pay for one relaxed temporal scan only after substantial work without any incumbent.
            if m.search.temporal_pruning
                && !checked_temporal
                && best.is_none()
                && stats.labels_improved >= 2048
            {
                checked_temporal = true;
                if !w.temporal.may_reach(t, &m.index.local, q) {
                    return Ok(None);
                }
            }
            stats.connections_scanned += 1;
            if c.arrival > horizon {
                continue;
            }
            if !w
                .bounds
                .get_regional(c.to, m.index.regional.as_ref())
                .is_finite()
            {
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
                    &w.bounds,
                    m.index.regional.as_ref(),
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
                    if cannot_finish(
                        cost,
                        c.to,
                        best.as_ref().map(|b| b.0),
                        &w.bounds,
                        m.index.regional.as_ref(),
                    ) {
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
                insert_non_dominated(&mut w.next[c.to as usize], label, m);

                any = true;
                stats.labels_improved += 1;
                for j in 0..w.targets[c.to as usize].len() {
                    let (i, walking, _) = w.targets[c.to as usize][j];
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
        if !any {
            break;
        }
        if m.search.cross_round_dominance {
            // Only completed earlier rounds may dominate the next round. Never remove a history
            // entry using more boardings: its smaller boarding budget remains relevant.
            for position in 0..w.next.active.len() {
                let stop = w.next.active[position];
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
        search_truncated: false,
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

#[cfg(test)]
mod activation_tests {
    use super::*;
    #[test]
    fn boarding_limits_are_outward_rounded_and_keep_zero_rate_and_large_horizon_cases() {
        assert_eq!(departure_limit(10, 2., None, 1., 1000), Some(1000));
        assert_eq!(departure_limit(10, 2., Some(12.), 1., 1000), Some(23));
        assert_eq!(departure_limit(10, 2., Some(12.), 0., 1000), Some(1000));
        assert_eq!(departure_limit(10, 20., Some(12.), 1., 1000), None);
        assert_eq!(departure_limit(10, 2., Some(12.), 1e-300, 1000), Some(1000));
        assert_eq!(departure_limit(995, 2., Some(12.), 1., 1000), Some(1000));
        assert!(departure_limit(10, 12., Some(12.), 1., 1000).unwrap() >= 10);
    }
}
