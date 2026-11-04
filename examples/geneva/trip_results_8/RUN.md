<!-- Records the reproducible command and warmup policy for the eight-worker comparison. -->
# Eight-worker comparison

Run from the MultiModalRouter project root:

```text
python examples/geneva/run_trips.py --config ../cmdp_scenario/geneva_config.xml --trips ../cmdp_scenario/simulation_output/output_trips.csv.gz --matsim-jar ../eqasim-java/switzerland/target/switzerland.jar --queries 1000 --threads 8 --warmup 2000 --seed 42 --output examples/geneva/trip_results_8
```

Both engines use eight routing workers and 2000 warmup requests per serial/parallel phase.
Java uses -Xmx10g. Rust uses the optimized release build, system allocator, default exact
search accelerations and automatic scheduling. The parallel warmup covers each of the 1000
queries twice, so Rust can schedule by learned execution costs. This is a warm repeated-work
comparison; every measured route is recomputed and no route-result cache is used.

Report timings are one serial pass and one parallel batch per engine. Route validation,
JSON I/O and plots are outside routing timing. Startup is reported separately. Native Raptor
costs and independently rescored costs remain separate because of the reference-build
transfer-cost accounting caveat. Original scenario and trip files remain unchanged.

The report and figures are in [REPORT.md](REPORT.md); exact inputs and hashes are in
[metadata.json](metadata.json). Earlier reports are preserved in their own directories.
