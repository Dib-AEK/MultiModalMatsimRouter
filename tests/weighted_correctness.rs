//! Checks weighted search against exhaustive journey enumeration, including mode memory and dwell.
use multimodal_matsim_router::{
    config::Preprocessing,
    cost::{CostModel, Costs, ModeTransfer, SearchOptions},
    model::*,
    preprocess,
    weighted::{self, Endpoint, Query, Workspace},
};

type Calls = Vec<(u32, u32, u32)>;
fn table(trips: Vec<(&str, Calls)>, paths: Vec<(u32, u32, u32)>) -> Timetable {
    preprocess::prepare(
        RawTimetable {
            coordinates: Coordinates::ProjectedMeters,
            stops: (0..4)
                .map(|i| Stop {
                    id: i.to_string(),
                    name: String::new(),
                    x: i as f64 * 1000.0,
                    y: 0.0,
                    transfer_buffer: 5000,
                })
                .collect(),
            trips: trips
                .into_iter()
                .enumerate()
                .map(|(i, (mode, calls))| RawTrip {
                    trip: Trip {
                        id: i.to_string(),
                        line: i.to_string(),
                        route: i.to_string(),
                        departure: i.to_string(),
                        mode: mode.into(),
                    },
                    calls: calls
                        .into_iter()
                        .map(|(stop, a, d)| Call {
                            stop,
                            arrival: a * 1000,
                            departure: d * 1000,
                            pickup: true,
                            dropoff: true,
                        })
                        .collect(),
                })
                .collect(),
            footpaths: paths,
        },
        &Preprocessing {
            walking_radius_m: 0.0,
            ..Default::default()
        },
    )
    .unwrap()
}
fn query(boardings: usize) -> Query {
    Query {
        departure: 0,
        max_boardings: boardings,
        max_duration: 1_000_000,
        access: vec![Endpoint {
            stop: 0,
            duration: 0,
        }],
        egress: vec![Endpoint {
            stop: 3,
            duration: 0,
        }],
    }
}
fn model() -> Costs {
    Costs {
        walk_per_hour: 7.2,
        wait_per_hour: 3.6,
        other_per_hour: 3.6,
        modes_per_hour: [("rail".into(), 1.8)].into(),
        transfer_base: 0.1,
        mode_transfers: vec![ModeTransfer {
            from: "bus".into(),
            to: "rail".into(),
            cost: 0.2,
        }],
        ..Default::default()
    }
}
/// Intentionally enumerates every boarding and every downstream alighting, without label dominance.
fn brute(t: &Timetable, q: &Query, costs: &Costs) -> Option<f64> {
    #[derive(Clone, Copy)]
    struct State {
        stop: u32,
        time: u32,
        cost: f64,
        first: u32,
        mode: Option<usize>,
        k: usize,
    }
    fn visit(t: &Timetable, q: &Query, c: &Costs, s: State, best: &mut f64) {
        if s.k > 0 {
            for e in &q.egress {
                let terminal = if e.stop == s.stop {
                    Some(0)
                } else {
                    t.walking_from(s.stop)
                        .iter()
                        .find(|edge| edge.to == e.stop)
                        .map(|edge| edge.duration)
                };
                if let Some(walk) = terminal
                    && s.time + walk + e.duration <= q.departure + q.max_duration
                {
                    let penalty = if c.transfer_per_travel_hour > 0.0 {
                        (s.k - 1) as f64
                            * (c.transfer_base
                                + c.transfer_per_travel_hour * (s.time - s.first) as f64
                                    / 3_600_000.0)
                                .clamp(c.transfer_min, c.transfer_max)
                    } else {
                        0.0
                    };
                    *best = best.min(
                        s.cost
                            + (e.duration as f64 + walk as f64) * c.walk_per_hour / 3_600_000.0
                            + penalty,
                    );
                }
            }
        }
        if s.k == q.max_boardings {
            return;
        }
        let transfers = if s.k == 0 {
            vec![(s.stop, 0)]
        } else {
            std::iter::once((s.stop, t.stops[s.stop as usize].transfer_buffer))
                .chain(t.walking_from(s.stop).iter().map(|e| (e.to, e.duration)))
                .collect()
        };
        for (stop, walk) in transfers {
            for (i, b) in t
                .connections
                .iter()
                .enumerate()
                .filter(|(_, b)| b.from == stop && b.pickup && b.departure >= s.time + walk)
            {
                let boarding = (s.time + walk).max(b.boarding_arrival);
                let mode = &t.trips[b.trip as usize].mode;
                let rate = c
                    .modes_per_hour
                    .get(mode)
                    .copied()
                    .unwrap_or(c.other_per_hour)
                    / 3_600_000.0;
                let penalty = if s.k == 0 || c.transfer_per_travel_hour > 0.0 {
                    0.0
                } else {
                    let previous = &t.trips[s.mode.unwrap()].mode;
                    (c.transfer_base
                        + c.mode_transfers
                            .iter()
                            .find(|p| p.from == *previous && p.to == *mode)
                            .map_or(0.0, |p| p.cost))
                    .clamp(c.transfer_min, c.transfer_max)
                };
                for a in t.connections[i..].iter().filter(|a| {
                    a.trip == b.trip && a.dropoff && a.arrival <= q.departure + q.max_duration
                }) {
                    visit(
                        t,
                        q,
                        c,
                        State {
                            stop: a.to,
                            time: a.arrival,
                            cost: s.cost
                                + walk as f64 * c.walk_per_hour / 3_600_000.0
                                + (boarding - s.time - walk) as f64 * c.wait_per_hour / 3_600_000.0
                                + (a.arrival - boarding) as f64 * rate
                                + penalty,
                            first: if s.k == 0 { boarding } else { s.first },
                            mode: Some(b.trip as usize),
                            k: s.k + 1,
                        },
                        best,
                    );
                }
            }
        }
    }
    let mut best = f64::INFINITY;
    for e in &q.access {
        visit(
            t,
            q,
            costs,
            State {
                stop: e.stop,
                time: q.departure + e.duration,
                cost: e.duration as f64 * costs.walk_per_hour / 3_600_000.0,
                first: 0,
                mode: None,
                k: 0,
            },
            &mut best,
        );
    }
    best.is_finite().then_some(best)
}
#[test]
fn later_cheaper_arrival_and_last_mode_are_preserved() {
    let t = table(
        vec![
            ("bus", vec![(0, 0, 0), (1, 10, 10)]),
            ("rail", vec![(0, 0, 0), (1, 15, 15)]),
            ("rail", vec![(1, 30, 30), (3, 40, 40)]),
        ],
        vec![],
    );
    let c = model();
    let q = query(2);
    let j = weighted::route(
        &t,
        &q,
        &CostModel::new(&t, &c).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    assert!((j.journey.generalized_cost.unwrap() - brute(&t, &q, &c).unwrap()).abs() < 1e-10);
    assert!(matches!(
        j.journey.legs[0],
        multimodal_matsim_router::routing::Leg::Transit { trip: 1, .. }
    ));
}
#[test]
fn generated_timetables_match_exhaustive_weighted_paths() {
    let mut seed = 42_u64;
    let mut random = |n: u32| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as u32 % n
    };
    let mut workspace = Workspace::default();
    for case in 0..200 {
        let mut trips = Vec::new();
        for _ in 0..5 {
            let mode = if random(2) == 0 { "bus" } else { "rail" };
            let start = random(40);
            let calls = (0..3)
                .map(|j| {
                    let time = start + j * 20;
                    (random(4), time, time + random(5))
                })
                .collect();
            trips.push((mode, calls));
        }
        let mut t = table(trips, vec![(1, 2, 3000)]);
        for connection in &mut t.connections {
            connection.pickup = random(6) > 0;
            connection.dropoff = random(6) > 0;
        }
        let mut c = model();
        if case % 2 == 0 {
            c.mode_transfers.clear();
            c.transfer_per_travel_hour = 0.4;
        }
        if case % 5 == 0 {
            c.transfer_base = -0.03;
            c.transfer_min = 0.0;
            c.transfer_max = 0.05;
        }
        if case % 7 == 0 {
            c.wait_per_hour = 0.0;
        }
        if case % 11 == 0 {
            c.walk_per_hour = 0.0;
        }
        let mut q = query(1 + case % 4);
        q.departure = random(8) * 1000;
        q.max_duration = (20 + random(120)) * 1000;
        q.access.push(Endpoint {
            stop: 1,
            duration: 2000,
        });
        q.egress.push(Endpoint {
            stop: 2,
            duration: 6000,
        });
        let expected = brute(&t, &q, &c);
        for flags in 0..2048 {
            let options = SearchOptions {
                local_bound_settlements: if flags & 1024 != 0 { 1 } else { 0 },
                network_partition: flags & 256 != 0,
                transfer_blocks: flags & 512 != 0,
                local_connections: flags & 16 != 0,
                local_bounds: flags & 32 != 0,
                temporal_pruning: flags & 128 != 0,
                regional_bound_cells: if flags & 64 != 0 { 2 } else { 0 },
                destination_bounds: flags & 1 != 0,
                cross_round_dominance: flags & 2 != 0,
                sorted_transfers: flags & 4 != 0,
                boarding_index: flags & 8 != 0,
            };
            let actual = weighted::route(
                &t,
                &q,
                &CostModel::with_search_options(&t, &c, options).unwrap(),
                &mut workspace,
            )
            .unwrap()
            .map(|j| j.journey.generalized_cost.unwrap());
            assert_eq!(
                actual.is_some(),
                expected.is_some(),
                "case {case}, options {flags}"
            );
            if let (Some(a), Some(b)) = (actual, expected) {
                assert!(
                    (a - b).abs() < 1e-10,
                    "case {case}, options {flags}: {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn bounds_handle_zero_cost_cycles_and_nonzero_egress_without_rounding_drift() {
    let t = table(
        vec![(
            "bus",
            vec![(0, 0, 0), (1, 10, 10), (0, 20, 20), (3, 30, 30)],
        )],
        vec![(1, 2, 0), (2, 1, 0)],
    );
    let c = Costs {
        other_per_hour: 0.0,
        wait_per_hour: 0.0,
        ..Costs::default()
    };
    let mut q = query(4);
    q.egress[0].duration = 7000;
    let j = weighted::route(
        &t,
        &q,
        &CostModel::new(&t, &c).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    assert!((j.journey.generalized_cost.unwrap() - brute(&t, &q, &c).unwrap()).abs() < 1e-12);
}

#[test]
fn boarding_index_keeps_later_overtaking_services_and_honors_the_horizon() {
    let t = table(
        vec![
            ("bus", vec![(0, 0, 0), (1, 100, 100)]),
            ("bus", vec![(0, 10, 10), (1, 20, 20)]),
            ("rail", vec![(1, 25, 25), (3, 30, 30)]),
        ],
        vec![],
    );
    let c = model();
    let mut q = query(3);
    let run = |q: &Query| {
        weighted::route(
            &t,
            q,
            &CostModel::new(&t, &c).unwrap(),
            &mut Workspace::default(),
        )
        .unwrap()
    };
    let j = run(&q).unwrap();
    assert_eq!(j.journey.arrival, 30_000);
    assert!(matches!(
        j.journey.legs[0],
        multimodal_matsim_router::routing::Leg::Transit { trip: 1, .. }
    ));
    q.max_duration = 29_999;
    assert!(run(&q).is_none());
}

#[test]
fn pruning_discards_costly_reboardings_but_keeps_the_same_optimum() {
    let t = table(
        vec![
            ("bus", vec![(0, 0, 0), (1, 10, 10), (3, 30, 30)]),
            (
                "bus",
                vec![(1, 20, 20), (0, 25, 25), (1, 35, 35), (3, 50, 50)],
            ),
            ("rail", vec![(0, 60, 60), (2, 70, 70), (3, 100, 100)]),
        ],
        vec![(1, 2, 80000)],
    );
    let c = model();
    let q = query(12);
    let a = weighted::route(
        &t,
        &q,
        &CostModel::new(&t, &c).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    let b = weighted::route(
        &t,
        &q,
        &CostModel::with_search_options(&t, &c, SearchOptions::disabled()).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(a.journey.arrival, b.journey.arrival);
    assert!(
        (a.journey.generalized_cost.unwrap() - b.journey.generalized_cost.unwrap()).abs() < 1e-12
    );
    assert!(
        a.journey.statistics.labels_pruned_by_bound
            + a.journey.statistics.labels_pruned_by_history
            + a.journey.statistics.transfers_pruned
            > 0
    );
    assert!(a.journey.statistics.path_nodes <= b.journey.statistics.path_nodes);
}
#[test]
fn dwell_wait_walk_and_fixed_penalty_have_distinct_costs() {
    let t = table(
        vec![
            ("bus", vec![(0, 10, 20), (1, 40, 40)]),
            ("rail", vec![(1, 60, 70), (3, 90, 90)]),
        ],
        vec![],
    );
    let c = model();
    let q = query(2);
    let j = weighted::route(
        &t,
        &q,
        &CostModel::new(&t, &c).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    // Wait 10+15 seconds; bus 30 seconds, rail 30; transfer walk/buffer 5 seconds; bus->rail 0.3.
    let expected = 25.0 * 0.001 + 30.0 * 0.001 + 30.0 * 0.0005 + 5.0 * 0.002 + 0.3;
    assert!((j.journey.generalized_cost.unwrap() - expected).abs() < 1e-10);
}

#[test]
fn zero_time_transfers_permissions_and_same_vehicle_continuation() {
    let mut t = table(
        vec![
            ("bus", vec![(0, 0, 0), (1, 0, 0)]),
            ("rail", vec![(1, 0, 0), (3, 0, 0)]),
        ],
        vec![],
    );
    for s in &mut t.stops {
        s.transfer_buffer = 0;
    }
    let c = Costs::default();
    let q = query(2);
    let run = |t: &Timetable| {
        weighted::route(
            t,
            &q,
            &CostModel::new(t, &c).unwrap(),
            &mut Workspace::default(),
        )
        .unwrap()
    };
    assert_eq!(run(&t).unwrap().journey.boardings, 2);
    t.connections[0].dropoff = false;
    assert!(run(&t).is_none());
    t.connections[0].dropoff = true;
    t.connections[0].pickup = false;
    assert!(run(&t).is_none());
    let mut t = table(
        vec![("bus", vec![(0, 10, 20), (1, 30, 40), (3, 50, 50)])],
        vec![],
    );
    t.connections[0].dropoff = false;
    t.connections[1].pickup = false;
    let j = run(&t).unwrap();
    assert_eq!(j.journey.boardings, 1);
    assert!((j.journey.generalized_cost.unwrap() - 50.0 / 3600.0).abs() < 1e-12);
}
