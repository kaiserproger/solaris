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

1. Get one owner-played survival session on the current build. Record concrete
   client-visible failures; do not substitute isolated parity probes for it.
2. Treat failures from the owner session as the playable queue. Fix the first
   common player-visible blocker, then rerun the shortest real-client scenario
   that reproduces it.
3. If that session has no common blocker, move to the first production plugin
   API slice while keeping the playable gate fixed. Defer optimization unless
   play exposes a catastrophic stall.
4. Defer deterministic livestock climbing and earned stonecutter hardening
   until they block ordinary survival or the plugin-backed gameplay loop.

## Recent Evidence

- Checkpoint `feba79a` passes full workspace tests, workspace all-target strict
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check. The `block_edit`
  target also passes both parallel and sequential runs with 94/94 tests.
- Ordinary wall torches have registry-backed tests for four horizontal facings,
  standing `UP`, rejected `DOWN`, and partial support. Raw TCP proves one debit
  after accepted update/ack and unchanged held-stack resync before rejected ack.
- Stair facing/half, slab top/bottom, matching-slab merge, and waterlogging use
  the inspected local 26.1.2 rule. Stair neighbour-shape selection remains open.
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
