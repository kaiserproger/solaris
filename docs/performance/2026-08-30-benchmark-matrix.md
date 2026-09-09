# Benchmark matrix — 2026-08-30 (v0.0.3-alpha.1 candidate)

Status: fresh full-matrix run on tree `6ea4e058ec1ef34e856d77e1043f94b27d01ab0c`
(Ryzen 5 7535HS, 6C/12T, ~32 GiB, Linux x64; rustc 1.94.1, LLVM 21.1.8; vanilla
sidecar 26.1.2). Gates were run sequentially with no concurrent build/test load
after an earlier contended chain produced inflated tails (explosion p99 77.4 ms
while solo reruns pass at ~44 ms; contended runs are excluded, see
`.analysis/codex-logs/`). This document does not relax any frozen budget in
[`CORE_PROFILE_MATRIX.md`](CORE_PROFILE_MATRIX.md) or
[`2026-07-27-benchmark-matrix.md`](2026-07-27-benchmark-matrix.md).

## Criterion light-engine (release bench)

Command: `cargo bench -p mc-world --bench light_engine -- --warm-up-time 1 --measurement-time 2 --sample-size 20 --noplot`.

| Variant | Estimate interval |
| --- | ---: |
| full recompute / flat | `2.920–2.952 ms` |
| full recompute / noisy | `3.624–3.701 ms` |
| full recompute / emissive scalar | `10.428–10.479 ms` |
| full recompute / emissive portable SIMD | `10.460–10.532 ms` |
| incremental 12-edit storm / flat | `1.249–1.266 ms` |

All rows at or below the 2026-07-27 intervals. Frozen `<50 ms` invocation budget holds (worst `10.5 ms`).

## Entity/ECS matrix (explicit `benchmark_report` gates, debug + release)

| Workload | Debug | Release |
| --- | ---: | ---: |
| sparse ECS 10,000/32, per-tick | `298 us` | `17 us` |
| dense ECS 1,000 full / indexed | `2486 / 3047 us` | `439 / 471 us` |
| 2,048-entity serial owner p99 | `55.3 ms` | `20.2 ms` |
| 2,048-entity four-lane owner p99 | `36.4 ms` | `15.3 ms` |
| 512-active subset serial p99 | `12.1 ms` | `2.4 ms` |
| 512-active subset four-lane p99 | `10.7 ms` | `3.6 ms` |
| coordinator raw/direct/actor p99 | `2.2 / 5.9 / 10.3 ms` | `0.10 / 1.4 / 3.1 ms` |
| cached animal direct/actor p99 | `4.9 / 5.0 ms` | `0.77 / 0.55 ms` |
| concurrent cached direct/actor p99 | `3.1 / 4.6 ms` | `0.43 / 0.52 ms` |
| 1,500×1,500 battle tick p99 (release) | — | `30.8 ms` (`max 41.1 ms`) |

All gates `test result: ok` with their machine assertions. The 2,048-entity
serial debug p99 (`55.3 ms`) is a debug diagnostic row, not a release capacity
claim; the release p99 (`20.2 ms`) is the frozen-budget row.

## O3 authority and durability

| Benchmark | Result | Status |
| --- | --- | --- |
| explosion authority 4,096 bg / 64 TNT | p50/p95/p99/max `29.6/42.0/43.6/43.6 ms` | PASS (frozen p99 `<50 ms`) |
| mob combat lethal / cleanup 4,096 | p99 `9.9 ms` / `42.7 ms` | PASS |
| regional journal fsync 40 iterations | record p99 `12.7 ms`; total p99 `12.8 ms` | PASS |
| 200-action break/drop/pickup (release) | tick p50/p95/p99/max `1.5/2.2/10.4/33.9 ms`; session max hold `4.4 ms`; player-persistence max hold `1.7 ms` | PASS |

The 200-action gate initially failed deterministically: the current seed's
safe spawn sits ~14 blocks from the gate's arena, so its first absolute
teleport exceeded the survival per-packet displacement ceiling
(`Displacement` rejection → correction). The gate now approaches the arena in
bounded hops with the arena-floor Y (the verifier was correct; the synthetic
driver was stale). Worst tail after the fix: `33.9 ms` max.

## Integrated protocol/load

| Scenario | Result | Status |
| --- | --- | --- |
| 20-client VD8 **release** | all 20 clients `289/289` chunks; first-chunk p99 `51 ms`; full-window p99 `2.0 s`; tick assertions pass | PASS |
| 20-client VD8 **debug** | tick p99 ≤ `34.5 ms`, p95 ≤ `22.8 ms`; rare `max` stall `52.0–53.5 ms` in 3 of 5 runs | **OPEN FLAP** (below) |
| multicore login/chunk/broadcast (release) | `5.1 s`; session/world max hold `628/6 us` | PASS |
| worldgen scaling, six physical CPUs | worker medians `206 / 588 / 1048 chunks/s`; floor `743.578` | PASS (`1048 ≥ 743.578`) |

### VD8 debug rare-stall flap (open)

The debug-only VD8 gate asserts total-tick `max ≤ 50 ms` and now flakes: worst
observed max `53.5 ms`, always dominated by `entity_physics` (`≤34.7 ms` max)
plus `inhabited_time` (`≤13.4 ms` max) in the same tick. Attribution: the
alpha-3 P0 axis-separated collision resolver and the separated inhabited-time
stage (both correctness work recorded in the alpha-3 plan) raised debug-mode
per-tick tails; the profile hook (`>256` steps) never fires at this
population, so no stage profile line is emitted. Release VD8 stays far inside
budget (tick p99 `≈1.1 ms`). No budget was changed; the flap is recorded for
an owner decision or a follow-up profiling checkpoint before/after tagging.

### Worldgen throughput note

The multi-scale biome-coherence model roughly halved worldgen throughput
versus the 2026-08-18 refresh (six-worker median `1048` vs `2689 chunks/s`;
single-worker `206` vs `519`). The frozen public-alpha floor
`743.578 chunks/s` still passes on six physical CPUs with ~40 % headroom.
Recorded as a measured cost of the owner-required coherence fix, not a gate
failure.

## Remaining matrix gaps (unchanged from 2026-07-27)

Exact low/balanced/high profile envelopes (2 vCPU/10 min, balanced 20 min,
VD32 high), fresh 30-minute and 2–4-hour soaks, slow-disk, memory-pressure,
reconnect-storm, autoscale-recovery scenarios, and real-client/vanilla-oracle
performance comparisons remain separate profile-acceptance evidence.
