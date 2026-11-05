<!-- Shows standalone Java startup, explicit input options and shared routing workers. -->
# Standalone client

```java
try (var router = NativeRouter.load(Path.of("examples/interop/router.toml"), library)) {
    // Application work can continue while the timetable loads in the background.
    router.awaitReady();
    System.out.println(router.routeJson("A", "B", "07:59:00"));
}
```

`library` is the path to `matsim_pt_router.dll`, `libmatsim_pt_router.so` or
`libmatsim_pt_router.dylib`. Add `mmrouter-java-0.2.0.jar` to the classpath. Where required by the
JDK, launch with `--enable-native-access=ALL-UNNAMED`.
The build helper bundles the host native library in that jar, so `NativeRouter.load(path)` also
works without an explicit library path. Use the explicit overload to select another build.

Explicit schedule and parameters (the GTFS service date is mandatory):

```java
var router = NativeRouter.start(Path.of("examples/interop/gtfs"), library, "gtfs",
    "{\"service_date\":\"2026-11-05\",\"execution\":{\"threads\":12}}");
```

Partial JSON parameters merge into loaded/default settings. Configuration, error/no-path,
millisecond units and batch result contracts are documented in [the shared API](../../docs/interop.md).
Auto-detection supports router TOML, MATSim config XML, schedule XML/XML.gz, GTFS folders/ZIP and
prepared `.mmrt` timetables. Use an explicit format to override detection. GTFS dates and options
must still be supplied explicitly or in TOML.

`routeJson` takes stop IDs. `routeCoordinatesJson(x,y,toX,toY,time)` searches served stops using
the imported/configured access radii and walks, retrying up to five radii if no PT route is found.
It returns successful null when no useful PT itinerary exists, including when direct walking is
preferred. It does not fabricate a walk-only MATSim journey. `batchJson` and `weightedBatchJson`
preserve order and per-query errors. The synchronous `NativeRouter` constructor remains available.
