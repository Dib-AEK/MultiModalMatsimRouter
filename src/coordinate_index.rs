//! Shares an immutable served-stop spatial index and exact walking endpoint construction.
use crate::{
    model::{Stop, Timetable},
    spatial, time,
    weighted::Endpoint,
};
use anyhow::{Result, ensure};
use rstar::{RTree, primitives::GeomWithData};

pub struct CoordinateIndex {
    tree: RTree<GeomWithData<[f64; 3], usize>>,
    access: Vec<f64>,
    egress: Vec<f64>,
}
impl CoordinateIndex {
    pub fn new(t: &Timetable, access: Vec<f64>, egress: Vec<f64>) -> Result<Self> {
        ensure!(
            access.len() == t.stops.len() && egress.len() == t.stops.len(),
            "stop duration dimensions differ"
        );
        ensure!(
            access
                .iter()
                .chain(&egress)
                .all(|x| x.is_finite() && *x >= 0.),
            "invalid stop walking duration"
        );
        let mut served = vec![false; t.stops.len()];
        for c in &t.connections {
            served[c.from as usize] = true;
            served[c.to as usize] = true;
        }
        let tree = RTree::bulk_load(
            t.stops
                .iter()
                .enumerate()
                .filter(|(i, _)| served[*i])
                .map(|(i, s)| GeomWithData::new(spatial::point(s, t.coordinates), i))
                .collect(),
        );
        Ok(Self {
            tree,
            access,
            egress,
        })
    }
    pub fn candidates(
        &self,
        t: &Timetable,
        xy: [f64; 2],
        mut radius: f64,
        extension: f64,
        speed: f64,
        access: bool,
    ) -> Result<(Vec<Endpoint>, f64)> {
        ensure!(
            xy.iter().all(|v| v.is_finite()),
            "coordinates must be finite"
        );
        ensure!(
            radius.is_finite()
                && radius >= 0.
                && extension.is_finite()
                && extension >= 0.
                && speed.is_finite()
                && speed > 0.,
            "invalid spatial search settings"
        );
        let query = Stop {
            id: String::new(),
            name: String::new(),
            x: xy[0],
            y: xy[1],
            transfer_buffer: 0,
        };
        if matches!(t.coordinates, crate::model::Coordinates::Geographic) {
            ensure!(
                xy[0].abs() <= 180. && xy[1].abs() <= 90.,
                "invalid longitude/latitude"
            );
        }
        let point = spatial::point(&query, t.coordinates);
        let nearby = |radius: f64| {
            self.tree
                .locate_within_distance(point, radius * radius)
                .filter_map(|s| {
                    let d = spatial::distance(&query, &t.stops[s.data], t.coordinates);
                    (d <= radius).then_some((s.data, d))
                })
                .collect::<Vec<_>>()
        };
        let mut stops = nearby(radius);
        if stops.len() < 2
            && let Some(nearest) = self.tree.nearest_neighbor(&point)
        {
            radius = spatial::distance(&query, &t.stops[nearest.data], t.coordinates) + extension;
            stops = nearby(radius);
        }
        stops.sort_unstable_by_key(|&(id, _)| id);
        let mut explored = 0_f64;
        let values = stops
            .into_iter()
            .map(|(i, distance)| {
                explored = explored.max(distance);
                Ok(Endpoint {
                    stop: i as u32,
                    duration: time::seconds(
                        (distance / speed).ceil()
                            + if access {
                                self.access[i]
                            } else {
                                self.egress[i]
                            },
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((values, explored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Preprocessing, model::*, preprocess};
    #[test]
    fn matches_brute_force_and_keeps_platform_durations_and_boundaries() {
        let stops = (0..5)
            .map(|i| Stop {
                id: i.to_string(),
                name: String::new(),
                x: i as f64 * 100.,
                y: 0.,
                transfer_buffer: 0,
            })
            .collect();
        let t = preprocess::prepare(
            RawTimetable {
                coordinates: Coordinates::ProjectedMeters,
                stops,
                trips: vec![RawTrip {
                    trip: Trip {
                        id: "t".into(),
                        line: "l".into(),
                        route: "r".into(),
                        departure: "d".into(),
                        mode: "bus".into(),
                    },
                    calls: (0..4)
                        .map(|stop| Call {
                            stop,
                            arrival: stop * 100000,
                            departure: stop * 100000,
                            pickup: true,
                            dropoff: true,
                        })
                        .collect(),
                }],
                footpaths: vec![],
            },
            &Preprocessing {
                walking_radius_m: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let index = CoordinateIndex::new(&t, vec![0.5; 5], vec![1.5; 5]).unwrap();
        let (access, explored) = index
            .candidates(&t, [0., 0.], 200., 50., 1.3, true)
            .unwrap();
        assert_eq!(
            access.iter().map(|e| e.stop).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(explored, 200.);
        for e in access {
            assert_eq!(
                e.duration,
                time::seconds((e.stop as f64 * 100. / 1.3).ceil() + 0.5).unwrap()
            );
        }
        let (far, _) = index
            .candidates(&t, [1000., 0.], 1., 100., 1., false)
            .unwrap();
        assert_eq!(far.iter().map(|e| e.stop).collect::<Vec<_>>(), vec![2, 3]);
        assert!(far.iter().all(|e| e.stop != 4)); // unserved facility excluded
        assert!(
            index
                .candidates(&t, [f64::NAN, 0.], 1., 1., 1., true)
                .is_err()
        );
        assert!(CoordinateIndex::new(&t, vec![], vec![]).is_err());
    }
}
