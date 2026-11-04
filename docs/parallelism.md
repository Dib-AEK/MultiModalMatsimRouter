<!-- Explains batch scheduling, repeated-work hints, profiling, scaling limits and reproducible measurements. -->
# Parallel routing

An engine owns one bounded Rayon pool. Timetable and cost indices are shared read-only;
each worker reuses its own routing workspace. A single query stays sequential. Increasing
workers improves throughput across independent queries, not the latency of one isolated route.

## Scheduling

`execution.scheduling` accepts:

| Setting | Behavior |
|---|---|
| `auto` (default) | Adaptive Rayon for cold work; learned priorities for familiar weighted batches |
| `dynamic` | One persistent loop per worker, claiming one query at a time |
| `rayon` | Original adaptive splitting |
| `fine_grained` | Rayon work stealing with one-query leaves |
| `estimated_work` | Dynamic claiming, larger geometry/timetable work estimates first |

Dynamic batches use [Rayon's broadcast primitive](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html#method.broadcast)
once, rather than submitting a separate job for each query. A relaxed atomic counter assigns
unique work indices. Per-input `OnceLock` slots publish results safely and preserve input order,
including missing routes and errors. There is no shared routing lock or shared mutable workspace.
The temporary slots and output gather use additional bounded batch memory; they contain no
duplicate deep copies of itineraries. Empty batches return immediately.

Arrival-only queries can be extremely short, especially component rejections. Auto retains their
adaptive scheduling to avoid fine-grained dispatch overhead. Geometry work estimates are advisory:
they were not consistently helpful on Geneva, so priority ordering is not forced by default.

## Repeated queries

For a later corresponding batch, actual previous costs are better scheduling hints than distance:

```rust
let first = engine.weighted_batch_profiled(&queries, None)?;
// Consume first.results normally; these routes are not discarded or cached by the engine.
let later = engine.weighted_batch_with_estimates(
    &queries,
    &first.metrics.query_nanoseconds,
)?;
```

Hints must align with the new query positions; callers selecting/reordering trips must align
their hint vector too. Incorrect estimates affect execution order only, never route validity.
Length mismatches are errors. No person attributes or route-result cache are added. Auto mode keeps at most execution.work_estimate_capacity fingerprint/score entries (default 65536), grown lazily with FIFO eviction and smoothed costs. Set the capacity to zero to disable history/timing; increase it for large repeated populations. Locks cover only a batch-level hint snapshot/update, never query execution.
The dynamic priority queue actually claims expensive jobs first; sorting a work-stealing iterator
alone does not guarantee that its heaviest jobs begin on all workers.

Auto weighted batches collect lightweight clocks for bounded cost hints. Explicit policies avoid those clocks; detailed worker profiling is opt-in. Profiled batches expose
query times in input order and per-worker counts, cumulative query wall time and longest query.
Wall time includes OS descheduling, so these metrics must not be called CPU utilization. A large
fraction of worker wall time inside queries can coexist with sublinear throughput if each query
gets slower under contention. Use hardware profiling to distinguish clocks, cache, memory and SMT.

## Scaling and application integration

Provide enough independent work: a small batch can be bounded by its longest query. Larger
`execution.batch_size` values (e.g. 1024 or 4096) can reduce end-of-batch idle time, at the cost of
more buffered input/output. The CLI still honors the configured bound; the library's slice API
routes the complete supplied slice. Creating one engine/pool per caller can oversubscribe CPUs.
Share an engine and coordinate its thread budget with Java replanning or other application pools.

Logical threads are not independent physical cores. Shared execution resources, memory bandwidth,
cache and CPU power/frequency limits prevent a general promise of linear gains. Larger NUMA
servers must be measured separately; this implementation does not claim NUMA placement or affinity
tuning based on a single eight-core laptop.

## Measurement

The [scaling report](../examples/geneva/scaling_results/REPORT.md) uses the same 1000 observed
Geneva PT trips, 1/2/4/8/12/16 workers, repeated timed batches and full-batch warmups. Every run
checks the same ordered fingerprint of legs, arrival, boardings, cost and endpoint choice.
Prior-batch priorities use measurements from preceding parallel warmup work, not a serial oracle.
There is no route-result caching. Profiling is a separate pass, excluded from timing medians.

```text
cargo build --release --example geneva-scaling
target/release/examples/geneva-scaling.exe examples/geneva/trip_results_12/router.toml examples/geneva/trip_results_12/requests.jsonl examples/geneva/trip_results_12/transfers.json examples/geneva/scaling_results/scaling_dynamic.json 3 1,2,4,8,12,16 2 dynamic
python examples/geneva/plot_scaling.py examples/geneva/scaling_results
```

On Linux omit `.exe`. The final optional argument can be `rayon`, `fine_grained`, `dynamic`,
`estimated_work`, `auto` or `learned`; the last learns priorities from preceding warmup batches. Generate
`scaling_rayon.json`, `scaling_dynamic.json`, `scaling_learned.json` and optionally `scaling_auto.json` before running the plotter.
The plotter reports throughput, normalized speedup, worker query-wall occupancy and query-wall
inflation. Re-run on the intended server rather than extrapolating to 24/48 cores.

## Optional allocator experiment

Application binaries can be built with `--features fast-allocator` to use
[mimalloc](https://github.com/microsoft/mimalloc). It showed no consistent improvement in the
initial twelve-worker trials, so the system allocator remains the default. This is not a claim
that either allocator wins on every OS/workload. The feature is opt-in and the library never
installs a global allocator; embedding applications retain ownership of that decision.
