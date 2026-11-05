<!-- Documents shared native APIs, clone/build workflow, protocol and application ownership. -->
# Python, Java and MATSim integration

## Asynchronous loading and shared workers (client API 2)

Current Java layout is `java/client` (standalone API), `java/native` (shared JNI), and
`java/matsim` (MATSim-config facade, numeric snapshot bridge and examples). Python code and its
small threaded example are under `python/`. See [Java module usage](../java/README.md),
[MATSim configuration tools](../java/matsim/README.md) and [Python loading options](../python/README.md).

`Router.load(file)` / `Router.start(file)` in Python and `NativeRouter.load(file, library)` in
Java return immediately, loading/preprocessing on a bounded background executor. A first query
waits automatically; explicit `wait()` / `awaitReady()` is optional. Ready checks do not block.
Loading errors propagate on the first wait/query. Close during startup arranges cleanup after a
running load finishes. One session owns the engine; `worker()` views share it. Do not initialize
an engine per routing thread. Query workspaces are native thread-local state, while timetable,
cost indices and the coordinate stop index are shared and read-only.

Inputs auto-detect router TOML, MATSim config XML, MATSim schedule XML/XML.gz, GTFS folders/ZIP,
and `.mmrt` binary timetables. Explicit `format` can be `config`, `matsim_config`, `matsim`, `gtfs`
or `binary`. GTFS additionally requires `service_date`. Partial `parameters` override imported
settings in `preprocessing`, `routing`, `execution`, `cache`, and `stop_search` sections. Raw
MATSim inputs can declare `coordinates`; GTFS coordinates come from the feed. Read the executable
examples before processing a full scenario:

- [Python background startup and shared threads](../python/examples/async_routing.py)
- [Java MATSim-config background startup](../java/matsim/examples/AsyncMatsimExample.java)
- [Tiny MATSim config](../examples/interop/matsim_config.xml), with a config-relative schedule
- [Tiny synthetic GTFS](../examples/interop/gtfs/stops.txt), active on 2026-11-05

`route_coordinates` / `routeCoordinatesJson` accepts origin/destination coordinates and a
departure time. It performs indexed access/egress stop searches, then weighted PT routing,
retrying up to five wider radii. Direct-walk preference yields successful null for the PT result;
the host decides whether to emit a walk-only route. Coordinate units follow the timetable.
Use schedule/config inputs when MATSim stop access/egress attributes matter: the source-neutral
`.mmrt` cache does not contain those extra host attributes. Infinite cost bounds appear as null
in JSON metadata; the native cost model retains its actual unbounded values.

The result-envelope protocol remains version 1 for backward compatibility. New clients also
require client API 2 (`mmr_api_version` / JNI `apiVersion`) so a stale library is rejected before
the new loading entry points are used. Old numeric MATSim calls and the library name are unchanged.
Build artifacts now use client version 0.2.0, including `mmrouter-matsim-0.2.0.jar`.

The raw open operation also accepts optional `format` and `parameters`, and a coordinate query is:

```json
{"op":"open","config":"/scenario/config.xml","format":"matsim_config","parameters":{"execution":{"threads":12}}}
{"op":"route_coordinates","origin":[2538000,1152400],"destination":[2535000,1153500],"departure":"08:00:00"}
```

Keep **MultiModalMatsimRouter in a separate repository**, cloned locally as `MultiModalRouter`.
The algorithm, readers, native boundaries, configuration, standalone clients and their tests live
here. MATSim module bindings, live scenario snapshotting, route reconstruction and mode-choice/cache
integration remain in eqasim-java. Version/test the router once and consume it from both languages.
The GitHub repository is `https://github.com/Dib-AEK/MultiModalMatsimRouter`.

## Clone and build

Install stable Rust/Cargo, a native linker (MSVC Build Tools on Windows), Python 3.11+ for the build
helper, and JDK 17+ for the standalone Java client. MATSim itself uses JDK 25. Build on the target
operating system/architecture; a Windows DLL cannot be used as a Linux SO.

```text
git clone https://github.com/Dib-AEK/MultiModalMatsimRouter.git MultiModalRouter
cd MultiModalRouter
python tools/build.py --test
python -m pip install -e ./python
```

The helper finds Cargo on PATH or under your home `.cargo/bin`, builds release libraries using
Cargo.lock, builds the standalone Java jar, and optionally tests all language boundaries.

| Artifact on Windows | Use |
| --- | --- |
| `target/release/multimodal_router.dll` | Python/C interface |
| `target/release/matsim_pt_router.dll` | Java JNI, including fast MATSim calls |
| `target/java/mmrouter-java-0.2.0.jar` | Standalone Java client without Java dependencies |
| `target/native-build.json` | Platform, protocol version and SHA-256 hashes |

Linux names are `libmultimodal_router.so` / `libmatsim_pt_router.so`; macOS uses `.dylib`.
Deployment needs the language client and matching library, not a compiler/source checkout/server.
Pin a router revision in deployment automation. Current package/API versions are 0.2.0/protocol 1.

## Python

Editable/source checkouts discover their own native build. Installed deployments can instead
set `MMROUTER_LIBRARY` or pass `library=`. The dependency-free client loads an explicit library
path, frees every response buffer in a finally block, and provides deterministic/context-manager
close plus finalization of abandoned handles.

```python
from multimodal_router import Router

with Router("examples/interop/router.toml",
            library="target/release/multimodal_router.dll") as router:
    print(router.route("A", "B", "07:59:00"))  # Journey or None; invalid input raises ValueError.
    results = router.route_many([
        {"from": "A", "to": "B", "departure": "07:59:00"},
        {"from": "A", "to": "C", "departure": "07:59:00"},
    ])
    entities = router.metadata(include_entities=True)
```

Batch entries retain order and contain `{ok: true, result: journey-or-null}` or
`{ok: false, error: message}`. An invalid stop does not prevent valid neighboring queries from
routing; malformed request shapes fail the request. `weighted_many` accepts explicit access/egress
sets using the millisecond schema below. Journey stop/trip integers index the metadata entity
arrays. Fetch entities once, rather than copying a national timetable's metadata per route.

`ctypes.CDLL` releases Python's GIL during native calls
([Python documentation](https://docs.python.org/3/library/ctypes.html)). Concurrent callers share
an immutable engine. Single queries run on the caller; batches use the bounded Rust pool configured
by `execution.threads`. JSON conversion occurs outside the search loop. Prefer reasonably sized
batches for throughput. For async applications use `await asyncio.to_thread(router.route_many,
queries)`; initialization can also run in an application-owned executor. Application batch sizes
must be bounded to fit memory; this API does not impose an arbitrary query-count ceiling.

## Standalone Java

```java
import java.nio.file.Path;
import org.multimodalmatsim.router.NativeRouter;

try (var router = new NativeRouter(Path.of("examples/interop/router.toml"),
                                  Path.of("target/release/matsim_pt_router.dll"))) {
    System.out.println(router.routeJson("A", "B", "07:59:00"));
    System.out.println(router.metadataJson(false));
}
```

Compile against `target/java/mmrouter-java-0.2.0.jar`. Methods return versioned JSON envelopes,
which your application's JSON library can parse. Initialization failures throw Java exceptions;
query errors return error envelopes. `batchJson`, `weightedBatchJson` and `requestJson` expose
the same protocol. Use try-with-resources. Close is idempotent; in-flight native searches retain
shared ownership of their engine. Include `--enable-native-access=ALL-UNNAMED` on applicable JDKs.

`examples/interop/JavaExample.java` and `python_example.py` use an included tiny schedule; no
external scenario is needed. The Java client targets Java 17; the build helper tests on the local
JVM. Windows validation is not a claim of tested execution on every OS/JDK combination.

## MATSim deployment

The eqasim-java adapter exports the **actual loaded/adjusted scenario** and starts native
preprocessing alongside controller setup. Existing DMC workers use numeric JNI calls directly,
with one immutable timetable and a reusable workspace per caller thread. There is no JSON per
simulation query, inner query pool, per-connection Java callback, or server. The PT route cache
can wrap the adapter and bypass native calls on hits.

With the router checkout beside eqasim-java:

```text
cd ../eqasim-java
mvn -pl switzerland -am package -Pstandalone,rust-router -DskipTests
java -Xmx10g -jar switzerland/target/switzerland.jar --config-path scenario/config.xml --useRustRouter true
```

Override `-Drust.router.project=/absolute/path/to/MultiModalRouter` for another clone location,
or `-Dcargo.executable=/absolute/path/to/cargo` when Cargo is absent from PATH. Maven bundles
the host JNI library in the simulation jar. Runtime users need only that jar and their scenario.
The loader checks protocol compatibility at startup. `swissRustRouting.libraryPath` can instead
select an external library, relative to the MATSim config.

Existing scope remains explicit: static scheduled PT with global Eqasim costs and teleported
walking. Capacity/range/intermodal/chained passenger routing is not silently approximated. Java
owns MATSim routes, attributes, pricing and subscriptions. Separate processes do not share RAM;
a future IPC/server adapter would be needed to share one timetable across processes.

## Protocol, clocks and lifecycle

`router-service` owns the safe engine registry/application protocol. `router-ffi` owns C pointers;
`matsim-jni` owns JNI and the specialized MATSim snapshot interface. The core algorithm depends on
neither boundary. Panics are contained at ABI boundaries. Errors belong to individual responses,
with no shared last-error slot. Registry locks protect lookup/removal only, never search work.

C exports: `mmr_protocol_version()`, `mmr_request(int64 handle, const char*)`, `mmr_free(char*)`.
Strings are NUL-terminated UTF-8. Every returned response must be freed exactly once with the same
library. Null request input yields an error envelope. The schema is `{ "ok": true, "result": ... }`
or `{ "ok": false, "error": "..." }`. Open uses handle 0 and returns a positive engine handle;
subsequent calls pass the handle separately. Supported command shapes:

```json
{"op":"version"}
{"op":"open","config":"/absolute/path/router.toml"}
{"op":"route","from":"A","to":"B","departure":"07:59:00"}
{"op":"batch","queries":[{"from":"A","to":"B","departure":"07:59:00"}]}
{"op":"weighted_batch","queries":[{"departure":28740000,"max_boardings":5,"max_duration":3600000,"access":[{"stop":0,"duration":0}],"egress":[{"stop":1,"duration":0}]}]}
{"op":"metadata","include_entities":false}
{"op":"close"}
```

Route/batch respects the configured objective; weighted batches explicitly request generalized
cost. Query/journey/leg clocks and durations are **unsigned milliseconds since service-day
midnight**, not Unix time. Human departure strings allow hours above 24. Configured durations are
seconds, and utility rates are per hour. See `config.example.toml`. Native allocations are outside
Java's `-Xmx` or Python's managed heap.

Matching Python and MATSim results requires matching schedule adaptations, cost coefficients,
access/egress sets and transfer graphs. `routing.matsim_config` imports supported cost coefficients;
standalone walking/preprocessing settings must also match deliberately. The MATSim adapter already
gets these from the live scenario. This work does not add road routing or Java-disutility translation.
