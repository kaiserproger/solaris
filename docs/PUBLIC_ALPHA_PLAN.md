# Solaris v0.0.2-alpha.1 Long-Running Public Alpha Plan

Date: 2026-07-30

Target: `v0.0.2-alpha.1`

This plan is based on the first owner-run public-alpha session with an unmodified
Minecraft Java Edition 26.1.2 client, seed `712816`, `view_distance = 16`, one
local player and the server running on the same i5-12600 host.

The original session evidence and concrete defects remain part of the release
contract. This file now also defines the longer engineering program needed to
reach the second public alpha: finish the test baseline, enforce crate boundaries,
remove measured bottlenecks, close ordinary-play vanilla parity, and deliver the
first production Luau runtime API.

This is a `/goal` north star, not permission to work on the whole repository in
one continuation. Each continuation owns one finite checkpoint, names one route
from [`AGENT_ROUTES.md`](AGENT_ROUTES.md), validates that slice, records the next
cursor, and, while owner/runtime commit authorization is active, closes with one
local Conventional Commit when its gates are green. Without that authorization,
it records the base tree, diff hash, changed files, validation, and next action as
required by `AGENTS.md`. Current code and runtime evidence override stale
checkboxes.

## Long-running program

Work proceeds in this owner-defined order. A later phase may supply a narrowly
required test or interface for the current phase, but it must not displace the
active phase merely because its code is interesting.

The numbered items are phase outcomes, not permission to combine a whole phase
into one checkpoint. A finite checkpoint closes one crate's test class, one
vertical-domain extraction, one measured bottleneck, one parity row, or one Luau
API vertical. Validation tiers still follow `AGENTS.md`; benchmark evidence does
not replace functional, package, workspace, or real-client gates.

### Phase 1 — Finish and trust the tests

1. [ ] Inventory every failing, ignored, flaky, feature-gated, and manual-only
   workspace test. Give each non-green gate an owner, reason, and exact close
   condition; do not silently delete or weaken it.
2. [ ] Replace wall-clock sleeps and polling with the exact packet, notification,
   process, world-state, or simulation event that proves progress.
3. [ ] Keep substantial tests beside their focused domain in `*_tests.rs`; stop
   growing aggregate `play.rs`, `session.rs`, and inline production test modules.
4. [ ] Separate behavioral tests from structural tripwires. Structural checks may
   enforce crate ownership and dependency direction, but may not assert Rust
   statement order or source-text layout.
5. [ ] Make the common debug development loop deterministic across affected-crate
   tests, TCP/harness tests, persistence/restart tests, and the exact real-client
   scenarios named by the active feature.
6. [ ] Close the phase with one clean L2 run and no unexplained ignored, flaky, or
   manual-pending gate. Graphical gates must be reported as graphical evidence,
   never inferred from unit or raw-TCP success.

### Phase 2 — Enforce crate and module boundaries

`mc-net` is transport and orchestration, not the default home for gameplay. It
may own connection state, packet decoding, session routing, immutable authority
snapshots, commit adapters, publication adapters, and outbound delivery. New or
touched domain rules belong in the lowest existing crate that owns the domain:

- `mc-protocol`: packet ids, layouts, codecs, and wire DTOs;
- `mc-data`: immutable vanilla-derived tables and identifiers;
- `mc-physics`: reusable geometry, collision, and motion calculations;
- `mc-world`: world state, chunk/block authority, and world persistence contracts;
- `mc-worldgen`: deterministic terrain and feature generation;
- `mc-entity`: entity state, lifecycle, simulation, combat, AI, and spawn rules;
- `mc-script`: Luau runtime, sandbox, plugin lifecycle, and typed plugin API;
- `mc-net`: network/session orchestration and wire publication only;
- `mc-server`: composition root, configuration, startup, and shutdown only.

Required migration:

1. [x] Generate a measured ownership inventory for `mc-net`: large files, domain
   state machines, dependency edges, test concentration, and code-health
   exceptions. This inventory selects work; line count alone is not a reason to
   extract code.
   Evidence: [`evidence/mc-net-ownership-inventory.md`](evidence/mc-net-ownership-inventory.md).
2. [x] Extract pure natural-spawn scheduling and planning into `mc-entity`.
   `mc-net` retains session snapshots, regional-owner commit, visibility
   publication, and wire adapters.
   Evidence: [`evidence/natural-spawn-crate-boundary.md`](evidence/natural-spawn-crate-boundary.md).
3. [ ] Continue one touched vertical domain at a time: move request/result types and
   pure rules first, keep authority mutation in its accepted owner, then narrow the
   adapter. Do not create generic service traits without two real implementations.
   The plant vertical is complete:
   [`evidence/plant-rules-crate-boundary.md`](evidence/plant-rules-crate-boundary.md).
4. [ ] Remove superseded in-crate APIs, compatibility shims, duplicate authorities,
   feature flags, and fallbacks after all callers move.
5. [ ] Reject dependency cycles and reverse edges into `mc-net`. Semantic domain
   results must not contain `OutboundCommand`, connection handles, packet writers,
   or session internals.
6. [ ] Keep root orchestration files small and behavioral-free. A touched legacy
   domain is mechanically split when bounded; unrelated domains are not rewritten
   in the same checkpoint.
7. [ ] Extend `xtask code-health` with stable ownership/dependency tripwires for
   each completed extraction, and update ADR 0006 or the actual authority ADR in
   the same commit when the policy changes.
8. [ ] Close the phase only when remaining `mc-net` domain logic is explicitly
   listed and justified as an adapter or a queued extraction, not merely hidden in
   child modules with `use super::*`.

### Phase 3 — Find and remove measured performance bottlenecks

1. [ ] Reproduce the public-alpha generation, chunk streaming, memory, tick, and
   M39 lock baselines on a documented host and exact tree.
2. [ ] Profile before optimizing. Rank hotspots by player-visible latency, tick
   budget, allocation/memory, lock hold/wait, and throughput; keep the raw profile
   or benchmark artifact outside Git and the compact result in the owning evidence
   document.
3. [ ] Close the known break/drop/pickup lock gate, worldgen throughput gate, chunk
   streaming gate, and autoscale startup-hysteresis investigation before selecting
   lower-impact micro-optimizations.
4. [ ] For every accepted optimization, document the bottleneck, ownership
   boundary, correctness fence, measured effect, and fallback/removal path.
5. [ ] Reject optimization claims based only on code shape, synthetic counters, or
   a different environment. A regression outside the feature's accepted threshold
   remains open even when functional tests pass.
6. [ ] Close the phase with a fresh ranked profile. Every remaining ordinary-play
   hotspot must have a metric, workload, and explicit threshold in
   the [`benchmark matrix`](performance/2026-07-27-benchmark-matrix.md); its
   mapped benchmark must meet that threshold or record the owner's explicit
   acceptance of the measured tradeoff. An unnamed general "budget" is not
   closeout evidence.

### Phase 4 — Close common vanilla parity

Parity targets unmodified Minecraft Java Edition 26.1.2 behavior used in an
ordinary multiplayer survival session. Rare bug-for-bug compatibility remains
below common play, persistence safety, and plugin progress.

1. [ ] Complete the clock, worldgen, natural-spawn, item transaction, restart, and
   real-client acceptance gates already specified below.
2. [ ] Build an evidence-backed common-play matrix for movement, block interaction,
   inventory/crafting/containers, combat, projectiles, fluids, redstone essentials,
   status/effects, death/respawn, weather/time, and persistence.
3. [ ] Finish common entity behavior and AI, including villagers, village defence,
   guardians, friendly/hostile population, navigation, combat, despawn, drops, and
   restart identity.
4. [ ] Prove multiplayer authority and publication behavior with at least two
   clients for shared blocks, containers, combat, pickups, entity visibility,
   disconnect, and reconnect.
5. [ ] Derive packet ids/layouts and behavioral rules only from the local
   26.1.2 oracle, decompiled source, vanilla capture, or side-by-side harness.
   Record deliberate Solaris bug fixes explicitly.
6. [ ] Close the phase with a clean fresh-world 20-minute survival run, restart and
   reconnect, without operator setup, unexplained warnings, duplicated entities,
   lost state, or a manual-pending common-play row.

### Phase 5 — Deliver the production Luau runtime API

[`PLUGINS.md`](PLUGINS.md) and ADR 0009 own the detailed contract. The first
production API must preserve regional/simulation authority rather than exposing
internal locks or mutable registries to scripts.

1. [ ] Finish plugin discovery and derived deployment reporting:
   `server_only | server_and_client`, permissions, loader support, bundle
   identities, and artifact sizes.
2. [ ] Implement deterministic runtime lifecycle, sandboxing, capability checks,
   resource budgets, failure isolation, reload/shutdown semantics, and actionable
   diagnostics.
3. [ ] Expose versioned typed events and cancellable/transactional commands for the
   common player, world, block, inventory, combat, and entity surfaces.
4. [ ] Route plugin mutations through accepted simulation/regional commands with
   semantic results. Scripts must not receive lock guards, authority internals,
   network DTOs, or direct persistence writers.
5. [ ] Add tick/event scheduling, plugin storage, command registration, and the
   first gameplay adapters required by menus, economy, zones, and colonies.
6. [ ] Preserve deterministic event ordering, cancellation, rollback,
   disconnect/reload behavior, and persistence across restart.
7. [ ] Ship server-only and client-required fixtures, API documentation, permission
   examples, harness coverage, and vanilla-client/Loader compatibility gates.
8. [ ] Close the phase with a real plugin implementing a small end-to-end gameplay
   loop using only the public API and no Solaris-private imports.

## Subagents and independent review

Subagents accelerate bounded work; they do not replace ownership or validation:

1. Give every implementation subagent one disjoint responsibility, exact owned
   paths, base commit, acceptance criteria, and required evidence. It must not
   revert or reformat another slice.
2. Keep the immediate blocker with the primary agent. Do not delegate it and idle.
   Never fork the full parent history; pass only the finite task and relevant diff.
3. Use no more than two concurrent subagents, and only where useful work can
   continue independently. The primary agent integrates and creates the checkpoint
   commit after validation.
4. After self-check, run exactly one independent read-only second-POV review for
   every checkpoint. The reviewer checks correctness, authority/module boundaries,
   evidence validity, scope, and regressions; it does not edit or spawn agents.
5. Record `verdict: pass | changes | blocked`, at most eight findings, reviewed
   files, and validation evidence. Address actionable findings and self-validate
   the fixes; do not create an unbounded reviewer cascade.
6. A timeout or missing verdict is not a pass. Record it honestly and leave the
   affected close condition open for a later finite checkpoint.

## Feature completion and benchmark reproduction

A feature or feature-sized refactor is complete only when its behavior,
ownership boundary, focused tests, affected-crate tests, documentation/ADR,
second-POV disposition, and mapped evidence are complete on the same tree.

Benchmark reproduction happens at that feature boundary, not after every edit:

1. During implementation, run focused functional and correctness checks only.
2. When a feature with an existing performance contract becomes complete,
   reproduce exactly its mapped benchmark once on the candidate tree and record
   command, commit, environment, workload, result, threshold, and artifact path.
3. For an optimization, compare base and candidate under the same environment and
   workload. Do not substitute an old result from a different host or configuration.
4. Do not run the full benchmark suite for formatting, review-only fixes, test-only
   corrections, or intermediate extraction commits. Record `benchmark: not
   applicable` with a short reason when the completed feature has no performance
   contract.
5. A later code change that materially touches the measured path invalidates that
   feature's benchmark evidence and requires reproduction at the next feature
   boundary, not immediately on each intermediate edit.
6. Release closeout reproduces the complete release benchmark/evidence matrix once
   on the exact release-candidate tree.

## Execution status

- [x] Consolidate the `v0.0.2-alpha.1` plan in this canonical file.
- [x] Classify all six ignored `mc-entity` tests. They are explicit opt-in
  performance reports, not hidden behavioral regressions; their workloads,
  executable correctness coverage, owner, and close conditions are recorded in
  [`evidence/mc-entity-ignored-tests.md`](evidence/mc-entity-ignored-tests.md).
  None was reproduced during this classification checkpoint.
- [x] Remove the scheduler-yield dependency from the `mc-net` shutdown-wait
  test class. The test now proves both notification registration before a
  request and state observation after an earlier request; evidence is recorded
  in
  [`evidence/mc-net-shutdown-wait-test.md`](evidence/mc-net-shutdown-wait-test.md).
- [x] Replace the `mc-test-harness` container-support chunk-pipeline idle
  snapshot/yield loop with the existing exact full-pipeline idle barrier. A
  narrow read-only handle preserves the barrier across `BoundServer::serve`;
  evidence is recorded in
  [`evidence/mc-test-harness-chunk-pipeline-idle-wait.md`](evidence/mc-test-harness-chunk-pipeline-idle-wait.md).
- [x] Replace the `mc-test-harness` load-scenario chunk-cancellation
  snapshot/yield loop with a race-safe notification wait. The published stream
  counter now fences the complete request-count snapshot; evidence is recorded
  in
  [`evidence/mc-test-harness-chunk-cancellation-wait.md`](evidence/mc-test-harness-chunk-cancellation-wait.md).
- [x] Classify the five explicit `mc-net` ignores: three mapped performance
  reports and two local 26.1.2 sidecar parity gates. The parity gates no longer
  self-skip as green when Mojang data is absent, and the sheep-mix behavior has
  an always-executable checked-in regression. Workspace validation also fixed
  ordinary finite pickup misses and projections of IDs removed after interest
  selection, plus recipient-fenced food sharing and stable courtship movement.
  Owners, current status, and close conditions are recorded in
  [`evidence/mc-net-ignored-tests.md`](evidence/mc-net-ignored-tests.md).
- [x] Classify the four explicit `mc-server` ignores: two startup performance
  gates and two process-level crash/shutdown integration gates. All four require
  explicit local 26.1.2 sidecar opt-in and fail rather than self-skip when a
  prerequisite is absent. Their ordinary-suite coverage, owners, current
  disposition, and exact close conditions are recorded in
  [`evidence/mc-server-ignored-tests.md`](evidence/mc-server-ignored-tests.md).
- [x] Classify the 27 explicit `mc-test-harness` ignores across block-edit,
  chunk-stream, entity-parity, protocol-parity, deterministic replay, load,
  soak, and profiling targets. Every ignore now states its reason; eight
  vanilla-oracle gates and the stale-break composition now fail missing
  prerequisites instead of returning a false-green result. Owners, executable
  fences, and exact close conditions are recorded in
  [`evidence/mc-test-harness-ignored-tests.md`](evidence/mc-test-harness-ignored-tests.md).
- [x] Classify the two explicit `mc-worldgen` ignores as debug-build stage and
  throughput probes. Ordinary executable tests remain the behavioral authority;
  the probes stay opt-in under the worldgen performance feature-boundary policy.
  Their workloads, limits, owners, and exact close conditions are recorded in
  [`evidence/mc-worldgen-ignored-tests.md`](evidence/mc-worldgen-ignored-tests.md).
- [x] Classify the 80 `mc-script` tests gated by `lua-runtime` across exact Lua
  event payloads, Loader manifests, player inventory, plugin configuration,
  sandbox/host behavior, timers, and startup worldgen declarations. The empty
  default feature set is an intentional dependency boundary; production
  `mc-server` enables the runtime. Both the 85-test default suite and 165-test
  feature suite are green, with ownership and close conditions recorded in
  [`evidence/mc-script-lua-runtime-tests.md`](evidence/mc-script-lua-runtime-tests.md).
- [x] Classify the `mc-net` `load-bench` gate as a performance-harness build
  boundary, not a hidden test class. Default and feature configurations expose
  the same 1,853 unit tests and three doc tests; the gated server API,
  seeding/readiness surface, and timing diagnostics are owned by the explicit
  feature suite and recorded in
  [`evidence/mc-net-load-bench-tests.md`](evidence/mc-net-load-bench-tests.md).
- [x] Move natural-spawn DTOs, bounded rotating scheduler, capacity/distance,
  terrain, collision, and candidate planning into `mc-entity`. `mc-net`
  retains live session/world snapshots, owner commit, visibility publication,
  and dispatch. The exact boundary and validation are recorded in
  [`evidence/natural-spawn-crate-boundary.md`](evidence/natural-spawn-crate-boundary.md).
- [x] Measure current `mc-net` size, test concentration, domain state machines,
  dependency edges, and structural exceptions. The inventory selects
  deterministic plant planning as the next bounded lower-crate cut rather than
  treating root line count as an extraction order:
  [`evidence/mc-net-ownership-inventory.md`](evidence/mc-net-ownership-inventory.md).
- [x] Inventory the manual and graphical client gate class. The 108
  `manual-pending` manifest scenarios are fail-closed declarations with
  agent-run real-client paths, not 108 owner-only tests. The exact graphical,
  unmapped M94, release-candidate, and subjective owner gates remain open with
  owners, prerequisites, and close conditions recorded in
  [`evidence/manual-client-test-gates.md`](evidence/manual-client-test-gates.md).
- [x] Classify 15 `mc-world` tests that depended on ignored local Mojang data or
  generated oracle worlds. They are now explicit opt-in gates and fail closed
  when deliberately invoked without their prerequisite or required oracle
  shape; the ordinary suite no longer treats a successful early return as a
  pass. Owners and close conditions are recorded in
  [`evidence/mc-world-local-artifact-tests.md`](evidence/mc-world-local-artifact-tests.md).
- [x] Classify 25 `mc-data` tests that depended on ignored local Mojang reports
  or data-pack sidecars. They are now explicit opt-in gates and fail closed
  when deliberately invoked without their prerequisite; the ordinary suite no
  longer treats a successful early return as a pass. Owners and close
  conditions are recorded in
  [`evidence/mc-data-local-artifact-tests.md`](evidence/mc-data-local-artifact-tests.md).
- [x] Classify 99 additional `mc-test-harness` tests whose local sidecar or
  external-oracle prerequisites returned success when absent. They are now
  explicit opt-in gates and fail closed when selected; together with the 27
  previously classified ignores, the crate exposes 126 ignored tests. Owners,
  exact inventory, and close conditions are recorded in
  [`evidence/mc-test-harness-local-artifact-tests.md`](evidence/mc-test-harness-local-artifact-tests.md).
- [x] Record the exact local 26.1.2 world-clock oracle in
  [`evidence/world-clock-26.1.2.md`](evidence/world-clock-26.1.2.md).
- [x] Implement the typed world-clock map, separating monotonic `game_time` from
  mutable overworld time of day.
- [x] Update `/time set`, sleep, server entry and continuing-clock tests to assert
  the overworld clock instead of accidentally treating `game_time` as day time.
- [x] Pass focused and affected-package Rust/TCP validation for the clock change.
- [x] Pass the single independent read-only clock-diff review.
- [x] Run the graphical 26.1.2 clock gate. The 2026-07-30 agent-run used the
  embedded NeoForge client on `DISPLAY=:1`, reached pushed `in_play=true`, and
  recorded `/time set day`, `/time set night`, restart, and rendered clock
  evidence in
  [`evidence/world-clock-26.1.2.md`](evidence/world-clock-26.1.2.md#graphical-client-gate).
- [x] Observe at least 600 advancing client clock ticks and one complete
  24,000-tick visual cycle on a graphical host. Structured client observations
  recorded a 766-tick first interval and matching `game_time` and overworld-clock
  deltas of 24,003 ticks across the complete rendered cycle.
- [x] Split production item-drop owner commit from session publication.
- [x] Stage production item pickup across immutable planning, an owner-owned runtime
  claim token, independent session/player validation, exact owner resolution, and
  short visibility publication.
- [x] Roll back stale player/session plans through the same owner claim token without
  inventory mutation, entity publication, or overwriting newer motion.
- [x] Keep pickup claim/finalize mutations checkpoint-only so the common simulation
  `SaveBarrier` captures matching player and entity state; no standalone pickup
  journal append can outlive the corresponding inventory state.
- [x] Prove blocked regional item-drop and pickup commits leave the session registry
  available; pickup also leaves player persistence available.
- [x] Preserve item-drop and pickup behavior across the complete `mc-net` suite.
- [x] Run one independent read-only lock-diff review session. It timed out after
  180 seconds without a verdict, after concentrating on unsafe inverse-CAS rollback;
  no second reviewer was run. The exposed risk was removed with token resolution,
  motion-preserving rollback, and checkpoint-only durability regressions.
- [ ] Run the 200-action break/drop/pickup latency and M39-warning gate on the
  release candidate.
- [x] Advance generated worlds to revision 10 and remove the fixed tree, stone,
  surface-iron, dry-origin, mountain-suppression, and river-suppression fixtures.
- [x] Locate and persist a natural seed-driven spawn, then center startup generation,
  light, spawn chunk preparation, and player support search on it.
- [x] Make `tellus_like` the explicit public configuration and default mode.
- [x] Preserve unversioned Anvil imports without invoking Solaris spawn selection;
  imported worlds retain the origin fallback and no Solaris contract is written.
- [x] Run one independent read-only worldgen-diff review session. It read the scoped
  diff but exhausted the 180-second limit without a verdict or actionable finding;
  no second reviewer was run. Final self-review then removed the imported-world
  dependency on the Solaris locator and added focused regressions.
- [x] Replace uniform per-column vegetation with a seed/moisture density field,
  sparse savanna acacia, treeless desert/snowy plains/ice spikes, and taiga/grove
  spruce; enforce 32-seed biome/feature diversity metrics.
- [x] Run one independent read-only vegetation-diff review session. It inspected the
  production and external-test diff, then exhausted the 180-second limit without a
  verdict or actionable finding; no second reviewer was run.
- [x] Enforce strictly downhill revision-10 drainage through a deterministic
  seeded hydraulic elevation, depth-three bounded accumulation, and
  seed/order/border regressions. The mapped debug stage and 25-chunk throughput
  probes are recorded in
  [`evidence/worldgen-downhill-drainage.md`](evidence/worldgen-downhill-drainage.md).
- [ ] Complete revision-10 drainage,
  [rendered height/biome/vegetation mosaics](evidence/worldgen-mosaics.md),
  clean seed-`712816` owner playtest, restart, and release-host throughput gates.
  The exact-seed
  [agent-run graphical preflight](evidence/worldgen-seed-712816-preflight.md)
  is complete; owner traversal and disposition remain open.

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

### P0 — Client day/night clock (complete)

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
5. [x] Close the graphical daylight-cycle and restart gate.
6. [x] Add an explicit daylight-cycle policy instead of relying on a permanently
   hard-coded rate.
   Evidence: `docs/evidence/world-clock-26.1.2.md`.

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

1. [x] Remove every fixed spawn tree, stone and iron fixture from production.
2. [x] Remove origin-based terrain deformation. Do not change terrain to make spawn
   safe.
3. [x] Add a deterministic seed-driven spawn locator that searches the generated
   terrain for a bounded safe land position, persists it in schema 3, and centers
   pre-generation, light baking, spawn-chunk preparation, and final player support
   search on that position.
4. [x] Make `tellus_like` explicit in the public starter config and make the public
   default match the advertised Earth-like Solaris world.
5. [x] Ensure seed participates in every macro field: continents, plate/ridge domains,
   erosion, drainage, climate, biome selection, vegetation, ores and structures.
6. Replace isolated noise peaks with coherent tectonic domains and long branching
   ranges. Keep smooth local slopes and bounded chunk-border steps.
7. Replace zero-contour-only rivers with a coarse drainage field: downhill flow,
   accumulation, valley carving and coast connection.
8. [x] Replace per-column modulo decoration with coherent vegetation-density fields.
   Add biome-correct clusters, including sparse acacia in savanna, forest edges and
   treeless dry/cold regions where appropriate.
9. Keep ordinary ores underground or exposed only through natural terrain/cave
   intersection. Geological deposits remain an optional plugin-owned profile.
10. [x] Persist schema/revision/mode/seed/geometry/profiles and the selected spawn;
    require a fresh Solaris world when any contract field changes.

This release targets a procedural Earth-like/Tellus-inspired world, not a literal
geographic Earth replica. Real DEM, land-cover and OSM inputs remain a separate
optional sidecar project and must not be silently implied by the mode name.

Acceptance:

- Seeds `0`, `712816`, `-1` and at least 29 generated regression seeds have
  different height, biome and feature fingerprints near their selected spawns.
- No production branch checks fixed world coordinates to place starter resources.
- Seed `712816` receives a clean fresh-world owner playtest and
  [rendered height/biome/vegetation mosaics](evidence/worldgen-mosaics.md) for
  at least a 2048x2048-block area.
- Multi-seed metrics reject a single biome occupying an unreasonable share of the
  sampled land unless the seed naturally produces that result at a larger scale.
- Chunk seams, cave surface shell, water fill, persistence and restart gates pass.
- On the same i5-12600 host, generation throughput regresses by no more than 20%
  from the recorded 929 chunks/s baseline, unless a measured quality gain is
  explicitly accepted.

### P0 — Item drop/pickup lock boundary (implementation complete; runtime gate pending)

`v0.0.1-alpha.1` called the synchronous regional entity owner while holding the
timed `SessionRegistryInner` guard. Pickup additionally nested the
player-persistence mutex. The owner log signature matched this exactly: the two
hold classes shared 38-43 ms averages and chunk-recipient readers occasionally
waited behind them.

Required change:

1. [x] Add debug phase timings around regional-owner commit, session validation,
   player commit, and publication.
2. [x] Never call a blocking `RegionalOwnerHandle` request while holding
   `SessionRegistryInner` or `PlayerPersistedState` in production drop/pickup.
3. [x] Split item-drop creation into an owner commit followed by a short
   publication lock consuming the immutable committed snapshot.
4. [x] Convert pickup into a staged owner-token transaction:
   - plan inventory credit from an immutable player snapshot;
   - install one runtime-only claim token through exact regional snapshot CAS while
     leaving the authoritative stack unchanged; the regional owner then rejects
     competing stack changes, removal, merge, lifecycle replacement, and damage while
     still allowing kinematics;
   - recheck session identity/range, release the session guard, then commit the
     complete matching player state;
   - atomically resolve the token against the current entity snapshot, applying the
     remainder/removal on success or restoring availability on stale player/session
     state without overwriting newer motion.
5. [x] Keep claim installation, rollback, and finalize as checkpoint-only owner
   mutations. Production saves enter through the simulation `SaveBarrier`, while the
   only direct snapshot path is after simulation-owner drain, so player and entity
   state cannot be durably split by an in-flight pickup.
6. [x] Preserve partial pickup, disconnect/requester-loss races, competing
   collectors, full inventory, item merge, persistence, and event ordering.
7. [x] Add deterministic lock-release, checkpoint, and interleaving regressions so a
   future synchronous owner wait or split durable pickup fails the focused suite.

Acceptance:

- A 200-block break/drop/pickup real or raw-TCP loop produces zero M39 warnings for
  `spawn item drop`, `credit item pickup` and `loaded recipients for chunks`.
- Session and player-persistence lock hold p99 remain below 5 ms in the one-player
  local gate.
- The loop has no tick above 50 ms attributable to item spawn/pickup.
- Conservation/race tests cover partial pickup, full inventory, disconnect,
  concurrent collectors, stale revisions, restart and requester loss.

### P1 — Periodic natural mob spawning runtime is implemented; client evidence remains

The current implementation registers bounded templates while preparing chunks,
then runs independent friendly and hostile attempts over a rotating subset of
simulation-loaded chunks. Admission rechecks active players, category and
per-chunk caps, distance, support/fluid, collision, world time and block light
before the regional owner commit. Movement and despawn release capacity for a
later attempt; attempts and rejection reasons are reported periodically.

The repo-owned fallback now contains concrete 26.1.2 supported-entity subsets
for plains, forest, taiga, savanna, desert, swamp, river and ocean. Other
land/water biomes still use the explicit plains/ocean fallback, so this is a
common-biome alpha baseline rather than a biome-complete parity claim.

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

1. [x] Add a bounded rotating scheduler over currently simulation-loaded chunks.
2. [x] Run friendly and hostile categories on their configured cadences.
3. [x] Retain internal category caps and player-distance fences; do not scan every
   loaded chunk every attempt.
4. [x] Require valid collision/support/fluid rules. Hostiles also require the exact
   supported darkness/time conditions.
5. [x] Refill populations after movement/despawn instead of marking a chunk spawned
   forever.
6. [x] Add bounded periodic metrics: attempts, accepted entities and rejection reasons.
7. [x] Expand repo-owned biome rules for the supported common biomes instead of mapping
   every land biome to plains indefinitely.

Acceptance:

- [ ] In a fresh 20-minute survival session with defaults, friendly mobs become
  observable near the player and hostiles become observable during night without
  operator setup.
- [x] Setting either interval to `0` disables only that category.
- [x] Halving an interval doubles attempt cadence without bypassing caps.
- [x] No spawn occurs inside the minimum player radius, in invalid blocks or outside
  loaded simulation chunks.
- [ ] Restart does not duplicate deterministic identities or lose retained entities.

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

The `/goal` runner selects one finite checkpoint from the first incomplete phase:

1. **Test completion** — classify and close unstable/missing gates; preserve the
   already-green clock, item, worldgen, natural-spawn, villager, guardian, and
   village-defence coverage.
2. **Crate boundaries** — begin with natural-spawn scheduler/planning extraction,
   then select the next measured `mc-net` domain without overlapping write sets.
3. **Performance** — close the item lock, worldgen, streaming, memory, tick, and
   autoscale measurements in player-visible impact order.
4. **Vanilla parity** — close the ordinary survival/multiplayer matrix with exact
   oracle and real-client evidence.
5. **Luau runtime API** — deliver deployment reporting, safe runtime foundations,
   typed API surfaces, persistence, gameplay adapters, and fixtures.
6. **Release closeout** — reproduce the full benchmark/evidence matrix, run one
   owner-equivalent fresh-world playtest plus restart, run L2 once, and build/smoke
   the release artifacts.

Do not parallelize overlapping worldgen, lock-authority, session-root, or runtime
composition edits. Disjoint tests, evidence capture, and isolated lower-crate work
may proceed in parallel under the subagent rules above.

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
