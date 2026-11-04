//! Measures warm batch scaling and verifies identical routes across worker counts without query-result caching.
#[path = "../../src/bin_allocator.rs"]
mod bin_allocator;
mod support;
use anyhow::{Result, ensure};
use multimodal_matsim_router::{config::Config, weighted::ResultJourney};
use serde_json::json;
use std::{fs::File, path::Path, time::Instant};
fn fingerprint(results: Vec<Result<Option<ResultJourney>>>) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    for result in results {
        let j = result?;
        hash.update(&serde_json::to_vec(&j.map(|j| {
            (
                j.journey.arrival,
                j.journey.boardings,
                j.journey.generalized_cost,
                j.journey.legs,
                j.access,
                j.egress,
            )
        }))?);
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 8 || args.len() == 9,
        "usage: geneva-scaling CONFIG REQUESTS TRANSFERS OUTPUT_JSON REPEATS THREADS_CSV WARMUP_BATCHES [auto|rayon|fine_grained|dynamic|estimated_work|learned]"
    );
    let mut base = Config::load(Path::new(&args[1]))?;
    let policy = args.get(8).map_or("auto", String::as_str);
    let learned = policy == "learned";
    base.execution.scheduling =
        serde_json::from_value(json!(if learned { "estimated_work" } else { policy }))?;
    let repeats: usize = args[5].parse()?;
    let warmup: usize = args[7].parse()?;
    ensure!(
        repeats > 0 && warmup > 0,
        "positive repeats and warmup batches required"
    );
    let counts = args[6]
        .split(',')
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(!counts.is_empty(), "no worker counts");
    let mut reference = None;
    let mut measurements = Vec::new();
    for threads in counts {
        let mut config = base.clone();
        config.execution.threads = threads;
        let load = Instant::now();
        let (engine, queries) = support::load(&config, Path::new(&args[2]), Path::new(&args[3]))?;
        let load_seconds = load.elapsed().as_secs_f64();
        ensure!(!queries.is_empty(), "no queries");
        let mut estimates = None;
        for _ in 0..warmup {
            let batch = engine.weighted_batch_profiled(&queries, estimates.as_deref())?;
            let hash = fingerprint(batch.results)?;
            if learned {
                estimates = Some(batch.metrics.query_nanoseconds);
            }
            if let Some(r) = &reference {
                ensure!(&hash == r, "route changed with worker count");
            } else {
                reference = Some(hash);
            }
        }
        let mut samples = Vec::new();
        for _ in 0..repeats {
            let start = Instant::now();
            let results = if let Some(estimates) = &estimates {
                engine.weighted_batch_with_estimates(&queries, estimates)?
            } else {
                engine.weighted_batch(&queries)
            };
            samples.push(start.elapsed().as_secs_f64());
            ensure!(
                Some(fingerprint(results)?) == reference,
                "route changed between batches"
            );
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let profiled = engine.weighted_batch_profiled(&queries, estimates.as_deref())?;
        ensure!(
            Some(fingerprint(profiled.results)?) == reference,
            "profiled routes changed"
        );
        let row = json!({"threads":threads,"queries":queries.len(),"median_seconds":median,"samples_seconds":samples,
            "load_seconds":load_seconds,"warmup_batches":warmup,"route_fingerprint":reference,"profile":profiled.metrics});
        println!(
            "{}",
            json!({"threads":threads,"median_seconds":median,"scheduling":policy,
            "profile_query_seconds":profiled.metrics.workers.iter().map(|w|w.query_seconds).sum::<f64>(),
            "profile_wall_seconds":profiled.metrics.total_seconds})
        );
        measurements.push(row);
    }
    serde_json::to_writer_pretty(
        File::create(&args[4])?,
        &json!({"measurements":measurements,"repeats":repeats,"warmup_batches":warmup,"scheduling":policy,
            "allocator":if cfg!(feature="fast-allocator") {"mimalloc"} else {"system"},
        "note":"Each pool loads identical inputs and warms full batches; routing and reconstruction timed; checking/drop outside timing; no result cache."}),
    )?;
    Ok(())
}
