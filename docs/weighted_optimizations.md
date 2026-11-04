<!-- Explains weighted-search acceleration invariants, preprocessing, numerical safety and measured validation. -->
# Weighted search optimizations

All four settings under `[routing.search]` default to true. Disable them individually to run
differential checks. They preserve the documented weighted objective, boarding limit and horizon;
they do not add heuristic boarding limits, result caching or station/platform merging.

## Destination bounds

`search_index` compiles a directed reverse graph. Between stops, transit edges use minimum
scheduled segment cost, ignoring onboard dwell; walking edges use their actual transfer durations
and the walking rate. Waiting, penalties, pickup/dropoff restrictions and boarding limits are
relaxed. Every real continuation therefore maps to a no-more-expensive path in this graph.

Each query runs reverse Dijkstra, seeded with every egress candidate's walking cost. The resulting
`h(stop)` includes the cheapest relaxed egress alternative. If accumulated base cost plus this
bound exceeds the incumbent, the state can be skipped. An infinite bound rejects an unreachable
continuation. Dynamic penalties remain omitted from the bound, since they are nonnegative.
The bound computation is included in query timings and is not cached between requests.

Graph products/additions round down in f64. Additions are also constrained to be no smaller than
the current distance: rounding a zero-cost cycle must not produce endlessly decreasing distances.
Pruning uses a small relative numerical guard and retains cost ties for boarding/arrival tie breaks.

## Dominance across boarding rounds

The history contains only completed earlier rounds. Label A can dominate B when A uses fewer
boardings, arrives no later, has the same previous mode and can compensate the entire time
difference at the maximum waiting/in-vehicle rate without costing more than B. For dynamic
transfer penalties, A must also have a no-earlier first boarding time. With fewer transfers,
this guarantees no higher nonnegative duration-dependent penalty on a common continuation.

This is sufficient, conservative dominance. Later cheaper labels survive. Earlier-round history
is never removed by a label using more boardings. Onboard continuation remains available even
when its off-vehicle arrival is dominated. These rules avoid discarding useful through journeys.

## Transfer and platform preprocessing

Physical transfers, including the same-stop buffer, are sorted once by duration. Once even
accumulated cost plus walking cost exceeds the incumbent, all remaining longer transfers can be
skipped. Destination bounds reject individual transfers before reconstruction records are created.
No new walking chains or implicit zero-time station hubs are introduced.

Platform departure indices contain sorted distinct actual pickup times, and a small per-platform
table gives the minimum mode-change cost for each incoming mode. A ready state with no feasible
departure before the horizon is discarded. Time until the first feasible departure contributes
only a minimum-rate lower bound: boarding can begin during vehicle dwell, so charging the entire
gap at the waiting rate would be unsafe. Mode-change minima are ignored for dynamic penalties.
The earliest departure is a bound only; later services, including overtaking/express trips, remain
in the search. This factors useful station information without a route-stop Cartesian product.

## Ownership and allocation

Indices are shared read-only across all engine workers. Weighted engines prepare them during
loading; arrival-only engines initialize them once only if explicitly asked for weighted routing.
No weighted preprocessing is imposed on ordinary arrival-only queries. Workspaces retain private
labels, history, paths and Dijkstra buffers. Copyable scalar states replace cloned collections;
transfer reconstruction records are allocated only after dominance checks.

The binary timetable remains MMRTR003; cost-dependent indices are rebuilt from the current
timetable/settings, so a changed cost profile cannot accidentally reuse persisted pruning data.
The reported index memory excludes the base timetable, settings, construction temporaries and
worker workspaces. Landmark bounds, FLASH-TB flags and multilevel transfer hierarchies are not
implemented in this milestone.

## Validation and reproduction

Generated-timetable tests compare all 16 combinations of the four switches against independent
exhaustive journey enumeration. Cases cover mode pairs, dynamic penalties, clamping, zero rates,
permissions, horizons, multiple endpoints and workspace reuse. Dedicated tests cover zero-cost
cycles, dwell, later cheaper arrivals, overtaking departures and 12-worker lazy initialization.

```text
cargo test -- --test-threads=12
python examples/geneva/run_trips.py --config ../cmdp_scenario/geneva_config.xml --trips ../cmdp_scenario/simulation_output/output_trips.csv.gz --matsim-jar ../eqasim-java/switzerland/target/switzerland.jar --queries 1000 --threads 12 --warmup 200 --output examples/geneva/trip_results_12 --baseline examples/geneva/trip_results_12_baseline
```

The [measured report](../examples/geneva/trip_results_12/REPORT.md) compares the same 1000 observed
trips before/after, using 12 workers in both runs. It checks costs, arrival/boarding counts and
actual leg sequences against the original executable, and independently validates all itineraries.
The comparison retains Raptor's reference-build cost-accounting caveat. These measurements do
not establish whole-simulation speedup or identical performance on the full Swiss timetable.
