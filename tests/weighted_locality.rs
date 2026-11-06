//! Proves local work avoids unrelated services without city cutoffs or changes to route semantics.
use multimodal_matsim_router::{
    config::Preprocessing,
    cost::{CostModel, Costs, SearchOptions},
    model::*,
    preprocess,
    weighted::{self, Endpoint, Query, Workspace},
};
fn table(calls: Vec<Vec<(u32, u32)>>) -> Timetable {
    preprocess::prepare(
        RawTimetable {
            coordinates: Coordinates::ProjectedMeters,
            stops: (0..4)
                .map(|i| Stop {
                    id: i.to_string(),
                    name: String::new(),
                    x: i as f64 * 100_000.,
                    y: 0.,
                    transfer_buffer: 0,
                })
                .collect(),
            trips: calls
                .into_iter()
                .enumerate()
                .map(|(i, calls)| RawTrip {
                    trip: Trip {
                        id: i.to_string(),
                        line: i.to_string(),
                        route: i.to_string(),
                        departure: i.to_string(),
                        mode: "bus".into(),
                    },
                    calls: calls
                        .into_iter()
                        .map(|(stop, time)| Call {
                            stop,
                            arrival: time,
                            departure: time,
                            pickup: true,
                            dropoff: true,
                        })
                        .collect(),
                })
                .collect(),
            footpaths: vec![],
        },
        &Preprocessing {
            walking_radius_m: 0.,
            ..Default::default()
        },
    )
    .unwrap()
}
fn query() -> Query {
    Query {
        departure: 0,
        max_boardings: 64,
        max_duration: 1_000_000,
        access: vec![Endpoint {
            stop: 0,
            duration: 0,
        }],
        egress: vec![Endpoint {
            stop: 1,
            duration: 0,
        }],
    }
}
fn compare(t: &Timetable, q: &Query, w: &mut Workspace) -> Option<weighted::ResultJourney> {
    let exact = CostModel::with_search_options(
        t,
        &Costs::default(),
        SearchOptions {
            local_connections: false,
            regional_bound_cells: 0,
            local_bounds: false,
            ..Default::default()
        },
    )
    .unwrap();
    let local = CostModel::new(t, &Costs::default()).unwrap();
    let a = weighted::route(t, q, &exact, w).unwrap();
    let b = weighted::route(t, q, &local, w).unwrap();
    assert_eq!(a.is_some(), b.is_some());
    if let (Some(a), Some(b)) = (&a, &b) {
        assert_eq!(a.journey.generalized_cost, b.journey.generalized_cost);
        assert_eq!(a.journey.arrival, b.journey.arrival);
        assert_eq!(a.journey.boardings, b.journey.boardings);
        assert_eq!(
            serde_json::to_value(&a.journey.legs).unwrap(),
            serde_json::to_value(&b.journey.legs).unwrap()
        );
        assert!(
            b.journey.statistics.connections_scanned <= a.journey.statistics.connections_scanned
        );
    }
    b
}
#[test]
fn unrelated_departures_do_not_increase_connection_scans() {
    let mut calls = vec![vec![(0, 100), (1, 1000)]];
    calls.extend((0..10_000).map(|_| vec![(2, 200), (3, 500)]));
    let t = table(calls);
    let result = compare(&t, &query(), &mut Workspace::default()).unwrap();
    assert_eq!(result.journey.statistics.connections_scanned, 1);
    assert_eq!(result.journey.statistics.bound_settled_stops, 2);
}
#[test]
fn route_can_leave_and_reenter_a_local_area() {
    // The express trip passes a distant stop; a geographic/city clipping would lose it.
    let t = table(vec![
        vec![(0, 100), (2, 200), (1, 300)],
        vec![(0, 100), (1, 900)],
    ]);
    let result = compare(&t, &query(), &mut Workspace::default()).unwrap();
    assert_eq!(result.journey.arrival, 300);
    assert_eq!(result.journey.boardings, 1);
}
#[test]
fn transfer_outside_the_local_area_remains_available() {
    let t = table(vec![
        vec![(0, 100), (2, 200)],
        vec![(2, 210), (1, 300)],
        vec![(0, 100), (1, 900)],
    ]);
    let result = compare(&t, &query(), &mut Workspace::default()).unwrap();
    assert_eq!(result.journey.arrival, 300);
    assert_eq!(result.journey.boardings, 2);
}

#[test]
fn reuses_sparse_state_across_models_sizes_targets_and_unreachable_queries() {
    let mut workspace = Workspace::default();
    let small = table(vec![vec![(0, 100), (1, 300)]]);
    let mut calls = vec![vec![(0, 100), (1, 300)]];
    calls.extend((0..200).map(|i| vec![(2, 200 + i), (3, 500 + i)]));
    let large = table(calls);
    for _ in 0..3 {
        assert!(compare(&large, &query(), &mut workspace).is_some());
        let mut reverse = query();
        reverse.access[0].stop = 1;
        reverse.egress[0].stop = 0;
        assert!(compare(&small, &reverse, &mut workspace).is_none());
        assert!(compare(&small, &query(), &mut workspace).is_some());
    }
}
#[test]
fn equal_time_connections_and_overtaking_keep_reference_order() {
    let t = table(vec![
        vec![(0, 100), (2, 100), (1, 100)],
        vec![(0, 100), (1, 300)],
        vec![(0, 150), (1, 200)],
    ]);
    let mut q = query();
    q.access.push(Endpoint {
        stop: 2,
        duration: 100,
    });
    assert_eq!(
        compare(&t, &q, &mut Workspace::default())
            .unwrap()
            .journey
            .arrival,
        100
    );
}

#[test]
fn bounded_heuristic_preparation_preserves_the_full_search() {
    let t = table(vec![
        vec![(0, 100), (2, 200)],
        vec![(2, 210), (1, 300)],
        vec![(0, 100), (1, 900)],
    ]);
    let q = query();
    let reference = weighted::route(
        &t,
        &q,
        &CostModel::with_search_options(&t, &Costs::default(), SearchOptions::disabled()).unwrap(),
        &mut Workspace::default(),
    )
    .unwrap()
    .unwrap();
    let mut workspace = Workspace::default();
    for budget in [1, 2, 0, 3, 1] {
        let model = CostModel::with_search_options(
            &t,
            &Costs::default(),
            SearchOptions {
                local_bound_settlements: budget,
                ..SearchOptions::default()
            },
        )
        .unwrap();
        let result = weighted::route(&t, &q, &model, &mut workspace)
            .unwrap()
            .unwrap();
        assert_eq!(result.journey.legs, reference.journey.legs);
        assert_eq!(
            result.journey.generalized_cost,
            reference.journey.generalized_cost
        );
        if budget > 0 {
            assert!(result.journey.statistics.bound_settled_stops <= budget);
        }
    }
}
