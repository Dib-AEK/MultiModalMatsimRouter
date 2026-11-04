<!-- Introduces the router, architecture, supported semantics, configuration, and reproducible commands. -->
# MultiModalMatsimRouter

Public-transport routing in Rust, usable from **Python**, **Java**, **MATSim**, or the command line.
The project provides scheduled routing with walking, waiting, transfers and configurable mode costs,
using an immutable timetable shared across concurrent routing workers.

## Install from GitHub

Prerequisites: stable Rust/Cargo, Python **3.11+** for the build helper, and JDK **17+** for the
standalone Java client. MATSim integration uses JDK **25**. Windows also needs MSVC C++ Build Tools;
Linux/macOS need their standard native build toolchain. Build on the platform where you will run it.

```bash
git clone https://github.com/Dib-AEK/MultiModalMatsimRouter.git MultiModalRouter
cd MultiModalRouter
python tools/build.py --test
python -m pip install ./python
```

This builds both native libraries and `target/java/mmrouter-java-0.1.0.jar`, then tests the Rust,
Python and Java interfaces. Deployment needs the matching compiled library and language client;
the simulation jar can bundle its library. No Rust toolchain is required at runtime.

## Python example

The included example timetable works immediately, without downloading a scenario:

```python
from multimodal_router import Router

with Router("examples/interop/router.toml",
            library="target/release/multimodal_router.dll") as router:
    print(router.route("A", "B", "07:59:00"))
    print(router.route_many([
        {"from": "A", "to": "B", "departure": "07:59:00"},
        {"from": "A", "to": "C", "departure": "07:59:00"},
    ]))
```

Use `target/release/libmultimodal_router.so` on Linux or `libmultimodal_router.dylib` on macOS.
Single routes return a journey or `None`; batch results preserve order and per-query errors.
Journey times are milliseconds since service-day midnight. Batch workers are bounded by the
TOML `execution.threads` setting. Fetch `metadata(include_entities=True)` to map stop/trip indices
to original IDs. [`docs/interop.md`](docs/interop.md) describes the complete API and lifecycle.

## Java example

Add `target/java/mmrouter-java-0.1.0.jar` to your classpath:

```java
import java.nio.file.Path;
import org.multimodalmatsim.router.NativeRouter;

try (var router = new NativeRouter(Path.of("examples/interop/router.toml"),
                                  Path.of("target/release/matsim_pt_router.dll"))) {
    System.out.println(router.routeJson("A", "B", "07:59:00"));
}
```

Linux/macOS use `libmatsim_pt_router.so` / `libmatsim_pt_router.dylib`. This client has no MATSim
dependency; responses are JSON envelopes. See the runnable
[`JavaExample.java`](examples/interop/JavaExample.java) and [integration documentation](docs/interop.md).

## MATSim / eqasim-java

Clone this project beside your compatible eqasim-java checkout, using the directory name
`MultiModalRouter`. In eqasim-java, build and run:

```bash
mvn -pl switzerland -am package -Pstandalone,rust-router -DskipTests
java -Xmx10g -jar switzerland/target/switzerland.jar --config-path scenario/config.xml --useRustRouter true
```

The Maven profile bundles the host's JNI library. Override
`-Drust.router.project=/absolute/path/to/MultiModalRouter` for another checkout location.
The eqasim-java checkout must include the `ch_cmdp.rustRouting` adapter; an arbitrary upstream
release may not contain it yet. MATSim workers use direct numeric calls, shared timetable data
and reusable workspaces. Native preprocessing runs alongside controller setup, and PT itinerary
caching can wrap the backend. [Supported behavior and restrictions](docs/interop.md#matsim-deployment).

## Project overview

A Rust public transport routing core, designed to grow into a multimodal routing library.
Python and standalone Java clients share a versioned native API. Build both with
`python tools/build.py --test`; see [clone/build, examples and MATSim deployment](docs/interop.md).
It runs locally as a library or CLI, and through an optional in-process JNI adapter in eqasim-java.
The adapter shares an immutable timetable across MATSim's existing mode-choice routing workers;
no server or nested routing pool is needed. See [the JNI bridge](bridges/matsim-jni/README.md).

The first engine is an exact Connection Scan Algorithm (CSA), with walking closure and a
boarding bound. It minimizes arrival time, breaking equal-arrival ties by fewer boardings.
Optional Pareto output retains the best arrival for every non-dominated boarding count.
This is a working foundation, not a complete SwissRailRaptor or FLASH-TB replacement.

## Command-line usage

Install stable Rust (`rustup`) and, on Windows, the MSVC C++ build tools. Both were installed
on the development machine. Open a new terminal so Cargo is on PATH.

From this directory (PowerShell, Linux and macOS use the same Cargo commands):

```text
cargo build --release
cargo run --release -- --config config.example.toml info
cargo run --release -- --config config.example.toml route --from A --to C --departure 07:59:00
cargo run --release -- --config config.example.toml prepare
cargo run --release -- --config examples/config.binary.toml route --from A --to C --departure 07:59:00
cargo run --release -- --config config.example.toml benchmark --queries 10000 --seed 42
```

The example journey boards a bus at **08:00**, changes at B, boards rail at **08:06**, and
arrives at C at **08:10**. The CLI uses original stop/trip IDs and formatted service-day times.
An unreachable route is JSON `null`. Invalid requests/inputs are errors and exit nonzero.

On Windows the built executable is `target/release/mmrouter.exe`; on Linux/macOS it is
`target/release/mmrouter`. The executable does not require Cargo to run.

## Code layout

| Module | Responsibility |
|---|---|
| `model` | Source-neutral import types and compact validated timetable |
| `readers/matsim` | Streaming MATSim XML or gzip schedule import |
| `readers/gtfs` | Dated GTFS directory/ZIP import |
| `preprocess` | Connection sorting, CSR walking graph, optional destination-cell index |
| `spatial` | Spatial index and geographic/projected distance calculations |
| `routing` | Query, reusable workspace, exact scan, journey reconstruction |
| `cache` | Atomic, checksummed, versioned compact binary timetable |
| `config` | Strict TOML settings, defaults and path resolution |
| `engine` | Loading and bounded parallel independent queries |
| `main` | CLI and user-facing result formatting |
| `tests` | Reader/cache fixtures and independent differential correctness checks |

See [AGENTS.md](AGENTS.md) for the architectural contracts and roadmap, and
[config.example.toml](config.example.toml) for **every configurable parameter**, its default,
valid range and role. Paths resolve relative to the config, even when invoked elsewhere.

## Readers and supported features

**MATSim:** supply the schedule, not the simulation config or network. `.xml` and `.xml.gz`
are supported. Coordinates must be declared as projected metres or geographic lon/lat.
The importer retains line, route, departure and mode identities and boarding/alighting flags.
Route offsets expand relative to each vehicle departure. A missing first arrival can use the
first departure; a missing final departure can use the final arrival. Interior missing offsets
and inconsistent chronology fail. MATSim minimal-transfer relations become directed footpaths or
same-stop buffers; the walking closure can differ from Raptor's event-transfer semantics.
`preprocessing.use_explicit_transfers=false` disables those imported relations explicitly. External DTDs are never fetched. Vehicle capacities,
through-service/vehicle interlining and real-time updates are outside this version.

**GTFS:** supply a directory or ZIP with root-level tables, and a service date. Weekly
calendars and date additions/removals are applied before importing selected trips. Stop sequence,
overnight hours and ordinary pickup/dropoff permissions are honored. Only platform stops
(`location_type=0` or empty) are routable. Parent stations, entrances, pathways, wheelchair rules,
fares, flex service and vehicle block/interlining are not modeled. Geometric walking assumes
unrestricted straight-line access; use radius zero and explicit footpaths in library input if
that assumption is unsuitable.

Active frequency-based service, missing/interpolated stop times, demand-responsive pickup,
route/trip-scoped transfers, guaranteed/prohibited transfers and distinct-stop minimum-time
constraints are rejected with errors. Recommended distinct-stop transfers become geometric walking
edges; same-stop minimum transfer buffers are supported. This deliberately limited, documented
scope avoids returning silently incorrect itineraries for unsupported constraints.

**Binary:** `prepare` writes a `.mmrt` compiled timetable. Load with `input.format="binary"`.
This skips parsing and preprocessing, but currently **decodes into owned arrays**, not zero-copy
memory mapping. The file contains a format version, declared payload length and BLAKE3 checksum;
indices, chronology and CSR structure are validated on load. Writes use a temporary file and
atomic replacement. Rebuild explicitly after changing feeds or preprocessing settings. The binary
contains its preprocessing results; `[preprocessing]` settings are not reapplied on binary load.
Treat caches as locally produced artifacts, not a security boundary for hostile input.

## Search semantics and extension boundaries

- Times/durations inside the library are unsigned **milliseconds since service midnight**, not
  Unix time. Hours above 24 and fractional MATSim seconds work; horizon/sentinel overflow is checked.
  Inputs are a local service-day snapshot; DST and conversion between agency time zones are not done.
- Maximum boardings counts vehicles, so one boarding means a direct service; staying aboard is free.
- Transfer buffers apply to subsequent boardings, never the first. Walking chains are relaxed with
  Dijkstra; boarding and alighting restrictions do not prevent remaining on the same vehicle.
- Equal-departure zero-duration connections are solved to a fixed point without riding backwards
  along a trip. The result is deterministic for the same imported feed and query.
- Default earliest-arrival mode stops once future departures cannot improve the incumbent. Enable
  `routing.return_pareto` to search the full horizon and return the complete supported Pareto set.
- The timetable is immutable and shared. The caller owns a `Workspace`; batch workers reuse one
  thread-local workspace. Touched-cell reset avoids filling every label array for each query.
- Optional cell flags store conservative topological reachability. They can safely skip impossible
  directions, but may prune little on connected national networks. They are **not FLASH-TB arc flags**.
  Preprocessing costs scale with cells - graph size; this feature is disabled by default.

`routing.objective = "generalized_cost"` selects the separate weighted engine. It keeps time/cost
frontiers and the previous mode, and accounts for walking, waiting, onboard dwell, mode-specific
in-vehicle rates and directional transfer penalties. It supports fixed/mode-pair penalties or a
duration-dependent penalty, with nonnegative effective costs. Walking transfers are one direct
edge between boardings, plus an optional terminal walk, without arbitrary walking-chain closure.
Earliest-arrival dominance is never reused for this objective. Fares, capacity-dependent utility
and the full discrete mode-choice utility are outside this routing model.

To import the effective Eqasim coefficients, set `routing.matsim_config = "scenario/config.xml"`.
The path is relative to the TOML file. This replaces all manual `routing.costs` settings; the
config example documents every manual coefficient. Physical transfer graphs and endpoint search
are separate inputs, not implicitly imported. `Engine::weighted_route` and `weighted_batch`
accept multiple access/egress stop candidates with walking durations, sharing the immutable
timetable and coefficients across worker-local frontiers. The default arrival-time engine remains
available. Cache format MMRTR003 includes vehicle arrival at boarding stops; rebuild older caches.

## Library usage

```rust
use std::path::Path;
use multimodal_matsim_router::{config::Config, engine::Engine, routing::Workspace};

fn main() -> anyhow::Result<()> {
let config = Config::load(Path::new("config.example.toml"))?;
let engine = Engine::load(&config)?;
let query = engine.query(&config, "A", "C", "07:59:00")?;
let journey = engine.route(&query, &mut Workspace::default())?;
let batch = engine.batch(&[query]); // Result order matches input order.
Ok(())
}
```

## Batch requests and measurements

`batch --requests requests.jsonl` reads rows with `from`, `to`, `departure`, and optional `id`:

```json
{"id":"trip-1","from":"A","to":"C","departure":"07:59:00"}
{"id":"trip-2","from":"C","to":"A","departure":"08:00:00"}
```

It processes bounded batches, writes ordered JSONL responses, and represents no route as null.
Malformed requests stop the command; already emitted batches remain valid partial output.
No new thread is created per request. `execution.threads` fixes the worker pool size.

`benchmark` generates reproducible queries with SplitMix64, random stop pairs and departures
between 05:00 and 22:00. Loading/preprocessing is reported separately. It warms up to 100 serial
queries, then measures batch routing including reconstruction and batch workspace allocation.
Amortized time per query is a throughput measure, not individual latency. Tiny example numbers
are not evidence of performance on Switzerland. Use representative feeds, release builds,
known hardware and identical queries before comparing against Java or another algorithm.

## Validation

```text
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=12
```

Tests compare routing to independent event-graph relaxation on 80 deterministic generated
networks, with all origin/destination pairs and several boarding bounds. They also check real
GTFS ZIP/directory and MATSim gzip imports, calendar exceptions, overnight times, zero-time
connections, walking chains, cache corruption, parallel result consistency and config errors.

A reproducible real-schedule comparison, raw data and figures are under
[examples/geneva](examples/geneva/README.md). It uses an explicitly controlled stop-to-stop
protocol and does not establish equivalence to full MATSim mode choice or whole-simulation speedup. See
[docs/architecture.md](docs/architecture.md) for development stages and acceleration choices.

The newer [observed-trip comparison](examples/geneva/README.md#observed-trips-with-eqasim-penalties)
samples the original MATSim trips file, preserves origin/destination coordinates and departure
times, imports the Eqasim/SwissRailRaptor coefficients and independently scores returned paths.
It records reference Raptor native costs separately because this bundled version can charge
mode-specific penalties at both transfer and subsequent ride evaluation.

Weighted searches now enable destination bounds, cross-round dominance, sorted transfer pruning
and platform departure indices by default. They preserve the existing objective and share immutable
preprocessing across workers. See [the implementation notes](docs/weighted_optimizations.md) and
[the 12-worker before/after report](examples/geneva/trip_results_12/REPORT.md).

Parallel batches now support dynamic claiming and explicit prior-work priorities, preserving input
order and worker-local workspaces. See [parallelism](docs/parallelism.md) and the measured
[1–16-worker scaling report](examples/geneva/scaling_results/REPORT.md). Linear speedup is not
assumed; the report separates work imbalance from increased query wall time under concurrency.
