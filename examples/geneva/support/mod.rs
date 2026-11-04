//! Loads identical prepared-transfer inputs for the Geneva routing and scaling benchmarks.
use anyhow::{Context, Result};
use multimodal_matsim_router::{
    config::{Config, Input},
    engine::Engine,
    preprocess, readers, time,
    weighted::{Endpoint, Query},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};
#[derive(Deserialize)]
struct NamedEndpoint {
    stop: String,
    duration: u32,
}
#[derive(Deserialize)]
struct Request {
    departure: String,
    access: Vec<NamedEndpoint>,
    egress: Vec<NamedEndpoint>,
}
#[derive(Deserialize)]
struct Transfer {
    from: String,
    to: String,
    duration: u32,
}
pub fn load(config: &Config, requests: &Path, transfers: &Path) -> Result<(Engine, Vec<Query>)> {
    let Input::Matsim { path, coordinates } = &config.input else {
        anyhow::bail!("benchmark requires MATSim input");
    };
    let mut raw = readers::matsim::read(path, *coordinates)?;
    let ids: HashMap<_, _> = raw
        .stops
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i as u32))
        .collect();
    raw.footpaths.clear();
    for s in &mut raw.stops {
        s.transfer_buffer = 0;
    }
    for e in serde_json::from_reader::<_, Vec<Transfer>>(File::open(transfers)?)? {
        let from = *ids.get(&e.from).context("unknown transfer origin")?;
        let to = *ids.get(&e.to).context("unknown transfer destination")?;
        if from == to {
            raw.stops[from as usize].transfer_buffer = e.duration;
        } else {
            raw.footpaths.push((from, to, e.duration));
        }
    }
    let mut p = config.preprocessing.clone();
    p.walking_radius_m = 0.0;
    p.default_transfer_seconds = 0.0;
    p.use_explicit_transfers = true;
    let engine = Engine::from_timetable(config, preprocess::prepare(raw, &p)?)?;
    let endpoints = |es: Vec<NamedEndpoint>| -> Result<Vec<Endpoint>> {
        es.into_iter()
            .map(|e| {
                Ok(Endpoint {
                    stop: engine.stop_id(&e.stop)?,
                    duration: e.duration,
                })
            })
            .collect()
    };
    let queries = BufReader::new(File::open(requests)?)
        .lines()
        .map(|l| -> Result<Query> {
            let r: Request = serde_json::from_str(&l?)?;
            Ok(Query {
                departure: time::parse_time(&r.departure)?,
                max_boardings: config.routing.max_boardings,
                max_duration: time::seconds(config.routing.max_duration_seconds)?,
                access: endpoints(r.access)?,
                egress: endpoints(r.egress)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((engine, queries))
}
