<!-- Reports measured results and limits of the controlled Geneva example. -->
# Geneva: Rust CSA and SwissRailRaptor

1000 shared requests; seed 42; 4 workers; 2000 warmup queries per mode.

| Engine | Serial total (s) | Parallel total (s) | Median query (ms) | p95 query (ms) | Reachable |
|---|---:|---:|---:|---:|---:|
| swiss | 4.088 | 1.792 | 0.4075 | 19.9344 | 170 |
| rust | 28.698 | 8.870 | 23.7591 | 67.1812 | 170 |

Reachability: {"both": 170, "neither": 830, "rust_only": 0, "swiss_only": 0}

Paired route agreement: {"count": 170, "within_1s": 166, "median_abs_difference_s": 0.0, "max_abs_difference_s": 38406.0, "same_transfer_count": 118}

## Interpretation

Input normalization (original untouched): {"excluded_routes": [{"line": "HAUTExSAVOIE:Line:1007006:LOC", "route": "HAUTExSAVOIE:ServiceJourney:1077827x1123551:LOC", "departures": 1, "reason": "non-monotonic offset at FR:74012:ZE:57917:HAUTExSAVOIE.link:stopMatch:1100237:1: {'refId': 'FR:74012:ZE:57917:HAUTExSAVOIE.link:stopMatch:1100237:1', 'arrivalOffset': '-06:05:00', 'departureOffset': '-06:05:00', 'awaitDeparture': 'true'}"}], "normalized_unused_endpoint_offsets": 6705}

This is a controlled **core stop-to-stop** benchmark on the real schedule, not a full simulation or the original mode-choice configuration. Walking between distinct stop IDs and coordinate access/egress are disabled. Pair-specific transfer overrides are replaced with the same constant same-stop buffer for both engines. Uniform time costs are used; mode/fare preferences are absent. Rust permits 21 boardings; SwissRailRaptor retains its normal round limit and additional-round stopping heuristic. Equal-arrival tie choices may differ.

Distances sum straight segments between successive scheduled stops on the selected services, not network-link distances or origin?destination chords. Ambiguous repeated-stop segments are counted in the CSV. Distances are computed outside timing by the same analysis code. Quality figures use only requests reachable in both; latency/throughput include all requests.

Serial timing includes each core search and internal reconstruction, excluding JSON I/O. Parallel timing includes scheduling and result collection (Java executor versus Rayon). Neither phase uses a route-result cache. Loading is separate; Java also loads the network, so startup is not equivalent work. Java uses -Xmx10g; Rust is optimized release. Exact inputs, hashes, CPU/OS and original settings are in metadata.json.

This repeated-query harness is not JMH/Criterion and does not establish whole-simulation speedup. See ../README.md for reproduction and limitations.

Original chart: `figures/travel_times.png` (not included in this archived baseline).

Original chart: `figures/runtime_ecdf.png` (not included in this archived baseline).

Original chart: `figures/distances.png` (not included in this archived baseline).

Original chart: `figures/transfers.png` (not included in this archived baseline).

Original chart: `figures/travel_time_differences.png` (not included in this archived baseline).

Original chart: `figures/throughput.png` (not included in this archived baseline).

Original chart: `figures/reachability.png` (not included in this archived baseline).
