# Samples observed MATSim trips and compares weighted routers using identical candidates and coefficients.
import argparse
import csv
import datetime
import gzip
import json
import math
import os
import platform
import random
import shutil
import xml.etree.ElementTree as ET
from pathlib import Path

from common import input_paths, millis, prepare_schedule, schedule_data
from run_benchmark import ROOT, HERE, digest, run


def profile_from_config(path):
    modules = {m.get("name"): m for m in ET.parse(path).getroot().findall("module")}

    def params(element):
        return {p.get("name"): p.get("value") for p in element.findall("param")}

    eq = params(modules["eqasim:raptor"])
    swiss = params(modules["swissRailRaptor"])
    transit = params(modules["transitRouter"])
    for flag in (
        "useCapacityConstraints",
        "useIntermodalAccessEgress",
        "useModeMappingForPassengers",
        "useRangeQuery",
    ):
        if swiss.get(flag, "false") != "false":
            raise ValueError(f"Unsupported benchmark setting: {flag}")
    if swiss.get("transferCalculation", "Initial") != "Initial":
        raise ValueError("This benchmark supports Initial transfer calculation")
    walks = {
        s["mode"]: s
        for e in modules["routing"].findall("parameterset")
        if (s := params(e)).get("mode") in ("transit_walk", "non_network_walk", "walk")
    }
    walk = next(
        walks[m] for m in ("transit_walk", "non_network_walk", "walk") if m in walks
    )
    factor = float(walk["beelineDistanceFactor"])
    effective_speed = float(walk["teleportedModeSpeed"]) / factor
    matrix = [
        dict(from_=s["fromMode"], to=s["toMode"], cost=float(s["transferPenalty"]))
        for e in modules["swissRailRaptor"].findall("parameterset")
        if e.get("type") == "modeToModeTransferPenalty" and (s := params(e))
    ]
    for entry in matrix:
        entry["from"] = entry.pop("from_")
    result = {
        "walk_per_hour": -float(eq.get("walkTime_u_h", -7)),
        "wait_per_hour": -float(eq.get("waitTime_u_h", -6)),
        "other_per_hour": -float(eq.get("travelTimeOther_u_h", -7)),
        "modes_per_hour": {
            mode: -float(eq.get(key, -7))
            for mode, key in (
                ("rail", "travelTimeRail_u_h"),
                ("subway", "travelTimeSubway_u_h"),
                ("bus", "travelTimeBus_u_h"),
                ("tram", "travelTimeTram_u_h"),
            )
        },
        "transfer_base": -float(eq.get("perTransfer_u", -1)),
        "transfer_per_travel_hour": float(
            swiss.get("transferPenaltyCostPerTravelTimeHour", 0)
        ),
        "transfer_min": swiss.get("transferPenaltyMinCost", "-Infinity"),
        "transfer_max": swiss.get("transferPenaltyMaxCost", "Infinity"),
        "mode_transfers": matrix,
        "effective_walk_speed_m_s": effective_speed,
        "walk_distance_factor": factor,
        "transfer_radius_m": float(
            transit.get("maxBeelineWalkConnectionDistance", 100)
        ),
        "transfer_buffer_s": float(transit.get("additionalTransferTime", 0)),
        "transfer_walk_margin_s": float(swiss.get("transferWalkMargin", 5)),
        "search_radius_m": float(transit.get("searchRadius", 1000)),
        "extension_radius_m": float(transit.get("extensionRadius", 200)),
    }
    if matrix and result["transfer_per_travel_hour"] != 0:
        raise ValueError(
            "Raptor forbids time-dependent and mode-specific transfer penalties together"
        )
    return result


def sample_trips(path, count, seed, mode):
    """Uniform reservoir over eligible observed trips, retaining original IDs, coordinates and times."""
    rng = random.Random(seed)
    sample, eligible = [], 0
    with (gzip.open if path.suffix == ".gz" else open)(
        path, "rt", encoding="utf-8-sig", newline=""
    ) as f:
        for row in csv.DictReader(f, delimiter=";"):
            if mode != "all" and row["main_mode"] != mode:
                continue
            try:
                coords = [
                    float(row[k]) for k in ("start_x", "start_y", "end_x", "end_y")
                ]
                valid = (
                    all(math.isfinite(x) for x in coords)
                    and millis(row["dep_time"]) >= 0
                )
            except (ValueError, TypeError):
                continue
            if not valid or coords[:2] == coords[2:]:
                continue
            eligible += 1
            selected = dict(
                trip_id=row["trip_id"],
                person=row["person"],
                departure=row["dep_time"],
                origin=coords[:2],
                destination=coords[2:],
                observed_mode=row["main_mode"],
            )
            if len(sample) < count:
                sample.append(selected)
            elif (index := rng.randrange(eligible)) < count:
                sample[index] = selected
    if len(sample) < count:
        raise ValueError(f"Only {eligible} eligible trips, requested {count}")
    return sample, eligible


def candidates(coord, stops, served, profile):
    """Shared, deterministic radius selection; both engines receive exactly these walking inputs."""
    distances = sorted((math.dist(coord, stops[s]), s) for s in served)
    radius = profile["search_radius_m"]
    selected = [(d, s) for d, s in distances if d <= radius]
    if len(selected) < 2:
        radius = distances[0][0] + profile["extension_radius_m"]
        selected = [(d, s) for d, s in distances if d <= radius]
    result = []
    for d, stop in selected:
        # Whole seconds match Raptor's integer event-time representation; costs use the same duration.
        duration = math.ceil(d / profile["effective_walk_speed_m_s"]) * 1000
        result.append(
            dict(
                stop=stop,
                duration=duration,
                cost=duration * profile["walk_per_hour"] / 3_600_000,
                distance=d * profile["walk_distance_factor"],
            )
        )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--trips", type=Path, required=True)
    parser.add_argument("--matsim-jar", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=HERE / "trip_results")
    parser.add_argument("--queries", type=int, default=1000)
    parser.add_argument("--threads", type=int, default=12)
    parser.add_argument("--warmup", type=int, default=200)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument(
        "--trip-mode",
        default="pt",
        help="Observed main mode to sample; 'all' includes all modes",
    )
    parser.add_argument("--plot-only", action="store_true")
    parser.add_argument(
        "--baseline",
        type=Path,
        help="Earlier run on exactly the same requests; compare Rust results and timings",
    )
    args = parser.parse_args()
    if args.queries <= 0 or args.threads <= 0 or args.warmup < 0:
        parser.error("Invalid counts")
    config, trips, jar, output = [
        p.resolve() for p in (args.config, args.trips, args.matsim_jar, args.output)
    ]
    output.mkdir(parents=True, exist_ok=True)
    if args.plot_only:
        from plot_trips import analyze

        analyze(output)
        return
    original, network, _ = input_paths(config)
    schedule = output / "benchmark_schedule.xml.gz"
    normalization = prepare_schedule(original, schedule, preserve_first_arrival=True)
    stops, _, served = schedule_data(schedule)
    profile = profile_from_config(config)
    rows, eligible = sample_trips(trips, args.queries, args.seed, args.trip_mode)
    for i, row in enumerate(rows):
        row.update(
            id=str(i),
            access=candidates(row["origin"], stops, served, profile),
            egress=candidates(row["destination"], stops, served, profile),
        )
        row.update(from_=row["access"][0]["stop"], to=row["egress"][0]["stop"])
        row["from"] = row.pop("from_")
    requests = output / "requests.jsonl"
    requests.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    profile_path = output / "profile.json"
    profile_path.write_text(json.dumps(profile, indent=2), encoding="utf-8")
    router_config = output / "router.toml"
    router_config.write_text(
        "\n".join(
            [
                "# Imports the real Eqasim costs; benchmark graph is replaced with Raptor's exported transfers.",
                "[input]",
                'format = "matsim"',
                'coordinates = "projected_meters"',
                "path = " + json.dumps(schedule.as_posix()),
                "[preprocessing]",
                "walking_radius_m = 0.0",
                "default_transfer_seconds = 0.0",
                "[routing]",
                'objective = "generalized_cost"',
                "matsim_config = " + json.dumps(config.as_posix()),
                "max_boardings = 21",
                "max_duration_seconds = 108000.0",
                "[execution]",
                f"threads = {args.threads}",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    cargo = shutil.which("cargo") or str(
        Path.home() / ".cargo/bin" / ("cargo.exe" if os.name == "nt" else "cargo")
    )
    run(
        [cargo, "build", "--release", "--example", "geneva-trips"], output / "cargo.log"
    )
    classes = output / "java-classes"
    classes.mkdir(exist_ok=True)
    run(
        ["javac", "-cp", jar, "-d", classes, HERE / "SwissRailRaptorBenchmark.java"],
        output / "javac.log",
    )
    run(
        [
            "java",
            "-Xmx10g",
            "-cp",
            str(classes) + os.pathsep + str(jar),
            "ch.sbb.matsim.routing.pt.raptor.SwissRailRaptorBenchmark",
            schedule,
            network,
            requests,
            output,
            args.threads,
            args.warmup,
            profile["transfer_buffer_s"],
            profile_path,
        ],
        output / "swiss.log",
    )
    exe = (
        ROOT
        / "target/release/examples"
        / ("geneva-trips.exe" if os.name == "nt" else "geneva-trips")
    )
    run(
        [exe, router_config, requests, output / "transfers.json", output, args.warmup],
        output / "rust.log",
    )
    metadata = dict(
        queries=args.queries,
        threads=args.threads,
        warmup=args.warmup,
        seed=args.seed,
        eligible_trips=eligible,
        trip_mode=args.trip_mode,
        input_normalization=normalization,
        profile=profile,
        schedule=str(schedule),
        trips=str(trips),
        config=str(config),
        trips_sha256=digest(trips),
        config_sha256=digest(config),
        jar_sha256=digest(jar),
        requests_sha256=digest(requests),
        transfer_graph_sha256=digest(output / "transfers.json"),
        original_schedule_sha256=digest(original),
        schedule_sha256=digest(schedule),
        network_sha256=digest(network),
        rust_executable_sha256=digest(exe),
        cargo_lock_sha256=digest(ROOT / "Cargo.lock"),
        cpu=platform.processor(),
        logical_cpus=os.cpu_count(),
        os=platform.platform(),
        python=platform.python_version(),
        completed_at_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        baseline=str(args.baseline.resolve()) if args.baseline else None,
    )
    (output / "metadata.json").write_text(
        json.dumps(metadata, indent=2), encoding="utf-8"
    )
    from plot_trips import analyze

    analyze(output)


if __name__ == "__main__":
    main()
