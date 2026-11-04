<!-- Defines the MATSim adapter boundary, build commands, protocol, and ownership contracts. -->
# MATSim JNI bridge

This crate exposes the weighted router to eqasim-java's `org.eqasim.switzerland.ch_cmdp.rustRouting`.
It deliberately contains no alternate routing implementation. The core library keeps its unsafe-code
ban and has no JNI dependencies. Only this ABI crate declares exported JNI symbols.

```shell
cargo build --release --locked -p matsim-pt-router-jni
cargo test --workspace -- --test-threads=12
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The release library is `target/release/matsim_pt_router.dll` on Windows,
`libmatsim_pt_router.so` on Linux, or `libmatsim_pt_router.dylib` on macOS.
Build with the same architecture as the JVM. The Java repo's `rust-router` Maven profile builds and
bundles this library in its standalone jar. No Rust toolchain is required at simulation runtime.

The three native operations are open(snapshot path, cost JSON), route(handle, numeric query arrays)
and close(handle). JSON is parsed once at initialization; queries only copy primitive arrays through
safe JNI region APIs. Results carry cost, chosen access/egress endpoints and six numbers per journey
leg: kind, trip index (or -1), from stop, to stop, departure, arrival. Empty output is ordinary no path.
All clocks/durations use unsigned milliseconds. String schedule IDs remain in the Java metadata.

The streamed snapshot uses magic `MMJNI001`, big-endian integers/doubles, and contains stop coordinates
and same-stop transfer buffers, trips with mode and chronological stop calls, and physical stop-pair
walking transfers. Counts/strings are length-prefixed u32; pickup/dropoff use one flag byte. It is a
temporary local integration format, not a substitute for the versioned persistent MMRTR cache.
Java exports the actual scenario after adaptations and checks chronology with original IDs.
The Rust reader additionally validates the model and compiles shared cost/search indices.

A registry maps integer handles to Arc-owned immutable timetable/model pairs. Its RwLock is held
only while cloning/removing the Arc. Each Java calling OS thread reuses a thread-local Workspace;
there is no Rust query thread pool or Java callback while searching. In-flight Arcs keep the model
alive across close. Every exported function catches panics and translates invalid input into a Java
exception; no Rust unwind crosses a JVM frame. Native allocations are outside Java's heap budget.

The production adapter supports static Eqasim scheduled PT costs and teleported walking. It explicitly
rejects unsupported capacity/range/intermodal/chained/mapped passenger behavior. Its intended cost
model does not emulate SwissRailRaptor's observed double charging of mode-pair penalties. Java owns
MATSim route reconstruction, walk fallback, fare/subscription evaluation and optional itinerary caching.
