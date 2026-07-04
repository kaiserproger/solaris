# Replacement Readiness

Solaris can be evaluated as a scoped vanilla 26.1.2 overworld-survival server,
not as a bit-perfect vanilla clone.

Replacement readiness is a stabilization/release claim, not a draft
implementation claim. Use the hard DoD in
[DEFINITION_OF_DONE.md](DEFINITION_OF_DONE.md): scoped mechanics need at
least 80% vanilla-observable coverage, documented divergences for the
rest, cargo/harness/oracle/client evidence, and measured performance or
concurrency behavior where relevant.

## Operator Setup

1. Put a Mojang 26.1.x `server.jar` at `.analysis/server.jar`.
2. Run `tools/extract-vanilla-data.sh` to populate `data/vanilla/`.
3. Prepare or reuse the local world under `.analysis/test-world`.
4. Start the debug server:

```sh
cargo run --bin mc-server -- --config example.toml
```

5. Connect with a vanilla 26.1.2 PrismLauncher client.

Do not commit `.analysis/`, `data/vanilla/`, local worlds, downloaded jars, or
client files.

## M77 Ledger Summary

M77 freezes the M100 denominator in
[VALIDATION_LEDGER.md](VALIDATION_LEDGER.md). This document no longer keeps a
broad `Ready Scope` list: broad login, chunk, lighting, block, farming, loot,
combat, persistence, multiplayer, performance, or ops claims remain
`stabilization` claims until the exact ledger row has normal-path runtime tests
plus separate vanilla oracle or real-client evidence.

The M95 conservative coverage audit reports 46 in-scope rows, 0 countable
`ready` rows, and 0.00% current conservative coverage. The result is
reproducible with `cargo run -p mc-test-harness --bin coverage-audit --
docs/VALIDATION_LEDGER.md` and summarized in
[VALIDATION_COVERAGE_AUDIT.md](VALIDATION_COVERAGE_AUDIT.md). Unit-only,
Solaris-only, wire-probe-only, protocol-metadata-only, negated, partial,
blocked, unknown, draft-debt, accepted-divergence, and non-goal rows do not
count toward the 80% target.

## 2026-06-13 Static Review Summary

A static documentation/code review on 2026-06-13 classified Solaris as a strong
stabilization-alpha/private vanilla-near base, not a release-ready vanilla
replacement. That review did not run cargo, a real vanilla client, or profiler
workloads; it adds no executable validation evidence by itself.

Positive readiness signals remain real: the docs are unusually honest about
claim boundaries, crates are modular, vanilla data sidecar discipline and
protocol-oracle discipline are explicit, several vertical slices exist end to
end, dense entity storage is the right direction, lock metrics are already
visible, and extension/script contracts are bounded boundary primitives rather
than a promised plugin API.

The same review keeps these blockers canonical for replacement claims:

- Real-client and vanilla-oracle evidence are still the largest gating gaps.
- Generated-world chunk/light streaming is functional but non-green: the M77
  view-distance-8 path emitted 289 chunks, yet the full window took 17.3s with
  `light_compute_ms=50489` plus tick, `chunk_prepare`, and `save_all_flush`
  warnings.
- The global `WorldStorage` mutex and `SessionRegistry` lock responsibilities
  still need narrowing, ownership review, and runtime evidence.
- Entity AI/pathfinding is draft, not broad vanilla behavior.
- Player water/swim feel, movement polish, and frozen-world/manual regressions
  remain open.
- Stale block edits and CAS-style transaction safety are not proven.
- Public auth/offline-mode defaults and session safety block public deployment.
- Persistence, crash recovery, soak, autoscale, and generated-world performance
  need real validation.
- Plugin API parity is a non-goal for M100; only bounded extension/script
  boundary primitives should be claimed.

## Evidence-Backed Partial Scope

- Login/config/play, chunk streaming, lighting, block edits, persistence,
  inventory, containers, mobs/combat, drops, XP, and multiplayer visibility have
  focused implementation and test coverage from earlier milestones, but still
  require current oracle/client and soak evidence before they count as broad
  M100 coverage.
- Rejected occupied-target block placement now has focused Solaris harness
  coverage requiring authoritative clicked-cell and target-cell block updates
  before the matching acknowledgement. A second focused occupied-target
  water-bucket fallback/resync harness also requires the authoritative held
  water-bucket slot with a fresh inventory state id before ack. A third focused
  harness covers survival out-of-reach `UseItemOn` against loaded clicked/target
  dirt cells and requires both cached loaded-cell block updates before ack. The
  out-of-reach water-bucket variant also guards that no held-slot correction is
  emitted before ack. A 2026-06-22 Java-agent run drove a real vanilla 26.1.2
  client through `m94-02b-rejected-block-resync` and validated the local
  artifact `.analysis/real-client-runs/20260622T135534Z-m94-regression-pack`;
  that run passed the occupied solid, out-of-reach solid, and occupied
  water-bucket fallback/resync observations and wrote
  `screenshots/m94-02b-rejected-block-resync.png`. These harnesses are
  local-sidecar-dependent and degrade/skip when required `data/vanilla` reports
  are absent. Broader stale edit/CAS paths, wider bucket/no-op edge cases,
  vanilla oracle evidence, and real-client desync coverage beyond that one
  scenario remain unproven.
- Early rejected survival block breaks now have focused packet/session coverage:
  `survival_break_requires_timed_stop_before_mutation` seeds a deterministic
  stone target, proves an early `STOP_DESTROY_BLOCK` does not mutate it, and
  requires an authoritative target-cell `BlockUpdate` before the matching ack.
  The first run failed because the ack arrived before the resync; broader
  stale-break/CAS behavior, anti-cheat variants, vanilla oracle, and real-client
  break-resync coverage remain unproven.
- Water buckets have focused real-client place/pickup evidence and packet
  harness coverage for scheduled spread timing. The scheduler now queues
  interaction-created fluid ticks from the shared simulation tick rather than a
  connection-local counter; `water_bucket_spread_waits_for_scheduled_fluid_delay`
  drives the normal `UseItemOn` packet path and guards against immediate
  post-placement water spread after the server has already ticked. This is not
  broad fluid parity: lava, full spread topology, water-lava interaction,
  bucket no-op variants, swim feel, and broad real-client/oracle coverage remain
  incomplete. A follow-up PrismLauncher run
  `.analysis/real-client-runs/20260622T174026Z-m94-regression-pack` reran the
  focused water place/pickup scenario after random-tick performance fixes and
  validated with no runtime tick budget warnings in the server log.
- Lava/water interaction now has focused Solaris packet/session coverage through
  `lava_bucket_next_to_water_solidifies_through_scheduled_fluid_tick`, which
  places lava with `UseItemOn` beside a water source and observes scheduled
  solidification to obsidian. This passed on the first run and is not real-client
  or fresh vanilla-oracle evidence; wider lava-water topology and visual/feel
  coverage remain open.
- Falling blocks now have focused Solaris packet/session coverage for one sand
  landing/removal path: the harness observes the falling-block `AddEntity`,
  requires sand to land as a block, requires a landing-chunk `LightUpdate`, and
  requires `RemoveEntities` for that same entity. This is not broad
  sand/gravel/anvil/drop parity, real-client evidence, or fresh vanilla-oracle
  evidence.
- A later broad `m94-02-blocks-fluids-farming-drops` run validated
  `.analysis/real-client-runs/20260622T180920Z-m94-regression-pack`; it composed
  the real-client solid break/drop/place and water-bucket place/pickup probes in
  one broad-row run. That row remains blocked because door/trapdoor,
  crop/bonemeal, sugar cane support/cascade/drop, broad fluid spread,
  water-lava interaction, and swim feel still lack in-client primitives and
  server evidence.
- A 2026-06-22 Java-agent run drove a real vanilla 26.1.2 client through
  `m94-01-join-rejoin-chunks-movement` and validated the local artifact
  `.analysis/real-client-runs/20260622T142127Z-m94-regression-pack`. The run
  reached Play, moved forward for `750` ms with `horizontal_delta=3.673`,
  observed the Solaris server-side session release marker, rejoined Play in
  `minecraft:overworld`, and wrote after-move plus after-rejoin screenshots.
  This is narrow evidence for one join/rejoin/movement path; it is not broad
  movement, water/swim, slow-client, two-client, or performance evidence.
- Beds set respawn points. A bounded draft sleep path skips a lone player from
  Solaris-defined night to the next morning through the existing `SetTime` path,
  but client-visible clock-map/time-sync behavior and sleep skip save/restart
  persistence are not proven. Multiplayer sleep quorum, weather, and
  client-visible bed animation parity are not claimed. Weather is currently
  disabled: Solaris does not start rain/thunder or send weather transitions.
- Signs support regular plain-text editing through Solaris harness coverage. A
  focused packet-path harness now flushes and reopens a disk-backed world after
  regular sign text edit and checks the persisted sign NBT plus client
  block-entity wire record. A broad `m94-04-signs-beds-campfires-and-block-entities`
  run validated `.analysis/real-client-runs/20260622T182159Z-m94-regression-pack`;
  it placed an oak sign, opened `SignEditScreen`, sent plain text, closed the
  editor, and rechecked the same text after close. Hanging signs,
  styled/filtered/clickable text, waxed semantics, sounds/statistics/game events,
  broad restart/client evidence, beds, campfires, and visual/manual parity are
  still not claimed.
- Crafting table, chest, barrel, furnace, smoker, and blast furnace paths have
  partial container/menu support. Barrels use single-container storage;
  smokers and blast furnaces share the furnace-family runtime. Focused shared
  chest and furnace stale-click guards now include storage/packet coverage, and
  both chest and furnace paths now have two-protocol-client stale-click resync
  harnesses after live peer slot updates. Focused unsupported/malformed chest
  and furnace click harnesses also prove Solaris ignores lying client
  slot/cursor deltas for `QuickCraft`, `Clone`, and `PickupAll`. For one
  malformed furnace pickup, Solaris rejects a conflicting non-empty post-click
  carried-item prediction, then resyncs authoritative content without mutating
  storage. Barrel
  animation, sounds/events, furnace lit-state, hopper/comparator automation,
  real-client stale-click or malformed-click coverage, and exact state-machine
  parity are not claimed.
- Common survival stations beyond crafting/furnace-family support are safe
  rejected in M87: brewing stand, anvil variants, enchanting table, smithing
  table, grindstone, stonecutter, loom, cartography table, composter,
  cauldron variants, lectern, fletching table, beacon, and crafter right-clicks
  are acknowledged without opening unimplemented menus or placing a held block
  through the station interaction. Their actual gameplay remains deferred.
- Recipe loading and runtime support are partial for furnace, blasting, smoking,
  and campfire variants. A 2026-06-22 Java-agent run drove a real vanilla
  26.1.2 client through `m94-03a-inventory-oak-log-to-planks` and validated
  `.analysis/real-client-runs/20260622T161221Z-m94-regression-pack`; that run
  loaded the configured vanilla sidecar recipe registry, sent the vanilla
  place-recipe packet for inventory container `0` with sidecar recipe display id
  `697`, consumed one oak log, and observed four added oak planks. Recipe-book/
  window sync, crafting-table UI, cursor recovery, containers/stations,
  malformed clicks, and broader recipe execution still need M80/M87 validation.
  A later broad `m94-03-inventory-crafting-containers-stations` run validated
  `.analysis/real-client-runs/20260622T180005Z-m94-regression-pack`; it again
  proved the oak-log recipe path, placed a chest, opened the vanilla
  `ContainerScreen`, and closed back to no screen. That broad row remains
  blocked because cursor transfer, chest/barrel clicks, furnace-family UI,
  common stations, malformed clicks, and recovery paths still lack in-client
  primitives and server evidence.
- Campfire cooking has usable partial support for held-input cooking, visible
  `Items` block-entity updates, cooked item drops, and pickup. In-flight
  persistence, automation, smoke/sound/particles, and exact ejection behavior are
  not claimed.
- Loot/drop support is scoped to deterministic one-stack block/entity drops from
  embedded fallback data or a configured local sidecar simple subset. Full
  vanilla loot execution, random rolls, predicates, fortune/silk touch, looting,
  equipment drops, exact XP, sounds/particles/statistics/events are not claimed.
- Farming and plants have partial deterministic Solaris support for common crops,
  nether wart, stems, sweet berries, cocoa, saplings, sugar cane, cactus, and
  bamboo. Cactus now clears visible existing columns when a player places a side
  neighbor through the normal placement path. Exact vanilla RNG,
  soil/moisture/light/support survivability, loot RNG, cactus damage/collision,
  particles/sounds/statistics/events, and several harness/client/oracle paths
  remain open.
- Bows/arrows and shields have partial local combat behavior. Exact projectile
  physics/metadata, durability, axe-disable, shield pose/timing/angle,
  attribution, effects, and broader damage-source parity are not claimed.
- Entities/combat/death now have one broad-row real-client partial:
  `.analysis/real-client-runs/20260622T190401Z-m94-regression-pack` observed a
  visible `minecraft:cow`, showed the real `DeathScreen`, performed vanilla
  respawn, and returned to Play with `current_screen=none`. That artifact also
  forced a server-side respawn recovery fix: Solaris now replays the completed
  chunk view and emits the vanilla-pinned level-chunks-load-start game event
  after `ClientboundRespawn`. Hostile combat, melee damage/knockback, mob drops,
  XP pickup, projectiles, shield timing, vehicles, and broad AI/pathing remain
  unclaimed.
- The named M40/M41 route now has one broad-row real-client partial:
  `.analysis/real-client-runs/20260622T191650Z-m94-regression-pack` placed and
  picked up water, broke a visible-drop target, picked up the dirt, placed dirt
  back into loaded air, observed a visible `minecraft:cow`, and captured a
  server log with no runtime tick-budget or background-relight warnings. Swim
  feel, sugar cane support/cascade/drop, the owner frozen-world route, and full
  TPS/lock performance evidence remain unclaimed.
- Save/restart now has one broad-row real-client partial:
  `.analysis/real-client-runs/20260622T210703Z-m94-regression-pack` placed a
  dirt marker through the real client, sent `save-all`, stopped Solaris with
  `kill -INT`, restarted, reconnected the same real client, and observed the
  marker persisted. The same artifact also launched `SolarisObserver` from a
  separate PrismLauncher root and passed one two-client live block-visibility
  check where the observer saw a primary-client dirt marker plus one
  two-client shared-drop visibility check where the observer saw a dirt item
  entity produced by the primary client, and one two-client shared-pickup
  removal check where the primary collected that dirt through the real client
  and the observer saw the item disappear. This is not full persistence or
  multiplayer evidence: block entities, containers, time/weather, crash
  recovery, shared-container state, contention, broad two-client join/move/edit
  coverage, and soak remain unclaimed.
- M37/M42 provide earlier metrics and lock-pressure evidence, but the M100
  low-end/balanced/high-end performance, concurrency, and autoscale gates remain
  non-green. M77 generated-world validation proved functional spawn-window
  generation/streaming, but also exposed a performance blocker: the
  view-distance-8 live stream emitted all 289 chunks only after 17.3s and logged
  repeated tick-budget plus `chunk_prepare` lock warnings. Later 2026-06-22
  debug `wire-probe` slices pre-generated the one-chunk light border, baked
  view-square light into generated chunks, reused persisted section light arrays,
  and warmed existing-world spawn chunks before listening. Those slices reduced
  the fresh generated-world stream to `327` ms and the warmed restart stream to
  `338` ms with `light_compute_ms=0`, `slow_light_compute_chunks=0`, and
  `slow_fetch_chunks=0`. A later existing-world diagnostic
  `.analysis/perf-runs/20260622T163447Z-generated-world-full-window` captured
  the remaining missing-light path at `light_compute_ms=47773`; the follow-up
  backfill and persistence probes moved that same stream to `light_compute_ms=0`
  and `323` ms, then proved an immediate restart stayed at `baked=0`/`flushed=0`
  after a client run. The work remains non-green: fresh startup paid `17273` ms
  total pregen-plus-bake cost, first existing-world missing-light backfill paid
  startup light cost for 289 chunks, earlier warmed streams logged lock waits,
  and broad performance profiles are unrun. A follow-up focused slice moved
  random block ticks and scheduled fluid ticks from desired/ticketed chunks to
  already-loaded chunks, skipped full relight for light-inert random/fluid edits,
  preserved baked light for those light-inert edits, and persisted recomputed
  relight, with `mc-net` regressions for those cases. The bounded before/after
  protocol probes
  `.analysis/perf-runs/20260622T162551Z-loaded-simulation-boundary` and
  `.analysis/perf-runs/20260622T162829Z-loaded-simulation-light-inert` moved the
  short-run runtime tick evidence from repeated random-tick-dominated budget
  warnings to no slow-tick warnings and `random_tick_us` below about `0.6` ms,
  but this is not a full generated-world performance gate. A real-client
  follow-up then reproduced the remaining PrismLauncher random-tick stall in
  `.analysis/real-client-runs/20260622T173420Z-m94-regression-pack`
  (`random_tick_us` up to `525492`) and cleared it in
  `.analysis/real-client-runs/20260622T174026Z-m94-regression-pack` by keeping
  random neighbor reads resident-only and using background incremental relight
  from saved baked light; that focused run still is not a full O1/O2 profile.
  A 2026-07-04 ignored protocol harness also keeps the 289-chunk
  generated-world stream green while proving `chunk_prepare` wait/hold counters
  advance and the in-memory stream does not enter `save_all_flush` or
  `player_persistence` lock paths. This is still not a disk-backed
  generated-world latency gate, slow-client test, hardware profile, broad lock
  review, or soak run.

## Blocked Or Unknown For M100

- Real-client automation/manual evidence is blocked at the full-pack level.
  M94 now has approved real vanilla 26.1.2 client gates for
  `m94-02b-rejected-block-resync` and
  `m94-01-join-rejoin-chunks-movement`,
  `m94-02a-solid-place-break-drop`,
  `m94-02c-water-bucket-place-pickup`,
  `m94-04a-regular-sign-place-text`, and
  `m94-03a-inventory-oak-log-to-planks`. Broad `m94-02` has blocked partial
  evidence for solid break/drop/place plus water place/pickup, broad `m94-03`
  has blocked partial evidence for recipe plus simple chest open/close, and
  broad `m94-04` has blocked partial evidence for regular sign text plus
  after-close visibility. Broad `m94-05` has blocked partial evidence for
  visible passive entity sync plus death/respawn. Broad `m94-07` has blocked
  partial evidence for the water/drop/entity parts of the named M40/M41 route,
  and broad `m94-06` has blocked partial evidence for same-client save/restart
  marker persistence, one two-client live block-visibility check, one
  two-client shared-drop visibility check, and one two-client shared-pickup
  removal check. No M94 manifest scenario remains unrun/manual-pending, but the
  broad pack is not green.
- Systematic vanilla oracle scenarios are blocked until M79 inventories and
  promotes usable captures. Protocol bots remain harness evidence, not client
  evidence.
- Water/swim feel and the M40/M41 frozen-world/manual regression route remain
  blocked until rerun green or explicitly reclassified; the `m94-07` artifact
  covers water placement/pickup, not client-local swimming feel.
- Online-mode/session auth, duplicate-name handling, permissions, chat policy,
  and malformed-action fail-closed behavior need M89. `mc-server --check` now
  emits structured public-bind warnings for offline-mode auth and local-dev
  operators, plus a missing-world-dir warning for configs that would start
  without persistent chunk streaming/storage. That is check-output evidence only;
  it is not online-mode auth, filesystem/permissions validation, a public
  deployment gate, or a broader security audit.
- Boats, minecarts, common survival station gameplay beyond M87 safe rejection,
  and redstone-lite automation need M81/M83/M94/M95 decisions, implementation,
  or evidence-backed deferral.
- Crash recovery, LZ4 Anvil compression, persisted light arrays, dropped-item
  lifecycle, long multiplayer soak, and autoscale behavior need M88-M96 evidence.
- Generated-world join/chunk streaming must be fixed or explicitly carried as
  non-release-ready debt before M100: no unexplained 150ms+ runtime ticks,
  `chunk_prepare`/`save_all_flush` warnings, unbudgeted startup prep, shutdown
  hangs, first-join generated-border work, or broad profile/slow-client gaps.

## Accepted Non-Goals And Divergences

- Other dimensions, Nether/End parity, full portal travel, structures, villages,
  trading, full biome parity, modded clients, plugin APIs, custom datapacks
  beyond the local sidecar, and resource-pack-specific behavior are outside the
  M100 core MVP unless a later ADR changes scope. Solaris now prefers the
  `minecraft:overworld` dimension type when present and otherwise falls back to
  the first configured dimension for degraded/test data; it does not implement
  portal transfer.
- Full redstone computer parity, pistons, observers, quasi-connectivity, and
  exact redstone update order are non-goals for M81 redstone-lite.
- Full vanilla worldgen and bit-perfect loot/RNG/sounds/particles/statistics/game
  events are accepted divergences unless M95 identifies a specific omission as a
  normal-survival blocker.
- Transparent shared-world horizontal sharding is not part of autoscale scope.

## Manual Gate

Run a PrismLauncher session against `cargo run --bin mc-server -- --config
example.toml` and cover:

1. Join/rejoin.
2. Exploration/chunk streaming.
3. Mining and block placement.
4. Doors, trapdoors, buckets, signs, and beds.
5. Crafting table, chest, barrel, furnace, smoker, blast furnace.
6. Farming and fluids.
7. Mobs, combat, death, respawn, drops, XP.
8. Save/restart persistence.
9. Two-client visibility, edits, pickups, containers, and reconnects.

Record any client disconnect, visual desync, missing packet, panic, or protocol
decode error back into the next milestone plan.

Until a real-client automation path exists, this gate is owner-run by
default. An MCP server or equivalent harness may take it over only if it
drives a real vanilla 26.1.2 client and records reproducible client-side
observations, not just server logs or protocol-bot success.
