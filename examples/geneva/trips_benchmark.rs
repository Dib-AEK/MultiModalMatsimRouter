//! Measures weighted routing on shared coordinate-derived stop candidates and Raptor's transfer graph.
#[path = "../../src/bin_allocator.rs"]
mod bin_allocator;
use anyhow::{Context, Result, ensure};
use multimodal_matsim_router::{
    config::{Config, Input},
    engine::Engine,
    preprocess, readers, time,
    weighted::{Endpoint, Query, Workspace},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    time::Instant,
};
#[derive(Deserialize)]
struct NamedEndpoint {
    stop: String,
    duration: u32,
}
#[derive(Deserialize)]
struct Request {
    id: String,
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
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 6,
        "usage: geneva-trips CONFIG REQUESTS TRANSFERS OUTPUT WARMUP"
    );
    let config = Config::load(Path::new(&args[1]))?;
    let output = Path::new(&args[4]);
    std::fs::create_dir_all(output)?;
    let started = Instant::now();
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
    let transfers: Vec<Transfer> = serde_json::from_reader(File::open(&args[3])?)?;
    raw.footpaths.clear();
    for stop in &mut raw.stops {
        stop.transfer_buffer = 0;
    }
    for e in transfers {
        let from = *ids.get(&e.from).context("unknown transfer origin")?;
        let to = *ids.get(&e.to).context("unknown transfer destination")?;
        if from == to {
            raw.stops[from as usize].transfer_buffer = e.duration;
        } else {
            raw.footpaths.push((from, to, e.duration));
        }
    }
    let mut preprocessing = config.preprocessing.clone();
    preprocessing.walking_radius_m = 0.0;
    preprocessing.default_transfer_seconds = 0.0;
    preprocessing.use_explicit_transfers = true;
    let engine = Engine::from_timetable(&config, preprocess::prepare(raw, &preprocessing)?)?;
    let load = started.elapsed().as_secs_f64();
    let rows: Vec<Request> = BufReader::new(File::open(&args[2])?)
        .lines()
        .map(|l| Ok(serde_json::from_str(&l?)?))
        .collect::<Result<_>>()?;
    ensure!(!rows.is_empty(), "no requests");
    let endpoints = |es: &Vec<NamedEndpoint>| -> Result<Vec<Endpoint>> {
        es.iter()
            .map(|e| {
                Ok(Endpoint {
                    stop: engine.stop_id(&e.stop)?,
                    duration: e.duration,
                })
            })
            .collect()
    };
    let queries: Vec<Query> = rows
        .iter()
        .map(|r| {
            Ok(Query {
                departure: time::parse_time(&r.departure)?,
                max_boardings: config.routing.max_boardings,
                max_duration: time::seconds(config.routing.max_duration_seconds)?,
                access: endpoints(&r.access)?,
                egress: endpoints(&r.egress)?,
            })
        })
        .collect::<Result<_>>()?;
    let warmup: usize = args[5].parse()?;
    let mut workspace = Workspace::default();
    for i in 0..warmup {
        let _ = engine.weighted_route(&queries[i % queries.len()], &mut workspace)?;
    }
    let serial_start = Instant::now();
    let mut results = Vec::new();
    for q in &queries {
        let started = Instant::now();
        let j = engine.weighted_route(q, &mut workspace)?;
        results.push((started.elapsed().as_nanos(), j));
    }
    let serial = serial_start.elapsed().as_secs_f64();
    let warm: Vec<_> = (0..warmup)
        .map(|i| queries[i % queries.len()].clone())
        .collect();
    for j in engine.weighted_batch(&warm) {
        j?;
    }
    let started = Instant::now();
    let parallel = engine.weighted_batch(&queries);
    let parallel_seconds = started.elapsed().as_secs_f64();
    for ((_, serial), parallel) in results.iter().zip(parallel) {
        let p = parallel?;
        ensure!(
            serial
                .as_ref()
                .map(|j| (&j.journey.legs, j.journey.generalized_cost))
                == p.as_ref()
                    .map(|j| (&j.journey.legs, j.journey.generalized_cost)),
            "parallel differs"
        );
    }
    let mut file = BufWriter::new(File::create(output.join("rust_results.jsonl"))?);
    for (r, (runtime, j)) in rows.iter().zip(results) {
        writeln!(
            file,
            "{}",
            json!({"id":r.id,"runtime_ns":runtime,"journey":j})
        )?;
    }
    file.flush()?;
    serde_json::to_writer(
        BufWriter::new(File::create(output.join("rust_metadata.json"))?),
        &json!({
        "engine":"Rust weighted connection scan", "serial_seconds":serial,"parallel_seconds":parallel_seconds,
        "load_seconds":load,"threads":config.execution.threads,"warmup":warmup,"parallel_matches_serial":true,
        "stops":engine.timetable.stops,"trips":engine.timetable.trips,"connections":engine.timetable.connections.len(),
        "costs":config.routing.costs,"search":config.routing.search,"index":engine.weighted_index_summary(),"footpaths":engine.timetable.walks.len()}),
    )?;
    println!(
        "{}",
        json!({"serial_seconds":serial,"parallel_seconds":parallel_seconds})
    );
    Ok(())
}
