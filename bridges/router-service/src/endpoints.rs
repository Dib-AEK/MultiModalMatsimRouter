//! Indexes served stops once and constructs request-local walking endpoints with bounded retries.
use crate::loading::{self, Settings};
use anyhow::Result;
use multimodal_matsim_router::{
    config::Config,
    engine::Engine,
    model::Timetable,
    time,
    weighted::{ResultJourney, Workspace},
};
use std::cell::RefCell;
thread_local! {static WORKSPACE:RefCell<Workspace>=RefCell::default();}
pub struct Index {
    spatial: multimodal_matsim_router::coordinate_index::CoordinateIndex,
}
impl Index {
    pub fn new(t: &Timetable, a: &loading::StopTimes, e: &loading::StopTimes) -> Self {
        Self {
            spatial: multimodal_matsim_router::coordinate_index::CoordinateIndex::new(
                t,
                t.stops
                    .iter()
                    .map(|s| a.get(&s.id).copied().unwrap_or(0.))
                    .collect(),
                t.stops
                    .iter()
                    .map(|s| e.get(&s.id).copied().unwrap_or(0.))
                    .collect(),
            )
            .expect("validated stop durations"),
        }
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
        let q = multimodal_matsim_router::coordinate_routing::Request {
            origin,
            destination,
            departure: time::parse_time(departure)?,
            max_boardings: config.routing.max_boardings,
            max_duration: time::seconds(config.routing.max_duration_seconds)?,
            radius: settings.search_radius_m,
            extension: settings.extension_radius_m,
            speed: config.preprocessing.walking_speed_m_s,
            walk_per_second: config.routing.costs.walk_per_hour / 3600.,
            direct_factor: settings.direct_walk_factor,
        };
        let costs = engine.cost_model()?;
        WORKSPACE.with(|w| {
            Ok(multimodal_matsim_router::coordinate_routing::route(
                &engine.timetable,
                &self.spatial,
                costs,
                &q,
                settings.policy,
                &mut w.borrow_mut(),
            )?
            .journey)
        })
    }
}
