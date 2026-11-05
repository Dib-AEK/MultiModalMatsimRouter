"""Builds version-matched native libraries and a standalone Java client on Windows/Linux/macOS."""

from pathlib import Path
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def run(*args, env=None):
    subprocess.run([str(a) for a in args], cwd=ROOT, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument(
        "--test",
        action="store_true",
        help="Run Rust, Python FFI and standalone Java integration checks",
    )
    args = parser.parse_args()
    cargo = shutil.which("cargo") or str(
        Path.home() / ".cargo" / "bin" / ("cargo.exe" if os.name == "nt" else "cargo")
    )
    run(cargo, "build", "--release", "--locked", "-p", "mmrouter-ffi", "-p", "matsim-pt-router-jni")
    prefix, suffix = {
        "Windows": ("", ".dll"),
        "Linux": ("lib", ".so"),
        "Darwin": ("lib", ".dylib"),
    }[platform.system()]
    ffi = ROOT / "target/release" / (prefix + "multimodal_router" + suffix)
    jni = ROOT / "target/release" / (prefix + "matsim_pt_router" + suffix)
    classes = ROOT / "target/java/client-classes"
    classes.mkdir(parents=True, exist_ok=True)
    sources = list((ROOT / "java/client/src/main/java").rglob("*.java"))
    run("javac", "--release", "17", "-d", classes, *sources)
    (classes / "native").mkdir(exist_ok=True)
    shutil.copy2(jni, classes / "native" / jni.name)
    jar = ROOT / "target/java/mmrouter-java-0.2.0.jar"
    run("jar", "--create", "--file", jar, "-C", classes, ".")
    matsim_classes = ROOT / "target/java/matsim-classes"
    matsim_classes.mkdir(parents=True, exist_ok=True)
    run(
        "javac",
        "--release",
        "17",
        "-cp",
        jar,
        "-d",
        matsim_classes,
        *list((ROOT / "java/matsim/src/main/java").rglob("*.java"))
    )
    matsim_jar = ROOT / "target/java/mmrouter-matsim-0.2.0.jar"
    run("jar", "--create", "--file", matsim_jar, "-C", matsim_classes, ".")
    artifacts = []
    for path in (ffi, jni, jar, matsim_jar):
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        artifacts.append({"file": str(path.relative_to(ROOT)), "sha256": digest})
    (ROOT / "target/native-build.json").write_text(
        json.dumps(
            {
                "platform": platform.platform(),
                "protocol_version": 1,
                "client_api_version": 2,
                "artifacts": artifacts,
            },
            indent=2,
        )
    )
    if args.test:
        run(cargo, "test", "--workspace", "--", "--test-threads=12")
        env = os.environ.copy()
        env["PYTHONPATH"] = str(ROOT / "python/src")
        env["MMROUTER_LIBRARY"] = str(ffi)
        run(sys.executable, "-m", "unittest", "discover", "-s", "python/tests", "-v", env=env)
        test_classes = ROOT / "target/java/test-classes"
        test_classes.mkdir(parents=True, exist_ok=True)
        java_classpath = str(jar) + os.pathsep + str(matsim_jar)
        run(
            "javac",
            "--release",
            "17",
            "-cp",
            java_classpath,
            "-d",
            test_classes,
            ROOT / "java/client/src/test/java/InteropTest.java",
            ROOT / "examples/interop/JavaExample.java",
            *list((ROOT / "java/matsim/examples").glob("*.java"))
        )
        java_version = subprocess.run(
            ["java", "-version"], capture_output=True, text=True, check=True
        ).stderr
        # The standalone client supports JDK 17; native-access enforcement flags were added later.
        import re

        major = int(re.search(r'version "(\d+)', java_version).group(1))
        native_access = ["--enable-native-access=ALL-UNNAMED"] if major >= 21 else []
        run(
            "java",
            "-Xmx10g",
            *native_access,
            "-cp",
            str(test_classes) + os.pathsep + java_classpath,
            "InteropTest",
            ROOT / "examples/interop/router.toml",
            jni
        )
        run(
            "java",
            "-Xmx10g",
            *native_access,
            "-cp",
            str(test_classes) + os.pathsep + java_classpath,
            "AsyncMatsimExample",
            ROOT / "examples/interop/matsim_config.xml"
        )
    print("Built native libraries and Java client; see target/native-build.json")


if __name__ == "__main__":
    main()
