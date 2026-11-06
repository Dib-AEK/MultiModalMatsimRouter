"""Compare exact legacy and default heuristic coordinate searches on the same private Swiss requests."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import statistics
import sys
from time import perf_counter

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python/src"))
from multimodal_router import Router


def clock(value):
    if isinstance(value, str):
        return value
    ms = round(value * 1000)
    return f"{ms // 3600000:02}:{ms // 60000 % 60:02}:{ms / 1000 % 60:06.3f}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("config", "queries", "library", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    queries = json.loads(args.queries.read_text())
    if not queries or args.threads < 1 or args.repeats < 1:
        parser.error("queries must be nonempty; threads and repeats must be positive")
    for query in queries:
        query["departure"] = clock(query["departure"])
    legacy = dict(work_budget=0, quality_wait_seconds=0, quality_journey_seconds=0, quality_radius_m=0)
    variants = []
    for label, policy in (("exact_legacy", legacy), ("heuristic", {})):
        with Router(args.config, library=args.library, threads=args.threads,
                    parameters={"stop_search": {"policy": policy}}) as router:
            def route(pair):
                index, query = pair
                start = perf_counter()
                result = router.route_coordinates(query["origin"], query["destination"], query["departure"])
                return dict(index=index, seconds=perf_counter()-start, result=result)
            with ThreadPoolExecutor(max_workers=args.threads) as pool:
                list(pool.map(route, enumerate(queries[:args.threads])))
                times = []
                for _ in range(args.repeats):
                    start = perf_counter()
                    rows = list(pool.map(route, enumerate(queries)))
                    times.append(perf_counter()-start)
            variants.append(dict(variant=label, policy=router.metadata()["stop_search"]["policy"],
                                 batch_seconds=times, median_seconds=statistics.median(times), rows=rows))
            print(label, times, flush=True)
    pairs = [(a["result"], b["result"]) for a,b in zip(variants[0]["rows"], variants[1]["rows"])
             if a["result"] and b["result"]]
    deltas = [b["generalized_cost"]-a["generalized_cost"] for a,b in pairs]
    report = dict(threads=args.threads, request_sha256=hashlib.sha256(args.queries.read_bytes()).hexdigest(),
                  variants=variants, paired_pt=len(pairs),
                  lower_cost=sum(d < -1e-8 for d in deltas), higher_cost=sum(d > 1e-8 for d in deltas),
                  budget_limited=sum(bool(r["result"] and r["result"].get("search_truncated"))
                                     for r in variants[1]["rows"]))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2))
    print(json.dumps({k:v for k,v in report.items() if k != "variants"}))


if __name__ == "__main__":
    main()
