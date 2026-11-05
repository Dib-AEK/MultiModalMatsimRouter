<!-- Records validation of the organized asynchronous Python and Java interfaces. -->
# Asynchronous client validation

Validated on Windows 11, Python 3.13 and Java 25 on 5 November 2026, using release native
libraries. These are functional and integration checks, not a new routing-speed benchmark.

Run the portable checks from the repository root:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
python tools/build.py --test
```

Results:

- 26 Rust tests passed across the workspace.
- 13 Python tests passed, including asynchronous startup, close during startup, startup failure,
  concurrent shared workers, raw GTFS directory/ZIP inputs, relative MATSim config paths,
  coefficient overrides, stop attributes, explicit zero transfers and geographic coordinates.
- Java interop passed 240 concurrent queries with 12 workers, covering original and asynchronous
  MATSim-config entry points. A separate JVM ran the example using the native library bundled
  in the standalone jar, without an explicit library path.
- An editable Python installation ran the example with source-checkout library discovery.
- The actual Lausanne MATSim config loaded 1,446 stops and 9,891 departures, with 12 routing
  threads and imported Eqasim settings. An illustrative local smoke run returned its loading
  session in 0.023 seconds and became ready in 0.352 seconds. These single observations are not
  comparative timings or a guarantee for other schedules.
- The existing eqasim-java live-scenario adapter passed its 5 routing tests and 12 PT-cache tests,
  including the saved 1,000-query Geneva comparison. Java runs used `-Xmx10g`. Its numeric JNI
  protocol remains unchanged; the standalone JSON facade is not introduced into that hot path.

The tests verify that caller threads share one immutable native engine and retain independent
reusable search workspaces. Worker views do not load another timetable. Startup failures surface
on `wait`/`awaitReady` or the first query; closing an unfinished session releases its eventual
native handle. No route is a successful null result, while malformed requests remain errors.

See [the API](interop.md), [Python example](../python/examples/async_routing.py), and
[Java example](../java/matsim/examples/AsyncMatsimExample.java).
