//! Compares CSA against independent event-reachability relaxation on small generated timetables.
use multimodal_matsim_router::{
    config::Preprocessing,
    model::*,
    preprocess,
    routing::{Leg, Query, Workspace, route},
    time::{NEVER, Time},
};
fn trip(id: usize, stops: &[(u32, u32, u32, bool, bool)]) -> RawTrip {
    RawTrip {
        trip: Trip {
            id: id.to_string(),
            line: "line".into(),
            route: "route".into(),
            departure: id.to_string(),
            mode: "bus".into(),
        },
        calls: stops
            .iter()
            .map(|&(stop, arrival, departure, pickup, dropoff)| Call {
                stop,
                arrival,
                departure,
                pickup,
                dropoff,
            })
            .collect(),
    }
}
fn raw(n: usize, trips: Vec<RawTrip>, footpaths: Vec<(u32, u32, u32)>) -> RawTimetable {
    RawTimetable {
        coordinates: Coordinates::ProjectedMeters,
        stops: (0..n)
            .map(|i| Stop {
                id: i.to_string(),
                name: i.to_string(),
                x: i as f64 * 1000.0,
                y: 0.0,
                transfer_buffer: 0,
            })
            .collect(),
        trips,
        footpaths,
    }
}
fn prepare(raw: RawTimetable, cells: usize) -> Timetable {
    preprocess::prepare(
        raw,
        &Preprocessing {
            walking_radius_m: 0.0,
            reachability_cells: cells,
            ..Default::default()
        },
    )
    .unwrap()
}
fn query(from: u32, to: u32, departure: Time, max_boardings: usize) -> Query {
    Query {
        from,
        to,
        departure,
        max_boardings,
        max_duration: 100000,
        return_pareto: false,
    }
}

/// Independent fixed-point event graph: explicit boarding, alighting, staying aboard and walking.
fn reference(t: &Timetable, q: &Query) -> Option<(Time, usize)> {
    let n = t.stops.len();
    let count = t.connections.len();
    let mut off = vec![NEVER; n * (q.max_boardings + 1)];
    let mut on = vec![false; count * (q.max_boardings + 1)];
    off[q.from as usize] = q.departure;
    let horizon = q.departure.saturating_add(q.max_duration).min(NEVER - 1);
    loop {
        let mut changed = false;
        for k in 0..=q.max_boardings {
            for from in 0..n {
                for edge in t.walking_from(from as u32) {
                    let next = off[k * n + from].saturating_add(edge.duration);
                    let index = k * n + edge.to as usize;
                    if next < off[index] && next <= horizon {
                        off[index] = next;
                        changed = true;
                    }
                }
            }
        }
        for (index, c) in t.connections.iter().enumerate() {
            for k in 1..=q.max_boardings {
                let buffer = if k == 1 {
                    0
                } else {
                    t.stops[c.from as usize].transfer_buffer
                };
                if c.pickup
                    && off[(k - 1) * n + c.from as usize] < NEVER
                    && off[(k - 1) * n + c.from as usize].saturating_add(buffer) <= c.departure
                    && c.arrival <= horizon
                    && !on[k * count + index]
                {
                    on[k * count + index] = true;
                    changed = true;
                }
                if on[k * count + index] {
                    if c.dropoff && c.arrival < off[k * n + c.to as usize] {
                        off[k * n + c.to as usize] = c.arrival;
                        changed = true;
                    }
                    if let Some(next) =
                        ((index + 1)..count).find(|&j| t.connections[j].trip == c.trip)
                        && !on[k * count + next]
                        && t.connections[next].arrival <= horizon
                    {
                        on[k * count + next] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    (0..=q.max_boardings)
        .filter_map(|k| {
            let a = off[k * n + q.to as usize];
            (a < NEVER).then_some((a, k))
        })
        .min()
}

#[test]
fn generated_networks_match_independent_solver_and_reuse_workspaces() {
    let mut seed = 123456_u64;
    let mut random = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed >> 32
    };
    let mut workspace = Workspace::default();
    for case in 0..80 {
        let n = 3 + (random() % 4) as usize;
        let mut trips = Vec::new();
        let mut walks = Vec::new();
        for id in 0..8 {
            let mut time = (random() % 30) as u32;
            let mut calls = Vec::new();
            for _ in 0..4 {
                time += (random() % 4) as u32;
                let arrival = time;
                time += (random() % 3) as u32;
                calls.push((
                    (random() % n as u64) as u32,
                    arrival,
                    time,
                    random() % 5 != 0,
                    random() % 5 != 0,
                ));
            }
            trips.push(trip(id, &calls));
        }
        for a in 0..n {
            for b in 0..n {
                if a != b && random() % 5 == 0 {
                    walks.push((a as u32, b as u32, (random() % 7) as u32));
                }
            }
        }
        let mut input = raw(n, trips, walks);
        for stop in &mut input.stops {
            stop.transfer_buffer = (random() % 4) as u32;
        }
        let table = prepare(input, if case % 2 == 0 { 2 } else { 0 });
        for from in 0..n {
            for to in 0..n {
                for boardings in 1..=3 {
                    let mut q = query(from as u32, to as u32, (random() % 30) as u32, boardings);
                    q.return_pareto = case % 2 == 0;
                    let expected = reference(&table, &q);
                    let found = route(&table, &q, &mut workspace).unwrap();
                    assert_eq!(
                        expected,
                        found.as_ref().map(|j| (j.arrival, j.boardings)),
                        "case={case}, query={q:?}"
                    );
                    if let Some(j) = found {
                        assert_eq!(
                            j.boardings,
                            j.legs
                                .iter()
                                .filter(|l| matches!(l, Leg::Transit { .. }))
                                .count()
                        );
                        let mut stop = q.from;
                        let mut time = q.departure;
                        for leg in &j.legs {
                            let (from, to, departure, arrival) = match leg {
                                Leg::Walk {
                                    from,
                                    to,
                                    departure,
                                    arrival,
                                }
                                | Leg::Transit {
                                    from,
                                    to,
                                    departure,
                                    arrival,
                                    ..
                                } => (*from, *to, *departure, *arrival),
                            };
                            assert_eq!(from, stop);
                            assert!(departure >= time && arrival >= departure);
                            stop = to;
                            time = arrival;
                        }
                        assert_eq!(stop, q.to);
                        assert_eq!(time, j.arrival);
                    }
                }
            }
        }
    }
}
#[test]
fn same_trip_continuation_does_not_charge_transfer_buffer_or_another_boarding() {
    let mut input = raw(
        3,
        vec![trip(
            0,
            &[
                (0, 0, 0, true, true),
                (1, 10, 10, false, false),
                (2, 20, 20, true, true),
            ],
        )],
        vec![],
    );
    input.stops[1].transfer_buffer = 1000;
    let table = prepare(input, 0);
    let j = route(&table, &query(0, 2, 0, 1), &mut Workspace::default())
        .unwrap()
        .unwrap();
    assert_eq!((j.arrival, j.boardings, j.legs.len()), (20, 1, 1));
}
#[test]
fn zero_duration_connections_cannot_ride_backwards_along_a_trip() {
    let table = prepare(
        raw(
            3,
            vec![trip(
                0,
                &[
                    (0, 0, 0, true, true),
                    (1, 0, 0, true, true),
                    (2, 1, 1, true, true),
                ],
            )],
            vec![],
        ),
        0,
    );
    assert!(
        route(&table, &query(1, 0, 0, 3), &mut Workspace::default())
            .unwrap()
            .is_none()
    );
}
#[test]
fn invalid_query_and_invalid_timetable_are_errors() {
    let table = prepare(raw(2, vec![], vec![]), 0);
    assert!(route(&table, &query(5, 1, 0, 2), &mut Workspace::default()).is_err());
    let bad = raw(
        2,
        vec![trip(
            0,
            &[(0, 100, 100, true, true), (1, 90, 90, true, true)],
        )],
        vec![],
    );
    assert!(preprocess::prepare(bad, &Preprocessing::default()).is_err());
}
#[test]
fn horizon_and_missing_departures_return_no_route() {
    let table = prepare(
        raw(
            2,
            vec![trip(
                0,
                &[(0, 100, 100, true, true), (1, 200, 200, true, true)],
            )],
            vec![],
        ),
        0,
    );
    let mut q = query(0, 1, 0, 2);
    q.max_duration = 150;
    assert!(
        route(&table, &q, &mut Workspace::default())
            .unwrap()
            .is_none()
    );
    assert!(
        route(&table, &query(0, 1, 101, 2), &mut Workspace::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn full_pareto_keeps_later_direct_services_while_fast_mode_stops_early() {
    let table = prepare(
        raw(
            3,
            vec![
                trip(0, &[(0, 0, 0, true, true), (1, 10, 10, true, true)]),
                trip(1, &[(1, 10, 10, true, true), (2, 20, 20, true, true)]),
                trip(2, &[(0, 30, 30, true, true), (2, 50, 50, true, true)]),
            ],
            vec![(0, 2, 100)],
        ),
        0,
    );
    let mut q = query(0, 2, 0, 3);
    let fast = route(&table, &q, &mut Workspace::default())
        .unwrap()
        .unwrap();
    assert_eq!((fast.arrival, fast.boardings), (20, 2));
    assert!(fast.pareto_arrivals.is_empty());
    q.return_pareto = true;
    let complete = route(&table, &q, &mut Workspace::default())
        .unwrap()
        .unwrap();
    assert_eq!(complete.pareto_arrivals, vec![(0, 100), (1, 50), (2, 20)]);
    assert!(fast.statistics.connections_scanned < complete.statistics.connections_scanned);
}
