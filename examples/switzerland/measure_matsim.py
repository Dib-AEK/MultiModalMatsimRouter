"""Compile and run the eight-worker MATSim/Java versus Rust coordinate-routing comparison."""
import argparse, os, subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
for name in ["eqasim-jar","config","network","schedule","queries","library","output"]:
    p.add_argument("--"+name,type=Path,required=True)
p.add_argument("--native-only",action="store_true",help="Measure only the full native-coordinate adapter; useful when comparing two libraries")
a=p.parse_args()
root=Path(__file__).resolve().parents[2]
classes=root/"target/matsim-coordinate-benchmark";classes.mkdir(parents=True,exist_ok=True)
jar=a.eqasim_jar.resolve()
subprocess.run(["javac","-cp",str(jar),"-d",str(classes),
    str(Path(__file__).parent/"java/CompareMatsimCoordinates.java")],check=True)
a.output.parent.mkdir(parents=True,exist_ok=True)
subprocess.run(["java","-Xmx10g","--enable-native-access=ALL-UNNAMED","-cp",os.pathsep.join([str(classes),str(jar)]),
    "org.eqasim.switzerland.ch_cmdp.rustRouting.CompareMatsimCoordinates",
    *[str(x.resolve()) for x in [a.config,a.network,a.schedule,a.queries,a.library,a.output]],
    *(["native-only"] if a.native_only else [])],check=True)
