# Benchmark matrix — 2026-07-27

Status: measured stabilization evidence. This document does not declare Solaris replacement-ready and does not relax the frozen budgets in [`CORE_PROFILE_MATRIX.md`](CORE_PROFILE_MATRIX.md).

## Provenance

- Git base at the start of the run: `94b001b1dbee5330ab102396aaca3dc56150d77f`.
- The benchmark worktree was intentionally dirty while stale harness fixtures and one mislabeled stage metric were corrected. The corrections are part of the same checkpoint that records this report.
- Rust: `rustc 1.94.1 (e408947bf 2026-03-25)`, LLVM `21.1.8`.
- Host: AMD Ryzen 5 7535HS, 6 physical / 12 logical CPUs, approximately 32 GiB RAM, Linux x86_64.
- Physical-core affinity variants used CPUs `0`, `0,2`, and `0,2,4,6`; SMT siblings were not presented as distinct physical cores.
- Local vanilla sidecar: Java `26.1.2`.

`PASS` means the exact invoked gate passed. `DEGRADED` means useful measurements were produced but the frozen duration, hardware quota, build mode, or scenario envelope was not fully satisfied. `FAIL` means an exact assertion or frozen budget failed.

## Criterion light-engine matrix

Command: `cargo bench -p mc-world --bench light_engine -- --warm-up-time 1 --measurement-time 2 --sample-size 20 --noplot`.

| Variant | Estimate interval |
| --- | ---: |
| full recompute / flat | `2.978–3.001 ms` |
| full recompute / noisy | `3.761–3.886 ms` |
| full recompute / emissive scalar | `10.650–11.010 ms` |
| full recompute / emissive portable SIMD | `10.726–10.963 ms` |
| incremental 12-edit storm / flat | `1.265–1.272 ms` |
| incremental 12-edit storm / noisy | `700.1–716.8 µs` |
| centre extraction / scalar | `35.37–36.34 µs` |
| centre extraction / portable SIMD | `35.15–36.41 µs` |

The portable-SIMD variants are effectively tied with scalar on this host; no speedup is claimed.

## Entity/ECS matrix

All explicit `benchmark_report` tests were run in debug and release with four physical CPUs available. The regional-owner scaling report was additionally run with one, two, and four physical CPUs.

### Release

| Workload | Result |
| --- | ---: |
| sparse ECS, 10,000 total / 32 active | `19 µs/tick` |
| dense ECS, 1,000 active, full scan | `442 µs/tick` |
| dense ECS, 1,000 active, indexed subset | `518 µs/tick` |
| regional coordinator raw ECS p99 | `100 µs` |
| regional coordinator direct owner p99 | `2,288 µs` |
| regional coordinator actor p99 | `4,411 µs` |
| cached animal mutation direct p99 | `932 µs` |
| cached animal mutation actor p99 | `1,586 µs` |
| concurrent cached mutation direct p99 | `2,807 µs` |
| concurrent cached mutation actor p99 | `1,048 µs` |
| 2,048 entities, serial owner p99 | `20,315 µs` |
| 2,048 entities, four-lane owner p99 | `10,964 µs` |
| 512-active subset, serial p99 | `1,385 µs` |
| 512-active subset, four-lane p99 | `1,740 µs` |

Four lanes materially help the dense full-set case, but are slower for the 512-active subset because coordination overhead dominates. The indexed dense ECS path is also slower than the full scan in this 1,000-active workload.

### Debug physical-core scaling

| Affinity | Full-set serial p99 | Full-set parallel p99 | Active-subset serial p99 | Active-subset parallel p99 |
| --- | ---: | ---: | ---: | ---: |
| 1 physical CPU | `59,970 µs` | `43,838 µs` (one lane) | `11,720 µs` | `11,422 µs` (one lane) |
| 2 physical CPUs | `54,426 µs` | `51,043 µs` | `12,033 µs` | `12,851 µs` |
| 4 physical CPUs | `51,120 µs` | `36,562 µs` | `12,638 µs` | `11,237 µs` |

These are debug diagnostic measurements, not release capacity claims.

## Integrated protocol/load matrix

### 20-client same-spawn VD8

The exact gate streams `289/289` chunks to every one of 20 protocol clients, keeps CPU/IO permits bounded, runs an explicit save, stops through the player command path, performs the final disk save, and checks that no dirty chunks remain.

| Build | Status | First-chunk p99 | Ring-2 p99 | Full-window p99 | Tick p99 / max | RSS | Total elapsed |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| debug | PASS focused | `510 ms` | `3,621 ms` | `34,681 ms` | `14.521 / 21.962 ms` | `497 MiB` | `41.8 s` |
| release | PASS focused | `69 ms` | `426 ms` | `3,409 ms` | `11.069 / 15.499 ms` | `398 MiB` | `5.28 s` |

Both runs had zero reliable-command drops, write timeouts, and slow-client sheds. The release run used two chunk CPU workers, one chunk IO worker, result queue capacity 64 per client, and completed with zero dirty chunks after shutdown. This remains `DEGRADED` relative to the frozen balanced profile because it is a bounded run, not the declared 20-minute envelope.

The runtime metric previously named `entity_save` measured inhabited-time bookkeeping rather than checkpoint I/O. This checkpoint separates `inhabited_time` from `entity_save`; the latter is now correctly zero inside the tick because checkpoint I/O runs on the dedicated save worker.

### Other load and transaction gates

| Scenario | Debug | Release | Notes |
| --- | --- | --- | --- |
| deterministic checked placement/chest replay | PASS | PASS | disk-backed, deterministic, persisted conservation |
| same-target placement, eight rounds | PASS | not separately repeated | exactly one Survival stack consumed per round |
| shared chest stale-state cursor race | PASS | not separately repeated | one authoritative cursor transaction |
| duplicate lethal commands + restart | PASS | not separately repeated | one item bundle and exact recoverable XP `21` |
| four-active + one-slow-reader 400-tick preflight | PASS | PASS | release: 400 ticks / 20 s, four transaction samples, zero reliable drops |
| bounded four-client replay + paused reader + reconnect | PASS | PASS | release: `10.46 s`, 9 bounded cancelled chunk requests |
| multicore login/chunk/broadcast | PASS | PASS | release: four clients, eight summons, `5.12 s` |
| paused reader, active observer remains live | PASS | PASS | bounded reliable retry count, no timeout or pressure shed |
| paused reader plus two healthy observers | PASS | PASS | both healthy observers received `128/128` spawns |
| four-client VD8 stop/flush | FAIL debug | PASS release | debug clients did not begin the 9-chunk stream within 30 s; release finished in `0.69 s`, dirty `29 → 0` |

The 36,000-tick Prompt-02 fallback soak was not rerun in this interactive checkpoint: its declared timeout is 45 minutes, while one verification invocation is capped at 180 seconds. The existing historical 30-minute artifact is not treated as a fresh-current-tree run.

## O3 authority and durability reports

| Benchmark | Status | Result |
| --- | --- | --- |
| mob combat / death cleanup, 4,096 entities | PASS | lethal p99 `16.301 ms`; cleanup p99 `27.681 ms` |
| regional journal fsync, 40 iterations | PASS | record p99 `5.592 ms`; total p99/max `5.602 ms` |
| explosion authority, 4,096 background entities, 64 queued explosions | FAIL | rerun p50 `27.186 ms`, p95 `44.012 ms`, p99/max `55.941 ms`; frozen p99 budget is `50 ms` |

The explosion failure reproduced twice (`59.864 ms` then `55.941 ms` p99). It is a real remaining performance blocker and was not hidden by changing the budget.

## Harness corrections found by the matrix

The matrix exposed stale or semantically incorrect fixtures. The fixes preserve production budgets:

- placement conservation now seeds inventory in Creative, waits for the authoritative slot, switches to Survival with an explicit game-mode event fence, then performs the race;
- transaction workloads that require regional durability now use a disk-backed world journal;
- disk-backed load servers use `serve_and_save`, while in-memory fixtures retain drain-only `serve` semantics;
- reconnect validation waits for the player’s restored chunk `(3,0)`, not the obsolete spawn chunk `(0,0)`;
- death replay expects the production policy’s recoverable XP (`level × 7`, capped at 100), not raw total XP;
- the bounded survival replay finishes each active client’s VD1 stream before introducing the paused reader;
- `inhabited_time` and off-tick checkpoint I/O are reported as separate stages.

## Remaining matrix gaps

- exact low profile under `2 vCPU / 4 GiB / 10 min`;
- exact balanced profile under the full declared `20 min` duration;
- exact high profile at VD32 under `12 vCPU / 16 GiB / 20 min`;
- fresh 30-minute and 2–4-hour transaction/slow-reader soaks;
- slow-disk, memory-pressure, reconnect-storm, and autoscale recovery scenarios;
- real-client and vanilla-oracle performance comparisons.
