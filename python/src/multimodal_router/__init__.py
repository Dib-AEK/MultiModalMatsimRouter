"""Routes through the shared Rust engine, releasing Python's GIL during native calls."""

from .router import Router

__all__ = ["Router"]
