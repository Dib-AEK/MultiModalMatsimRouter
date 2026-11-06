"""Measure concurrent coordinate queries with one shared native engine and ordered, inspectable results."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sys
import threading
from time import perf_counter

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python" / "src"))
from multimodal_router import Router


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path, help="MATSim XML or router TOML configuration")
    parser.add_argument("queries", type=Path, help="JSON array of origin/destination coordinate pairs and departure clocks")
    parser.add_argument("--library", type=Path, required=True, help="C ABI library, e.g. multimodal_router.dll or libmultimodal_router.so")
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.threads < 1 or args.repeats < 1:
        parser.error("threads and repeats must be positive")
    queries = json.loads(args.queries.read_text())
    if not queries:
        parser.error("queries must not be empty")
    started = perf_counter()
    with Router.load(args.config, library=args.library, threads=args.threads) as router:
        router.wait()
        loading = perf_counter() - started
        local = threading.local()

        def route(item):
            index, query = item
            if not hasattr(local, "worker"):
                local.worker = router.worker()
            tick = perf_counter()
            result = local.worker.route_coordinates(query["origin"], query["destination"], query["departure"])
            return {"index": index, "seconds": perf_counter() - tick, "journey": result}

        runs = []
        # Retain workers/workspaces across repeats. No routes are cached by this tool.
        with ThreadPoolExecutor(max_workers=args.threads) as pool:
            for repeat in range(args.repeats):
                tick = perf_counter()
                results = list(pool.map(route, enumerate(queries)))
                runs.append({"repeat": repeat, "seconds": perf_counter() - tick, "results": results})
        report = {"workers": args.threads, "queries": len(queries), "load_seconds": loading, "runs": runs}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2))
        print(json.dumps({"load_seconds": loading, "run_seconds": [run["seconds"] for run in runs]}))


if __name__ == "__main__":
    main()
