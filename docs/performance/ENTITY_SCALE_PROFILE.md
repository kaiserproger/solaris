# 40k hostile entities / 60 clients profile

This benchmark is an explicit release-only profiling workload. It does not run in the normal test suite.

## Default workload

- 60 real headless TCP clients using the complete Handshake -> Login -> Configuration -> Play path.
- 16 regional-owner regions arranged as a 4x4 grid.
- 2,500 hostile entities per region, 40,000 total.
- Mixed zombies, skeletons, and creepers with normal entity facts and default hostile goals.
- Clients are teleported across all regions before entity publication.
- 200 warm-up ticks followed by 1,200 measured ticks and one extra metrics publication window.
- View distance 2. Each region has at least three clients; entities are kept inside the central four chunks so the whole regional population is active.

The runtime still applies its bounded overload policy. The report includes the regional-owner lane count, the estimated `owner_lanes * 256` update budget per tick, and the estimated number of ticks required to rotate through the complete population.

## Run

```bash
tools/profile-entity-scale.sh
```

Artifacts are written to `.analysis/bench/entity-scale/`:

- `baseline.json`: Solaris tick-phase, queue, memory, client and workload data.
- `baseline.log`: complete benchmark output.
- `baseline.time.txt`: `/usr/bin/time -v` process statistics.
- `metadata.txt`: commit, toolchain, kernel, CPU and workload parameters.
- `perf.data`, `perf-report.txt`, `perf-run.json`: created only when Linux perf events are allowed.

The runner first performs an unprofiled baseline. A perf run is separate because DWARF call-stack sampling affects tick latency.

## Enable `perf`

The current development host reports `kernel.perf_event_paranoid=4`, which blocks unprivileged profiling. The runner does not change system policy automatically. To enable profiling until reboot:

```bash
sudo sysctl kernel.perf_event_paranoid=1
```

Then rerun the script. To request only the profiled run:

```bash
SOLARIS_ENTITY_BENCH_MODE=perf tools/profile-entity-scale.sh
```

The release profile receives debug symbols through environment overrides; no repository-wide Cargo profile is changed.

## Useful overrides

```bash
SOLARIS_ENTITY_BENCH_MODE=baseline \
SOLARIS_ENTITY_BENCH_CPUSET=0-5 \
SOLARIS_ENTITY_BENCH_CLIENTS=60 \
SOLARIS_ENTITY_BENCH_REGIONS=16 \
SOLARIS_ENTITY_BENCH_ENTITIES_PER_REGION=2500 \
SOLARIS_ENTITY_BENCH_WARMUP_TICKS=200 \
SOLARIS_ENTITY_BENCH_MEASURE_TICKS=1200 \
tools/profile-entity-scale.sh
```

`SOLARIS_ENTITY_BENCH_REGIONS` must be a perfect square. `entities_per_region` is bounded at 3,000 by the benchmark.

## Exact release evidence

The default workload completed on 2026-07-29 with 60 clients, 16 regions,
2,500 hostiles per region, 200 warm-up ticks, and 1,200 measured ticks. The
process remained functional for the complete measured window and shut down
cleanly.

Release result on the local Ryzen 5 7535HS host with six requested owner lanes:

- tick p50/p95/p99/max: `37.289/41.932/43.886/52.863 ms`;
- entity-goal p99: `19.838 ms`;
- entity-physics p99: `0.330 ms`;
- hostile-attack p99: `9.766 ms`;
- full-population rotation estimate: `28 ticks`;
- final selected update budget: `1,458` entities/tick;
- runtime-observed memory: `1,045 MiB`; process maximum RSS: `2,358 MiB`;
- 5,131,515 client frames and 74,961,662 payload bytes;
- zero client disconnects, reliable drops, write timeouts, and pressure sheds;
- session-registry max hold: `13.673 ms`; world-storage max hold during the
  measured report: `9 us`.

This exact workload satisfies the frozen sustained p95/p99 tick requirement in
`CORE_PROFILE_MATRIX.md`. It is a focused entity-scale profile, not proof of the
full balanced/high deployment envelopes, disk-backed save throughput, or a
long-duration production soak. The post-report shutdown path emitted isolated
slow ticks while removing clients and tearing down the in-memory fixture; those
events are outside the captured 1,200-tick measurement window.

The companion regional-owner `1,500 vs 1,500` battle benchmark completed 320
ticks with 2,593 deaths, 16,402 applied attacks, and no missing follow targets.
Its release tick p50/p95/p99/max was
`18.883/29.421/32.610/35.021 ms`; goal, movement, attack, and retarget p99 were
`2.317/17.755/15.458/1.306 ms` respectively.

## Isolation

The benchmark is developed in the `bench-profile` worktree. It does not edit `mc-script`, Lua/Luau runtime files, plugin APIs, or the other agent's worktree. The `load-bench` API is feature-gated and unavailable in normal server builds.

One independent read-only review session inspected the initial benchmark diff
and named files. It was externally terminated after 129 seconds without a
verdict or actionable finding; the final integrated diff still requires one
fresh read-only review before a release claim.
