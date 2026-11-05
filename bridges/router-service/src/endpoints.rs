//! Indexes served stops once and constructs request-local walking endpoints with bounded retries.
use crate::loading::{self, Settings};
use anyhow::{Result, ensure};
use multimodal_matsim_router::{
    config::Config,
    engine::Engine,
    model::{Stop, Timetable},
    spatial, time,
    weighted::{Endpoint, Query, ResultJourney, Workspace},
};
use rstar::{RTree, primitives::GeomWithData};
use std::cell::RefCell;
thread_local! {static WORKSPACE:RefCell<Workspace>=RefCell::default();}
pub struct Index {
    tree: RTree<GeomWithData<[f64; 3], usize>>,
    access: Vec<f64>,
    egress: Vec<f64>,
}
impl Index {
    pub fn new(t: &Timetable, a: &loading::StopTimes, e: &loading::StopTimes) -> Self {
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
        Self {
            tree,
            access: t
                .stops
                .iter()
                .map(|s| a.get(&s.id).copied().unwrap_or(0.))
                .collect(),
            egress: t
                .stops
                .iter()
                .map(|s| e.get(&s.id).copied().unwrap_or(0.))
                .collect(),
        }
    }
    fn candidates(
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
        let query = Stop {
            id: String::new(),
            name: String::new(),
            x: xy[0],
            y: xy[1],
            transfer_buffer: 0,
        };
        if matches!(
            t.coordinates,
            multimodal_matsim_router::model::Coordinates::Geographic
        ) {
            ensure!(
                xy[0].abs() <= 180. && xy[1].abs() <= 90.,
                "invalid longitude/latitude"
            );
        }
        let point = spatial::point(&query, t.coordinates);
        let nearby = |radius: f64| {
            self.tree
                .locate_within_distance(point, radius * radius)
                .filter(|s| spatial::distance(&query, &t.stops[s.data], t.coordinates) <= radius)
                .map(|s| s.data)
                .collect::<Vec<_>>()
        };
        let mut stops = nearby(radius);
        if stops.len() < 2
            && let Some(nearest) = self.tree.nearest_neighbor(&point)
        {
            radius = spatial::distance(&query, &t.stops[nearest.data], t.coordinates) + extension;
            stops = nearby(radius);
        }
        stops.sort_unstable();
        let mut explored = 0_f64;
        let values = stops
            .into_iter()
            .map(|i| {
                let distance = spatial::distance(&query, &t.stops[i], t.coordinates);
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
    pub fn route(
        &self,
        engine: &Engine,
        config: &Config,
        settings: &Settings,
        origin: [f64; 2],
        destination: [f64; 2],
        departure: &str,
    ) -> Result<Option<ResultJourney>> {
        let departure = time::parse_time(departure)?;
        let mut radius = settings.search_radius_m;
        for _ in 0..5 {
            let (access, a) = self.candidates(
                &engine.timetable,
                origin,
                radius,
                settings.extension_radius_m,
                config.preprocessing.walking_speed_m_s,
                true,
            )?;
            let (egress, e) = self.candidates(
                &engine.timetable,
                destination,
                radius,
                settings.extension_radius_m,
                config.preprocessing.walking_speed_m_s,
                false,
            )?;
            if access.is_empty() || egress.is_empty() {
                return Ok(None);
            }
            let query = Query {
                departure,
                max_boardings: config.routing.max_boardings,
                max_duration: time::seconds(config.routing.max_duration_seconds)?,
                access,
                egress,
            };
            if let Some(result) =
                WORKSPACE.with(|w| engine.weighted_route(&query, &mut w.borrow_mut()))?
            {
                let from = Stop {
                    id: String::new(),
                    name: String::new(),
                    x: origin[0],
                    y: origin[1],
                    transfer_buffer: 0,
                };
                let to = Stop {
                    x: destination[0],
                    y: destination[1],
                    ..from.clone()
                };
                let walking = spatial::distance(&from, &to, engine.timetable.coordinates)
                    / config.preprocessing.walking_speed_m_s
                    * config.routing.costs.walk_per_hour
                    / 3600.;
                if walking * settings.direct_walk_factor
                    < result.journey.generalized_cost.unwrap_or(f64::INFINITY)
                {
                    return Ok(None);
                }
                return Ok(Some(result));
            }
            let explored = radius.max(a).max(e);
            radius = (2. * explored).max(explored + settings.extension_radius_m.max(1.));
        }
        Ok(None)
    }
}
