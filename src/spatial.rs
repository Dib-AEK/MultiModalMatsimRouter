//! Computes correct metric distances and indexes stop coordinates for bounded walking-neighbor searches.
use crate::model::{Coordinates, Stop};
use rstar::RTree;
const EARTH: f64 = 6_371_008.8;
pub fn point(stop: &Stop, coordinates: Coordinates) -> [f64; 3] {
    match coordinates {
        Coordinates::ProjectedMeters => [stop.x, stop.y, 0.0],
        Coordinates::Geographic => {
            let (lon, lat) = (stop.x.to_radians(), stop.y.to_radians());
            [
                EARTH * lat.cos() * lon.cos(),
                EARTH * lat.cos() * lon.sin(),
                EARTH * lat.sin(),
            ]
        }
    }
}
pub fn distance(a: &Stop, b: &Stop, coordinates: Coordinates) -> f64 {
    let a = point(a, coordinates);
    let b = point(b, coordinates);
    let chord = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt();
    match coordinates {
        Coordinates::ProjectedMeters => chord,
        Coordinates::Geographic => 2.0 * EARTH * (chord / (2.0 * EARTH)).min(1.0).asin(),
    }
}
pub fn neighbors(stops: &[Stop], coordinates: Coordinates, radius: f64) -> Vec<Vec<usize>> {
    let points: Vec<_> = stops.iter().map(|s| point(s, coordinates)).collect();
    let tree = RTree::bulk_load(
        points
            .iter()
            .enumerate()
            .map(|(i, p)| rstar::primitives::GeomWithData::new(*p, i))
            .collect(),
    );
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            tree.locate_within_distance(*p, radius * radius)
                .filter_map(|entry| {
                    (entry.data != i
                        && distance(&stops[i], &stops[entry.data], coordinates) <= radius)
                        .then_some(entry.data)
                })
                .collect()
        })
        .collect()
}
