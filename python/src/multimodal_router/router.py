"""Owns native handles and response buffers; keeps JSON conversion outside Rust search workers."""

from __future__ import annotations

import ctypes
import json
import os
from pathlib import Path
import platform
import weakref


class _Native:
    def __init__(self, library):
        name = {
            "Windows": "multimodal_router.dll",
            "Linux": "libmultimodal_router.so",
            "Darwin": "libmultimodal_router.dylib",
        }[platform.system()]
        path = library or os.environ.get("MMROUTER_LIBRARY")
        if not path:
            candidate = Path(__file__).parent / "_native" / name
            if not candidate.is_file():
                # Editable/source checkouts may discover their own build, never an arbitrary cwd.
                checkout = Path(__file__).resolve().parents[3]
                if (checkout / "Cargo.toml").is_file():
                    candidate = checkout / "target" / "release" / name
            if not candidate.is_file():
                raise RuntimeError(
                    "Provide library=... or MMROUTER_LIBRARY after building mmrouter-ffi; see docs/interop.md"
                )
            path = candidate
        self.lib = ctypes.CDLL(str(Path(path).expanduser().resolve(strict=True)))
        self.lib.mmr_protocol_version.argtypes = []
        self.lib.mmr_protocol_version.restype = ctypes.c_int32
        if self.lib.mmr_protocol_version() != 1:
            raise RuntimeError(
                "Unsupported native router protocol; rebuild the library and client together"
            )
        try:
            self.lib.mmr_api_version.argtypes = []
            self.lib.mmr_api_version.restype = ctypes.c_int32
            if self.lib.mmr_api_version() < 2:
                raise RuntimeError("Native library needs client API 2; rebuild with tools/build.py")
        except AttributeError as error:
            raise RuntimeError(
                "Native library predates client API 2; rebuild with tools/build.py"
            ) from error
        self.lib.mmr_request.argtypes = [ctypes.c_int64, ctypes.c_char_p]
        self.lib.mmr_request.restype = ctypes.c_void_p
        self.lib.mmr_free.argtypes = [ctypes.c_void_p]
        self.lib.mmr_free.restype = None

    def call(self, handle, command):
        payload = json.dumps(command, allow_nan=False, separators=(",", ":")).encode("utf-8")
        pointer = self.lib.mmr_request(handle, payload)
        if not pointer:
            raise RuntimeError("Native router returned a null response")
        try:
            response = json.loads(ctypes.string_at(pointer).decode("utf-8"))
        finally:
            self.lib.mmr_free(pointer)
        if not response["ok"]:
            raise ValueError(response["error"])
        return response["result"]


class Router:
    """Loads once and shares an immutable timetable across concurrent callers.

    Single routes run on the calling thread. Batches use execution.threads from TOML.
    Route results contain millisecond clocks and numeric stop/trip indices. Metadata
    with include_entities=True supplies the corresponding original IDs once.
    """

    def __init__(
        self,
        config,
        *,
        library=None,
        format="auto",
        parameters=None,
        service_date=None,
        coordinates=None,
        threads=None,
    ):
        import copy

        settings = copy.deepcopy(parameters) if parameters is not None else {}
        if service_date is not None:
            settings["service_date"] = service_date
        if coordinates is not None:
            settings["coordinates"] = coordinates
        if threads is not None:
            settings.setdefault("execution", {})["threads"] = threads
        self._native = _Native(library)
        self._handle = self._native.call(
            0,
            {
                "op": "open",
                "config": str(Path(config).resolve(strict=True)),
                "format": format,
                "parameters": settings,
            },
        )
        self._finalizer = weakref.finalize(self, self._native.call, self._handle, {"op": "close"})

    @classmethod
    def load(cls, file, **options):
        """Starts loading in the background; returns a shared session immediately."""
        from .session import AsyncRouter

        return AsyncRouter(cls, file, **options)

    start = load

    def worker(self):
        """Creates a lightweight view; no timetable or native engine is copied."""
        from .session import Worker

        return Worker(self)

    def _call(self, command):
        if self.closed:
            raise RuntimeError("Router is closed")
        return self._native.call(self._handle, command)

    @property
    def closed(self):
        return not self._finalizer.alive

    def route(self, origin: str, destination: str, departure: str):
        """Returns a journey or None. Departure is service-day HH:MM:SS, allowing >24h."""
        return self._call(
            {"op": "route", "from": origin, "to": destination, "departure": departure}
        )

    def route_many(self, queries):
        """Returns ordered {ok,result|error} entries, including per-query validation errors."""
        return self._call({"op": "batch", "queries": list(queries)})

    def route_coordinates(self, origin, destination, departure):
        """Searches served stops and retries wider radii; None means no useful PT itinerary."""
        return self._call(
            {
                "op": "route_coordinates",
                "origin": list(origin),
                "destination": list(destination),
                "departure": departure,
            }
        )

    def weighted_many(self, queries):
        """Accepts explicit access/egress arrays and millisecond clocks, using generalized costs."""
        return self._call({"op": "weighted_batch", "queries": list(queries)})

    def metadata(self, *, include_entities=False):
        return self._call({"op": "metadata", "include_entities": include_entities})

    def close(self):
        self._finalizer()

    def __enter__(self):
        if self.closed:
            raise RuntimeError("Router is closed")
        return self

    def __exit__(self, *_):
        self.close()
