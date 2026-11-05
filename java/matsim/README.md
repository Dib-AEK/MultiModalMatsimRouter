<!-- Explains standalone MATSim-config utilities and their relationship to live simulation integration. -->
# MATSim routing tools

`MatsimRouter.load(config)` returns immediately, resolving the schedule and supported
parameters from a MATSim config in the background. It requires no MATSim Java dependency for
standalone utilities. Add both `mmrouter-java-0.2.0.jar` and `mmrouter-matsim-0.2.0.jar` to the
classpath. The Java client jar bundles its host's native library. Optional `load(config, library)`
selects an explicit external binary; `load(config, library, overridesJson)` permits partial overrides.

```java
try (var router = MatsimRouter.load(Path.of("examples/interop/matsim_config.xml"))) {
    // Do other work here; loading is already running.
    router.awaitReady();
    var pool = Executors.newFixedThreadPool(12);
    try {
        var worker = router.worker();
        var result = pool.submit(() -> worker.routeJson("A", "B", "07:59:00"));
        System.out.println(result.get());
    } finally { pool.shutdownNow(); }
}
```

Full executable example: [AsyncMatsimExample.java](examples/AsyncMatsimExample.java). It is compiled
by the repository build helper. Example execution on Windows after `python tools/build.py --test`:

```text
java -Xmx10g --enable-native-access=ALL-UNNAMED -cp "target/java/test-classes;target/java/mmrouter-java-0.2.0.jar;target/java/mmrouter-matsim-0.2.0.jar" AsyncMatsimExample examples/interop/matsim_config.xml target/release/matsim_pt_router.dll
```

On Linux/macOS use `:` as the classpath separator and the corresponding SO/dylib path.

Imported settings include the config-relative `transit.transitScheduleFile`, global thread count,
walking speed/distance factor, stop search/extension radii, direct-walk comparison, minimum transfer
time, transfer-walk margin, Eqasim coefficients or default-population MATSim scoring, and Swiss
mode-to-mode/duration-dependent transfer penalties. Times at stops include explicit access/egress
attributes. Physical transfers use MATSim's whole-second rounding; explicit self-transfer zero
overrides the default buffer. The unused terminal departure offset is ignored, including legacy
zero values. Used offsets are validated and invalid services are not silently removed.

The transfer-walk margin is retained in metadata; source-neutral journeys keep the full transfer
readiness interval. The live MATSim plan converter applies its output margin when building legs.

Capacity constraints, range queries, intermodal/network walking, passenger-mode mappings, individual
scoring and chained departures are rejected explicitly. The core still uses its intended
once-per-transfer objective; it does not reproduce the reference's duplicate mode-penalty charging.
Coordinates are read as stored; EPSG:4326/WGS84 are recognized as lon/lat, and other projected
MATSim schedules must be in metres. Config URLs/remote
schedule URLs and automatic coordinate transformations are outside this loader.

## Inside an actual simulation

The native snapshot reader and numeric query bridge are under `native/`. The existing
`org.eqasim.switzerland.ch_cmdp.rustRouting` Java glue in eqasim-java constructs MATSim plan elements,
owns controller lifecycle hooks and the itinerary-cache binding. That application-specific glue
stays in eqasim-java; it calls this repository's bridge from the scenario **after** its adaptations.
This preserves in-memory scenario changes that re-reading an original config would miss.

Continue using the existing `rust-router` Maven profile and `--useRustRouter true`. The native
package/library name and numeric ABI are unchanged by this folder reorganization. No JSON or inner
Rust query pool is added to the simulation's hot path. Use the config-loading facade above for
standalone routing utilities; use the live-scenario adapter for simulation and discrete mode choice.
