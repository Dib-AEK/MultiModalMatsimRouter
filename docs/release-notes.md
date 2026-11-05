<!-- Describes the first precompiled Python, Java, MATSim bridge and CLI distribution. -->
Compiled Python wheels and platform ZIPs: **no Rust toolchain is required**.

- Python wheels bundle the native engine; install the matching `.whl` with `python -m pip install`.
- ZIPs include Java client/MATSim facade jars, JNI and C libraries, the CLI and runnable examples.
- Asynchronous MATSim config/schedule and GTFS loading, coordinate/stop-ID routing and shared
  concurrent workers are available in both language clients.
- Builds are tested separately on Windows x64, Linux x64, macOS Apple Silicon and macOS Intel.
- Client API 2; existing MATSim numeric JNI protocol 1 is preserved.

Choose a download for your OS/architecture. See the ZIP's README or
[installation instructions](https://github.com/Dib-AEK/MultiModalMatsimRouter/blob/main/docs/binary-distribution.md)
for runtime requirements and usage. SHA-256 files accompany each download.
