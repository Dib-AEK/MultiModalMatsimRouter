<!-- Defines the architecture, correctness contracts, engineering rules, and implementation roadmap. -->
# MultiModalMatsimRouter engineering guide

## Purpose and scope
Build a readable, extensible Rust routing library, beginning with scheduled public transport.
The CLI is a local development tool. The MATSim integration milestone is now authorized.
`java/native` is a separate workspace crate holding JNI and snapshot protocol concerns.
`bridges/router-service` owns the safe shared application protocol and engine registry;
`bridges/router-ffi` owns C pointer lifetimes for Python's dependency-free ctypes client.
`python` and `java` contain standalone clients. Keep either runtime out of the core algorithm.
Java sources are organized into `java/client`, shared JNI under `java/native`, and MATSim facade,
examples and snapshot bridge under `java/matsim`. Client API 2 adds input auto-detection, partial
parameter overrides, asynchronous sessions and lightweight worker views. Loading must never copy
an engine per caller thread. Test close-before-ready, startup failure, mixed batch errors and
MATSim relative paths/coefficients, including explicit zero transfers and stop access attributes.
Never put transport-protocol concerns into the routing algorithm. The core still forbids unsafe code.

## Architecture and dependency direction
- `model`: source-neutral data types, compact timetable, time representation, structural validation.
- `readers`: GTFS directory/ZIP and streaming MATSim XML/gzip. Readers produce `RawTimetable`.
- `preprocess`: normalize, validate, sort connections, construct CSR walking adjacency, optional
  conservative destination-cell reachability flags. No reader-specific types survive this boundary.
- `routing`: exact connection-scan search, immutable timetable, caller-owned reusable workspace,
  journey reconstruction, query statistics. No file I/O or global mutable state.
- `cost` / `weighted`: compiled mode costs and an independent time/cost frontier search by
  boarding round. Preserve previous mode and first boarding time when required by transfer costs.
- `search_index`: immutable sorted transfers, platform departure/mode indices and a relaxed
  reverse cost graph. Shared destination-cell potentials complement sparse, bounded query-local refinement.
- `batching`: bounded batch dispatch, optional work priorities, ordered results and opt-in
  worker diagnostics. It must never change the routing objective or cache query results.
- `spatial`: shared geometric distance and indexed walking-neighbor generation.
- `cache`: versioned, checksummed Postcard representation of the prepared timetable. Validate on read.
- `config`: strict TOML, explicit defaults, paths relative to config location, validation before loading.
- `engine`: public orchestration API; one shared timetable and one workspace per parallel worker.
- `main`: CLI parsing and presentation only. Batch order is preserved.
- `tests`: deterministic correctness, reader and cache fixtures, and differential routing tests.

Keep the routing algorithms in a single library crate; the native bridge is a separate ABI boundary.
Expose small data-oriented APIs, avoiding speculative trait layers and giant utility modules.
New readers must terminate at RawTimetable; new routing algorithms consume prepared timetable data.
A future street router can add its own model and query types without changing transit readers.

## First implementation decisions
1. Connection Scan Algorithm (CSA) is the first exact/reference engine. Objective: earliest arrival
   and, among equal arrivals, fewer vehicle boardings. Return Pareto arrivals by boarding count.
   Pareto output is optional; default queries stop when later departures cannot improve the incumbent.
   It is NOT a port of SwissRailRaptor's arbitrary generalized cost objective.
2. Compact integer stop/trip IDs. All absolute times and durations use unsigned milliseconds since
   service-day midnight, supporting >24-hour services and fractional MATSim offsets. Strings and
   coordinates stay outside the scan's hot loop. Overflow and sentinel collisions must be rejected.
3. Flat departure-sorted connections; walking graph uses compressed sparse rows. Permission flags
   distinguish boarding and alighting. Staying on the same trip does not consume another boarding.
4. Workspace labels and trip-reached arrays reuse allocations and reset touched cells only.
   Dominated off-vehicle labels and ride states are pruned, and only active boarding layers are scanned.
   Walking closure uses a reusable Dijkstra heap. Zero-duration departure batches reach a fixed
   point, avoiding order-dependent missed connections. No unsafe code.
5. Mandatory weak-component checks reject disconnected pairs without scanning connections.
   The binary format includes this index and validates that it cuts no transit or walking edge.
6. Independent requests run in a bounded Rayon pool sharing one immutable timetable. Each worker
   reuses its own workspace. Never parallelize individual label relaxations by default.
7. Optional destination-cell flags are conservative topological reachability, valid at every time;
   they are NOT FLASH-TB's full event-transfer arc flags. They may prune little on connected feeds.
   Do not claim FLASH-TB speedups. Stronger pruning requires a proof for supported objectives.
8. Binary cache avoids repeated CSV/XML parsing and preprocessing; it currently decodes into owned
   arrays, not zero-copy mmap. Never claim mmap/zero-copy without implementing alignment/lifetimes.

## Correctness contracts
- Validate IDs, coordinates, stop references, all trip chronology, pickups/dropoffs and CSR indices.
- GTFS import requires a service date and applies weekly calendars plus date exceptions. Do not
  merge all services indiscriminately. Dates/times are service-local; no wall-clock/DST conversion.
- MATSim offsets are relative to each departure. Missing first arrival/last departure may use the
  paired offset; interior missing offsets are errors. Never repair chronology silently.
- Unsupported GTFS constraints (frequency-based service, demand-responsive pickup, scoped transfer
  rules) must produce actionable errors rather than silently wrong itineraries. Document exact scope.
- Earliest-arrival labels may discard later paths with less walking: do not expose weighted-walking,
  fares, mode-specific costs, accessibility or reliability as supported objectives without new labels.
- No route is `None` and serializes as null, not an exception. Invalid input/query is an error.
- Journey timing, boarding counts and reconstruction must agree; preserve deterministic ties.
- Generated walking links use actual geometry; geographic distance is great-circle, projected
  inputs must use metre coordinates. Do not interpret latitude/longitude as planar metres.

## Engineering and documentation
Each authored source/config/document begins with a short purpose comment. Standard machine formats
that disallow comments and generated Cargo.lock/build products are exceptions. Keep functions focused;
write rustdoc on public APIs and explain non-obvious invariants. Do not duplicate the algorithm in the CLI.
Keep `config.example.toml` exhaustive and update it alongside config defaults. Deny unknown settings.
Use contextual errors identifying the input entity. Avoid catch-all fallbacks and hidden approximation.

## Validation workflow
Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and release CLI
smoke runs. Add an independent tiny-network reference solver for differential checks, including
zero-time transfers, overnight times, missed departures, walking chains and pickup restrictions.
Benchmark warm queries and batch throughput separately from import/preprocessing and reconstruction.
Report hardware, dataset, build profile and query count. Do not infer whole-simulation speedup from
search-loop timing. Do not add fake test results or guessed benchmark numbers.

## Roadmap
- Milestone 1: validated CSA core, two text readers, binary cache, config/CLI, differential tests.
- Milestone 2: representative Swiss timetable benchmark, allocation/memory measurements, improve
  route/event indexing based on profiling; compare CSA and RAPTOR or trip-based alternatives.
- Milestone 3: evaluate FLASH-TB event-transfer preprocessing, arbitrary access/egress stop sets,
  expanded Pareto criteria and reusable preprocessing fingerprints.
- Milestone 4: stable binary API/optional local server and MATSim adapter; keep the core reusable.

Keep this guide current as implementation decisions and supported features change.

## Binary distributions
`tools/package_release.py` stages native libraries without changing the source Python package,
builds platform-tagged ctypes wheels, tests a clean installation outside the checkout, and packages
host-specific Java jars, native libraries, CLI, examples and checksums. Never label a binary wheel
`any` or claim manylinux compatibility without a suitable build/audit. The release workflow builds
and tests each OS/architecture before publishing a tag; no compiler is required by release users.

## Weighted routing and observed-trip comparisons
The optional generalized-cost engine supports distinct walking, waiting and in-vehicle rates,
directional mode-pair transfer penalties or duration-dependent transfer penalties. All effective
increments are nonnegative; unsupported utility/capacity combinations must fail validation.
It uses conservative time/cost dominance, not the earliest-arrival engine's pruning rules.
Vehicle arrival at each boarding stop is retained: boarding during dwell is in-vehicle time.
Prepared cache format is now MMRTR003; older caches must be rebuilt.
Eqasim config import must follow EqasimRaptorUtils overrides and actual Eqasim default values.
Never assume imported coefficients make the algorithms or their native costs identical.
`examples/geneva/run_trips.py` samples real trips, shares candidate stops and transfer durations,
and independently rescores both engines. Preserve the older controlled benchmark results.
The reference MATSim 2026 mode-specific transfer calculator is invoked at transfer handling and
again on the following ride. Record native totals and independently scored totals separately;
do not silently patch the reference jar or port accidental provider-state behavior into Rust.

## Current milestone status
The initial Rust core, GTFS/MATSim readers, binary cache, config and CLI are implemented.
The Geneva comparison in `examples/geneva` keeps benchmark-only Java code out of the core.
Its no-interstop-walking protocol, source exclusions and timing scope must remain explicit.
Do not generalize results dominated by unreachable queries to connected walking-enabled networks.

## Weighted acceleration milestone
The default weighted engine now uses shared regional and query-local destination lower bounds, cross-round dominance, sorted
transfer early termination and platform boarding indices. See docs/weighted_optimizations.md
for invariants and floating-point handling. Preserve later cheaper/overtaking alternatives and
zero-time/zero-cost paths; do not use earliest-arrival-only dominance or truncate candidate lists.
The shared indices initialize once, eagerly for weighted engines and lazily for explicit weighted
queries on arrival-only engines. They do not change the MMRTR003 timetable cache format.
Run benchmark pools and Rust test workers with 12 threads for this development workflow.
Keep the pre-change 12-thread reference and all 1000 matching routes, including legs, alongside
new results. Intermediate results are preserved under trip_results_12/stage_goal_history.
FLASH-TB flags, landmarks, full station departure envelopes and transfer hierarchies remain research
extensions; none is silently approximated by the current exact indices.

## Parallel scaling workflow
Auto scheduling uses adaptive Rayon for cold work, then a dynamic priority queue for familiar
weighted batches using bounded fingerprint/cost hints. Arrival-only queries retain adaptive Rayon.
Hints affect priority only; auto weighted batches collect lightweight clocks, with detailed worker
profiling remaining opt-in. work_estimate_capacity=0 disables automatic history/timing. Preserve result/error positions and verify
concurrent calls on the same engine. Never call worker query-wall occupancy CPU utilization.
The normal development/test budget remains 12 threads; scaling experiments intentionally sweep
1, 2, 4, 8, 12 and 16 on the current 8-core/16-logical-thread machine. No linear-scaling guarantee.
Use docs/parallelism.md and examples/geneva/scaling_results for reproducibility. Do not run other
CPU-heavy validation/builds concurrently with timed scaling passes. The optional fast-allocator
feature is installed only in application binaries, never in the library, and remains opt-in.

## MATSim native integration
The Java adapter lives in eqasim-java's `ch_cmdp.rustRouting` package. It exports the loaded schedule
after scenario adaptations, asynchronously, using the versioned MMJNI001 binary snapshot. All costs
come from the actual Eqasim/Raptor parameters after command overrides. The native library shares
immutable timetable/cost indices and uses one reusable thread-local workspace per Java routing worker.
Do not add an inner query pool: MATSim already parallelizes queries. JNI uses safe region copies,
panic containment and Arc-owned handles, without raw pointer ownership. No-path returns an empty array.
No production input repair or route exclusion is allowed; unused terminal departure offsets can be
normalized because they are never used. Unsupported chained/capacity/intermodal/range behavior fails
explicitly. Bound maxBoardings/maxDuration are documented configuration limits. The core's intended
transfer objective is retained, without emulating the reference's duplicate mode-penalty charging.

## Large-timetable locality
Weighted queries index pickup events by stop and merge activated trip continuations in the original
connection-index order. Sparse rows clear only touched stops/trips; retain deterministic tie order.
Balanced geographic cells supply shared f32 potentials rounded downward. They are lower bounds,
never allowed-region masks. Searches may leave and re-enter cells, including on intercity services.
The temporal negative check preserves pickup/dropoff, walking times/direction and transfer buffers,
but ignores the boarding-count limit; only a negative answer may prune. Preserve zero-time batch closure. Keep full scans/exact reverse
bounds available through SearchOptions for differential validation. Cell indices rebuild with the
cost model, not with the timetable cache. Default 128 cells cost about 512 bytes per stop, shared.
Lazy initialization inside Rayon workers must not yield while holding a TLS workspace RefCell borrow.
Use tools/benchmark_coordinates.py for real-feed concurrency tests. Separate preprocessing, query
worker time and elapsed throughput; compare full itineraries while excluding diagnostic counters.

Network-aware partitions and transfer suffix blocks are independent SearchOptions. Partition
refinement may change cell shape, never allowed journeys. Transfer suffix pruning must combine
only admissible minima; retain mode-dependent lower bounds only for static costs. Do not call
these bounds T-REX hard transfer ranks. Validate all acceleration combinations and dense transfer
lists, including zero-duration walks. Keep the portable JNI protocol unchanged.
