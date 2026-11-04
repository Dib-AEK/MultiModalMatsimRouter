<!-- Records completed checks, build outputs, measured benchmark scope, and implementation limits. -->
# Validation: MultiModalMatsimRouter 0.1.0

Validated on Windows 11 with Rust 1.99.1 stable and MSVC build tools, using a release build
with thin LTO and one codegen unit. Rust/Cargo/rustfmt/Clippy and the MSVC C++ build tools were
installed during setup. The library and examples are intended to remain portable; this run
was performed on Windows, not a Linux machine.

## Checks completed

- `cargo fmt --all --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo test`: **11 integration tests passed**, including an independent event-graph solver
  comparison on 80 generated timetables (all stop pairs, three boarding limits, reused scratch).
- `python -m unittest discover -s examples/geneva -p test_example.py`: **3 tests passed**.
- `cargo build --release`: passed; executable `target/release/mmrouter.exe`.
- `cargo build --release --example geneva-benchmark`: passed.
- Example XML preparation, binary load/routing, and standalone CLI benchmark: passed.
- Java benchmark compiled against the provided MATSim 2026.0/eqasim jar and ran with `-Xmx10g`.
- All returned reachable benchmark itineraries were validated against schedule segments,
  service departure times, transfer gaps, endpoints and boarding counts before plotting.
- Eight PNG/SVG charts were produced. Layouts and representative charts were visually inspected.

## Real Geneva measurement

Dataset: supplied `cmdp_scenario/geneva_config.xml`. Original files were read-only. The derived
benchmark schedule excludes 1 malformed route
and normalizes 6704 unused endpoint
offsets; these operations and exact input hashes are recorded in the benchmark metadata.
The Rust import contains 4270 stops, 13820 dated departures and
275800 connections. The controlled protocol contains no interstop footpaths.

There are 1,000 shared unique requests, 2,000 warmups per execution mode, and four workers.
Each engine measures those same 1,000 requests once serially and once as a parallel batch.
Both engines return a route on 170 requests and no route on 830. Of the 170 paired routes,
166 agree within one second; four earlier Rust arrivals remain documented for inspection.
All 1,000 final Rust arrivals/boarding counts match the initial unoptimized Rust baseline.

| Engine | Serial seconds / 1000 | Four-worker seconds / 1000 | Median reachable query ms |
|---|---:|---:|---:|
| SwissRailRaptor | 4.599 | 1.896 | 4.469 |
| Rust CSA | 3.061 | 1.152 | 2.351 |

See [the full report](../examples/geneva/results/REPORT.md), raw CSV/JSONL and metadata. Timings
include core reconstruction, exclude JSON/plots, and are not JMH/Criterion or an end-to-end
MATSim measurement. Most requests are unreachable under the deliberately restricted walking
protocol, making component filtering particularly useful. Reachable-only latencies are also
shown. Distance figures use scheduled-stop-chain geometry, not network link lengths.

## Current limits

The default core uses arrival time/boarding count; the optional weighted engine supports the
documented nonnegative time and transfer disutilities with independent frontier labels. It is
not a port of every SwissRailRaptor heuristic or its mode-choice/fare model.
Full FLASH-TB, street routing, servers/JNI, zero-copy mmap and real-time updates
are future work. GTFS unsupported constraints are documented and rejected. Input MATSim
minimal-transfer relations are represented as walking edges/buffers; that graph closure is
not a claim of identical event-transfer semantics to SwissRailRaptor.

Weighted tests exhaustively enumerate all boardings/alightings on 200 additional small generated
timetables and compare costs under all 16 acceleration-switch combinations. Hand-calculated fixtures distinguish dwell, waiting and walking,
preserve later cheaper arrivals/last-mode alternatives, and verify real Eqasim override semantics.
The observed-trip benchmark validates both engines' returned legs against the schedule and shared
transfer graph. All Rust native costs must agree with an independent itinerary scorer; Raptor
native totals and independently scored totals are both retained. Algorithmic differences and the
reference build's mode-transfer charging behavior are explicitly reported, not hidden as ties.
