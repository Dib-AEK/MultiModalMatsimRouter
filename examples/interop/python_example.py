"""Loads once and routes individual or parallel batches using the public Python package."""
from pathlib import Path
from multimodal_router import Router

with Router(Path(__file__).with_name("router.toml")) as router:
    print(router.metadata())
    print(router.route("A", "B", "07:59:00"))
    print(router.route_many([
        {"from": "A", "to": "B", "departure": "07:59:00"},
        {"from": "A", "to": "C", "departure": "07:59:00"},
    ]))
