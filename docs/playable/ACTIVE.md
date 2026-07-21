# Active Playable Task

This file contains only the current playable queue and recent evidence. The
previous detailed log is preserved in
[`../archive/status/2026-07-19-playable-active.md`](../archive/status/2026-07-19-playable-active.md)
for targeted lookup.

## Target

Keep the normal 26.1.2 client stable through a useful survival session, then
broaden the loop beyond wood -> tools -> restart. Optimize for common gameplay,
multiplayer correctness, and visible failures before rare parity edges.

The baseline loop remains:

```text
join -> move -> gather -> craft -> build -> fight/farm -> save/rejoin
```

This is Playable Spike Mode. Do not turn focused playable evidence into M100
replacement-readiness claims.

## Current Queue

This queue is binding across context compaction: common vanilla gameplay first,
then production plugin API, then measured optimization, and only then rare
hardening. An already-open lower-priority diff does not override this order.

1. Get one owner-played survival session on the current build. Record concrete
   client-visible failures; do not substitute isolated parity probes for it.
2. Treat failures from the owner session as the playable queue. Fix the first
   common player-visible blocker, then rerun the shortest real-client scenario
   that reproduces it.
   The owner rerun still disconnected periodically in the dense 5,132-entity
   world. A clean O3 real-client rerun on a fresh small world did not reproduce
   it, so the dense owner-world gate remains open. The reported water gap
   now has server-owned air/drowning, vanilla swimming metadata, and aquatic
   physics that no longer pushes fish to the surface; owner-client verification
   remains pending.
3. After the disconnect, close the remaining common-play reports: dry-land
   random-seed spawn, player water movement, vanilla reach, and lag-free mob
   damage/death plus skeleton-arrow and creeper-explosion paths.
4. Ship the requested basic economy and land-claim plugins on the production
   Lua API, then document operator TOML options beside their values.
5. Improve terrain generation toward the concrete Tellus/Tectonic traits that
   can be measured without breaking vanilla world persistence. Then run a
   bounded explosion load benchmark and record the exact envelope.
6. Close this owner batch with an MCP-driven, unscripted survival session run
   by one fast subagent. Treat its visible failures as the next playable queue.
7. Keep the rare multi-region save recovery `fsync`/metrics issue documented
   but deferred. Do not resume it unless it becomes ordinary save corruption or
   blocks the playable or plugin path.

## Recent Evidence

- The current O3 binary (`f299a01c1dd281cf6cb82b587b40390be2a35a8f294de32f199f45048d0fb60f`)
  passed a short embedded-MCP real-client
  gate on an isolated fresh world: the 26.1.2 client joined, reached play,
  loaded blocks, observed 53 entities, and accepted a forward-input request.
  Server logs contained no slow-tick, autoscale, reliable-drop, or disconnect
  warning. This proves the ordinary small-world path only; it does not replace
  the pending 5,132-entity owner-world rerun and does not establish binary
  provenance from a clean tracked tree. The same gate found the player
  standing over `minecraft:water` on fresh seed `20260721`, directly confirming
  dry-land spawn selection as the next reproducible playable checkpoint.
- This checkpoint fixes two concrete hot-path faults from the owner's dense
  5,132-entity O3 run. Autoscale recovery now requires 20% tick headroom, so
  50-57 ms boundary jitter cannot alternate `ScaleDown` and `ScaleUp`.
  Per-tick `Hold` and capacity-capped actions no longer synchronously
  reconfigure regional owner lanes or invalidate their read routes. Continuous
  slow-tick warnings emit on episode entry and then every 100 ticks, not every
  tick. Direct tests preserve memory shedding and drain-to-one behavior; all
  1,600 `mc-net` tests and all workspace L2 gates pass. The independent review found the
  missing application-path coverage, which was added. This removes observed
  autoscaler churn but does not yet prove or claim that the separate periodic
  client disconnect is fixed; an owner-client rerun is required.
- This checkpoint adds the ordinary water survival path. Player eye immersion
  now consumes the vanilla 300-tick air supply, publishes metadata index 1,
  deals two drowning damage at the vanilla `-20` boundary, and recovers four
  air per tick outside water or in invulnerable modes. Swimming publishes the
  shared entity flag `0x10`. Aquatic entity queries use fish water drag without
  generic buoyancy, so canonical 26.1.2 aquatic and amphibious mobs are no
  longer driven to the surface by the shared body kernel. Focused breathing,
  metadata, classification, and sampled-water
  physics regressions and full workspace tests, strict Clippy, fmt, and
  code-health pass. The independent reviewer found incomplete aquatic class
  coverage, Adventure immunity, respawn air carryover, and a rejected-commit
  damage loss; all four were fixed. Owner-client verification is pending.
- The owner-run O3 build exposed a server-triggered disconnect while loading a
  dense world: 5,132 entities produced 5,702 per-entity spawn dispatches and
  overflowed the bounded reliable queue (`reliable_command_drops=963`). Chunk
  visibility now publishes one ordered spawn batch per loaded chunk, pauses
  further chunk emission at outbound pressure, and writes at most 16 entity
  spawns per play-loop turn so keepalive and timeout boundaries keep making
  progress. The 17-entity/channel-capacity-1 regression passes with zero drops
  and exact entity accounting; all 1,589 `mc-net` tests and three doc tests
  pass. A `sol high` reviewer confirmed ordering and state-loss behavior and
  requested the bounded write turns, which were added. Owner-client rerun on
  the rebuilt O3 binary remains pending.
- Checkpoint `7cdd917` fixes a normal active-game save conflict found by the
  natural furnace scenario. The first artifact
  `.analysis/real-client-runs/20260721T110747Z-real-client-playable-loop-yFIIqx`
  completed birch -> table -> wooden pickaxe -> cobblestone -> furnace ->
  charcoal, but the runner exited 1 after repeated false `region changed before
  replace` dirty-flush warnings. The resident chunk had changed while its whole
  Anvil region encoded outside the world lock; this is not a filesystem CAS
  failure. The normal flush now skips that region before disk installation,
  keeps it dirty for bounded replanning, and continues stable regions. The
  second artifact
  `.analysis/real-client-runs/20260721T112014Z-real-client-playable-loop-pURskM`
  passed the same no-debug natural loop with runner exit 0, no dirty-flush or
  pressure-flush warning, and periodic saves draining to zero dirty chunks.
  The observed warned tick peak was about 55 ms. Full workspace tests, strict
  workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass.
  The rare partial-install `fsync`/counting debt remains deferred. This does
  not replace the pending owner-played 20-minute session or a vanilla oracle.
- Checkpoint `5e0d93b` adds bounded host-local Lua timers driven by pushed
  monotonic simulation ticks. Queue pressure coalesces the newest tick instead
  of blocking the simulation thread or requiring plugin polling. Timer
  callbacks are ordered, capped at eight per pushed tick, and share one command
  and instruction budget with `on_server_tick`. Focused tests cover replacement,
  cancellation, invalid input, exact capacity, stale/coalesced ticks, handler
  rollback, same-tick cancellation, close/drain, and shared-budget failure. A
  real TCP/Lua gate proves a player command schedules a timer and receives its
  targeted result without subscribing to `server.tick`. Full workspace tests,
  strict workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check
  pass. A `sol high` re-review found no blocker/high/medium issue. No
  manual-client or vanilla-oracle gate was run for this plugin-only slice.
- Checkpoint `d59bd57` adds optional bounded `config.toml` snapshots to Lua API
  `0.6.0`. Configuration is validated before command ownership, read once, and
  returned as a fresh recursive Lua table. The shipped currency catalog now
  takes currency, zone, and products from operator configuration. Its real
  TCP/Lua gate overrides all three and proves exact menu content, purchase,
  stale rejection without mutation, and refund. Full workspace tests, strict
  workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass. A
  `sol high` re-review found no blocker/high/medium issue. No manual-client or
  vanilla-oracle gate was run for this plugin-only slice.
- P02 real-client artifact
  `.analysis/real-client-runs/20260721T095305Z-real-client-playable-loop-hXlAv8`
  passes natural birch breaking with visible progress, drop and pickup, then
  crafts twelve planks, a table, sticks, and a wooden pickaxe and opens/closes
  the table. It used the Gradle client adapter without debug grants. The server
  emitted sub-500 ms tick-budget warnings in `animal_breeding`, peaking at 133
  ms, but the scenario had no client-visible failure. This does not replace the
  pending owner-played 20-minute session or broad performance evidence.
- Checkpoint `9aee245` adds production Lua player-inventory transactions for
  atomic grants and exchanges over the connected player's main inventory and
  hotbar. Planning precedes canonical state replacement, so unknown resources,
  insufficient input, full output, stale/disconnected sessions, and worldless
  runtimes do not partially mutate inventory. Results are correlated and
  targeted to the issuing plugin. The real TCP/Lua gate proves grant, exchange,
  two rejected mixed transactions, unchanged state after each rejection,
  targeted non-leak, and a worldless `runtime_unavailable` rejection without an
  inventory packet. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found
  no remaining blocker/high/medium issue. No manual-client or vanilla-oracle
  gate was run for this plugin-only slice.
- Checkpoint `c82c344` adds production Lua same-dimension player teleports
  through the exact reliable session and authoritative simulation owner. The
  result distinguishes unavailable/stale players, an outstanding vanilla
  teleport confirmation, and runtime failure; success cannot become failure if
  the session waiter is cancelled after owner commit. The real TCP/Lua gate
  proves pending rejection, exact cross-chunk position and center publication,
  zone observation, targeted non-leak, repeated pending rejection, and the
  authoritative follow-up pose. A direct queue test proves the post-commit
  cancellation case. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found
  no blocker. No manual-client or vanilla-oracle gate was run for this
  plugin-only slice.
- Checkpoint `0969620` completes the production Lua zone membership lifecycle
  with owner-targeted `player.zone_exited` snapshots. Accepted absolute movement
  publishes deterministic exits before entries with the authoritative new pose;
  stale, rejected, no-op, disconnect, and zone-removal paths publish nothing.
  The real TCP/Lua gate proves pre-teleport movement rejection, entry, exit,
  outside no-op, re-entry, and isolation from another subscribed plugin through
  exact pushed chat fences. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` review found no
  blocker; its rejected-movement finding was added. No manual-client or vanilla
  oracle gate was run for this plugin-only slice.
- Checkpoint `d9c0804` replaces the narrow furnace-fuel matcher with the exact
  default-feature 26.1.2 builder order over resolved item tags plus a complete
  repo-owned 280-item fallback. Sidecar startup rejects membership or duration
  drift instead of silently accepting a partial fuel graph. Furnace, smoker,
  blast-furnace, menu, quick-move/swap/pickup, and hopper paths share the same
  immutable snapshot; smoker and blast-furnace durations are halved, while
  crimson/warped wood is removed after all additions. The local decompiled
  `FuelValues` oracle and full sidecar match the fallback for all 280 ids and
  durations. The real TCP container gate smelts iron with oak stairs; focused
  tests also prove accepted hopper transfer and mutation-free rejected menu and
  hopper transfers. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` review found a
  partial-sidecar acceptance gap and missing transaction sad paths; both were
  fixed and the re-review found no blocker. No manual Prism-client gate was run.
- Checkpoint `99b9879` adds production `player.entity_interacted` Lua events for
  authoritative reachable right-click gestures against alive server-owned
  living entities. The exact event carries actor identity/pose/mode, target
  id/type, hand, and secondary-action state; it does not claim a vanilla side
  effect. Missing, far, nonliving, dying, Spectator, and dead-actor paths emit
  nothing. Vanilla feed/shear handling and client writes complete before
  required Lua admission can wait, and write errors retain immediate cleanup.
  The production TCP/Lua gate proves rejected far/missing attempts followed by
  exact off-hand/secondary and main-hand events without a quiet-window success
  condition. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. A `sol high` review found and verified
  the delivery-order fixes. No manual-client or vanilla-oracle gate was run for
  this plugin-only slice.
- Checkpoint `aabea52` adds the production `player.entity_killed` Lua event for
  exact direct player-melee kills. It publishes only after target lethality and
  attacker costs commit, captures the transaction pose, and does not attribute
  nonlethal, unreachable, stale-cost, repeated-dying, projectile, explosion,
  environmental, or non-player damage. The real TCP/Lua gate observes two
  distinct kills from the same committed-event FIFO and proves no delayed
  duplicate; direct tests also cover a moved session snapshot and closed
  outbox. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. One overloaded workspace attempt exposed
  unrelated existing probe/TNT timing failures; each focused rerun passed and a
  clean single workspace run passed. A `sol high` review found no blocker; the
  stale-position finding was fixed, while global cross-producer script ordering
  remains explicitly outside the outbox FIFO contract. No manual/client or
  vanilla-oracle gate was run for this plugin-only slice.
- Checkpoint `e09c6ec` replaces smoke-only confidence in the shipped Lua
  examples with production wire evidence. The exact currency catalog files now
  pass zone activation, rendered menu contents, an atomic three-emerald/two-
  apple purchase, insufficient-funds rejection with unchanged ledger, and a
  refund. The exact colony scaffold files now pass `/colony recruit worker`,
  durable activation, initial `home`, a later owner-accepted `hold`, and status
  reload. The gate then removes the bound villager, proves cached-token owner
  rejection, one fresh binding attempt, and the explicit no-villager result.
  Plugin-emitted readiness messages and exact combat-cooldown tick events are
  push barriers; no elapsed-time success condition is used. Full workspace
  tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`, diff-check,
  focused plugin `2/2`, colony router `17/17`, and exact loader `1/1` pass. A
  final `sol high` review found no blocker, high, or medium issue. This is
  server/wire plugin evidence, not a manual-client or vanilla-oracle gate.
- Checkpoint `9330336` adds required `player.died` events at the authoritative
  live-to-dead survival commit. Operator, fall, starvation, contact, hostile,
  PvP, and projectile paths converge there; nonlethal, shield-blocked, stale,
  unsupported-mode, already-dead, and respawn paths publish nothing. The owner
  snapshots the event before fallible client writes, shutdown drains all
  producers and this outbox before required `server.stopping`, and Lua admission
  remains bounded and push-driven. The owner outbox is intentionally unbounded
  because its synchronous commit cannot await capacity while holding state
  locks; revisit that debt only if measured workloads make it material. A real
  packet/Lua gate observes health zero, one death event, respawn, and no
  duplicate. Its prior pickup timeout was a genuine two-client race in the test:
  both clients shared spawn and could claim the same dirt. Causally fenced peer
  movement now isolates the collector without longer timeouts. Full workspace
  tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`, and
  diff-check pass; `commands` passes `14/14` and `mc-server --test play` passes
  `16/16`. A final `sol high` review found no blocker, high, or medium issue. No
  manual/client or vanilla-oracle gate was run for this plugin-only event slice.
- Checkpoint `51b2659` adds required post-commit `player.item_picked_up`
  events for exact item-entity and grounded-arrow inventory credits. Partial
  pickup reports only the merged count. Stationary drops now wake nearby
  sessions from an exact simulation-tick readiness index instead of depending
  on entity movement or polling. A regression found by the full workspace gate
  removes duplicate candidate publication when physics and readiness meet on
  the transition tick. `sol high` review found that deferred campfire outputs
  entered the index before journal acknowledgement; the index now activates
  only after entity publication, and a focused regression proves hidden
  outputs cannot be picked up. Full workspace tests, strict workspace Clippy,
  fmt, code-health `0 fail / KEEP`, and diff-check pass. The command wire gate
  passes `14/14`; no manual/client or vanilla-oracle gate was run for this
  plugin event slice.
- The current plugin slice adds post-commit `player.item_crafted` events for
  2x2 inventory crafting, 3x3 crafting-table result clicks, and recipe-book
  crafting. Max crafting reports one aggregate output/count pair. Direct tests
  prove stale owner state, mismatched/no-op clicks, missing ingredients, and
  unsupported game modes publish nothing before pushed FIFO fences. The real
  packet/Lua gate observes an inventory commit of two oak logs into eight
  planks and one exact `craft_count = 2` event, then proves a missing-input
  retry emits no event. Focused `mc-script`, `mc-net`, and wire tests pass.
  Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found no
  remaining blocker, high, or medium issue. No manual/client or vanilla-oracle
  gate was run for this plugin-only event slice.
- The current plugin slice adds required post-commit `player.block_placed`
  events for the actual registry-backed root state. The shared real packet/Lua
  gate observes creative and survival commits and FIFO-fences blocked and
  empty-hand retries. A direct owner-stale stair dependency test proves no
  placement event before a pushed `server.tick` fence. Focused contract,
  adapter, and wire tests pass. Full workspace tests, strict workspace Clippy,
  fmt, code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review
  found no remaining blocker, high, or medium issue. No manual/client or
  vanilla-oracle gate was run for this plugin-only event slice.
- The current production plugin slice publishes `player.block_broken` only
  after an authoritative root block transition. A real packet/Lua wire gate
  observes exact creative and survival events. FIFO command fences prove abort,
  repeated-air attempts after both modes, and a two-client owner-stale survival
  completion publish nothing. Focused `mc-script`, `mc-net`, and wire tests
  pass. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check also pass.
- Checkpoint `5ea197b` makes the active save install its exact simulation
  barrier snapshot and makes accepted PvP attacks observable from the
  simulation owner. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. The broad `block_edit` gate
  passes `94/94`.
- Real-client artifact
  `.analysis/real-client-runs/20260720T234001Z-real-client-playable-loop-QHfB9Z`
  passed natural log breaking with visible progress, drop pickup, and log-to-
  planks crafting through the real Gradle client. With no new common blocker in
  that focused loop, work moved to the production plugin API as required by the
  queue above.
- Checkpoint `feba79a` passes full workspace tests, workspace all-target strict
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check. The `block_edit`
  target also passes both parallel and sequential runs with 94/94 tests.
- Ordinary wall torches have registry-backed tests for four horizontal facings,
  standing `UP`, rejected `DOWN`, and partial support. Raw TCP proves one debit
  after accepted update/ack and unchanged held-stack resync before rejected ack.
- Stair facing/half, slab top/bottom, matching-slab merge, waterlogging, and
  stair neighbour-shape recomputation use the inspected local 26.1.2 rule.
  Selector and real placement/break adapter tests cover every corner shape and
  stale dependency rollback; a dedicated raw-TCP corner assertion remains open.
- The regional mutation extraction is architecture-only and makes no gameplay
  or performance claim.
- Latest P44 artifact is
  `.analysis/real-client-runs/20260720T120018Z-real-client-playable-loop-UJtsgc`.
  Sheep and chicken passed, including chicken yaw. The selected cow moved 2.69
  blocks on a flat Y=78 surface but never encountered a rise, so the scenario
  failed its 0.8-block climb condition. The prior artifact
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`
  already observed a real-client cow climb of 1.0 block. P44 therefore needs a
  deterministic climb candidate before another run can be strong evidence.
- The unrestricted P44 run exposed a 3.47-second entity-journal checkpoint
  stall. Entity checkpoint acknowledgement is now memory-only after the durable
  world checkpoint: older append-only WAL records are filtered by the saved
  lifecycle/sequence watermark, and physical compaction runs on normal journal
  shutdown. A production-journal regression proves the next mutation queues an
  `Append`, not a checkpoint `Replace`; crash-before-compaction replay and
  shutdown compaction both have focused coverage. This is not a broad
  performance claim.
- The PvP wire oracle now waits on an accepted attack event published by the
  simulation owner instead of assuming TCP ingress lands on a chosen tick. The
  event separates cooldown and hurt-resistance clocks and carries owner-order
  sequence and attacker identity. Focused wire runs, the authority-clock unit,
  the reciprocal concurrent-owner test, full `mc-net`, and `sol high` review
  pass. The first broad `block_edit` run found the separate persistence failure
  below, so this is focused PvP evidence rather than a broad gate claim.
- The persistence barrier defect found by broad validation is fixed locally.
  Background flush now validates the resident region before install and
  replans stale work. Active save installs its exact owner-barrier snapshot,
  leaves post-barrier mutations dirty, and waits for the exact journal-fence
  release before recapturing an incomplete snapshot. Final post-drain save
  rejects an orphaned fence instead of retrying a fixed number of times or
  acknowledging it. Focused `mc-world` dirty-flush tests pass `20/20`, active
  save tests pass `4/4`, the orphan-fence sad path passes, and the parallel
  `mc-net` passes `1534/1534` runnable tests, and a fresh parallel `block_edit`
  gate passes `94/94`, including restart persistence and in-flight campfire
  state. Full workspace passes. A second independent `sol high` review found no
  blocker or high-severity issue. It found only a rare multi-region recovery
  path that can `fsync` an installed prefix under the world mutex and omit that
  prefix from aggregate metrics; this is explicitly deferred behind common
  gameplay and production plugin work.
- P47 artifact
  `.analysis/real-client-runs/20260720T122329Z-real-client-playable-loop-Dbzfoj`
  passed stonecutter placement, menu open, normal take (1 input to 2 slabs),
  close/reopen conservation, and shift-click (3 inputs to 6 slabs). The outer
  runner returned degraded because startup emitted 350 ms and 52 ms slow-tick
  warnings; the client scenario itself exited 0. Its setup used three
  `giveAndSelect` debug commands, so this is real-client wire/menu evidence,
  not earned-survival gameplay evidence. Earned setup and rejected invalid
  input remain open.
- P48 artifact
  `.analysis/real-client-runs/20260720T124754Z-real-client-playable-loop-l8eWbc`
  passed the no-debug, no-op real-client building scenario. The client earned
  wood, stone, a furnace, charcoal, torches, and matching planks; crafted
  stairs and slabs; then proved wall-torch facing, stair facing/half, bottom
  and top slabs, matching-slab merge, exact inventory debits, and rejected
  bottom-slab wall-torch support without a debit. The scenario and driver
  exited successfully and produced a valid screenshot. The outer gate remains
  degraded because `server.log` contains slow-tick warnings, including one
  342 ms entity-physics tick and one 271 ms entity-goals tick, so this is
  gameplay evidence rather than a clean combined gameplay/performance gate.
- P04 artifact
  `.analysis/real-client-runs/20260720T143912Z-real-client-playable-loop-rjWZVp`
  passed the full no-debug `24,000`-tick continuity gate. The real Gradle
  client gathered natural birch, observed break progress and drops, crafted a
  table, sticks, wooden pickaxe, and wooden sword, completed 27 later resource
  cycles, then survived a clean server stop/restart and proved the table and
  pickaxe persisted after rejoin. The continuity profile disables only natural
  hostile spawning; manual play and combat scenarios keep it enabled. Earlier
  real-client evidence separately proved wooden-sword zombie and skeleton
  combat. The run emitted 44 tick-budget warnings with a maximum of 412.302 ms,
  so this is functional/playable evidence, not a clean performance result.
- P11 artifact
  `.analysis/real-client-runs/20260720T155306Z-real-client-playable-loop-YAMHzs`
  passed the no-debug food loop. The real client killed a natural chicken,
  collected its drop, sprinted until food fell from 20 to 19, then consumed the
  earned chicken and observed food return to 20 while the stack fell from one
  to zero. The failed predecessor exposed that fractional movement exhaustion
  was discarded before reaching the 4.0 threshold. Accepted movement now adds
  every positive exhaustion increment in the same owner turn that commits the
  pose, while health packets remain limited to visible food or saturation
  changes. The repeat after review emitted no tick-budget or slow-tick
  warnings. This is focused gameplay evidence, not a broad performance result.
- An interactive embedded-MCP run on the isolated world under
  `.analysis/mcp-smoke-ZZm7qf` passed
  `playable-05-stone-tool-progression` in 25.7 seconds after connection. The
  real 26.1.2 client mined three natural birch logs, crafted planks, a table,
  sticks, and a wooden pickaxe, mined three natural stone blocks into collected
  cobblestone, reopened the earned table, and crafted a stone pickaxe without
  debug commands. The structured MCP response proved exact inventory and world
  transitions. Server output had two boundary tick warnings at 56.150 and
  54.209 ms, but no multi-second journal stall. This is focused gameplay
  evidence, not a broad performance result.
- The first live Lua gameplay adapter now connects admitted `upsert_zone` and
  `remove_zone` commands to initial/accepted player poses and disconnect
  cleanup. A wire client waits for a plugin readiness message, enters the zone
  through a normal movement packet, and receives the owning plugin's targeted
  `player.zone_entered` reply. Changed bounds do not repeat entry while the
  player remains inside. Workspace tests, strict Clippy, fmt, code-health and
  the 94-test `block_edit` target pass.
- Lua inventory menus now have an end-to-end wire gate in an embedded playable
  world. The test proves admitted Lua open, the client `OpenScreen` and fixed
  content, stale-state rejection, a normal predicted primary click, and the
  plugin's response. A second subscribed plugin plus a later targeted command
  fence proves `inventory.menu.clicked` did not leak beyond the menu owner.
  Atomic inventory/storage transactions now route through the storage actor.
  A disk-backed wire test gives a player currency, commits a purchase and one
  ledger CAS, observes the authoritative inventory, then proves a stale CAS
  rejects without another inventory mutation or leaking its targeted result.
  Storage unit coverage proves multi-key restart replay, one batch revision,
  stale/quota rejection, definite write failure, and unknown-sync replay. The
  runtime transaction excludes concurrent player inventory mutation, but the
  plugin WAL and vanilla playerdata are not yet one crash-recovery log.
- The embedded 26.1.2 client MCP now exposes ordinary primary and secondary
  container-slot clicks and waits for an applied server update instead of a
  guessed delay. An agent-run client on a fresh local world entered the catalog
  zone, received the inventory menu, bought two apples for three emeralds, and
  refunded them. Structured observations proved exact `64 -> 61 -> 64`
  emerald and `0 -> 2 -> 0` apple counts, menu reopen IDs `1 -> 2 -> 3`, ledger
  labels `owned 0 -> 1 -> 0`, and both plugin messages. A stale slot click after
  closing the menu was rejected before packet dispatch. This is focused plugin
  gameplay evidence, not a broad survival or readiness gate.
- Admitted Lua colony upserts now reach a bounded owner-scoped production
  registry and publish a correlated `colony.record_result` only to the owning
  plugin. Villager binding now validates colony ownership and the current
  overworld dimension, then uses the bounded regional-owner query to install an
  atomic 600-tick opaque claim without scanning session snapshots. Missing,
  foreign, out-of-dimension, capacity-exhausted, and no-villager requests return
  a targeted empty result; broken owner availability remains fatal. Unit
  coverage includes the real Lua admission path, and a TCP client wire gate
  observes colony upsert. Records remain in-memory and plugin storage is the
  durable intent source.
- The first production villager-order slice accepts only `home` and `hold` for
  an owner-scoped, unexpired binding token. It routes the mutation through the
  regional entity owner, validates that the live entity is still a villager,
  journals the goal, and publishes a correlated result only to the owning
  plugin. Unit coverage includes stale, removed, foreign-owner, publication-
  close, and owner-failure paths. A disk-backed Lua wire test proves bind ->
  home order -> targeted result through an ordinary joined client. General
  roles, work orders, and direct path or memory control remain deliberately
  absent.
- Restart evidence now requires the stopped server process to exit with status
  0. A recorded interrupt without a clean exit can no longer pass validation.
- Multi-entity physics dispatch no longer sends one cached owner mutation per
  entity, each of which ran the complete ECS `PhysicsApply` schedule. The actor
  groups cached same-lane updates by region without serializing unrelated
  lanes; multi-lane work uses the coordinator's equivalent grouping.
  Deterministic tests prove 76 same-region entities run one schedule, same-lane
  and multi-lane regions run one each, stale input runs none, and journal
  failure rolls the whole batch back. The existing 512-entity debug benchmark
  reported actor `p50 5.107 ms` and `p99 6.400 ms`. A real 26.1.2 client
  observed all 31 persisted passive entities
  through 255 client ticks; warned dispatch samples were `4.814`, `10.640`, and
  `2.913 ms`, rather than the earlier repeated `300+ ms` stalls. Canonical
  pathing and collision tables now prewarm before the entity ticker starts. A
  fresh real-client rerun built 5,436 pathing facts before listening, kept all
  32 client-visible entities across 255 ticks, and no longer reproduced the
  earlier `282.512 ms` physics or `316.242 ms` goals first-use stalls. Its only
  warned tick was `56.709 ms`, with goals at `9.575 ms`, physics at `1.264 ms`,
  and dispatch at `11.671 ms`. This closes the catastrophic cold-table stall;
  it is not a replacement for a longer performance soak.

## Manual And Agent Gates

Default playable server:

```sh
cargo run --bin mc-server -- --config playable.toml
```

Use the embedded client MCP for reproducible agent-run observations when the
scenario exists. Record whether a result is owner-run, agent-run, prepared
only, or not run. Screenshots may support a visual finding, but world/protocol
state should come from structured client observations when available.

## Stop Conditions

- Do not update readiness or validation-ledger rows in Playable Spike Mode
  unless the owner explicitly requests readiness work.
- Do not call parity from unit or Solaris-only wire evidence.
- Stop hardening a rare edge once dominant risk is proved and the next common
  gameplay blocker is more valuable.
