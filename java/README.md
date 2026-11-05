<!-- Describes Java module ownership and asynchronous shared-memory routing. -->
# Java clients

```text
java/
  client/          standalone Java API and shared asynchronous sessions
  native/          shared JNI crate (library name remains matsim_pt_router)
  matsim/
    src/           MatsimRouter.load(config) convenience API
    native/        actual MATSim snapshot/numeric routing bridge
    examples/      a small asynchronous MATSim-config example
```

Build with `python tools/build.py --test` from the repository root. It produces
`target/java/mmrouter-java-0.2.0.jar`, `target/java/mmrouter-matsim-0.2.0.jar` and the host JNI
library. The standalone API targets Java 17; a full MATSim simulation uses its required JDK 25.

Use `NativeRouter.load(file)` for TOML or auto-detected MATSim XML input. The Java client jar
bundles its native library; an optional second argument selects an external one. It returns a
`RouterSession` immediately. `NativeRouter.start(file, library, format, parametersJson)` also accepts
explicit GTFS/MATSim schedule inputs and partial settings. `MatsimRouter.load` explicitly selects a
MATSim config, including config-relative schedule paths and supported router parameters.

One session owns one native engine. `session.worker()` creates a lightweight view; it does not
create another timetable or pool. Single calls reuse caller-thread native workspaces; batches use
the engine's bounded pool. Keep the session open until all worker futures complete. The first
query waits if loading is unfinished; `awaitReady()` is optional. Initialization failures propagate
on that first wait/query. `isReady()` never blocks. Closing during startup arranges native cleanup
when loading finishes; it does not block the application thread waiting for a file read.

See [the client guide](client/README.md) and [MATSim tools](matsim/README.md).
