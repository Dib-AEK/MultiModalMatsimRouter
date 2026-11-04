# Produces raw comparisons, seven figures, and a measured report without rerunning either router.
import json
import math
from common import millis, named_legs, scheduled_distance, validate_journey


def analyze(output, stops, routes, metadata):
    import numpy as np
    import pandas as pd
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    plt.rcParams.update(
        {
            "figure.dpi": 140,
            "savefig.dpi": 180,
            "font.size": 10,
            "axes.spines.top": False,
            "axes.spines.right": False,
        }
    )
    requests = [
        json.loads(line)
        for line in (output / "requests.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    rust = {
        r["id"]: r
        for r in map(
            json.loads, (output / "rust_results.jsonl").read_text().splitlines()
        )
    }
    java = {
        r["id"]: r
        for r in map(
            json.loads, (output / "swiss_results.jsonl").read_text().splitlines()
        )
    }
    rm = json.loads((output / "rust_metadata.json").read_text())
    jm = json.loads((output / "swiss_metadata.json").read_text())
    records = []
    for q in requests:
        row = {
            "id": q["id"],
            "from": q["from"],
            "to": q["to"],
            "departure": q["departure"],
            "od_distance_m": math.dist(stops[q["from"]], stops[q["to"]]),
        }
        for name, source, meta in (("rust", rust, rm), ("swiss", java, jm)):
            value = source[q["id"]]
            j = value["journey"]
            row[name + "_runtime_ms"] = value["runtime_ns"] / 1e6
            row[name + "_reachable"] = j is not None
            if j is not None:
                row[name + "_travel_time_s"] = (
                    j["arrival"] - millis(q["departure"])
                ) / 1000
                row[name + "_transfers"] = max(j["boardings"] - 1, 0)
                legs = named_legs(j, meta)
                validate_journey(q, j, legs, metadata["transfer_buffer_seconds"])
                distance, ambiguous = scheduled_distance(legs, stops, routes)
                row[name + "_stop_chain_distance_m"] = distance
                row[name + "_ambiguous_distance_legs"] = ambiguous
        records.append(row)
    frame = pd.DataFrame(records)
    frame.to_csv(output / "comparison.csv", index=False)
    paired = frame[frame.rust_reachable & frame.swiss_reachable].copy()
    if not paired.empty:
        paired["difference_s"] = paired.rust_travel_time_s - paired.swiss_travel_time_s
    figures = output / "figures"
    figures.mkdir(exist_ok=True)

    def save(name):
        plt.tight_layout()
        plt.savefig(figures / (name + ".png"))
        plt.savefig(figures / (name + ".svg"))
        plt.close()

    def scatter(column, label, scale, filename):
        fig, ax = plt.subplots(figsize=(6, 5))
        if not paired.empty:
            x, y = paired["swiss_" + column] / scale, paired["rust_" + column] / scale
            ax.scatter(x, y, s=14, alpha=0.45, edgecolors="none")
            top = max(x.max(), y.max(), 1)
            ax.plot([0, top], [0, top], "--", color="gray", linewidth=1)
        ax.set(
            xlabel="SwissRailRaptor " + label,
            ylabel="Rust " + label,
            title=f"{label.capitalize()} - {len(paired)} reachable in both",
        )
        save(filename)

    scatter("travel_time_s", "travel time (minutes)", 60, "travel_times")
    scatter("stop_chain_distance_m", "stop-chain distance (km)", 1000, "distances")
    fig, ax = plt.subplots(figsize=(7, 4.5))
    for name, label in (("swiss", "SwissRailRaptor"), ("rust", "Rust CSA")):
        values = np.sort(frame[name + "_runtime_ms"].to_numpy())
        ax.plot(
            values,
            np.arange(1, len(values) + 1) / len(values),
            label=label,
            color={"swiss": "#f58518", "rust": "#4c78a8"}[name],
        )
    ax.set(
        xscale="log",
        xlabel="Serial query time (ms, core reconstruction included)",
        ylabel="Fraction of queries",
        title="Warm serial routing latency, all requests",
    )
    ax.legend()
    save("runtime_ecdf")
    fig, ax = plt.subplots(figsize=(7, 4.5))
    if not paired.empty:
        for name, label in (("swiss", "SwissRailRaptor"), ("rust", "Rust CSA")):
            values = np.sort(paired[name + "_runtime_ms"].to_numpy())
            ax.plot(
                values,
                np.arange(1, len(values) + 1) / len(values),
                label=label,
                color={"swiss": "#f58518", "rust": "#4c78a8"}[name],
            )
        ax.legend()
    ax.set(
        xscale="log",
        xlabel="Serial query time (ms)",
        ylabel="Fraction of reachable queries",
        title="Reachable journeys only: warm core latency",
    )
    save("runtime_reachable_ecdf")
    fig, ax = plt.subplots(figsize=(6, 4.5))
    if not paired.empty:
        bins = np.arange(
            -0.5, max(paired.rust_transfers.max(), paired.swiss_transfers.max()) + 1.5
        )
        ax.hist(
            [paired.swiss_transfers, paired.rust_transfers],
            bins=bins,
            label=["SwissRailRaptor", "Rust CSA"],
            color=["#f58518", "#4c78a8"],
            alpha=0.85,
        )
        ax.legend()
    ax.set(
        xlabel="Transfers (boardings - 1)",
        ylabel="Requests",
        title="Transfers for routes reachable in both",
    )
    save("transfers")
    fig, ax = plt.subplots(figsize=(6, 4.5))
    if not paired.empty:
        ax.hist(paired.difference_s / 60, bins=40, color="#4c78a8")
    ax.axvline(0, color="black", linewidth=1)
    ax.set(
        xlabel="Rust - SwissRailRaptor travel time (minutes)",
        ylabel="Requests",
        title="Paired travel-time differences",
    )
    save("travel_time_differences")
    fig, ax = plt.subplots(figsize=(6, 4.5))
    labels = [
        "Swiss serial",
        "Rust serial",
        f"Swiss {metadata['threads']} workers",
        f"Rust {metadata['threads']} workers",
    ]
    values = [
        len(frame) / jm["serial_seconds"],
        len(frame) / rm["serial_seconds"],
        len(frame) / jm["parallel_seconds"],
        len(frame) / rm["parallel_seconds"],
    ]
    bars = ax.bar(labels, values, color=["#f58518", "#4c78a8"] * 2)
    ax.bar_label(bars, fmt="%.0f", padding=3)
    ax.set(
        ylabel="Queries per second",
        title="Warm throughput, same requests; JSON I/O excluded",
    )
    ax.tick_params(axis="x", labelrotation=15)
    save("throughput")
    counts = [
        int((frame.rust_reachable & frame.swiss_reachable).sum()),
        int((~frame.rust_reachable & ~frame.swiss_reachable).sum()),
        int((frame.rust_reachable & ~frame.swiss_reachable).sum()),
        int((~frame.rust_reachable & frame.swiss_reachable).sum()),
    ]
    fig, ax = plt.subplots(figsize=(6, 4))
    bars = ax.bar(
        ["Both reachable", "Both unreachable", "Rust only", "Swiss only"],
        counts,
        color=["#54a24b", "#bab0ac", "#4c78a8", "#f58518"],
    )
    ax.bar_label(bars)
    ax.set(ylabel="Requests", title="Reachability on shared stop-to-stop requests")
    save("reachability")
    baseline_path = output / "baseline_before_dominance" / "rust_results.jsonl"
    baseline_check = None
    if baseline_path.exists():
        baseline = {
            row["id"]: row["journey"]
            for row in map(json.loads, baseline_path.read_text().splitlines())
        }
        differences = []
        for id, value in rust.items():
            old, new = baseline[id], value["journey"]
            score = lambda journey: (
                None if journey is None else (journey["arrival"], journey["boardings"])
            )
            if score(old) != score(new):
                differences.append(id)
        baseline_check = {
            "matching_arrival_and_boardings": len(rust) - len(differences),
            "changed_request_ids": differences,
        }
    summary = {
        "itinerary_validation": "all reachable itineraries validated against schedule and transfer constraints",
        "baseline_equivalence": baseline_check,
        "requests": len(frame),
        "reachability": dict(
            zip(["both", "neither", "rust_only", "swiss_only"], counts)
        ),
        "engines": {},
    }
    for name, meta in (("swiss", jm), ("rust", rm)):
        latencies = frame[name + "_runtime_ms"]
        summary["engines"][name] = {
            "serial_seconds": meta["serial_seconds"],
            "parallel_seconds": meta["parallel_seconds"],
            "load_seconds": meta["load_seconds"],
            "median_query_ms": float(latencies.median()),
            "p95_query_ms": float(latencies.quantile(0.95)),
            "reachable": int(frame[name + "_reachable"].sum()),
            "median_reachable_query_ms": float(
                frame.loc[frame[name + "_reachable"], name + "_runtime_ms"].median()
            ),
            "median_unreachable_query_ms": float(
                frame.loc[~frame[name + "_reachable"], name + "_runtime_ms"].median()
            ),
        }
    if not paired.empty:
        summary["paired"] = {
            "count": len(paired),
            "within_1s": int((paired.difference_s.abs() <= 1).sum()),
            "median_abs_difference_s": float(paired.difference_s.abs().median()),
            "max_abs_difference_s": float(paired.difference_s.abs().max()),
            "same_transfer_count": int(
                (paired.rust_transfers == paired.swiss_transfers).sum()
            ),
        }
    (output / "summary.json").write_text(
        json.dumps(summary, indent=2), encoding="utf-8"
    )
    normalization = metadata.get("input_normalization", {})
    normalization_text = f"{len(normalization.get('excluded_routes',[]))} malformed route(s) excluded; {normalization.get('normalized_unused_endpoint_offsets',0)} unused endpoint offsets normalized. Detailed IDs/reasons are in metadata.json. Original source files are unchanged."
    report = [
        "<!-- Reports measured results and limits of the controlled Geneva example. -->",
        "# Geneva: Rust CSA and SwissRailRaptor",
        "",
        f"{len(frame)} shared requests; seed {metadata['seed']}; {metadata['threads']} workers; {metadata['warmup']} warmup queries per mode.",
        "",
        "| Engine | Serial total (s) | Parallel total (s) | Median query (ms) | p95 query (ms) | Reachable |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for name, v in summary["engines"].items():
        report.append(
            f"| {name} | {v['serial_seconds']:.3f} | {v['parallel_seconds']:.3f} | {v['median_query_ms']:.4f} | {v['p95_query_ms']:.4f} | {v['reachable']} |"
        )
    report += [
        "",
        "Reachability: " + json.dumps(summary["reachability"]),
        "",
        "Paired route agreement: " + json.dumps(summary.get("paired", {})),
        "",
        "Optimized Rust versus initial Rust arrival/boarding equality: "
        + json.dumps(summary.get("baseline_equivalence")),
        "",
        "Median reachable-query latency (ms): "
        + json.dumps(
            {
                name: values["median_reachable_query_ms"]
                for name, values in summary["engines"].items()
            }
        ),
        "",
        "## Interpretation",
        "",
        "Input normalization: " + normalization_text,
        "",
        "This is a controlled **core stop-to-stop** benchmark on the real schedule, not a full simulation or the original mode-choice configuration. Walking between distinct stop IDs and coordinate access/egress are disabled. Pair-specific transfer overrides are replaced with the same constant same-stop buffer for both engines. Uniform time costs are used; mode/fare preferences are absent. Rust permits 21 boardings; SwissRailRaptor retains its normal round limit and additional-round stopping heuristic. Equal-arrival tie choices may differ.",
        "",
        "Distances sum straight segments between successive scheduled stops on the selected services, not network-link distances or origin-destination chords. Ambiguous repeated-stop segments are counted in the CSV. Distances are computed outside timing by the same analysis code. Quality figures use only requests reachable in both; latency/throughput include all requests.",
        "",
        "Serial timing includes each core search and internal reconstruction, excluding JSON I/O. Parallel timing includes scheduling and result collection (Java executor versus Rayon). Neither phase uses a route-result cache. Loading is separate; Java also loads the network, so startup is not equivalent work. Java uses -Xmx10g; Rust is optimized release. Exact inputs, hashes, CPU/OS and original settings are in metadata.json.",
        "",
        "Most requests are unreachable under the no-interstop-walking protocol. Rust's component rejection can strongly affect aggregate throughput; inspect the reachable-only latency figure as well. Restoring realistic walking may connect many components and change the comparison.",
        "",
        "This repeated-query harness is not JMH/Criterion and does not establish whole-simulation speedup. See ../README.md for reproduction and limitations.",
        "",
    ]
    if not paired.empty:
        outliers = paired[paired.difference_s.abs() > 1]
        if not outliers.empty:
            report += [
                "## Arrival-time differences over one second",
                "",
                "All returned reachable itineraries were checked against the timetable and transfer buffer. These are valid returned journeys, not a proof of equivalent optimality. The differing stopping/tie policies remain part of the benchmark.",
                "",
                "| Request | Rust time (min) | Swiss time (min) | Rust transfers | Swiss transfers |",
                "|---|---:|---:|---:|---:|",
            ]
            for _, row in outliers.iterrows():
                report.append(
                    f"| {row['id']} | {row.rust_travel_time_s/60:.2f} | {row.swiss_travel_time_s/60:.2f} | {int(row.rust_transfers)} | {int(row.swiss_transfers)} |"
                )
            report.append("")
    for name in [
        "travel_times",
        "runtime_ecdf",
        "runtime_reachable_ecdf",
        "distances",
        "transfers",
        "travel_time_differences",
        "throughput",
        "reachability",
    ]:
        report += [f"![{name}](figures/{name}.png)", ""]
    (output / "REPORT.md").write_text("\n".join(report), encoding="utf-8")
    print(json.dumps(summary, indent=2), flush=True)
