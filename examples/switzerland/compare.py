"""Compare two native router builds on identical private coordinate queries using eight shared-memory workers."""
import argparse, concurrent.futures, json, statistics, sys, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python/src"))
from multimodal_router import Router

def clean(value):
    if isinstance(value, dict): return {k:clean(v) for k,v in value.items() if k != "statistics"}
    if isinstance(value, list): return [clean(v) for v in value]
    return value

def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ["config","queries","baseline","candidate","output"]: p.add_argument("--"+name,type=Path,required=True)
    p.add_argument("--threads",type=int,default=8);p.add_argument("--repeats",type=int,default=3)
    args=p.parse_args();queries=json.loads(args.queries.read_text())
    routers=[Router(args.config,library=str(x.resolve()),threads=args.threads) for x in [args.baseline,args.candidate]]

    timings=[[],[]];reference=None
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.threads) as pool:
            for repeat in range(args.repeats):
                for i in ([0,1] if repeat%2==0 else [1,0]):
                    tick=time.perf_counter()
                    results=list(pool.map(lambda q:routers[i].route_coordinates(q["origin"],q["destination"],q["departure"]),queries))
                    elapsed=time.perf_counter()-tick;timings[i].append(elapsed)
                    results=clean(results)
                    if reference is None:reference=results
                    else:assert results==reference,"itinerary difference"
                    print(repeat,i,elapsed,flush=True)
    finally:
        for router in routers:router.close()
    report={"workers":args.threads,"queries":len(queries),"seconds":timings,"median_seconds":[statistics.median(x) for x in timings],"identical_itineraries":True}
    args.output.write_text(json.dumps(report,indent=2));print(json.dumps(report))
if __name__=="__main__":main()

