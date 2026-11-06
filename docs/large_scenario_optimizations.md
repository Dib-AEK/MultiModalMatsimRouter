<!-- Explains exact large-timetable accelerations, their memory costs, and reproducible concurrency validation. -->
# Routing on large timetables

Geographic cells guide the search; they never determine which routes are legal. A route may leave
and re-enter a cell, and an intercity train may beat a service staying within the destination city.
The generalized-cost objective, access/egress candidates, transfer penalties and boarding limits
are unchanged.

## Shared preprocessing

`local_connections` indexes pickup connections by physical stop and links each trip's connections.
A query activates only trips boardable from its current frontiers and merges their continuations
in the same global connection-index order as the reference scan. It considers later departures,
express alternatives and onward transfers; it does not retain only the next service.

`regional_bound_cells` partitions stops by balanced spatial bisection and computes optimistic
reverse costs to each cell. The default is 128 cells. Potentials use f32 rounded downward, requiring
about `4 * stops * cells` bytes shared by all workers (about 40 MiB for 80,000 stops). Ignoring waits,
mode changes and the exact destination within a cell gives lower bounds, not route restrictions.
Search costs themselves remain f64. Indices rebuild with the cost model; timetable cache MMRTR003
is unchanged. Preprocessing uses parallel workers outside an existing Rayon routing worker, avoiding
nested work stealing while a thread-local workspace is borrowed.

## Query-local work

Sparse frontiers clear only touched stops and trips. Incoming walking edges build final-egress
candidates without scanning every walking edge in the country. Local reverse refinement settles
stops up to four times the first settled access-stop bound. Unsettled vertices use a conservative
cutoff, strengthened by the shared regional potential; unreachable regions are never guessed.

A difficult search without a feasible incumbent can trigger `temporal_pruning` after 2,048 improved
labels. A reverse timetable scan checks whether any time-feasible PT itinerary could exist, preserving
pickup/dropoff, stay-seated travel, physical walking times and transfer buffers. It ignores the
boarding-count limit. Only a negative result may prune; zero-time connection batches reach a fixed
point. This prevents repeated radius expansion from first exhausting large cost frontiers.

## Parallel callers and MATSim

Queries use their caller threads and private reusable workspaces. Timetables, boarding indices and
regional potentials are shared and immutable. MATSim's existing mode-choice workers call JNI directly;
there is no inner routing pool. Balanced work is still important: native speedups do not promise
linear scaling at 48 threads or imply an identical improvement in whole-simulation runtime.

The Eqasim adapter snapshots static stop access/egress times once, builds primitive candidate arrays,
and logs native query count, empty results, radius expansions, mean call time, and summed parallel
worker time at iteration end. Summed worker time is neither wall-clock time nor CPU utilization.
The native protocol is unchanged, so existing Java/Python clients remain compatible.

## Reproducing a benchmark

Build the C ABI library with `cargo build --release -p mmrouter-ffi`. Prepare a JSON array such as:

```json
[{"origin":[2683000,1248000],"destination":[2685000,1249000],"departure":"08:00:00"}]
```

Run the same configuration and request file against each version in a separate process:

```sh
python tools/benchmark_coordinates.py scenario/config.xml queries.json --library target/release/libmultimodal_router.so --threads 8 --repeats 3 --output results.json
```

On Windows use `target/release/multimodal_router.dll`. Configuration import supplies the schedule,
walking policy and Eqasim/SwissRailRaptor coefficients. These coordinate-service measurements include
candidate search, radius retries, native routing and JSON reconstruction. They are not MATSim JNI
or entire mode-choice timings. Run no builds or other heavy validation concurrently with timing.
Compare complete journeys and costs while excluding diagnostic statistics. Keep failed/unreachable
requests in the workload; filtering them out can hide the largest regressions.

All acceleration options can be disabled for differential tests. `regional_bound_cells = 0` disables
shared potentials. With regions disabled and `local_bounds = false`, destination bounds use the full
exact reverse calculation. Disabling `local_connections` restores the complete connection scan.

## Switzerland validation, 6 November 2026

The full Switzerland schedule was loaded with the scenario's Eqasim/SwissRailRaptor coefficients,
300 m generated transfer connections and configured stop-access policy. A deterministic sample
of 1,000 real iteration-0 trips contained 100 each within 20 km of Zurich, Geneva, Lausanne and Bern,
and 600 other trips. Original modes were not restricted. Departures were between 05:00 and 22:00;
trips shorter than 200 m were excluded. The schedule remained national for every request.

Hardware: AMD Ryzen 7 PRO 6850U, 8 physical / 16 logical cores, Windows. Release build with thin LTO
and one codegen unit. No build or other heavy validation ran concurrently with timed queries.
Preprocessing/loading is outside the measured interval. Both versions used the same Rust harness,
8 independent calling threads and 64-boardings / 48-hour query limits.

| Measurement | Before | Optimized |
|---|---:|---:|
| 1,000 coordinate queries, elapsed | 41.79 s | 7.08 s |
| PT results | 884 | 884 |
| No useful PT result | 116 | 116 |

Measured speedup in this pass: 5.90x. All 1,000 requests retained the same reachability,
generalized cost, arrival time and boarding count. Complete itineraries matched for 999; one
selected a different first bus at the same optimal cost, final arrival and boarding count.
The initial 128-request development sample also matched all complete itineraries.

A separate scaling pass of the optimized algorithm measured 26.33 s with one caller, 10.42 s with
four, 8.10 s with eight and 6.81 s with twelve. These are individual passes, not confidence intervals;
several costly requests and memory contention still limit scaling. Forty-eight callers were not
benchmarked on this 16-logical-core machine. Do not extrapolate linear scaling or these speedups
to a complete MATSim iteration.

Validation passed: 34 Rust tests, formatting and Clippy with warnings denied; 33 selected Java tests
including 1,000 saved Geneva JNI requests and concurrent callers; 13 Python interoperability tests.
The standalone Eqasim jar embeds updated Windows x86-64 and Linux x86-64 libraries. The latter was
cross-compiled against glibc 2.17 and checked for ELF architecture, exports, dependencies and embedded
checksums, but was not executed on Linux here. The native protocol and snapshot format are unchanged.

## Early-Pruning and partitioning extension

`network_partition` refines each level of a geographic bisection using weighted service adjacency
and walking connectivity. Moves must reduce the local cut and respect size limits, including enough
stops for all descendant cells. This deterministic in-process heuristic avoids an external
partitioner dependency in the standalone jar. The geographic seed remains available for comparison.

`transfer_blocks` stores downward-rounded lower bounds for suffixes beginning at every 16th
transfer. For each destination cell, a suffix bound includes minimum walking plus onward cost.
A separate suffix minimum supplies the next mode-change penalty for static mode-specific costs.
Adding these two minima remains a lower bound even if different edges attain them. Duration-based
penalties omit the mode-change bound. A query skips a suffix only when its accumulated cost plus
that bound strictly exceeds the incumbent with the existing numerical guard.

This borrows sorted early stopping and contiguous transfer pruning from Early Pruning (2026),
HypRAPTOR and T-REX (ESA 2026). It is a cost-aware bound implementation, not a port of T-REX's
arrival-time/trip-count transfer ranks. It retains mode context, candidate walks and the original
weighted objective. Hard route exclusion based only on geographic cells is never performed.

Additional shared memory is approximately four bytes per destination cell per transfer block,
plus four bytes per mode per block; block counts sum ceil(outgoing transfers / 16) over stops.
Both switches can be disabled independently. No query-time inner worker pool or locks are added.

### Additional Swiss validation of the paper-inspired extension

With the same 1,000 real trips and eight callers, the previous optimized router measured 7.081 s
and the new network-aware/suffix-bound version measured 6.832 s in individual passes. Full
itineraries, costs and reachability matched for all 1,000. Transfer examinations fell from
36,839,709 to 31,721,301 (13.9%). These passes indicate a modest incremental gain; they are not
confidence intervals or whole-simulation measurements. The original pre-optimization baseline
was 41.790 s. Differential tests cover all 1,024 acceleration-switch combinations on generated
small timetables, plus a dense, zero-duration transfer fixture that exercises suffix skipping.

### Default selection and ablation

A repeated paired comparison showed that elapsed-time gains are workload-sensitive. A subsequent
four-way, alternating-order comparison (two passes, eight callers, the same 1,000 requests) measured
median elapsed times of 7.864 s with neither extension, 7.473 s with suffix blocks, 7.402 s with
network refinement, and 7.412 s with both. All objectives matched in every pass. These are exploratory
measurements, not statistical confidence intervals. Network refinement is enabled by default;
transfer blocks are optional, because combining them did not improve on refinement alone and costs
additional shared memory. Earlier fast locality optimizations remain enabled.

Eqasim exposes `swissRustRouting.networkPartition` (default true) and
`swissRustRouting.transferBlocks` (default false). For example:

```text
--config:swissRustRouting.transferBlocks true
```

The JNI cost envelope accepts optional `_search` settings. Default settings omit this extension,
retaining compatibility with older protocol-1 libraries. Explicit experimental settings require
the rebuilt native library. Python and standalone Java clients use `routing.search` options.

## Bounded heuristic preparation and boarding activation

Query-local bound preparation now settles at most 1024 stops by default. This is not a route-search
limit: after stopping relaxed Dijkstra, the last settled distance is a valid lower bound for every
unsettled vertex. Known settled distances remain available, and shared destination-region potentials
continue to guide all other stops. Shared regional preprocessing uses an uncapped exact relaxed search.
`routing.search.local_bound_settlements = 0` removes the settlement-count cap while retaining the original distance cutoff.
In MATSim the corresponding option is `swissRustRouting.localBoundSettlements`.

Once an incumbent exists, boarding activation excludes departures that cannot beat it under the
minimum time rate. For each ready label, retain departures through label time plus remaining cost
budget divided by the minimum rate, with outward rounding and numerical slack. Take the maximum
across all labels, including later-cheaper alternatives; zero time rates leave the horizon unrestricted.
Connection processing order, actual costs, mode history, transfer costs and route reconstruction stay
unchanged. Two repeated frontier-dominance checks are also removed after their preallocation guards.

Two alternating-order Swiss experiments used 1000 real coordinate queries and eight concurrent callers.
The first compared before/unlimited/1024/4096 budgets: median seconds were 7.997 / 7.332 / 6.938 / 7.001.
The second compared before/regional-only/256/1024: 8.132 / 7.081 / 7.174 / 6.968. Every complete itinerary
matched in every pass after excluding diagnostics. This is approximately 13-14% lower native elapsed
time on this local workload, not a claim about whole-iteration or 48-worker performance. Clocks vary
across passes; small differences between budgets are not statistically established.

Inline SmallVec frontier storage was prototyped but discarded: its combined median (7.859 seconds)
was effectively unchanged from the 7.861-second baseline in that comparison. No new container
library remains in the routing core. See the Switzerland example for MATSim-level measurements.
