"""Starts preprocessing in the background, then routes on shared-memory thread views."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from multimodal_router import Router

config = Path(__file__).resolve().parents[2] / "examples/interop/matsim_config.xml"
with Router.load(config) as router:  # MMROUTER_LIBRARY selects the platform library.
    print("Loading in background; the application can continue other work here.")
    router.wait()  # Optional: the first route also waits for startup.
    with ThreadPoolExecutor(max_workers=12) as pool:
        workers = [router.worker() for _ in range(12)]  # Views, not timetable copies.
        routes = list(pool.map(lambda w: w.route("A", "B", "07:59:00"), workers))
    print(routes[0])
    print(router.route_coordinates((0, 0), (1000, 0), "07:59:00"))

# Raw feeds also work with the same interface:
# Router.load("schedule.xml.gz", format="matsim", parameters={"routing":{"costs":{"wait_per_hour":2}}})
# Router.load("feed.zip", format="gtfs", service_date="2026-11-05", threads=12)
