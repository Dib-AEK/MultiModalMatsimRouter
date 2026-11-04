# Reports measured worker scaling, ordering equivalence and query-wall occupancy without claiming CPU utilization.
import argparse
import datetime
import hashlib
import json
import os
import platform
import subprocess
from pathlib import Path


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def machine():
    result = dict(
        os=platform.platform(), cpu=platform.processor(), logical_threads=os.cpu_count()
    )
    if os.name == "nt":
        command = "Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors | ConvertTo-Json -Compress"
        completed = subprocess.run(
            ["powershell", "-NoProfile", "-Command", command],
            capture_output=True,
            text=True,
        )
        if completed.returncode == 0:
            data = json.loads(completed.stdout.lstrip("\ufeff"))
            data = data if isinstance(data, list) else [data]
            result.update(
                cpu=", ".join(d["Name"] for d in data),
                physical_cores=sum(d["NumberOfCores"] for d in data),
                logical_threads=sum(d["NumberOfLogicalProcessors"] for d in data),
            )
    return result


def analyze(directory):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    directory = Path(directory)
    policies = {
        p: json.loads((directory / f"scaling_{p}.json").read_text())
        for p in ("rayon", "dynamic", "learned")
    }
    if (directory / "scaling_auto.json").exists():
        policies["auto"] = json.loads((directory / "scaling_auto.json").read_text())
    fingerprints = {
        r["route_fingerprint"]
        for data in policies.values()
        for r in data["measurements"]
    }
    if len(fingerprints) != 1:
        raise ValueError("Route fingerprints changed between schedulers/worker counts")
    info = machine()
    colors = dict(rayon="#bab0ac", dynamic="#b279a2", learned="#54a24b", auto="#4c78a8")
    labels = dict(
        rayon="Adaptive Rayon",
        dynamic="Dynamic queue",
        learned="Explicit prior-batch priorities",
        auto="Automatic learned priorities",
    )
    summary = dict(
        machine=info,
        route_fingerprint=next(iter(fingerprints)),
        all_routes_identical=True,
        policies={},
    )
    for name, data in policies.items():
        rows = data["measurements"]
        one = next(r for r in rows if r["threads"] == 1)
        one_work = sum(w["query_seconds"] for w in one["profile"]["workers"])
        values = []
        for row in rows:
            n = row["threads"]
            measured = row["profile"]
            work = sum(w["query_seconds"] for w in measured["workers"])
            values.append(
                dict(
                    threads=n,
                    seconds=row["median_seconds"],
                    queries=row["queries"],
                    queries_per_second=row["queries"] / row["median_seconds"],
                    speedup=one["median_seconds"] / row["median_seconds"],
                    worker_wall_occupancy=work / (n * measured["total_seconds"]),
                    query_wall_inflation=work / one_work,
                    longest_parallel_query_ms=max(measured["query_nanoseconds"]) / 1e6,
                )
            )
        summary["policies"][name] = values
    figures = directory / "figures"
    figures.mkdir(exist_ok=True)

    def save(name):
        plt.tight_layout()
        for suffix in ("png", "svg"):
            plt.savefig(figures / f"{name}.{suffix}", dpi=160)
        plt.close()

    for key, ylabel, name in (
        ("seconds", "Median batch time (seconds)", "runtime"),
        ("speedup", "Speedup over one worker (same policy)", "speedup"),
        (
            "worker_wall_occupancy",
            "Fraction of worker wall time inside query calls",
            "occupancy",
        ),
        (
            "query_wall_inflation",
            "Sum of query wall times / one-worker query wall time",
            "work_inflation",
        ),
    ):
        _, ax = plt.subplots(figsize=(7, 4.5))
        for policy, values in summary["policies"].items():
            ax.plot(
                [r["threads"] for r in values],
                [r[key] for r in values],
                "o-",
                color=colors[policy],
                label=labels[policy],
            )
        if key == "speedup":
            threads = [r["threads"] for r in summary["policies"]["dynamic"]]
            ax.plot(threads, threads, "--", color="gray", label="Ideal linear scaling")
        if key == "worker_wall_occupancy":
            ax.set_ylim(0, 1.05)
        if info.get("physical_cores"):
            ax.axvline(
                info["physical_cores"],
                color="gray",
                alpha=0.35,
                linestyle=":",
                label="Physical cores",
            )
        ax.set(
            xlabel="Worker threads",
            ylabel=ylabel,
            title="Same 1,000 observed PT trips; warm repeated batches",
        )
        ax.legend(fontsize=8)
        save(name)
    (directory / "summary.json").write_text(
        json.dumps(summary, indent=2), encoding="utf-8"
    )
    report = [
        "<!-- Reports measured routing parallelism, scheduling trade-offs and hardware limits. -->",
        "# Geneva parallel scaling",
        "",
        f"Machine: {info['cpu']}; {info.get('physical_cores','unknown')} physical cores, {info['logical_threads']} logical threads. {info['os']}.",
        "",
        "Three timed repetitions per point after two complete warmup batches; medians below. Each row routes the same 1,000 requests, including reconstruction. Loading, result validation and profiling passes are separate. No route-result cache is used. All schedulers and worker counts produced the same ordered route fingerprint, covering legs, costs, arrivals, boardings and endpoint choices.",
        "",
        "| Workers | Adaptive Rayon (s) | Dynamic queue (s) | Prior-batch priorities (s) | Dynamic speedup | Priority speedup |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    indexed = {
        p: {r["threads"]: r for r in rows} for p, rows in summary["policies"].items()
    }
    for n, row in indexed["dynamic"].items():
        old, learned = indexed["rayon"][n], indexed["learned"][n]
        report.append(
            f"| {n} | {old['seconds']:.3f} | {row['seconds']:.3f} | {learned['seconds']:.3f} | {row['speedup']:.2f}x | {learned['speedup']:.2f}x |"
        )
    if "auto" in indexed:
        report += [
            "",
            "## Default automatic scheduling",
            "",
            "| Workers | Auto median (s) | Auto speedup | Query-wall occupancy |",
            "|---|---:|---:|---:|",
        ]
        for n, r in indexed["auto"].items():
            report.append(
                f"| {n} | {r['seconds']:.3f} | {r['speedup']:.2f}x | {100*r['worker_wall_occupancy']:.1f}% |"
            )
    report += [
        "",
        "## What changed",
        "",
        "The default auto policy starts with adaptive Rayon for cold weighted work, learns a bounded history of query fingerprints and execution costs, then selects a dynamic priority queue once at least half a batch is familiar. No itineraries or person attributes are stored in that history. Setting execution.work_estimate_capacity to zero disables automatic learning. Dynamic claiming uses a persistent work loop on each worker. One loop runs on each Rayon worker and claims one query at a time through an atomic counter. Results are published into per-input slots and returned in original order. Timetables/indices remain shared and workspaces remain thread-local. Arrival-only routing retains adaptive Rayon splitting by default, avoiding fine-grained dispatch overhead for very short/unreachable queries.",
        "",
        "For repeated work, callers can supply prior query costs. Expensive jobs are then claimed first. The priority measurements obtain estimates from actual preceding parallel warmup work, not from serial timings or the timed batch being predicted. Automatic mode also includes fingerprint lookup, score updates and lightweight clocks inside the measured batch time. Initial geometry/timetable estimates are also available but were not consistently helpful in pilots, so they are optional. Explicit-priority mode requires caller-aligned estimates; auto mode uses its documented bounded advisory history. Neither mode caches routes.",
        "",
        "## Interpreting scaling",
        "",
        "Ideal linear scaling is a reference line, not a guarantee. This host has fewer physical cores than logical threads. At higher concurrency, threads share execution resources, cache, memory bandwidth and power/clock budgets. The profiling pass reports the sum of query wall times, including OS descheduling: it cannot identify which hardware effect dominates. Worker wall occupancy is time inside query calls divided by total worker wall capacity, not measured CPU utilization. The profiling pass is separate from the timing medians.",
        "",
        "A near-full work queue can coexist with sublinear throughput if queries themselves take longer under concurrency. The longest individual query also bounds batch completion because one route stays single-threaded. Use large batches and a single shared routing pool; creating one pool per caller can oversubscribe the machine. Coordinate the Rust pool with other application worker pools when integrating with MATSim.",
        "",
        "## Scope",
        "",
        "This is one mobile CPU and one Geneva query mix, not a scaling guarantee for a 24/48-core server. Rerun on the target server. Trial timings for fine-grained stealing, geometry estimates and optional mimalloc are retained in pilot/queue/mimalloc JSON files. The alternate allocator showed no consistent improvement here; system allocation remains the default. The fast-allocator Cargo feature affects application binaries only, never installs an allocator in the library, and requires target-specific measurement.",
        "",
    ]
    for name in ("runtime", "speedup", "occupancy", "work_inflation"):
        report += [f"![{name}](figures/{name}.png)", ""]
    (directory / "REPORT.md").write_text("\n".join(report), encoding="utf-8")
    metadata = dict(
        machine=info,
        generated_at_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        inputs={p: digest(directory / f"scaling_{p}.json") for p in policies},
    )
    (directory / "metadata.json").write_text(
        json.dumps(metadata, indent=2), encoding="utf-8"
    )
    print(json.dumps({"machine": info, "policies": summary["policies"]}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    analyze(parser.parse_args().directory)
