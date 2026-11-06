//! Verifies bounded coordinate searches preserve reachability and expose heuristic truncation explicitly.
use multimodal_matsim_router::{
    config::Preprocessing,
    coordinate_index::CoordinateIndex,
    coordinate_routing::{self, Policy, Request},
    cost::{CostModel, Costs},
    model::*,
    preprocess,
    weighted::{self, Endpoint, Query, Workspace},
};
fn fixture() -> (Timetable, CoordinateIndex, CostModel, Request) {
    let stops = [5000., 100., 101., 700.]
        .iter()
        .enumerate()
        .map(|(i, &x)| Stop {
            id: i.to_string(),
            name: String::new(),
            x,
            y: 0.,
            transfer_buffer: 0,
        })
        .collect();
    let trips = [
        vec![(0, 0), (3, 600000)],
        vec![(3, 36000000), (1, 36100000)],
        vec![(3, 36000000), (2, 36100000)],
    ]
    .into_iter()
    .enumerate()
    .map(|(i, v)| RawTrip {
        trip: Trip {
            id: i.to_string(),
            line: i.to_string(),
            route: i.to_string(),
            departure: i.to_string(),
            mode: "bus".into(),
        },
        calls: v
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
    .collect();
    let t = preprocess::prepare(
        RawTimetable {
            coordinates: Coordinates::ProjectedMeters,
            stops,
            trips,
            footpaths: vec![],
        },
        &Preprocessing {
            walking_radius_m: 0.,
            ..Default::default()
        },
    )
    .unwrap();
    let index = CoordinateIndex::new(&t, vec![0.; 4], vec![0.; 4]).unwrap();
    let model = CostModel::new(&t, &Costs::default()).unwrap();
    let q = Request {
        origin: [5000., 0.],
        destination: [0., 0.],
        departure: 0,
        max_boardings: 8,
        max_duration: 86400000,
        radius: 200.,
        extension: 100.,
        speed: 1.,
        walk_per_second: 6. / 3600.,
        direct_factor: 100.,
    };
    (t, index, model, q)
}
#[test]
fn expands_a_poor_existing_path_to_find_a_useful_station() {
    let (t, index, model, q) = fixture();
    let mut w = Workspace::default();
    let old = coordinate_routing::route(&t, &index, &model, &q, Policy::legacy(), &mut w).unwrap();
    let improved = coordinate_routing::route(
        &t,
        &index,
        &model,
        &q,
        Policy {
            work_budget: 0,
            quality_radius_m: 1000.,
            ..Policy::default()
        },
        &mut w,
    )
    .unwrap();
    let a = old.journey.unwrap();
    let b = improved.journey.unwrap();
    assert!(a.journey.arrival > 36000000);
    assert_eq!(b.journey.arrival, 1300000);
    assert!(b.journey.generalized_cost < a.journey.generalized_cost);
    assert!(!b.search_truncated);
    assert!(improved.attempts > 1);
}
#[test]
fn incomplete_probe_is_not_interpreted_as_unreachable() {
    let (t, index, model, q) = fixture();
    let mut w = Workspace::default();
    let old = coordinate_routing::route(&t, &index, &model, &q, Policy::legacy(), &mut w)
        .unwrap()
        .journey
        .unwrap();
    let limited = coordinate_routing::route(
        &t,
        &index,
        &model,
        &q,
        Policy {
            work_budget: 1,
            quality_radius_m: 0.,
            quality_wait_seconds: 0.,
            quality_journey_seconds: 0.,
        },
        &mut w,
    )
    .unwrap();
    let result = limited.journey.unwrap();
    assert!(!result.search_truncated);
    assert_eq!(result.journey.legs, old.journey.legs);
    assert!(limited.attempts > 1);
}
#[test]
fn truncated_weighted_search_returns_a_real_witness_and_marks_it() {
    let (t, _, model, _) = fixture();
    let mut w = Workspace::default();
    let q = Query {
        departure: 0,
        max_boardings: 8,
        max_duration: 86400000,
        access: vec![Endpoint {
            stop: 0,
            duration: 0,
        }],
        egress: vec![Endpoint {
            stop: 3,
            duration: 0,
        }],
    };
    let result = weighted::route_with_budget(&t, &q, &model, &mut w, 1).unwrap();
    assert!(result.budget_reached);
    let j = result.journey.unwrap();
    assert!(j.search_truncated);
    assert_eq!(j.journey.arrival, 600000);
    let exact = weighted::route(&t, &q, &model, &mut w).unwrap().unwrap();
    assert!(!exact.search_truncated);
    assert_eq!(j.journey.legs, exact.journey.legs);
}
#[test]
fn a_budget_does_not_manufacture_a_walking_preference() {
    let (t, index, model, mut q) = fixture();
    q.direct_factor = 1.;
    let mut w = Workspace::default();
    let expected =
        coordinate_routing::route(&t, &index, &model, &q, Policy::legacy(), &mut w).unwrap();
    let result = coordinate_routing::route(
        &t,
        &index,
        &model,
        &q,
        Policy {
            work_budget: 1,
            quality_radius_m: 0.,
            quality_wait_seconds: 0.,
            quality_journey_seconds: 0.,
        },
        &mut w,
    )
    .unwrap();
    assert!(expected.journey.is_none());
    assert!(result.journey.is_none());
}
