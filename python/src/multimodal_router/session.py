"""Owns asynchronous startup and lightweight thread views without duplicating native timetables."""

from concurrent.futures import ThreadPoolExecutor
import copy
import weakref

_LOADERS = ThreadPoolExecutor(max_workers=2, thread_name_prefix="mmrouter-load")


def _release(future):
    if future.cancel():
        return

    def finish(completed):
        if not completed.cancelled() and completed.exception() is None:
            completed.result().close()

    future.add_done_callback(finish)


class Worker:
    """Non-owning API view that keeps its shared session alive; workspaces follow calling threads."""

    def __init__(self, owner):
        self._owner = owner

    def route(self, *args):
        return self._owner.route(*args)

    def route_many(self, queries):
        return self._owner.route_many(queries)

    def weighted_many(self, queries):
        return self._owner.weighted_many(queries)

    def route_coordinates(self, *args):
        return self._owner.route_coordinates(*args)


class AsyncRouter:
    """Returns immediately; the first query/wait observes startup success or failure.

    Close is nonblocking during startup. When an already-running loader finishes, its
    handle is closed automatically. Keep the session open while worker views are used.
    """

    def __init__(self, factory, file, **options):
        # Freeze settings before returning so later caller mutations cannot affect loading.
        self._future = _LOADERS.submit(factory, file, **copy.deepcopy(options))
        self._finalizer = weakref.finalize(self, _release, self._future)

    @property
    def closed(self):
        return not self._finalizer.alive

    @property
    def ready(self):
        return not self.closed and self._future.done() and self._future.exception() is None

    def _client(self, timeout=None):
        if self.closed:
            raise RuntimeError("Router session is closed")
        return self._future.result(timeout)

    def wait(self, timeout=None):
        self._client(timeout)
        return self

    def worker(self):
        return Worker(self)

    def route(self, *args):
        return self._client().route(*args)

    def route_many(self, queries):
        return self._client().route_many(queries)

    def weighted_many(self, queries):
        return self._client().weighted_many(queries)

    def route_coordinates(self, *args):
        return self._client().route_coordinates(*args)

    def metadata(self, **options):
        return self._client().metadata(**options)

    def close(self):
        self._finalizer()

    def __enter__(self):
        if self.closed:
            raise RuntimeError("Router session is closed")
        return self

    def __exit__(self, *_):
        self.close()
