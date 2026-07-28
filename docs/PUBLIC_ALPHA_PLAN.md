# Solaris v0.0.2-alpha.1 Stabilization Plan

Date: 2026-07-28

Target: `v0.0.2-alpha.1`

This plan is based on the first owner-run public-alpha session with an unmodified
Minecraft Java Edition 26.1.2 client, seed `712816`, `view_distance = 16`, one
local player and the server running on the same i5-12600 host.

The release goal is not broad feature growth. It is to fix the first public
alpha's concrete client-visible failures while preserving its strong generation,
streaming and memory baseline.

## Execution status

- [x] Consolidate the `v0.0.2-alpha.1` plan in this canonical file.
- [x] Record the exact local 26.1.2 world-clock oracle in
  [`evidence/world-clock-26.1.2.md`](evidence/world-clock-26.1.2.md).
- [x] Implement the typed world-clock map, separating monotonic `game_time` from
  mutable overworld time of day.
- [x] Update `/time set`, sleep, server entry and continuing-clock tests to assert
  the overworld clock instead of accidentally treating `game_time` as day time.
- [x] Pass focused and affected-package Rust/TCP validation for the clock change.
- [x] Pass the single independent read-only clock-diff review.
- [ ] Run the graphical 26.1.2 clock gate. The current CodexPro execution
  environment exposes no `DISPLAY`, `WAYLAND_DISPLAY`, or `XDG_RUNTIME_DIR`; the
  attempted NeoForge client failed with `glfwInit failed`. No visual sun-movement
  claim is made from this environment.
- [ ] Observe at least 600 advancing client clock ticks and one complete
  24,000-tick visual cycle on a graphical host.
- [ ] Continue with the item drop/pickup lock-boundary checkpoint.

## Observed baseline

- Empty-world pre-generation completed 225 chunks in 242 ms: 929.473 chunks/s.
- Spawn light bake completed 169 chunks in 153 ms.
- All 40 chunk streams completed without absent chunks, pressure abandonment,
  degraded delivery or memory-pressure shedding.
- The owner observed roughly 300 MiB process memory at VD16.
- The server shut down cleanly and flushed all dirty chunks.
- There were 379 warnings, of which 369 were M39 lock-budget overruns.
- Six ticks took 55.064-81.273 ms. The dominant lock operations were item-drop
  spawn and item-pickup credit.

## Confirmed findings

### P0 — Client day/night clock (wire implementation complete; graphical gate pending)

`v0.0.1-alpha.1` sent `game_time` followed by an empty clock-update map.
Minecraft 26.1.2 uses that map for client-visible clocks such as time of day, so
tests could observe an increasing counter while the sun remained fixed.

Required change:

1. [x] Inspect the local 26.1.2 `ClientboundSetTimePacket`/clock definitions and
   record the exact clock identifiers, payload types and ticking representation.
2. [x] Replace the placeholder empty map with a typed protocol model.
3. [x] Publish both the monotonic game clock and the overworld time-of-day clock.
4. [x] Preserve immediate `/time set` updates and persisted world time in the TCP
   contract.
5. [ ] Close the graphical daylight-cycle and restart gate.
6. [ ] Add an explicit daylight-cycle policy instead of relying on a permanently
   hard-coded rate.

Acceptance:

- A real 26.1.2 client observes sun movement over at least 600 ticks.
- A complete 24,000-tick day reaches daytime, sunset, night and sunrise.
- `/time set day`, `/time set night` and restart preserve the expected visual time.
- Packet tests validate the complete clock-update payload, not only `game_time`.

### P0 — Production worldgen contains test fixtures and suppresses seed diversity near spawn

The seed is used by the noise/router and different seeds do change terrain away
from the origin. The owner's perception is nevertheless valid because production
worldgen deliberately normalizes the spawn region and contains fixed-coordinate
starter fixtures:

- the public `example.toml` leaves `worldgen_mode = "tellus_like"` commented, so
  the default `vanilla_like` mode runs;
- a 384-block origin weight forces dry land, suppresses mountains and rivers, and
  blends continentalness toward the same value for every seed;
- fixed coordinates `(8..11, 4..8)` are forced to exposed stone;
- fixed coordinates `(11..13, 4..8)` are forced to exposed iron ore;
- a fixed-coordinate starter-tree anchor is attempted at `(-8, -8)` when the
  local surface passes the tree-site checks;
- the test contract explicitly requires at least ten exposed iron blocks within
  64 blocks of the default spawn.

These are playable-loop fixtures accidentally shipped as world rules. The
surface iron observed on seed `712816` is therefore expected from the current
code and is not natural ore generation.

Required change — worldgen revision 10:

1. Remove every fixed spawn tree, stone and iron fixture from production.
2. Remove origin-based terrain deformation. Do not change terrain to make spawn
   safe.
3. Add a deterministic seed-driven spawn locator that searches the generated
   terrain for a bounded safe land position, persists it, and pre-generates around
   that position.
4. Make `tellus_like` explicit in the public starter config and make the public
   default match the advertised Earth-like Solaris world.
5. Ensure seed participates in every macro field: continents, plate/ridge domains,
   erosion, drainage, climate, biome selection, vegetation, ores and structures.
6. Replace isolated noise peaks with coherent tectonic domains and long branching
   ranges. Keep smooth local slopes and bounded chunk-border steps.
7. Replace zero-contour-only rivers with a coarse drainage field: downhill flow,
   accumulation, valley carving and coast connection.
8. Replace per-column modulo decoration with coherent vegetation-density fields.
   Add biome-correct clusters, including sparse acacia in savanna, forest edges and
   treeless dry/cold regions where appropriate.
9. Keep ordinary ores underground or exposed only through natural terrain/cave
   intersection. Geological deposits remain an optional plugin-owned profile.
10. Persist revision/mode/seed and require a fresh world when any of them changes.

This release targets a procedural Earth-like/Tellus-inspired world, not a literal
geographic Earth replica. Real DEM, land-cover and OSM inputs remain a separate
optional sidecar project and must not be silently implied by the mode name.

Acceptance:

- Seeds `0`, `712816`, `-1` and at least 29 generated regression seeds have
  different height, biome and feature fingerprints near their selected spawns.
- No production branch checks fixed world coordinates to place starter resources.
- Seed `712816` receives a clean fresh-world owner playtest and rendered
  height/biome/vegetation mosaics for at least a 2048x2048-block area.
- Multi-seed metrics reject a single biome occupying an unreasonable share of the
  sampled land unless the seed naturally produces that result at a larger scale.
- Chunk seams, cave surface shell, water fill, persistence and restart gates pass.
- On the same i5-12600 host, generation throughput regresses by no more than 20%
  from the recorded 929 chunks/s baseline, unless a measured quality gain is
  explicitly accepted.

### P0 — Item drop/pickup waits on the regional entity owner while holding global locks

`lock_session_entities()` acquires the timed `SessionRegistryInner` guard. Item
spawn then calls the synchronous regional-owner `spawn()` path, which sends a
command and blocks on `result.recv()`. Item pickup performs synchronous snapshot
and CAS/remove/update owner calls while the session lock is held and while the
player-persistence lock is nested inside it.

The log signature matches this exactly: session and player-persistence holds have
nearly identical timestamps and 38-43 ms averages, while chunk-recipient readers
occasionally wait behind them.

Required change:

1. Add phase metrics around regional-owner request queueing, response wait,
   publication and inventory commit to preserve proof of the cause.
2. Never call a blocking `RegionalOwnerHandle` request while holding
   `SessionRegistryInner` or `PlayerPersistedState`.
3. Split item-drop creation into an owner commit followed by a short publication
   lock; publication consumes the immutable committed snapshot.
4. Convert pickup into a staged revision-fenced transaction:
   - plan inventory credit from an immutable player revision;
   - reserve/claim the exact item snapshot through owner CAS;
   - commit the player inventory through a matching revision CAS;
   - compensate/retry on a stale player or entity revision without duplication or
     loss.
5. Preserve partial pickup, disconnect races, competing collectors, full inventory,
   item merge, persistence and event ordering.
6. Add a code-health/architecture tripwire preventing new synchronous owner waits
   under these timed guards where it can be expressed structurally.

Acceptance:

- A 200-block break/drop/pickup real or raw-TCP loop produces zero M39 warnings for
  `spawn item drop`, `credit item pickup` and `loaded recipients for chunks`.
- Session and player-persistence lock hold p99 remain below 5 ms in the one-player
  local gate.
- The loop has no tick above 50 ms attributable to item spawn/pickup.
- Conservation/race tests cover partial pickup, full inventory, disconnect,
  concurrent collectors, stale revisions, restart and requester loss.

### P1 — Natural mob spawning is a one-shot chunk materialization, not a continuing spawn cycle

The current implementation plans herds while preparing chunks. Passive candidates
are admitted only for a deterministic subset of newly seen chunks and share a
global creature cap of 10. Hostiles are deferred until the server's internal
night window. There is no complete periodic loaded-chunk spawn cycle. The frozen
client sun does not prove that the internal game clock also stopped; v0.0.2 must
test and report visual clock progression and hostile activation separately.

The embedded fallback contains only two concrete biome spawn tables (plains and
ocean), with other land/water biomes mapped to those defaults. This is enough for
a baseline, not biome-complete spawning.

Required config:

```toml
[simulation]
# Natural friendly-mob spawn attempt cadence. Solaris runs at 20 ticks/second:
# 400 = once every 20 seconds; 200 = twice as often; 0 disables friendly spawning.
friendly_spawn_interval_ticks = 400

# Natural hostile-mob spawn attempt cadence:
# 20 = once per second; 10 = twice per second; 0 disables hostile spawning.
hostile_spawn_interval_ticks = 20
```

`spawn_monsters` should be removed rather than kept as a second conflicting
hostile authority. Alpha compatibility is not promised.

Required runtime change:

1. Add a bounded rotating scheduler over currently simulation-loaded chunks.
2. Run friendly and hostile categories on their configured cadences.
3. Retain internal category caps and player-distance fences; do not scan every
   loaded chunk every attempt.
4. Require valid collision/support/fluid rules. Hostiles also require the exact
   supported darkness/time conditions.
5. Refill populations after movement/despawn instead of marking a chunk spawned
   forever.
6. Add bounded periodic metrics: attempts, accepted entities and rejection reasons.
7. Expand repo-owned biome rules for the supported common biomes instead of mapping
   every land biome to plains indefinitely.

Acceptance:

- In a fresh 20-minute survival session with defaults, friendly mobs become
  observable near the player and hostiles become observable during night without
  operator setup.
- Setting either interval to `0` disables only that category.
- Halving an interval approximately doubles attempt cadence without bypassing caps.
- No spawn occurs inside the minimum player radius, in invalid blocks or outside
  loaded simulation chunks.
- Restart does not duplicate deterministic identities or lose retained entities.

### P1 — Plugin deployment requirements are implicit instead of operator-visible

The manifest already provides the authoritative distinction:

- no `[client]` bundles means a server-only Lua plugin;
- one or more `[client.bundles]` means the plugin requires Solaris Loader on both
  server and client.

Do not add a manually maintained duplicate boolean that can disagree with the
bundle declaration. Derive and expose the classification.

Required change:

1. Add derived `deployment = server_only | server_and_client` to plugin discovery
   results.
2. Show it in startup logs and `solaris --check` JSON for every plugin.
3. For client-required plugins, report supported loaders, bundle identities,
   permissions and total artifact bytes.
4. Label every shipped example plugin README and the main plugin documentation as
   **Server-only** or **Requires Solaris Loader on client**.
5. Use a precise disconnect message when a client-required plugin cannot complete
   the Loader handshake.

Acceptance:

- Operators can determine requirements before starting the server from docs and
  `--check` output.
- A server-only plugin accepts an ordinary vanilla 26.1.2 client.
- A client-required fixture rejects an unmodified client clearly and accepts each
  supported Loader fixture after exact bundle acknowledgement.

## Secondary observations

- Autoscale changed CPU admission 18 times during the first ten seconds of play.
  This did not break chunk delivery, but its startup hysteresis should be reviewed
  after P0 fixes so it does not obscure performance measurements.
- The run loaded embedded fallback tables with zero block-mining and block-explosion
  states. This did not cause the reported failures, but public `--check` should
  distinguish optional fallback behavior from data required for exact parity.
- Warning output needs aggregation/rate limiting only after the underlying lock
  defect is fixed. Suppressing 369 valid warnings before fixing the lock path is
  not acceptable.

## Checkpoint order

1. **Clock correctness** — exact 26.1.2 clock payload and real-client day cycle.
2. **Item transaction lock boundary** — eliminate blocking owner waits under
   session/player locks and close the 200-action gate.
3. **Worldgen revision 10** — remove fixtures, add seed-driven spawn location,
   make Earth-like mode the public default and improve macro terrain/biomes/details.
4. **Periodic natural spawning** — category cadences, loaded-chunk scheduler,
   darkness/support rules and configuration comments.
5. **Plugin deployment reporting** — derived classification in check/log/docs.
6. **Release closeout** — one complete owner-equivalent playtest and L2 validation.

Do not parallelize the worldgen and lock-authority edits in the same files. The
clock and plugin-reporting slices are bounded and may be reviewed independently.

## Release gates

All must pass on the exact release-candidate tree:

- `cargo run -p xtask -- code-health`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- release build and installer/archive smoke for Linux x86_64 and AArch64
- real-client 24,000-tick day/night gate
- seed `712816` fresh-world visual/playable gate
- 200-action break/drop/pickup lock gate
- 20-minute no-operator survival session proving friendly and hostile spawning
- plugin matrix: server-only vanilla client plus client-required Loader fixture

## Explicitly not sufficient for release

- Merely uncommenting `tellus_like` without removing spawn fixtures and origin
  deformation.
- Merely raising the M39 warning threshold or rate-limiting the messages.
- Merely adding spawn interval fields while retaining one-shot chunk spawning.
- Claiming seed correctness from unit noise tests without a fresh-world visual and
  feature fingerprint comparison.
- Calling synthetic Earth-like generation a literal Earth replica.
