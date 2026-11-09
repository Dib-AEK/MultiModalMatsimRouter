<!-- Explains how to install compiled release packages without Rust or a native compiler. -->
# Ready-to-use binary packages

Download a wheel or platform ZIP from
[GitHub Releases](https://github.com/matsim-eth/RustRouter/releases/latest).
No Rust/Cargo or C++ compiler is needed. The repository is private, so GitHub login and repository
access are currently required. This is a GitHub distribution, not a package published to PyPI.

Only attached release assets are available to download. Version 0.2.0 initially provides tested
Windows x64 packages. Linux/macOS builds are configured, but GitHub Actions currently fails before
creating any jobs in this repository; those platform binaries have not been built or published.
The build targets and their runtime requirements are:

| Archive suffix | Runtime |
| --- | --- |
| `windows-x86_64` | 64-bit Windows, x64 Python/JVM; Microsoft Visual C++ 2015–2022 runtime |
| `linux-x86_64` | x64 glibc Linux, Ubuntu 22.04/glibc 2.35 or newer; not Alpine/musl |
| `macos-aarch64` | Apple Silicon macOS 15 or newer, ARM64 Python/JVM |
| `macos-x86_64` | Intel macOS 15 or newer, x64 Python/JVM |

Linux wheels have a native `linux_x86_64` tag; they are not advertised as portable manylinux wheels.
Python clients require Python 3.10+. Standalone Java clients require Java 17+; use the Java version
required by your MATSim application when integrating there. The OS/architecture of the native
library must match the Python/JVM process.

## Python

Download the matching `.whl` asset, then install it using its exact filename:

```text
python -m pip install multimodal_matsim_router-0.2.0-py3-none-win_amd64.whl
```

The native library is inside the wheel; no library environment variable is needed.
Alternatively extract the ZIP and install the wheel in its `python/` directory.

```python
from multimodal_router import Router

with Router.load("scenario/config.xml") as router:
    # Loading/preprocessing runs in the background. A query waits if needed.
    route = router.route_coordinates((origin_x, origin_y), (dest_x, dest_y), "08:00:00")
```

Use `Router.load("feed.zip", service_date="2026-11-05")` for GTFS or
`Router.load("schedule.xml.gz", format="matsim", parameters={...})` for a raw MATSim schedule.
Coordinates must match the schedule's coordinate system. Share the session across threads, or
create lightweight `router.worker()` views. No second timetable is loaded by a worker view.

## Java and MATSim config utilities

Extract the ZIP and add both jars in `java/` to your application's classpath. The client jar
includes its platform's JNI library and extracts it automatically at startup.

```java
import java.nio.file.Path;
import org.multimodalmatsim.router.matsim.MatsimRouter;

try (var router = MatsimRouter.load(Path.of("scenario/config.xml"))) {
    var route = router.routeCoordinatesJson(originX, originY, destX, destY, "08:00:00");
}
```

From the extracted directory, run the included example on Windows:

```text
java -Xmx10g --enable-native-access=ALL-UNNAMED -cp "java/*" examples/AsyncMatsimExample.java examples/interop/matsim_config.xml
```

The same quoted wildcard classpath works on Linux/macOS. On Java 17 omit the native-access flag.
The general Java client is `org.multimodalmatsim.router.NativeRouter`.

Inside a live MATSim simulation, use the compatible eqasim-java adapter, which exports the actual
adapted Scenario and uses numeric JNI queries. The native library is provided under `native/`.
The standalone jars are not a replacement for that controller/plan-conversion adapter. A simulation
jar already bundled with this library needs neither Rust nor this ZIP at runtime.

## Command line and integrity

The ZIP also provides `bin/mmrouter` (`mmrouter.exe` on Windows), examples, build metadata and
checksums. Run `bin/mmrouter --help` for the CLI. On Unix, if your ZIP extractor drops executable
permissions, run `chmod +x bin/mmrouter` first.

Each release asset has an adjacent `.sha256`; the ZIP also contains `SHA256SUMS.txt` for its files.
Build locally with `python tools/package_release.py` to reproduce the build/test/package procedure.
Tagged `v*` commits trigger the release workflow; automated publication occurs only after every
platform passes Rust, Python, Java and installed-wheel tests. A tested host package may also be
uploaded manually, with its platform and validation scope stated in the release notes.
