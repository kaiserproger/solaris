# CORE-SPAWN-002 — natural population contract and starter calibration

Date: 2026-08-30

## Scope

CORE-SPAWN-002 makes natural population an operator-visible bounded policy rather than cadence-only tuning. CORE-SPAWN-001 legality remains unchanged: biome/species selection, loaded simulation chunks, support/fluid, hostile light/time, player-distance and collision fences still decide whether a sampled template may spawn.

The policy exposes:

- `friendly_spawn_cap`, `aquatic_spawn_cap`, `hostile_spawn_cap` — global server natural-population ceilings by category;
- `friendly_spawn_chunk_budget`, `hostile_spawn_chunk_budget` — bounded rotating active-chunk samples per due attempt;
- existing friendly/hostile intervals remain the cadence controls; interval `0` disables that category.

Caps are global, not per-player. Multiple players widen the union of eligible active chunks; overlapping players do not multiply capacity or duplicate a chunk.

## Starter profile decision

Retained caps:

- friendly: `32`
- aquatic: `20`
- hostile: `70`
- hostile chunk budget: `4`
- friendly interval: `400` ticks
- hostile interval: `20` ticks

Friendly chunk budget changes from the inherited candidate `16` to **`48`**.

Reason: passive templates intentionally exist in only roughly one ninth of chunks (`passive_chunk_spawns`), so cap `32` was not the sparse-world limiter. In the first measured default run, three friendly attempts sampled 48 total chunks and committed only three friendly mobs while `friendly_rejected_cap=0`. Raising the global cap would therefore not address the observed bottleneck; wider bounded spatial sampling does.

## Real-client measurement

All runs used the real Java 26.1.2 client under Xvfb, seed `712816`, the same playable profile, 60 seconds daylight followed by 30 seconds midnight, and process RSS sampling. Server entity-ticker debug telemetry supplied its own rolling tick percentile window. These are bounded local debug-build measurements, not production capacity claims.

| Friendly budget | Day friendly visible max | Night hostile visible max | RSS range MiB | Final available built-in tick p95 | Result |
| ---: | ---: | ---: | ---: | ---: | --- |
| 16 | 3 | 11 | 244.1–254.6 | ~24.9 ms | sparse |
| 32 | 9 | 11 | 239.3–250.6 | ~33.9 ms | still below alpha-3 population target |
| 48 | 14 | 7 | 243.3–251.8 | ~41.8 ms | selected |
| 64 | 18 | 7 | 241.9–251.1 | ~44.1 ms | denser, less tick headroom |

Evidence directories:

- budget 16: `.analysis/core-spawn-002-population/20260829T075314Z/`
- budget 32: `.analysis/core-spawn-002-population/20260829T075633Z/`
- budget 48: `.analysis/core-spawn-002-population/20260829T080059Z/`
- budget 64: `.analysis/core-spawn-002-population/20260829T075837Z/`

The selected 48-budget run materially exceeds the previously observed ~10-friendly sparse alpha baseline within the bounded daylight window while keeping p95 below the 50 ms tick budget in this measurement. Budget 64 adds population but buys relatively little RSS advantage and leaves less p95 margin, so it is not the starter default.

## Operator contract

`example.toml`, `playable.toml`, config parsing/normalization, startup validation and `--check` effective simulation output expose the policy. `docs/OPERATING.md` documents global-cap semantics, interval disable behavior, budget semantics, simulation-distance interaction and despawn/refill behavior.

Spawn telemetry logs cumulative attempts, sampled chunks, commits and the major rejection classes. Population tuning never turns a legality rejection into an admission.

## Validation status

Deterministic coverage now includes:

- one-player cap/refill to the exact configured ceiling;
- global-cap enforcement across separated players for friendly, aquatic and
  hostile categories;
- overlapping-player chunk de-duplication;
- cap `0` rejection;
- high-budget support/fluid/light/collision/player-distance fences;
- simulation-distance filtering before template planning.

- `cargo test -p mc-entity natural_spawn_26_1_2 --lib`: 6 passed;
- `cargo test -p mc-net herd_spawn_authority --lib -- --nocapture`: 13 passed;
- `cargo test -p mc-server spawn --lib --bin mc-server`: 6 passed, 1 ignored;
- `cargo fmt --all -- --check`: passed;
- `cargo run -p xtask -- code-health`: passed, `0 fail`, verdict `KEEP`;
- strict scoped Clippy for `mc-entity`, `mc-net` and `mc-server`: passed with
  `-D warnings`;
- the repaired focused M37 ignored load scenario passed with four clients:
  `cargo test -p mc-test-harness --test load_scenarios
  reports_spawn_exploration_block_entity_and_multi_client_load
  -- --ignored --nocapture`: 1 passed, 15 filtered, 17.61s;
- scoped chunk and movement validation passed: chunk-stream 81 tests and
  movement 57 tests.

The M37 repair was driven by two observed test-contract failures, not a
population-rule change. The original 16.5-block absolute movement steps
exceeded the authoritative survival displacement limit and were rejected as
`Displacement`. After reducing the traversal to 8.5-block steps, the
in-memory load fixture exhausted its dirty-chunk cache during continued
exploration; the long traversal now uses a temporary disk-backed world.
Exploration runs in spectator mode to remove terrain-dependent collision
corrections from this chunk-replanning load measurement, then returns to
creative mode for block and entity pressure actions.

The deterministic population behavior, supplementary M37 load behavior and
all scoped code-health, format and lint gates are green.

## Status

COMPLETE — deterministic population contract, bounded starter calibration and
the repaired supplementary M37 load gate are green.
