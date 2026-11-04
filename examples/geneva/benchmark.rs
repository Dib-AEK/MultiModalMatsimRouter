//! Benchmarks shared stop-to-stop requests with serial latency and bounded parallel batch throughput.
#[path = "../../src/bin_allocator.rs"]
mod bin_allocator;
use anyhow::{Context, Result, ensure};
use multimodal_matsim_router::{
    config::Config,
    engine::Engine,
    routing::{Journey, Workspace},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    time::Instant,
};
#[derive(Deserialize)]
struct Request {
    id: String,
    from: String,
    to: String,
    departure: String,
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "usage: geneva-benchmark CONFIG REQUESTS OUTPUT_DIRECTORY WARMUP"
    );
    let config = Config::load(Path::new(&args[1]))?;
    let output = Path::new(&args[3]);
    std::fs::create_dir_all(output)?;
    let start = Instant::now();
    let engine = Engine::load(&config)?;
    let load = start.elapsed().as_secs_f64();
    let rows: Vec<Request> = BufReader::new(File::open(&args[2])?)
        .lines()
        .map(|line| Ok(serde_json::from_str(&line?)?))
        .collect::<Result<_>>()?;
    ensure!(!rows.is_empty(), "no requests");
    let queries = rows
        .iter()
        .map(|r| engine.query(&config, &r.from, &r.to, &r.departure))
        .collect::<Result<Vec<_>>>()?;
    let warmup: usize = args[4].parse()?;
    let mut workspace = Workspace::default();
    for i in 0..warmup {
        let _ = engine.route(&queries[i % queries.len()], &mut workspace)?;
    }
    let mut results: Vec<(u128, Option<Journey>)> = Vec::with_capacity(rows.len());
    let serial_start = Instant::now();
    for query in &queries {
        let start = Instant::now();
        let journey = engine.route(query, &mut workspace)?;
        results.push((start.elapsed().as_nanos(), journey));
    }
    let serial = serial_start.elapsed().as_secs_f64();
    let warm: Vec<_> = (0..warmup)
        .map(|i| queries[i % queries.len()].clone())
        .collect();
    for r in engine.batch(&warm) {
        r?;
    }
    let parallel_start = Instant::now();
    let parallel = engine.batch(&queries);
    let parallel_seconds = parallel_start.elapsed().as_secs_f64();
    for ((_, serial), batch) in results.iter().zip(parallel) {
        let batch = batch?;
        ensure!(
            serial.as_ref().map(|j| (j.arrival, j.boardings))
                == batch.as_ref().map(|j| (j.arrival, j.boardings)),
            "parallel result differs"
        );
    }
    let mut file = BufWriter::new(File::create(output.join("rust_results.jsonl"))?);
    for (row, (nanos, journey)) in rows.iter().zip(&results) {
        writeln!(
            file,
            "{}",
            json!({"id":row.id,"runtime_ns":nanos,"journey":journey})
        )?;
    }
    file.flush()?;
    serde_json::to_writer(BufWriter::new(File::create(output.join("rust_metadata.json"))?),&json!({
        "engine":"MultiModalMatsimRouter CSA","release":!cfg!(debug_assertions),"load_seconds":load,
        "serial_seconds":serial,"parallel_seconds":parallel_seconds,"threads":config.execution.threads,
        "queries":rows.len(),"warmup_per_mode":warmup,"parallel_matches_serial":true,
        "stops":engine.timetable.stops,"trips":engine.timetable.trips,"connections":engine.timetable.connections.len(),"footpaths":engine.timetable.walks.len()
    })).context("writing metadata")?;
    println!(
        "{}",
        json!({"serial_seconds":serial,"parallel_seconds":parallel_seconds,"queries":rows.len()})
    );
    Ok(())
}
