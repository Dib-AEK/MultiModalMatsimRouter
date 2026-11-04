<!-- Explains how to reproduce and interpret the real Geneva cross-language routing example. -->
# Geneva: 1,000 requests through SwissRailRaptor and Rust

Run from the project root after installing Rust, JDK 25, and Python 3 with NumPy, pandas,
and Matplotlib. Supply your scenario's config and a MATSim 2026.0/eqasim fat jar:

```text
python examples/geneva/run_benchmark.py --config C:/Users/dabdelkader/Desktop/work/codes/cmdp_scenario/geneva_config.xml --matsim-jar C:/Users/dabdelkader/Desktop/work/codes/eqasim-java/switzerland/target/switzerland.jar
```

On another machine replace both paths. The same Python command works on Windows and Linux.
The driver locates Cargo on PATH or under the standard user `.cargo/bin` directory. No Java
classes or changes are installed into MATSim. Rust builds as a Cargo example; Java compiles
into the results directory against the supplied jar.

Defaults: **1,000 unique shared requests**, four workers, seed 42, 2,000 warmup requests per
execution mode. Both serial and parallel phases measure the same 1,000 requests, so each
engine executes 2,000 timed calls plus warmups. Departures are uniform from 05:00 to 22:00;
origins and destinations are distinct randomly selected served stop IDs. Requests are stop pairs,
not sampled population trips or coordinate queries. Override `--queries`, `--threads`, `--seed`,
`--warmup` or `--output` as needed.

## Controlled protocol

This first comparison isolates the routing cores. It does **not** reproduce all mode-choice
routing settings in the original config:

- Same timetable, requests, service times and same-stop transfer buffer for both engines.
- No walking or transfers between distinct stop IDs, even if their coordinates coincide.
- No coordinate access/egress stop search and no direct-walk fallback.
- Uniform time objective; no mode-specific preferences, fare costs or result caching.
- Rust bound: 21 boardings. SwissRailRaptor retains its built-in round limit and the rule
  that stops after a limited number of additional rounds following the first arrival.
- Java runs with **-Xmx10g**; Rust uses an optimized release build.
- Search and internal journey reconstruction are timed; JSON, plots and the common distance
  calculation are outside timing. Worker/executor scheduling is included in parallel wall time.

The Java harness sits in Raptor's package to configure the benchmark's transfer graph before
constructing any router: schedule-specific transfer overrides are removed in memory, identical
stop buffers installed, and cross-ID transfer ranges compacted out. The Raptor search algorithm
is unchanged. This adapter targets MATSim 2026.0 internals and must be reviewed on upgrades.

This deliberately narrow common protocol is useful for the first implementation. A later
walking-enabled comparison needs matching transfer-graph and access/egress semantics; it must
not silently compare different passenger preferences. Most pairs can be unreachable when
interstop walking is disabled. Inspect reachable-only latency, not just aggregate throughput.

## Input validation and normalization

The source config/network/schedule are **never edited**. The driver creates
`results/benchmark_schedule.xml.gz` for both engines. It explicitly excludes routes whose
operative offsets are missing, negative or non-monotonic, and records route IDs/reasons/counts
in `metadata.json`. Unused first-arrival/final-departure fields are normalized to their paired
value; no connection departure or arrival is altered by those boundary normalizations.

The supplied Geneva source contains a malformed interior negative offset. Using the exact
uncorrected source would prevent the strict Rust reader from producing a timetable. The
report therefore states exclusions and hashes both source and derived input. Do not present
these results as validation of malformed timetables or as an unmodified full-scenario run.

## Output and figures

Open [results/REPORT.md](results/REPORT.md) after running. The results directory contains:

| File | Purpose |
|---|---|
| `requests.jsonl` | Identical input requests for both engines |
| `router.toml` | Complete generated Rust benchmark config |
| `rust_results.jsonl`, `swiss_results.jsonl` | Per-query core latency and complete itineraries |
| `comparison.csv` | Travel times, transfers, reachability, common distances and query times |
| `metadata.json` | Input paths/hashes, normalization, protocol, seed and machine information |
| `rust_metadata.json`, `swiss_metadata.json` | Loading and serial/parallel totals; Rust ID mappings |
| `summary.json` | Numerical comparison and, when present, initial/optimized Rust equivalence |
| `figures/*.png`, `figures/*.svg` | Eight shareable charts, including reachable-only query latency |
| `baseline_before_dominance/`, `after_dominance/` | Preserved measurements from development, when available |

Distances sum straight segments between successive timetable stops on each selected transit
service. They are **not network-link distances** and not origin-destination chords. For repeated
stop sequences, vehicle timing selects the segment; unresolved ties use the shortest matching
segment and are counted in the CSV. The same Python function computes both engines' distances. Raptor raw `departure` is its
boarding/waiting timestamp; `vehicle_departure` is the scheduled departure at the boarding
stop, not the service origin. The analysis normalizes this distinction before matching
service segments and validating transfer times.

To regenerate charts without routing again, rerun the same command with `--plot-only`.
Raw scenario copies, Java classes and build products are generated artifacts. The source
scenario is supplied separately; it is not embedded in the project.

## Meaning of differences

Earlier Rust arrivals do not automatically indicate an error: the baseline CSA searches all
allowed boarding layers, whereas Raptor has its normal additional-round termination rule.
Likewise equal-arrival routes may have different transfers or distances due to tie handling.
Inspect the raw itineraries rather than requiring identical links or claiming equivalence from
mean travel time. Both parallel runs verify arrival/boarding consistency with their own serial
results. Optimized Rust output is also compared against the preserved initial Rust results.

These are measured example results, not a JMH/Criterion study, a nationwide speedup guarantee,
or a prediction of total MATSim replanning time. Startup work differs: Java loads the network
as well as the schedule, while Rust does not require the network for this protocol.

## Observed trips with Eqasim penalties

Run from the project root (one command, Windows or Linux; adjust paths):

```text
python examples/geneva/run_trips.py --config ../cmdp_scenario/geneva_config.xml --trips ../cmdp_scenario/simulation_output/output_trips.csv.gz --matsim-jar ../eqasim-java/switzerland/target/switzerland.jar --queries 1000 --threads 12 --warmup 200 --output examples/geneva/trip_results_12
```

This separate example preserves the earlier controlled benchmark. It samples observed PT trips
uniformly with seed 42, using their original coordinates, departure times and trip IDs. Use
`--trip-mode all` to sample all modes, or another observed main-mode name. Zero-distance and invalid
coordinate/time rows are excluded. Use `--output PATH` for a separate run or `--plot-only` to
regenerate analysis. Inputs remain unchanged; the derived schedule retains valid first-stop dwell
and normalizes only missing first-arrival fields and unused final-departure fields. Invalid operative
chronology is explicitly excluded and recorded.

Rust imports effective coefficients directly from the MATSim config through `routing.matsim_config`.
The benchmark independently reads the config in Python and supplies the same coefficients to Raptor,
then checks both imports agree. This follows EqasimRaptorUtils: perTransfer_u replaces the generic
fixed base; mode travel times, walk and wait coefficients are negative utilities converted to positive
disutilities. The full directional mode-transfer matrix and transfer min/max bounds are preserved.
For this Geneva config: rail 1.0, bus/other 1.584, tram/subway 1.122, walk 1.97, waiting 1.72 units/hour.
The mode-pair penalties are already utility units and must not be divided by 3600.

Both engines receive identical coordinate-derived access/egress candidates and whole-second walking
durations, with the configured speed and beeline factor. Raptor builds transfers from the original
schedule and physical config, then exports their stop-pair durations for Rust. Rust supports one direct
transfer between boardings and an optional terminal walk. Route-pair pruning and search stopping
heuristics still differ. The core benchmark does not compare pure walking or retry failed searches at
larger radii. Transfer-walk margin belongs to MATSim leg conversion, outside timed core routing.

Important reference-build caveat: the bundled MATSim 2026 mode-specific transfer calculator can be
invoked both at transfer handling and on the next ride, with a cached transfer provider. This can
make its native cost differ from one configured penalty per actual vehicle change. We preserve the
reference jar, save both native totals, and independently rescore each returned itinerary under the
same explicit additive objective. The report distinguishes route-quality agreement from native-cost
agreement. This example does not assert bitwise equality or implement all SwissRailRaptor internals.

Outputs are in [trip_results_12/REPORT.md](trip_results_12/REPORT.md): raw requests/results, config/profile,
transfer graph, comparison CSV, validation summary and eight PNG/SVG figures (cost, travel time,
distance, transfers, walking, waiting, latency and throughput). Serial and parallel routes are checked
for consistency. Java uses `-Xmx10g`; Rust uses release optimizations. Loading, input/output, candidate
selection and validation are excluded from routing timings.

The current optimization comparison uses 12 workers for both the previous Rust executable and
the optimized engine. All 1000 routes retain the same legs, arrival times, boarding counts and
costs. `--baseline PATH` validates identical requests and compares an earlier run; a mismatch
in reachability or cost stops analysis. The report includes before/after timing, work counters,
index memory and a ninth figure. `routing.search` exposes all four exact accelerations independently.
Destination-bound calculation is included in every timed query. Both benchmark drivers now default
to 12 workers; pass `--threads` explicitly when reproducing older four-worker reports.

## Parallel scaling

[scaling_results/REPORT.md](scaling_results/REPORT.md) compares adaptive Rayon, the dynamic queue
and priorities from a preceding batch at 1/2/4/8/12/16 workers. The same route fingerprint must
match at every point. Profiling is separate from median batch timings and there is no result
cache. [The parallelism guide](../../docs/parallelism.md) contains commands, API examples and
the distinction between worker wall occupancy and CPU utilization.
