"""Packages and smoke-tests host-native wheels, Java jars and a ready-to-use release archive."""

from pathlib import Path
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import tomllib
import venv
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    subprocess.run([str(arg) for arg in args], cwd=ROOT, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument(
        "--skip-build", action="store_true", help="Reuse tested native libraries and jars"
    )
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "python/pyproject.toml").read_text())["project"]["version"]
    if (
        os.environ.get("GITHUB_REF_TYPE") == "tag"
        and os.environ.get("GITHUB_REF_NAME") != "v" + version
    ):
        raise ValueError("Release tag must match the Python client version")
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}[
        machine
    ]
    system = platform.system()
    label = {"Windows": "windows", "Linux": "linux", "Darwin": "macos"}[system] + "-" + arch
    if not args.skip_build:
        run(sys.executable, ROOT / "tools/build.py", "--test")
    cargo = shutil.which("cargo") or Path.home() / ".cargo/bin" / (
        "cargo.exe" if os.name == "nt" else "cargo"
    )
    run(cargo, "build", "--release", "--locked", "--bin", "mmrouter")
    manifest = json.loads((ROOT / "target/native-build.json").read_text())
    artifacts = [ROOT / item["file"] for item in manifest["artifacts"]]
    # The manifest prevents accidentally packaging a stale or mismatched local library.
    for item, path in zip(manifest["artifacts"], artifacts):
        if hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
            raise ValueError(f"Changed build artifact: {path}; rerun tools/build.py")
    ffi = next(p for p in artifacts if "multimodal_router" in p.name)
    output = ROOT / "target/distributions"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="release-", dir=ROOT / "target") as temporary:
        stage = Path(temporary)
        package = stage / "python"
        shutil.copytree(
            ROOT / "python",
            package,
            ignore=shutil.ignore_patterns("__pycache__", "*.egg-info", "build", "dist", "_native"),
        )
        native = package / "src/multimodal_router/_native"
        native.mkdir()
        shutil.copy2(ffi, native / ffi.name)
        wheels = stage / "wheels"
        run(sys.executable, "-m", "pip", "wheel", package, "--no-deps", "--wheel-dir", wheels)
        (wheel,) = wheels.glob("*.whl")
        # Install in a clean environment and run outside the checkout without discovery variables.
        environment = stage / "environment"
        venv.EnvBuilder(with_pip=True).create(environment)
        python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        run(python, "-m", "pip", "install", "--no-deps", wheel)
        env = {k: v for k, v in os.environ.items() if k not in {"PYTHONPATH", "MMROUTER_LIBRARY"}}
        smoke = """from concurrent.futures import ThreadPoolExecutor
from multimodal_router import Router
import sys
with Router.load(sys.argv[1]) as router:
    with ThreadPoolExecutor(max_workers=12) as pool:
        routes = list(pool.map(lambda _: router.worker().route('A', 'B', '07:59:00'), range(120)))
    assert all(r['arrival'] == 29400000 for r in routes)
    assert router.route_coordinates((0,0), (1000,0), '07:59:00') is not None
print('Installed binary wheel passed: 120 shared concurrent queries and coordinate routing')
"""
        subprocess.run(
            [str(python), "-c", smoke, str(ROOT / "examples/interop/matsim_config.xml")],
            cwd=stage,
            env=env,
            check=True,
        )
        bundle = stage / f"MultiModalMatsimRouter-{version}-{label}"
        for folder in ("native", "java", "python/examples", "bin"):
            (bundle / folder).mkdir(parents=True, exist_ok=True)
        for path in artifacts:
            shutil.copy2(path, bundle / ("java" if path.suffix == ".jar" else "native") / path.name)
        shutil.copy2(wheel, bundle / "python" / wheel.name)
        executable = ROOT / "target/release" / ("mmrouter.exe" if os.name == "nt" else "mmrouter")
        run(executable, "--help", stdout=subprocess.DEVNULL)
        shutil.copy2(executable, bundle / "bin" / executable.name)
        shutil.copytree(
            ROOT / "examples/interop",
            bundle / "examples/interop",
            ignore=shutil.ignore_patterns("__pycache__"),
        )
        for source, target in [
            ("python/examples/async_routing.py", "python/examples/async_routing.py"),
            ("java/matsim/examples/AsyncMatsimExample.java", "examples/AsyncMatsimExample.java"),
            ("LICENSE", "LICENSE"),
            ("docs/binary-distribution.md", "README.md"),
        ]:
            shutil.copy2(ROOT / source, bundle / target)
        info = {
            "version": version,
            "platform": label,
            "build_platform": platform.platform(),
            "revision": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip(),
            "protocol_version": 1,
            "client_api_version": 2,
        }
        (bundle / "build-info.json").write_text(json.dumps(info, indent=2))
        checksums = [
            f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(bundle).as_posix()}"
            for p in sorted(bundle.rglob("*"))
            if p.is_file()
        ]
        (bundle / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n")
        archive = output / (bundle.name + ".zip")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zip_file:
            for path in sorted(bundle.rglob("*")):
                if path.is_file():
                    zip_file.write(path, path.relative_to(bundle.parent))
        shutil.copy2(wheel, output / wheel.name)
        for path in (archive, output / wheel.name):
            path.with_name(path.name + ".sha256").write_text(
                f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
            )
        print(f"Packaged {archive.name} and {wheel.name}")


if __name__ == "__main__":
    main()
