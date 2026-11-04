<!-- Records actual cross-language, packaging and MATSim regression validation for protocol 1. -->
# Interoperability validation — 4 November 2026

Validated on Windows x86-64 with Python 3.13.5, JDK 25 and the installed stable Rust toolchain.
The standalone Java client was compiled targeting Java 17; only JDK 25 execution was tested here.

- `python tools/build.py --test`: passed. This builds release C/JNI libraries and the standalone
  Java jar, runs the Rust workspace tests (26 passed), three Python tests and the standalone Java
  integration test. JSON errors/no-path distinction, dwell/time/cost semantics, original ID metadata,
  mixed-validity batch ordering, weighted endpoint queries, double-close and closed-handle access
  are exercised against actual native libraries.
- Concurrent Python calls: 36 batches from 12 Python threads share a Rust engine configured for
  12 workers. Ordered results agree, including independent unknown-stop and no-path outcomes.
- Concurrent Java calls: 120 queries from 12 Java threads agree and use independently owned results.
- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- Python installation: installed `./python` with pip into a disposable virtual environment and
  ran `examples/interop/python_example.py` using the installed package and explicit library path.
  Both individual and batch routes succeeded, with successful null for the disconnected stop.
- MATSim Maven `standalone,rust-router` build: passed with 17 targeted Java tests (5 adapter
  tests, 12 PT cache tests), none skipped. The local-data test routed the saved 1,000 Geneva
  queries on 12 callers against the updated native library, preserving its cost assertions and
  valid route timings/distances. Java heaps were limited to 10 GiB.

The rebuilt eqasim-java `switzerland/target/switzerland.jar` contains the updated host JNI library
and the protocol check. No MATSim per-query JSON conversion or inner Rust pool was introduced.
The older Lausanne five-iteration comparison was not rerun for this packaging/API change; its
results remain historical, not measurements of this new build.

Artifacts: `target/interop-validation.log`, `target/python-install-validation.log`,
`target/native-build.json` and `target/java/mmrouter-java-0.1.0.jar` in this checkout;
`target/rust-interop-matsim-validation.log` and `switzerland/target/surefire-reports` in eqasim-java.
The build manifest records current native/Java artifact hashes. The libraries were built for
Windows here; Linux/macOS builds require their own host builds and execution checks.
