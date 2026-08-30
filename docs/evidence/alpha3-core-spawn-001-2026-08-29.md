# CORE-SPAWN-001 — hostile natural-spawn light/time parity

Date: 2026-08-29

## Scope

The periodic hostile spawn path no longer uses a binary `world_time_is_night()` gate. It now evaluates the Minecraft Java 26.1.2 hostile light predicate from persisted `ChunkLight`, current world time and weather while preserving cap, player-distance, support/fluid, collision and loaded-simulation-chunk fences.

Inherited population-cap/default changes remain CORE-SPAWN-002 work; they were preserved rather than reverted or retuned here.

## Live 26.1.2 oracle

Source artifact: `/home/kaiserroman/research/server-26.1.2.jar`, nested runtime `META-INF/versions/26.1.2/server-26.1.2.jar`.

Direct `javap -c -p` inspection of `net.minecraft.world.entity.monster.Monster.isDarkEnoughToSpawn(ServerLevelAccessor, BlockPos, RandomSource)` shows this order:

1. `SKY` brightness must be `<= random.nextInt(32)`;
2. dimension `monsterSpawnBlockLightLimit` is read; Overworld 26.1.2 sets it to `0`, so block light above zero rejects;
3. local raw brightness is computed with normal sky darkening, or sky subtraction `10` while thundering;
4. raw brightness must be `<= dimension.monsterSpawnLightTest.sample(random)`.

`data/minecraft/dimension_type/overworld.json` sets `monster_spawn_block_light_limit: 0` and `monster_spawn_light_level` to uniform `0..7` inclusive.

`data/minecraft/timeline/day.json` supplies the Overworld `minecraft:gameplay/sky_light_level` track: `1.0` at ticks 133..11867, linear transition to `0.26666668` at tick 13670, flat through 22330, then linear transition back to 1.0 by wrapped tick 24133. `Level.updateSkyBrightness()` stores `15 - SKY_LIGHT_LEVEL * 15` as the sky-darken integer. `KeyframeTrack` bytecode confirms omitted `ease` defaults to `EasingType.LINEAR`.

The first failed attempt to verify this used the stale extracted `research/version-research/26.1.2/server-26.1.2-runtime.jar`; that artifact is documented elsewhere in the research tree as stale/misassigned. This checkpoint uses the live nested runtime from the current direct 26.1.2 server artifact instead.

## Implementation

`mc-entity` now mirrors the predicate structure:

- raw persisted sky light vs deterministic `0..31` attempt roll;
- block-light limit `0`;
- raw brightness `max(block, sky - sky_darken)`;
- Overworld 26.1.2 timeline-derived sky darkening;
- thunder sky subtraction `10`;
- deterministic uniform `0..7` attempt roll for the final monster light test.

Solaris uses deterministic per-attempt hashes instead of reproducing Mojang's global `RandomSource` state sequence. This intentionally preserves predicate order, ranges and probabilities without claiming bit-identical vanilla RNG sequencing.

`rejected_time` records the first sky/exposure rejection; block-light and final raw-brightness failures remain `rejected_darkness`.

## Deterministic matrix

Focused integration coverage proves:

- exposed daytime surface -> no hostile commit;
- dark loaded daytime cave (`sky=0`, `block=0`) -> hostile commit allowed;
- lit cave (`sky=0`, `block>0`) -> darkness rejection;
- tick 13000 is twilight, not an unconditional hostile-spawn switch: exposed surface remains rejected by raw brightness;
- deep night at tick 18000 can admit an exposed zero-block-light surface when both vanilla-range random tests pass;
- daytime thunder can admit an exposed zero-block-light surface when both random tests pass;
- aquatic/fluid, support and existing authority fences remain active.

The synthetic all-dark cave fixture keeps one unrelated sky-light cell non-zero because `ChunkLight::from_chunk` treats an entirely omitted all-zero light volume as unavailable. Candidate light remains exactly zero.

## Validation

- `cargo test -p mc-net periodic_aquatic_and_hostile_admission_use_fluid_time_and_darkness --lib -- --nocapture`: PASS.
- `cargo test -p mc-net herd_spawn_authority --lib -- --nocapture`: 6 passed.
- `cargo test -p mc-entity natural_spawn_26_1_2 --lib -- --nocapture`: 6 passed, including exact timeline and cave-template coverage.
- `cargo test -p mc-entity --lib --quiet`: 612 passed / 6 ignored / 0 failed.
- `cargo test -p mc-net --lib --quiet`: 1986 passed / 5 ignored / 0 failed after the QA-found chunk-stream fix.
- `cargo test -p mc-net view_distance_replan_does_not_requeue_retained_loaded_chunk --lib -- --nocapture`: PASS.
- `cargo test -p mc-net runtime_control_queue_pressure_replans_live_view_distance --lib -- --nocapture`: PASS.
- `cargo run -p xtask -- code-health`: `0 fail`, verdict `KEEP`.
- `cargo fmt --all -- --check`: PASS.
- strict scoped Clippy for `mc-entity` / `mc-net`: PASS.

## Graphical QA and QA-found defects

The CodexPro QA runtime-sidecar fix is active after restart. QA boxes receive 7,842 `data/vanilla` files / 15,134,115 bytes, SHA-256 `4fe87b97a4f184b22e6e6e6160133ad086976ec976eecbb5ecaa893b3a11bb74`; the previous missing-`version.json` blocker is closed.

Independent Luna graphical QA found two real defects during this checkpoint rather than merely confirming the focused tests:

1. hostile templates were surface-only, so real daytime cave spawning was impossible even after fixing light admission. `plan_hostile_spawns` now takes bounded enclosed-cave candidates plus a surface fallback; `hostile_template_positions_include_enclosed_cave_and_surface` is the regression fence;
2. runtime view-distance replanning used `ChunkScheduler::replay_view`, which cleared the scheduler `finished` set and could re-emit a retained loaded chunk without an unload. The real client hit the debug assertion `chunk emitted twice without unload`. `replan_effective_view_distance` now uses `replace_view`, preserving the finished intersection; intentional full replay still uses `replay_view` only after loaded chunks are drained.

Post-fix real-client evidence:

- `.analysis/core-spawn-001-final-qa/20260829T074239Z/result.json`: PASS under the real Java 26.1.2 client + Xvfb. New UUIDs only were considered between phases. Day clocks `1030..1124` produced a new zombie in loaded air with `sky_light=0`, `block_light=0` and no new exposed hostile. Early twilight clocks `12955..12988` produced no new exposed hostile. Midnight clocks `18030..18188` produced a new exposed hostile. Server panic and duplicate-chunk panic counts were both zero.
- `.analysis/core-chunk-replan-postfix-qa/20260829T074022Z/result.json`: PASS. The live client exercised an effective runtime view-distance transition `4 -> 3` with retained chunks, then disconnected/reconnected. Observed chunk-stream distances were `[4,4,4,4,3,3,3,3]`; server panic count `0`, `chunk emitted twice without unload` count `0`.
- The earlier longer parent run `.analysis/core-spawn-001-postfix-qa/20260829T073256Z/result.json` independently observed a daytime cave zombie around `y=-36` with `sky_light=0` / `block_light=0`, completed midnight and reconnect, and recorded zero server/duplicate-chunk panics after the fix. Its old all-twilight boolean was intentionally discarded because later twilight (`~13142`) can legally admit exposed spawning under the vanilla probabilistic threshold; the final gate isolates early twilight instead.

OpenAL failure under Xvfb was environment-only where present; the graphical client reached and remained in Play.

## Status

**CORE-SPAWN-001 CLOSED.** The coarse binary nighttime gate is gone, the live 26.1.2 light/time predicate is represented with deterministic equivalent-range rolls, daytime cave candidates exist, real-client day/cave/early-twilight/midnight behavior is demonstrated, and the QA-found chunk replay panic is fixed and reproduced cleanly after repair.

CORE-SPAWN-002 population/default calibration remains separate and may now begin.
