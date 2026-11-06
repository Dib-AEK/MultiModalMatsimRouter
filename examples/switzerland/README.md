<!-- Documents reproducible private-timetable coordinate benchmarks with eight concurrent workers. -->
# Switzerland coordinate routing

`local/` is ignored by Git. It contains private scenario inputs and query records, not redistributable example data.
`prepare.py --schedule PATH --config PATH` copies the schedule and original config there and rewrites only the
schedule path in a routing-only config. No cluster reads occur during measured routing. The full MATSim scenario
is not copied; this example loads only the timetable and routing parameters.

Build the C ABI library with `cargo build --release --locked -p mmrouter-ffi`. Before replacing an older build,
save its library under `local/baseline.dll` (Windows) or an equivalent Linux filename. Supply a JSON query array
containing `origin: [x,y]`, `destination: [x,y]`, and `departure: "HH:MM:SS"` for every request.

```text
python examples/switzerland/compare.py --config examples/switzerland/local/config.xml --queries examples/switzerland/local/queries.json --baseline examples/switzerland/local/baseline.dll --candidate target/release/multimodal_router.dll --output examples/switzerland/local/comparison.json
```

The default is eight workers and three alternating-order passes per build. Each build owns a shared immutable
engine; workers reuse private search workspaces. Timetable import and preprocessing are excluded from query
timings. Every complete itinerary must match after removing search diagnostic counters; no route results are cached.
Keep other CPU-intensive jobs stopped during timing. This measures native coordinate routing and language-bridge
throughput, not full MATSim replanning or 48-worker cluster performance.

## Local validation (2026-11-06)

Full Swiss timetable: 81,446 imported facilities; 1,000 real trip-coordinate requests sampled from
iteration 0, eight concurrent callers, three alternating-order passes. Private source copies live
in `local/`. Host: Windows, Ryzen 7 PRO 6850U, 8 physical / 16 logical CPUs; release build.

| Build | Seconds, three passes | Median |
|---|---|---|
|Previous optimized router|8.281, 7.688, 7.832|7.832|
|Compact frontiers + heap root replacement + memoized bounds + shared coordinate index|7.770, 7.813, 7.922|7.813|

All 1,000 full itineraries matched in every pass, excluding search diagnostic counters.
These timings are effectively equal; do not claim a native throughput improvement. The initial
compact-frontier version without bound memoization had medians 8.068 / 8.205 seconds (baseline/candidate).
The retained layout reduces dense frontier metadata from one Vec header per stop/trip (24 bytes on
64-bit hosts) to a four-byte slot index plus vectors for active rows. The bound memo adds private
per-stop state. Overall process memory was not measured, so this is not a claim of a measured RSS reduction.

The Python/C ABI path already constructed coordinate candidates in Rust before this change. These
timings therefore do not measure the Java-to-Rust migration benefit. The MATSim adapter now makes
one native call per coordinate request, including all radius attempts, and only reconstructs Java
plan elements afterwards. Its exact endpoint protocol remains available and is tested against the
new coordinate entry point. A legacy endpoint fallback supports older released libraries; startup
logs explicitly identify native coordinate routing versus legacy Java endpoints.

NUMA-local timetable replication and per-worker handle registration were not introduced: the current
hardware/test does not establish a benefit, and retaining extra router references must respect close
semantics. There is no new inner routing pool. No 48-worker or full MATSim speedup is claimed.

## MATSim integration measurement (2026-11-06)

`measure_matsim.py` compiles the example Java harness against the eqasim standalone jar. It takes
`--eqasim-jar`, `--config`, `--network`, `--schedule`, `--queries`, `--library`, and `--output`.
Use local copies of all three MATSim input files. This test uses `-Xmx10g` and exactly eight workers.
The jar must include the coordinate JNI adapter; the library must implement its coordinate capability.

Both variants share the same loaded network, schedule, effective Eqasim/Raptor parameters, native
engine and worker pool. The older path selects stops and expands radii in Java and sends explicit
endpoints; the newer path sends coordinates and performs that work in Rust. Each stage warms all
1,000 queries on each variant before three alternating-order measured passes. Route comparison is
outside the measured interval. The second stage includes the actual TransitRouter adapter's MATSim
leg construction, as used by mode choice; it excludes controller replanning, scoring and simulation.
The harness switches the adapter capability branch only while workers are idle. It is a standalone
benchmark, not code to install in a running simulation. No route-result cache is enabled.

Full Swiss snapshot: 80,229 served facilities and 354,949 departures. Timetable loading, network
loading and native preprocessing are excluded from the following timings.

| Stage, 1,000 queries | Java endpoints, median seconds | Rust endpoints, median seconds |
|---|---:|---:|
| Coordinate request through returned payload | 6.045 | 5.938 |
| Including MATSim transit-leg reconstruction | 5.947 | 6.051 |

Every route payload matched exactly in every pass; reconstructed legs also matched modes, times,
route descriptions, links, distances and costs. The complete-path difference overlaps run-to-run
variation; this experiment does **not** demonstrate an end-to-end speedup from migration to Rust.
Do not apply these numbers to 48-worker cluster or whole-iteration performance.

The Swiss config sets walking speed to 1.3 m/s and beeline distance factor to 1.3, yielding the
resolved Raptor beeline speed of 1.0 m/s. The Rust export is exactly 1.0 m/s. Export always uses
`EqasimRaptorUtils.createParameters(...).getBeelineWalkSpeed()`, not a hardcoded walk-mode lookup:
MATSim prefers `transit_walk`, then `non_network_walk`, then `walk`. Stop `accessTime`/`egressTime`
values are exported separately. A regression test uses a non-default effective speed and fractional
stop durations to check the native coordinate output against independently supplied endpoints.

## Further search improvements (2026-11-06)

The selected default now limits only relaxed-bound preparation to 1024 settled stops. The actual
route search stays unrestricted. It also avoids activating future departures that cannot beat an
incumbent under a conservative minimum-rate bound, and removes repeated dominance probes after
preallocation checks. `--config:swissRustRouting.localBoundSettlements 0` restores unlimited bound
preparation. This knob requires a library implementing the new search option.

Paired native experiments improved median query time by approximately 13-14%, with every complete
itinerary identical. The selected budget beat 256, 4096 and regional-only variants by small margins
on the sampled workload; these budget differences are not statistically established. An inline
SmallVec experiment did not help and was discarded, including its dependency.

The complete MATSim transit-leg adapter was then measured with the same loaded Swiss inputs,
eight workers and `-Xmx10g`, in separate JVMs using the before/after native libraries. Each JVM
warmed all 1000 requests, then ran three timed passes. Import/preprocessing and untimed itinerary
validation are excluded. No other builds/tests ran concurrently with measured routing.

| Complete adapter, 1000 queries | Seconds |
|---|---|
| Before, three passes | 7.817, 8.278, 8.060 |
| After, three passes | 7.321, 7.607, 7.907 |
| Before median | 8.060 |
| After median | 7.607 |

This is about 5.6% lower elapsed time for the complete adapter in this local run.
Both libraries produced the same full itinerary digest, including MATSim leg times, route
identifiers, distances and costs. Walking parameters remained unchanged. See
`search_improvements.json` for the numeric report. To repeat this comparison, use
`measure_matsim.py --native-only` once per library, changing only the library and report arguments.
This result does not establish whole-iteration improvements or 48-worker cluster scaling.

## Coordinate heuristics (November 6)

Coordinate routing now has an additional explicit approximation policy; the previous table measures
the exact search improvements alone. Defaults use cheap 250000-work probes, a 2000000-work search
budget, and wider stop searches for waits above 30 minutes or journeys above three hours, capped at
2000 m. A truncated result is feasible but may not minimize cost. Unresolved searches fall back
to exact routing. Stop-to-stop and explicit-endpoint weighted APIs remain exact.

To compare exact legacy coordinates with the new defaults on a single native build:

```sh
python examples/switzerland/compare_policies.py \
  --config examples/switzerland/local/cluster_benchmark_config.xml \
  --queries examples/switzerland/local/cluster-requests.json \
  --library target/release/libmultimodal_router.so \
  --threads 8 --repeats 3 --output examples/switzerland/local/policy-comparison.json
```

On Windows use `target/release/multimodal_router.dll`. Inputs remain private and ignored.
`cluster-requests.json` is the request export of the Java cluster comparison, not the earlier
`queries.json` sample. Numeric departure seconds and HH:MM:SS strings are both accepted.
The earlier `compare.py` requires identical itineraries and is intended only for builds with the
same search policy. Use `compare_policies.py` when deliberately changing the coordinate policy.

The local eight-worker cluster-sample comparison measured median 7.929 seconds for exact legacy
coordinates versus 2.272 seconds with the default policy, excluding import and preprocessing.
Among 867 paired PT results, 55 costs improved, 812 stayed equal and none increased in this sample.
Eleven requests gained a PT alternative. Two routes remained budget-limited; their costs equalled
the exact legacy results. This is sample evidence, not a guaranteed approximation bound.
The worst prior request dropped from 5.449 to 0.278 seconds and from an 8h11 journey to 2h17.
See `docs/interop.md` for all policy fields and the exact legacy settings.
