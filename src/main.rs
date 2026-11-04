//! Exposes preparation, single/batch routing, inspection and repeatable end-to-end query benchmarks.
mod bin_allocator;
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use multimodal_matsim_router::{
    cache,
    config::{Config, Input},
    engine::Engine,
    routing::{Journey, Leg, Query, Workspace},
    time,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    time::Instant,
};
#[derive(Parser)]
#[command(
    version,
    about = "MultiModalMatsimRouter: reusable public transport routing core"
)]
struct Cli {
    #[arg(long, default_value = "config.example.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Import, validate and preprocess, then atomically write the configured binary cache.
    Prepare,
    /// Print source-neutral timetable sizes after loading.
    Info,
    /// Route between external stop IDs at a service-day time (hours may exceed 24).
    Route {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        departure: String,
    },
    /// Stream JSONL requests in bounded batches and emit ordered JSONL responses.
    Batch {
        #[arg(long)]
        requests: PathBuf,
    },
    /// Measure warm query throughput including reconstruction, separate from loading.
    Benchmark {
        #[arg(long, default_value_t = 1000)]
        queries: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    id: Option<String>,
    from: String,
    to: String,
    departure: String,
}
fn result(engine: &Engine, journey: Option<Journey>) -> Value {
    let Some(j) = journey else { return Value::Null };
    let t = &engine.timetable;
    let legs:Vec<_>=j.legs.into_iter().map(|leg|match leg {
        Leg::Walk{from,to,departure,arrival}=>json!({"kind":"walk","from":t.stops[from as usize].id,"to":t.stops[to as usize].id,"departure":time::format_time(departure),"arrival":time::format_time(arrival)}),
        Leg::Transit{trip,from,to,departure,arrival}=>json!({"kind":"transit","trip_id":t.trips[trip as usize].id,"line":t.trips[trip as usize].line,"route":t.trips[trip as usize].route,"mode":t.trips[trip as usize].mode,"from":t.stops[from as usize].id,"to":t.stops[to as usize].id,"departure":time::format_time(departure),"arrival":time::format_time(arrival)})
    }).collect();
    json!({"departure":time::format_time(j.departure),"arrival":time::format_time(j.arrival),"boardings":j.boardings,"generalized_cost":j.generalized_cost,"legs":legs,"pareto_arrivals_ms":j.pareto_arrivals,"statistics":j.statistics})
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    let started = Instant::now();
    let engine = Engine::load(&config)?;
    let load_seconds = started.elapsed().as_secs_f64();
    match cli.command {
        Command::Prepare => {
            let input = match &config.input {
                Input::Matsim { path, .. } | Input::Gtfs { path, .. } | Input::Binary { path } => {
                    path
                }
            };
            if config.cache.path.exists() {
                ensure!(
                    input.canonicalize()? != config.cache.path.canonicalize()?,
                    "cache output must not overwrite input"
                );
            }
            cache::write(&config.cache.path, &engine.timetable)?;
            println!(
                "{}",
                json!({"cache":config.cache.path,"load_and_prepare_seconds":load_seconds})
            );
        }
        Command::Info => println!(
            "{}",
            json!({"stops":engine.timetable.stops.len(),"trips":engine.timetable.trips.len(),"connections":engine.timetable.connections.len(),"footpaths":engine.timetable.walks.len(),"reachability_bytes":engine.timetable.reachability.len()*8,"load_seconds":load_seconds})
        ),
        Command::Route {
            from,
            to,
            departure,
        } => {
            let query = engine.query(&config, &from, &to, &departure)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result(
                    &engine,
                    engine.route(&query, &mut Workspace::default())?
                ))?
            );
        }
        Command::Batch { requests } => {
            let mut lines = BufReader::new(std::fs::File::open(requests)?)
                .lines()
                .enumerate();
            let stdout = std::io::stdout();
            let mut output = std::io::BufWriter::new(stdout.lock());
            loop {
                let mut ids = Vec::new();
                let mut queries = Vec::new();
                for (line, text) in lines.by_ref().take(config.execution.batch_size) {
                    let request: Request = serde_json::from_str(&text?)
                        .with_context(|| format!("invalid request at line {}", line + 1))?;
                    queries.push(engine.query(
                        &config,
                        &request.from,
                        &request.to,
                        &request.departure,
                    )?);
                    ids.push(request.id.unwrap_or_else(|| (line + 1).to_string()));
                }
                if queries.is_empty() {
                    break;
                }
                for (id, route) in ids.into_iter().zip(engine.batch(&queries)) {
                    writeln!(
                        output,
                        "{}",
                        json!({"id":id,"journey":result(&engine,route?)})
                    )?;
                }
            }
            output.flush()?;
        }
        Command::Benchmark { queries, mut seed } => {
            ensure!(queries > 0, "query count must be positive");
            let mut random = || {
                seed = seed.wrapping_add(0x9e3779b97f4a7c15);
                let mut z = seed;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                z ^ (z >> 31)
            };
            let n = engine.timetable.stops.len() as u64;
            let input: Vec<_> = (0..queries)
                .map(|_| Query {
                    from: (random() % n) as u32,
                    to: (random() % n) as u32,
                    departure: ((5 * 3600 + random() % (17 * 3600)) * 1000) as u32,
                    max_boardings: config.routing.max_boardings,
                    return_pareto: config.routing.return_pareto,
                    max_duration: time::seconds(config.routing.max_duration_seconds).unwrap_or(0),
                })
                .collect();
            let mut workspace = Workspace::default();
            for query in input.iter().take(100) {
                let _ = engine.route(query, &mut workspace)?;
            }
            let now = Instant::now();
            let responses = engine.batch(&input);
            let elapsed = now.elapsed().as_secs_f64();
            let mut reachable = 0;
            for response in responses {
                if response?.is_some() {
                    reachable += 1;
                }
            }
            println!(
                "{}",
                json!({"queries":queries,"reachable":reachable,"threads":config.execution.threads,"release_build":!cfg!(debug_assertions),"load_seconds":load_seconds,"routing_seconds":elapsed,"queries_per_second":queries as f64/elapsed,"milliseconds_per_query_amortized":elapsed*1000.0/queries as f64})
            );
        }
    }
    Ok(())
}
