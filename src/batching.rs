//! Schedules independent queries with fine-grained stealing, optional work priorities and ordered results.
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheduling {
    /// Adaptive Rayon for cold work; learned priorities for sufficiently familiar weighted batches.
    #[default]
    Auto,
    /// Original adaptive Rayon splitting, retained for comparisons.
    Rayon,
    /// Each leaf contains one query so expensive chunks cannot strand idle workers.
    FineGrained,
    /// Persistent worker loops claim one query at a time from a shared counter.
    Dynamic,
    /// Dynamic claiming with larger work estimates first; results retain input order.
    EstimatedWork,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkerMetrics {
    pub queries: usize,
    /// Sum of query wall times, including any OS descheduling; this is not CPU time.
    pub query_seconds: f64,
    pub longest_query_seconds: f64,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatchMetrics {
    pub scheduling: Scheduling,
    pub total_seconds: f64,
    pub workers: Vec<WorkerMetrics>,
    /// Input order; advisory estimates for a later batch, never cached routing results.
    pub query_nanoseconds: Vec<u64>,
    pub advisory_history_hits: usize,
    pub advisory_history_entries: usize,
}
pub struct ProfiledBatch<T> {
    pub results: Vec<T>,
    pub metrics: BatchMetrics,
}

fn order(n: usize, policy: Scheduling, estimates: &[u64]) -> Vec<usize> {
    let mut order: Vec<_> = (0..n).collect();
    if policy == Scheduling::EstimatedWork {
        order.sort_unstable_by(|&a, &b| estimates[b].cmp(&estimates[a]).then_with(|| a.cmp(&b)));
    }
    order
}
fn dispatch<T: Send + Sync, F: Fn(usize) -> T + Sync>(
    pool: &rayon::ThreadPool,
    policy: Scheduling,
    order: &[usize],
    run: F,
) -> Vec<T> {
    if order.is_empty() {
        return Vec::new();
    }
    if order.len() == 1 {
        return pool.install(|| vec![run(order[0])]);
    }
    if matches!(policy, Scheduling::Dynamic | Scheduling::EstimatedWork) {
        let next = AtomicUsize::new(0);
        let slots: Vec<OnceLock<T>> = (0..order.len()).map(|_| OnceLock::new()).collect();
        // One loop per worker rather than one task/Future per query. Relaxed ordering is enough
        // for unique index assignment: Rayon publishes immutable inputs; OnceLock publishes outputs.
        pool.broadcast(|_| {
            loop {
                let position = next.fetch_add(1, Ordering::Relaxed);
                if position >= order.len() {
                    break;
                }
                let index = order[position];
                assert!(
                    slots[index].set(run(index)).is_ok(),
                    "each result slot is assigned once"
                );
            }
        });
        return slots
            .into_iter()
            .map(|s| s.into_inner().expect("every scheduled query completed"))
            .collect();
    }
    pool.install(|| {
        if matches!(policy, Scheduling::Rayon | Scheduling::Auto) {
            order.par_iter().map(|&i| run(i)).collect()
        } else {
            order.par_iter().with_max_len(1).map(|&i| run(i)).collect()
        }
    })
}
pub(crate) fn execute<T: Send + Sync, F: Fn(usize) -> T + Sync>(
    pool: &rayon::ThreadPool,
    n: usize,
    policy: Scheduling,
    estimates: &[u64],
    run: F,
) -> Vec<T> {
    if n == 0 {
        return Vec::new();
    }
    let order = order(n, policy, estimates);
    dispatch(pool, policy, &order, run)
}
pub(crate) fn profile<T: Send + Sync, F: Fn(usize) -> T + Sync>(
    pool: &rayon::ThreadPool,
    n: usize,
    policy: Scheduling,
    estimates: &[u64],
    run: F,
) -> ProfiledBatch<T> {
    let started = Instant::now();
    let order = order(n, policy, estimates);
    let records = dispatch(pool, policy, &order, |i| {
        let started = Instant::now();
        let result = run(i);
        let elapsed = started.elapsed();
        (
            result,
            rayon::current_thread_index().expect("batch executes inside its worker pool"),
            elapsed,
        )
    });
    let mut metrics = BatchMetrics {
        scheduling: policy,
        workers: vec![WorkerMetrics::default(); pool.current_num_threads()],
        ..BatchMetrics::default()
    };
    let mut results = Vec::with_capacity(n);
    for (result, worker, elapsed) in records {
        let w = &mut metrics.workers[worker];
        w.queries += 1;
        w.query_seconds += elapsed.as_secs_f64();
        w.longest_query_seconds = w.longest_query_seconds.max(elapsed.as_secs_f64());
        metrics
            .query_nanoseconds
            .push(elapsed.as_nanos().min(u64::MAX as u128) as u64);
        results.push(result);
    }
    metrics.total_seconds = started.elapsed().as_secs_f64();
    ProfiledBatch { results, metrics }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permutations_preserve_results_and_errors_with_tied_priorities() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(12)
            .build()
            .unwrap();
        for n in [0, 1, 7, 137] {
            let priorities: Vec<_> = (0..n).map(|i| ((i * 37) % 11) as u64).collect();
            for policy in [
                Scheduling::Auto,
                Scheduling::Rayon,
                Scheduling::FineGrained,
                Scheduling::Dynamic,
                Scheduling::EstimatedWork,
            ] {
                let expected: Vec<_> = (0..n)
                    .map(|i| if i % 3 == 0 { Err(i) } else { Ok(i) })
                    .collect();
                let f = |i| if i % 3 == 0 { Err(i) } else { Ok(i) };
                assert_eq!(execute(&pool, n, policy, &priorities, f), expected);
                let measured = profile(&pool, n, policy, &priorities, f);
                assert_eq!(measured.results, expected);
                assert_eq!(measured.metrics.query_nanoseconds.len(), n);
                assert_eq!(
                    measured
                        .metrics
                        .workers
                        .iter()
                        .map(|w| w.queries)
                        .sum::<usize>(),
                    n
                );
            }
        }
    }
}
