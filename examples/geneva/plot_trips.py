# Validates observed-trip journeys and compares both engines with a shared, explicit disutility scorer.
import json
import math
from pathlib import Path
from common import millis, named_legs, schedule_data, scheduled_distance


def service_segment(leg, routes):
    """Resolve a real scheduled boarding/alighting pair, preserving dwell at the boarding stop."""
    route = routes[leg["line"], leg["route"]]
    choices = []
    for i, call in enumerate(route["stops"][:-1]):
        if call["refId"] != leg["from"] or call.get("allowBoarding") == "false":
            continue
        leave = millis(call["departureOffset"])
        fixed = route["departures"].get(leg.get("departure_id"))
        start = fixed if fixed is not None else leg["departure"] - leave
        service_error = min(abs(start - d) for d in route["departures"].values())
        for last in route["stops"][i + 1 :]:
            if last["refId"] != leg["to"] or last.get("allowAlighting") == "false":
                continue
            error = (
                service_error
                + abs(start + leave - leg["departure"])
                + abs(start + millis(last["arrivalOffset"]) - leg["arrival"])
            )
            choices.append(
                (error, start + millis(call["arrivalOffset"]), route["mode"])
            )
    if not choices or min(choices)[0] > 2000:
        raise ValueError("Leg does not match a scheduled service: " + str(leg))
    return min(choices)[1:]


def score(query, journey, meta, routes, stops, profile, graph):
    """Rescore returned paths rather than comparing incompatible native engine cost totals."""
    legs = named_legs(journey, meta)

    def endpoint(value):
        stop = value["stop"]
        return meta["stops"][stop]["id"] if isinstance(stop, int) else stop

    access, egress = journey["access"], journey["egress"]
    if access is None or egress is None:
        raise ValueError("Missing access/egress leg")
    origin, destination = endpoint(access), endpoint(egress)
    # Raptor merges a final transfer walk into the egress leg. Expand it for shared validation.
    exact = any(
        e["stop"] == destination and abs(e["duration"] - egress["duration"]) <= 1000
        for e in query["egress"]
    )
    if not exact:
        choices = [
            (e, graph[destination, e["stop"]])
            for e in query["egress"]
            if (destination, e["stop"]) in graph
            and abs(graph[destination, e["stop"]] + e["duration"] - egress["duration"])
            <= 1000
        ]
        if not choices:
            raise ValueError("Merged egress does not match a shared candidate/transfer")
        selected, duration = choices[0]
        departure = legs[-1]["arrival"]
        legs.append(
            dict(
                kind="walk",
                **{"from": destination, "to": selected["stop"]},
                departure=departure,
                arrival=departure + duration,
            )
        )
        egress = selected
        destination = selected["stop"]
    for values, stop, actual in (
        (query["access"], origin, access),
        (query["egress"], destination, egress),
    ):
        expected = next(e for e in values if e["stop"] == stop)
        if abs(expected["duration"] - actual["duration"]) > 1000:
            raise ValueError("Access/egress differs from shared candidates")
    stop = origin
    now = millis(query["departure"]) + access["duration"]
    walk = access["duration"] + egress["duration"]
    waiting, vehicle_cost, penalty, boardings = 0, 0, 0, 0
    previous_mode, first, last_pt_arrival = None, None, None
    matrix = {(p["from"], p["to"]): p["cost"] for p in profile["mode_transfers"]}
    transfer_walk_since_ride = False
    for leg in legs:
        if (
            leg["from"] != stop
            or leg["departure"] < now - 1000
            or leg["arrival"] < leg["departure"]
        ):
            raise ValueError("Broken chronology/continuity")
        if leg["kind"] == "walk":
            duration = leg["arrival"] - leg["departure"]
            expected = graph.get((leg["from"], leg["to"]))
            if expected is None or abs(duration - expected) > 1000:
                raise ValueError("Transfer missing from shared graph")
            walk += duration
            transfer_walk_since_ride = True
        else:
            vehicle_arrival, mode = service_segment(leg, routes)
            boarding = max(now, vehicle_arrival)
            if boarding > leg["departure"] + 1000:
                raise ValueError("Missed scheduled departure")
            waiting += boarding - now
            vehicle_cost += (
                (leg["arrival"] - boarding)
                * profile["modes_per_hour"].get(mode, profile["other_per_hour"])
                / 3_600_000
            )
            if boardings:
                if not transfer_walk_since_ride:
                    raise ValueError("Missing transfer leg")
                penalty += min(
                    float(profile["transfer_max"]),
                    max(
                        float(profile["transfer_min"]),
                        profile["transfer_base"] + matrix.get((previous_mode, mode), 0),
                    ),
                )
            else:
                first = boarding
            last_pt_arrival = leg["arrival"]
            previous_mode = mode
            boardings += 1
            transfer_walk_since_ride = False
        stop, now = leg["to"], leg["arrival"]
    if (
        stop != destination
        or abs(journey["arrival"] - now - egress["duration"]) > 1000
        or boardings != journey["boardings"]
    ):
        raise ValueError("Journey endpoint, arrival or boarding count mismatch")
    if profile["transfer_per_travel_hour"]:
        penalty = max(boardings - 1, 0) * min(
            float(profile["transfer_max"]),
            max(
                float(profile["transfer_min"]),
                profile["transfer_base"]
                + profile["transfer_per_travel_hour"]
                * (last_pt_arrival - first)
                / 3_600_000,
            ),
        )
    cost = (
        vehicle_cost
        + walk * profile["walk_per_hour"] / 3_600_000
        + waiting * profile["wait_per_hour"] / 3_600_000
        + penalty
    )
    distance, ambiguous = scheduled_distance(legs, stops, routes)
    walk_distance = (
        math.dist(query["origin"], stops[origin])
        + math.dist(query["destination"], stops[destination])
        + sum(
            math.dist(stops[l["from"]], stops[l["to"]])
            for l in legs
            if l["kind"] == "walk"
        )
    ) * profile["walk_distance_factor"]
    return dict(
        cost=cost,
        travel_time_s=(journey["arrival"] - millis(query["departure"])) / 1000,
        transfers=max(boardings - 1, 0),
        pt_distance_m=distance,
        walk_distance_m=walk_distance,
        waiting_s=waiting / 1000,
        walking_s=walk / 1000,
        ambiguous_segments=ambiguous,
        native_cost=journey["generalized_cost"],
        native_cost_error=journey["generalized_cost"] - cost,
    )


def analyze(output):
    import pandas as pd
    import numpy as np
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    output = Path(output)
    metadata = json.loads((output / "metadata.json").read_text())
    profile = metadata["profile"]
    stops, routes, _ = schedule_data(Path(metadata["schedule"]))
    graph = {
        (r["from"], r["to"]): r["duration"]
        for r in json.loads((output / "transfers.json").read_text())
    }
    requests = [
        json.loads(l) for l in (output / "requests.jsonl").read_text().splitlines()
    ]
    engines = {}
    for name in ("rust", "swiss"):
        engines[name] = (
            json.loads((output / f"{name}_metadata.json").read_text()),
            {
                r["id"]: r
                for r in map(
                    json.loads,
                    (output / f"{name}_results.jsonl").read_text().splitlines(),
                )
            },
        )
    # The Rust importer and benchmark profile are independent readers of the same original config.
    for key in (
        "walk_per_hour",
        "wait_per_hour",
        "other_per_hour",
        "transfer_base",
        "transfer_per_travel_hour",
        "modes_per_hour",
        "mode_transfers",
    ):
        if engines["rust"][0]["costs"][key] != profile[key]:
            raise ValueError("Coefficient import mismatch: " + key)
    rows = []
    for q in requests:
        row = dict(id=q["id"], trip_id=q["trip_id"], departure=q["departure"])
        for name, (meta, values) in engines.items():
            value = values[q["id"]]
            row[name + "_runtime_ms"] = value["runtime_ns"] / 1e6
            row[name + "_reachable"] = value["journey"] is not None
            if value["journey"] is not None:
                values = score(q, value["journey"], meta, routes, stops, profile, graph)
                if name == "rust" and abs(values["native_cost_error"]) > 1e-8:
                    raise ValueError(
                        f"Rust score mismatch on request {q['id']}: {values}"
                    )
                row.update({name + "_" + k: v for k, v in values.items()})
        rows.append(row)
    frame = pd.DataFrame(rows)
    frame.to_csv(output / "comparison.csv", index=False)
    both = frame[frame.rust_reachable & frame.swiss_reachable]
    figures = output / "figures"
    figures.mkdir(exist_ok=True)

    def save(name):
        plt.tight_layout()
        for ext in ("png", "svg"):
            plt.savefig(figures / f"{name}.{ext}", dpi=160)
        plt.close()

    for column, label, scale in (
        ("cost", "Generalized disutility", 1),
        ("travel_time_s", "Travel time (min)", 60),
        ("pt_distance_m", "PT stop-chain distance (km)", 1000),
        ("transfers", "Transfers", 1),
        ("walking_s", "Walking time (min)", 60),
        ("waiting_s", "Waiting time (min)", 60),
    ):
        _, ax = plt.subplots(figsize=(6, 5))
        x, y = both["swiss_" + column] / scale, both["rust_" + column] / scale
        ax.scatter(x, y, s=12, alpha=0.4)
        top = max(x.max(), y.max(), 1)
        ax.plot([0, top], [0, top], "--", color="gray")
        ax.set(
            xlabel="SwissRailRaptor " + label,
            ylabel="Rust " + label,
            title=f"Observed trips: {len(both)} reachable in both",
        )
        save(column)
    _, ax = plt.subplots(figsize=(7, 4))
    for name in engines:
        values = np.sort(frame[name + "_runtime_ms"].to_numpy())
        ax.plot(values, np.arange(1, len(values) + 1) / len(values), label=name)
    ax.set(
        xscale="log", xlabel="Warm serial latency (ms)", ylabel="Fraction of requests"
    )
    ax.legend()
    save("runtime")
    _, ax = plt.subplots(figsize=(6, 4))
    ax.bar(
        ["SwissRailRaptor", "Rust"],
        [m["parallel_seconds"] for m, _ in (engines["swiss"], engines["rust"])],
    )
    ax.set(
        ylabel="Batch time (seconds)",
        title=f"{len(frame)} weighted requests, {metadata['threads']} workers",
    )
    save("throughput")
    summary = dict(
        requests=len(frame),
        reachable_both=len(both),
        rust_only=int((frame.rust_reachable & ~frame.swiss_reachable).sum()),
        swiss_only=int((frame.swiss_reachable & ~frame.rust_reachable).sum()),
        neither=int((~frame.rust_reachable & ~frame.swiss_reachable).sum()),
        all_returned_paths_validated=True,
        engines={},
    )
    for name, (meta, _) in engines.items():
        summary["engines"][name] = {
            k: meta[k]
            for k in (
                "serial_seconds",
                "parallel_seconds",
                "load_seconds",
                "parallel_matches_serial",
            )
        }
        summary["engines"][name].update(
            median_runtime_ms=float(frame[name + "_runtime_ms"].median()),
            p95_runtime_ms=float(frame[name + "_runtime_ms"].quantile(0.95)),
            p99_runtime_ms=float(frame[name + "_runtime_ms"].quantile(0.99)),
            max_runtime_ms=float(frame[name + "_runtime_ms"].max()),
            max_native_cost_error=float(frame[name + "_native_cost_error"].abs().max()),
        )
    summary["paired"] = dict(
        within_1s=int(
            (abs(both.rust_travel_time_s - both.swiss_travel_time_s) <= 1).sum()
        ),
        same_cost_within_1e_6=int(
            (abs(both.rust_cost - both.swiss_cost) <= 1e-6).sum()
        ),
        rust_lower_cost=int((both.rust_cost < both.swiss_cost - 1e-6).sum()),
        swiss_lower_cost=int((both.swiss_cost < both.rust_cost - 1e-6).sum()),
        same_transfer_count=int((both.rust_transfers == both.swiss_transfers).sum()),
    )
    counters = (
        "connections_scanned",
        "labels_improved",
        "boarding_rounds",
        "labels_pruned_by_history",
        "labels_pruned_by_bound",
        "transfers_considered",
        "transfers_pruned",
        "path_nodes",
        "bound_settled_stops",
    )

    def work(results):
        return {
            key: (
                sum(
                    (r["journey"] or {}).get("statistics", {}).get(key, 0)
                    for r in results.values()
                )
                if any(
                    key in (r["journey"] or {}).get("statistics", {})
                    for r in results.values()
                )
                else None
            )
            for key in counters
        }

    summary["rust_index"] = engines["rust"][0].get("index")
    summary["search_work"] = dict(current=work(engines["rust"][1]))
    if metadata.get("baseline"):
        baseline = Path(metadata["baseline"])
        previous = {
            r["id"]: r
            for r in map(
                json.loads, (baseline / "rust_results.jsonl").read_text().splitlines()
            )
        }
        old_requests = (baseline / "requests.jsonl").read_bytes()
        if old_requests != (output / "requests.jsonl").read_bytes():
            raise ValueError(
                "Baseline requests differ; refusing a misleading comparison"
            )
        quality = dict(same_cost=0, same_arrival_and_boardings=0, same_legs=0)
        for id, current in engines["rust"][1].items():
            a, b = previous[id]["journey"], current["journey"]
            if (a is None) != (b is None):
                raise ValueError("Baseline reachability changed: " + id)
            if a is None:
                for k in quality:
                    quality[k] += 1
                continue
            if abs(a["generalized_cost"] - b["generalized_cost"]) > 1e-9 * (
                1 + abs(a["generalized_cost"])
            ):
                raise ValueError("Baseline optimal cost changed: " + id)
            quality["same_cost"] += 1
            quality["same_arrival_and_boardings"] += int(
                (a["arrival"], a["boardings"]) == (b["arrival"], b["boardings"])
            )
            quality["same_legs"] += int(a["legs"] == b["legs"])
        before = json.loads((baseline / "rust_metadata.json").read_text())
        summary["search_work"]["before"] = work(previous)
        old_times = [r["runtime_ns"] / 1e6 for r in previous.values()]
        summary["baseline"] = dict(
            path=str(baseline),
            quality=quality,
            old_threads=before["threads"],
            new_threads=engines["rust"][0]["threads"],
            old_serial_seconds=before["serial_seconds"],
            old_parallel_seconds=before["parallel_seconds"],
            old_max_query_ms=max(old_times),
            serial_speedup=before["serial_seconds"]
            / engines["rust"][0]["serial_seconds"],
            parallel_speedup=before["parallel_seconds"]
            / engines["rust"][0]["parallel_seconds"],
        )
        _, ax = plt.subplots(figsize=(7, 4))
        x = np.arange(2)
        width = 0.25
        for offset, label, times in [
            (
                -width,
                "Previous Rust",
                [before["serial_seconds"], before["parallel_seconds"]],
            ),
            (
                0,
                "Optimized Rust",
                [
                    engines["rust"][0]["serial_seconds"],
                    engines["rust"][0]["parallel_seconds"],
                ],
            ),
            (
                width,
                "SwissRailRaptor",
                [
                    engines["swiss"][0]["serial_seconds"],
                    engines["swiss"][0]["parallel_seconds"],
                ],
            ),
        ]:
            bars = ax.bar(
                x + offset,
                times,
                width,
                label=label,
                color={
                    "Previous Rust": "#bab0ac",
                    "Optimized Rust": "#4c78a8",
                    "SwissRailRaptor": "#f58518",
                }[label],
            )
            ax.bar_label(bars, fmt="%.2f", fontsize=8)
        ax.set(
            xticks=x,
            xticklabels=["Serial", f"{metadata['threads']} workers"],
            ylabel="Total routing time (seconds)",
            title=f"{len(frame)} observed Geneva PT trips",
        )
        ax.legend()
        save("before_after")
    (output / "summary.json").write_text(
        json.dumps(summary, indent=2), encoding="utf-8"
    )
    report = [
        "<!-- Reports measured routing comparisons on sampled observed Geneva PT trips. -->",
        "# Observed-trip comparison with Eqasim costs",
        "",
        f"{len(frame)} sampled trips from {metadata['eligible_trips']} eligible observed {metadata['trip_mode']} trips; seed {metadata['seed']}; {metadata['threads']} workers.",
        "",
        "| Engine | Serial (s) | Parallel (s) | Median latency (ms) | p95 (ms) | Max (ms) |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for name, values in summary["engines"].items():
        report.append(
            f"| {name} | {values['serial_seconds']:.3f} | {values['parallel_seconds']:.3f} | {values['median_runtime_ms']:.3f} | {values['p95_runtime_ms']:.3f} | {values['max_runtime_ms']:.3f} |"
        )
    if "baseline" in summary:
        b = summary["baseline"]
        report += [
            "",
            "## Before and after optimization",
            "",
            f"Same requests and {b['new_threads']} workers: serial speedup {b['serial_speedup']:.2f}x; parallel speedup {b['parallel_speedup']:.2f}x.",
            "",
            "Preserved results: `" + json.dumps(b["quality"]) + "`.",
            "",
            f"Previous worst query: {b['old_max_query_ms']:.2f} ms; current worst query: {summary['engines']['rust']['max_runtime_ms']:.2f} ms.",
            "",
            "![Before and after](figures/before_after.png)",
            "",
        ]
    if summary["rust_index"]:
        ix = summary["rust_index"]
        work_now = summary["search_work"]["current"]
        work_old = summary["search_work"].get("before", {})
        report += [
            "",
            "## Search work and preprocessing",
            "",
            f"Shared weighted index: {ix['allocated_bytes']/1048576:.2f} MiB; {ix['relaxed_edges']} relaxed edges, {ix['transfer_entries']} physical transfer entries and {ix.get('boarding_events',0)} indexed platform departure events. This excludes the base timetable, cost settings, construction temporaries and worker workspaces.",
            "",
            "| Work counter | Before | Optimized |",
            "|---|---:|---:|",
            *[
                f"| {k} | {work_old.get(k) if work_old.get(k) is not None else 'not recorded'} | {work_now[k]} |"
                for k in counters
            ],
            "",
            "Destination bounds are recomputed inside every timed query. They ignore waiting, penalties and restrictions to provide optimistic costs. Cross-round dominance preserves previous mode and, when needed, first boarding time. Transfers are sorted once; platform departure/mode indices reject infeasible or provably too costly next boardings. Later express/overtaking services remain searchable. All four exact accelerations default on and can be disabled independently under routing.search. No landmarks, destination-cell arc flags or transfer hierarchy are claimed by this implementation.",
            "",
        ]
    report += [
        "",
        "All returned journeys were validated against the schedule, shared transfer graph and endpoint walking times. Rust's reported cost matches the independent scorer.",
        "",
        "Reachability and agreement: `"
        + json.dumps(
            {
                k: v
                for k, v in summary.items()
                if k not in ("engines", "rust_index", "search_work", "baseline")
            }
        )
        + "`",
        "",
        "## Shared inputs",
        "",
        "Actual origin/destination coordinates and departure times are preserved. Both engines receive exactly the same candidate stops and access/egress durations. Candidates use the configured search radius, expanding once to nearest-stop distance plus extension radius if fewer than two stops are found. Candidate selection is outside timing. Failed routes do not trigger further radius retries in this core benchmark. Pure walking alternatives are excluded.",
        "",
        "Eqasim mode, walking and waiting coefficients and the complete mode-transfer penalty matrix are imported from the original config. Positive disutility is negative utility; hourly coefficients are divided by 3600 for seconds. The fixed Eqasim perTransfer_u overrides SwissRailRaptor's generic base. See profile.json and router.toml. This is a routing objective, not the entire mode-choice utility or fare model.",
        "",
        "Raptor builds transfers using the original schedule's minimum transfer times and configured radius/speed. Their stop-pair union is exported to Rust, including same-stop times; walking time is charged at the walking coefficient. Rust allows one direct transfer between boardings, with no walking-chain closure. Raptor also filters transfers per route pair and uses its normal stopping heuristic; the algorithms need not choose identical paths. Rust considers up to 21 boardings within 30 hours.",
        "",
        "## Native Raptor cost caveat",
        "",
        "The bundled MATSim 2026 ModeSpecificTransferCostCalculator returns an additive penalty on each invocation. Its core invokes it both during transfer handling and while evaluating the following transit ride; the cached transfer provider can also retain another transfer. We keep that unmodified reference behavior and record its native cost separately. The cost figure and CSV *_cost columns independently rescore each returned itinerary with one configured mode-pair penalty per actual transfer. Native totals and differences are retained in *_native_cost and *_native_cost_error. Matching coefficients does not establish identical native objectives in this build.",
        "",
        "## Timing and scope",
        "",
        "Release Rust; Java -Xmx10g. Warm serial timing includes core search and reconstruction; parallel timing includes scheduling and result collection. Import, candidate search, transfer export, JSON, validation and figures are outside routing timing. No result cache. Java also loads the network, so load times are different work. This is a single-machine example, not a whole-simulation speedup claim.",
        "",
        "The source schedule and trips are unchanged. A derived schedule excludes the malformed route and normalizes unused endpoint offsets; metadata.json records details and hashes. Distances are scheduled-stop-chain geometry, not network-link distances. Transfer-walk margin affects MATSim leg conversion, not this core routing objective.",
        "",
    ]
    for name in (
        "cost",
        "travel_time_s",
        "pt_distance_m",
        "transfers",
        "walking_s",
        "waiting_s",
        "runtime",
        "throughput",
    ):
        report += [f"![{name}](figures/{name}.png)", ""]
    (output / "REPORT.md").write_text("\n".join(report), encoding="utf-8")
    print(json.dumps(summary, indent=2))
