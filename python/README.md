<!-- Introduces installation and the native-library requirement for the Python client. -->
# MultiModalMatsimRouter Python client

Install with `python -m pip install ./python` from the router checkout. Build native libraries
with `python tools/build.py --test`, then set `MMROUTER_LIBRARY` to the absolute path of
`target/release/multimodal_router.dll` (Linux: `libmultimodal_router.so`; macOS: `.dylib`).
This package contains the Python client; the platform library is an explicit deployment artifact.

```python
from multimodal_router import Router

with Router("examples/interop/router.toml") as router:
    print(router.route("A", "B", "07:59:00"))
```

One immutable native engine is shared across concurrent callers. Batches use a bounded Rust pool;
the GIL is released during native calls. No-path is None; invalid queries raise ValueError.
Batch results preserve order and per-query errors. Use metadata(include_entities=True) once to map
journey stop/trip indices back to original IDs. Full lifecycle/protocol/deployment documentation
and examples are in `docs/interop.md` in the router repository.
