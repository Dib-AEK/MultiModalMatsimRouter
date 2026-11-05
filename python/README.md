<!-- Introduces installation and the native-library requirement for the Python client. -->
# MultiModalMatsimRouter Python client

To use without Rust, install a platform wheel from
[GitHub Releases](https://github.com/Dib-AEK/MultiModalMatsimRouter/releases/latest).
It includes the native engine; no library path is required. See
[binary installation](../docs/binary-distribution.md) for supported systems.

Install locally with `python -m pip install -e ./python` from the router checkout. Build native libraries
with `python tools/build.py --test`. For a regular deployed installation, set `MMROUTER_LIBRARY`
to the absolute native-library path or pass `library=` (Windows: `multimodal_router.dll`,
Linux: `libmultimodal_router.so`; macOS: `.dylib`).
Editable/source checkouts automatically find their own `target/release` library. The installed
standalone package can instead use `MMROUTER_LIBRARY` or `library=` as an explicit deployment artifact.

```python
from multimodal_router import Router

with Router.load("examples/interop/matsim_config.xml") as router:
    # Continue other application work while loading runs.
    router.wait()  # Optional: the first query also waits for startup.
    print(router.route("A", "B", "07:59:00"))
```

The asynchronous `load` and `start` aliases auto-detect router TOML, MATSim config XML, MATSim
schedule XML/XML.gz, GTFS directories/ZIP and binary `.mmrt` files. Loading failures are raised by
`wait()` or the first route; `ready` is nonblocking. Closing a session while it loads arranges
cleanup when startup finishes. The synchronous `Router(...)` constructor remains available.

```python
with Router.load("schedule.xml.gz", format="matsim", threads=12,
                 parameters={"routing":{"objective":"generalized_cost",
                                        "costs":{"wait_per_hour":2}}}) as router:
    print(router.route("origin_stop", "destination_stop", "08:00:00"))

with Router.load("examples/interop/gtfs", format="gtfs",
                 service_date="2026-11-05", threads=12) as router:
    print(router.route("A", "B", "07:59:00"))
```

Partial `parameters` merge into imported/default `preprocessing`, `routing`, `execution` and
`cache` sections. `stop_search` controls `search_radius_m`, `extension_radius_m`,
`direct_walk_factor`, `distance_factor` and `transfer_walk_margin_s`. Coordinates use the input
declaration (projected metres for MATSim, lon/lat for GTFS); raw MATSim input can specify
`coordinates="geographic"`. GTFS always requires a service date. GTFS mode-specific utility keys
use feed `route_type` strings (e.g. `"3"` for bus) in `routing.costs.modes_per_hour`.

For concurrent calls, share the session or create a lightweight `router.worker()` view for each
caller. Both reuse one timetable and cost model; native workspace storage follows the calling
thread. `route_many` uses a bounded Rust batch pool. Do not load a new engine per worker.
`route_coordinates(origin_xy, destination_xy, time)` also supports indexed stop searches with
up to five expansions. Successful None means no useful PT itinerary; walking-only output remains
an application concern. Coordinate queries use the weighted objective, like `weighted_many`.

See [the short parallel example](examples/async_routing.py). No separate process/server is needed;
memory is shared between threads in the same process. Separate Python processes load separate engines.

One immutable native engine is shared across concurrent callers. Batches use a bounded Rust pool;
the GIL is released during native calls. No-path is None; invalid queries raise ValueError.
Batch results preserve order and per-query errors. Use metadata(include_entities=True) once to map
journey stop/trip indices back to original IDs. Full lifecycle/protocol/deployment documentation
and examples are in `docs/interop.md` in the router repository.
