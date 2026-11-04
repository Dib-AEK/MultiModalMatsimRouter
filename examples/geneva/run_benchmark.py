# Generates shared requests, runs both core routers, records provenance, and produces comparison figures.
import argparse
import hashlib
import datetime
import json
import os
import platform
import random
import shutil
import subprocess
from pathlib import Path
from common import input_paths, schedule_data, prepare_schedule

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(command, log):
    print("Running:", " ".join(map(str, command)), flush=True)
    with open(log, "w", encoding="utf-8") as output:
        process = subprocess.run(
            list(map(str, command)), cwd=ROOT, stdout=output, stderr=subprocess.STDOUT
        )
    if process.returncode:
        raise RuntimeError(
            f"Command failed ({process.returncode}); see {log}\n"
            + Path(log).read_text(encoding="utf-8", errors="replace")[-5000:]
        )


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser(
        description="Controlled Geneva stop-to-stop router comparison"
    )
    parser.add_argument(
        "--config", type=Path, required=True, help="MATSim geneva_config.xml"
    )
    parser.add_argument(
        "--matsim-jar",
        type=Path,
        required=True,
        help="MATSim 2026.0/eqasim fat jar with SBB routing",
    )
    parser.add_argument("--output", type=Path, default=HERE / "results")
    parser.add_argument("--queries", type=int, default=1000)
    parser.add_argument("--threads", type=int, default=12)
    parser.add_argument("--warmup", type=int, default=2000)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--plot-only", action="store_true")
    args = parser.parse_args()
    if args.queries <= 0 or args.threads <= 0 or args.warmup < 0:
        parser.error("invalid counts")
    config, jar, output = (
        args.config.resolve(),
        args.matsim_jar.resolve(),
        args.output.resolve(),
    )
    output.mkdir(parents=True, exist_ok=True)
    original_schedule, network, modules = input_paths(config)
    schedule = output / "benchmark_schedule.xml.gz"
    normalization = (
        prepare_schedule(original_schedule, schedule) if not args.plot_only else None
    )
    stops, routes, used = schedule_data(schedule)
    if len(used) < 2:
        raise ValueError("Need two served stops")
    if not args.plot_only:
        rng = random.Random(args.seed)
        requests = []
        for i in range(args.queries):
            a, b = rng.sample(used, 2)
            t = rng.randrange(5 * 3600, 22 * 3600)
            requests.append(
                {
                    "id": str(i),
                    "from": a,
                    "to": b,
                    "departure": f"{t//3600:02}:{t//60%60:02}:{t%60:02}",
                }
            )
        request_file = output / "requests.jsonl"
        request_file.write_text(
            "".join(json.dumps(q) + "\n" for q in requests), encoding="utf-8"
        )
        transfer = float(
            modules.get("transitRouter", {}).get("additionalTransferTime", "5")
        )
        rust_config = output / "router.toml"
        lines = [
            "# Configures the controlled no-interstop-walking comparison on the real timetable.",
            "[input]",
            'format = "matsim"',
            "path = " + json.dumps(str(schedule).replace("\\", "/")),
            'coordinates = "projected_meters"',
            "[preprocessing]",
            "walking_radius_m = 0.0",
            "walking_speed_m_s = 1.3",
            f"default_transfer_seconds = {transfer}",
            "use_explicit_transfers = false",
            "reachability_cells = 0",
            "[routing]",
            "max_boardings = 21",
            "max_duration_seconds = 108000.0",
            "return_pareto = false",
            "[execution]",
            f"threads = {args.threads}",
            "batch_size = 256",
            "[cache]",
            'path = "geneva.mmrt"',
            "max_bytes = 2147483648",
        ]
        rust_config.write_text("\n".join(lines) + "\n", encoding="utf-8")
        cargo = shutil.which("cargo") or str(
            Path.home() / ".cargo/bin" / ("cargo.exe" if os.name == "nt" else "cargo")
        )
        run(
            [cargo, "build", "--release", "--example", "geneva-benchmark"],
            output / "cargo.log",
        )
        classes = output / "java-classes"
        classes.mkdir(exist_ok=True)
        run(
            [
                "javac",
                "-cp",
                jar,
                "-d",
                classes,
                HERE / "SwissRailRaptorBenchmark.java",
            ],
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
                request_file,
                output,
                args.threads,
                args.warmup,
                transfer,
            ],
            output / "swiss.log",
        )
        executable = (
            ROOT
            / "target/release/examples"
            / ("geneva-benchmark.exe" if os.name == "nt" else "geneva-benchmark")
        )
        run(
            [executable, rust_config, request_file, output, args.warmup],
            output / "rust.log",
        )
        metadata = {
            "scenario_config": str(config),
            "original_schedule": str(original_schedule),
            "original_schedule_sha256": digest(original_schedule),
            "config_sha256": digest(config),
            "network_sha256": digest(network),
            "completed_at_utc": datetime.datetime.now(
                datetime.timezone.utc
            ).isoformat(),
            "profile": "release, thin LTO, codegen-units=1",
            "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
            "input_normalization": normalization,
            "schedule": str(schedule),
            "network": str(network),
            "matsim_jar": str(jar),
            "jar_sha256": digest(jar),
            "schedule_sha256": digest(schedule),
            "requests_sha256": digest(request_file),
            "threads": args.threads,
            "warmup": args.warmup,
            "seed": args.seed,
            "queries": args.queries,
            "cpu": platform.processor(),
            "logical_cpus": os.cpu_count(),
            "os": platform.platform(),
            "python": platform.python_version(),
            "protocol": "same-stop transfers only; uniform time objective; no coordinate access/egress; no result cache",
            "transfer_buffer_seconds": transfer,
            "original_transit_router_config": modules.get("transitRouter", {}),
        }
        (output / "metadata.json").write_text(
            json.dumps(metadata, indent=2), encoding="utf-8"
        )
    else:
        metadata = json.loads((output / "metadata.json").read_text())
    from plot_results import analyze

    analyze(output, stops, routes, metadata)


if __name__ == "__main__":
    main()
