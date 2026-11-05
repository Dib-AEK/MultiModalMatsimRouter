<!-- Describes the first precompiled Python, Java, MATSim bridge and CLI distribution. -->
Compiled Python wheels and platform ZIPs: **no Rust toolchain is required**.

- Python wheels bundle the native engine; install the matching `.whl` with `python -m pip install`.
- ZIPs include Java client/MATSim facade jars, JNI and C libraries, the CLI and runnable examples.
- Asynchronous MATSim config/schedule and GTFS loading, coordinate/stop-ID routing and shared
  concurrent workers are available in both language clients.
- These initial downloads are **Windows x64** builds. A clean installed wheel passed 120 concurrent
  queries and coordinate routing; the Java/native integration checks also passed.
- Linux x64 and macOS Apple Silicon/Intel automation is configured, but GitHub Actions currently
  fails before creating any jobs (`BuildFailed` / `startup_failure`). Those downloads are not yet
  available; no Linux/macOS validation is claimed for this release.
- Client API 2; existing MATSim numeric JNI protocol 1 is preserved.

Choose a download for your OS/architecture. See the ZIP's README or
[installation instructions](https://github.com/Dib-AEK/MultiModalMatsimRouter/blob/main/docs/binary-distribution.md)
for runtime requirements and usage. SHA-256 files accompany each download.
