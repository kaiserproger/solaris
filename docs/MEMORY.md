# Solaris Durable Memory Index

This is the short continuity index for long `/goal` runs. It records current
head and routes detail to its canonical owner. Historical checkpoint prose is
kept in [`archive/status/2026-07-19-memory.md`](archive/status/2026-07-19-memory.md)
and is not startup context.

## Current Checkpoint

- Date: 2026-07-21.
- Branch: `dev/M100-client-agent`.
- Latest checkpoint: `aabea52` (`feat(plugins): publish committed entity
  kills`). Delivery-order checkpoint `5e2908a` remains binding.
- The worktree may contain unrelated owner files and local artifacts. Inspect
  exact ownership before editing; never clean or stage them by accident.
- Full workspace tests, workspace all-target strict Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check passed immediately before `aabea52`. The real
  TCP/Lua gate proved two exact direct-melee kill events without a delayed
  duplicate; direct tests cover nonlethal, unreachable, stale-cost,
  repeated-dying, moved-session-snapshot, and closed-outbox paths. A `sol high`
  review found no blocker; its stale-position finding was fixed. Global ordering
  against concurrent lossy/script producers remains outside the committed
  outbox FIFO contract and was not broadened into a script-bus redesign. No
  manual/client or vanilla-oracle gate was run for this plugin-only slice.
  Pre-existing entity-scale and local artifact changes were not staged.
- Ignored oracle/load/benchmark rows remain explicit. The P04 real-client soak
  ran; broad performance and dedicated concurrency gates did not.

## Delivery Priority Lock

After compaction, resume in this order unless the owner explicitly changes it:

1. Common vanilla-client gameplay and multiplayer parity.
2. Production Lua plugin API and its gameplay adapters.
3. Measured optimization, regional ownership, ECS, and autoscaling.
4. Rare error-path hardening and uncommon parity edges.

The current multi-region save recovery has a narrow deferred error path: a
later-region install failure can synchronously `fsync` the already-installed
prefix while the caller still holds the world mutex, and that recovered prefix
is not included in aggregate flush metrics. The normal save path and ordinary
crash-safety fences are covered. Do not resume this hardening before the first
two priorities unless it becomes a common-play blocker or corruption risk.

## Current Head

### Core And Ownership

- `play.rs` is 13,108 lines, `session.rs` 1,571, `simulation.rs` 15,855,
  `server.rs` 8,356, and `chunk_stream.rs` 8,221. The migration is staged, not
  complete.
- `simulation/queue.rs` owns bounded admission, accounting, pushed wakeup,
  batching, shutdown, and channel construction.
- `simulation/regional_mutation.rs` owns the existing regional block/container
  mutation lane behind explicit imports and code-health tripwires. The parent
  still owns classification, batching, world access, lighting/publication, and
  `SimulationOwner`.
- `EntityStore` is the production ECS runtime; the old vector comparison state,
  `Shadow*` API, aliases, and `shadow-compare` feature are deleted. Exact
  26.1.2 modules now cover entity contracts, attributes, effects, equipment,
  living damage, navigation, projectiles, synced data, and runtime transactions.
  Gameplay-significant side maps and live-scale propagation still need removal
  or explicit authority fences, so broad sole-authority readiness is not yet
  established. ADR 0004/0005 are the authority source of truth.
- Runtime work control has no operator worker-percentage knobs. Capacity is
  derived once; pushed measurements and bounded admissions drive allocation.
- Serverbound protocol collections/strings/blobs have a complete bounded
  allocation audit, symmetric encode limits, and no-partial-output tests.
- Production worldgen now consumes explicit `ChunkGeometry` for terrain, ores,
  structures, and biome assignment. Extreme valid geometries use checked/wide
  arithmetic, and the default Overworld path has a deterministic serialized-NBT
  fingerprint. The algorithm remains Solaris-owned rather than Mojang
  NoiseRouter parity.
- Lua API 0.6 has bounded DTO/files/batches, one-shot host admission, an
  attested `mc-net` router, and durable plugin storage. Production adapters now
  cover menus, inventory/storage transactions, zones, colony records, ephemeral
  villager binding, owner-scoped `home`/`hold` orders through journaled regional
  goals, and required post-commit `player.block_broken` and
  `player.block_placed` events. `player.item_crafted` now covers committed 2x2,
  3x3, and recipe-book crafts with aggregate max-craft counts and required
  queue admission. `player.item_picked_up` now reports exact authoritative
  item-entity and grounded-arrow credits, including partial stack pickup.
  Stationary item readiness is push-driven from an exact-tick index, and
  deferred campfire outputs enter that index only after durable acknowledgement
  and publication. `player.died` now publishes one immutable event from the
  authoritative live-to-dead owner commit for common operator, fall, starvation,
  contact, hostile, PvP, and projectile damage. It is captured before fallible
  client writes and drained before required `server.stopping`; nonlethal,
  shield-blocked, stale, unsupported-mode, already-dead, and respawn paths emit
  nothing. Killer/cause attribution remains deliberately absent until every
  source carries exact facts. Direct player melee entity kills publish a
  separate exact `player.entity_killed` fact with target id/type and explicit
  `source = melee`; nonlethal, unreachable, stale, repeated-dying, projectile,
  explosion, environmental, and non-player paths do not claim attribution. The
  shared owner-to-server outbox is unbounded to avoid waiting under owner locks;
  do not revisit it before playable/Lua work unless a measured hostile workload
  makes its memory material. Direct tests cover
  cursor mismatch, full output inventory,
  owner-stale rejection, no-op, queue closure after commit, aggregate counts
  above `u32`, invalid pickup identities/modes, transition-tick deduplication,
  and unpublished campfire outputs; the wire gate covers exact committed event
  fields and rejected retries. Block DTOs expose player pose separately from
  integer block coordinates. The exact shipped currency catalog now has a
  production wire gate for zone activation, buy, insufficient-funds rejection,
  unchanged ledger, and refund. The exact shipped colony scaffold has a
  production wire gate for durable recruit, `home`, later accepted `hold`, and
  removed-villager recovery. It retains the active binding token in Lua memory,
  retries one rejected cached token through a fresh binding, and reports an
  applied order only after the targeted owner result. Plugin readiness and the
  combat-cooldown fixture are push-fenced by exact Lua messages and simulation
  ticks; timeouts only fail. General villager roles/work orders and durable
  entity handles remain absent.
- Production and test waits must remain event-driven. Timeouts only fail stuck
  work and never prove success.

### Playable And Client-Visible

- Stair facing/half, slab top/bottom, adjacent matching-slab merge,
  waterlogging, and stair neighbour-shape recomputation follow the inspected
  local 26.1.2 rule. Unit and adapter coverage at `feba79a` includes all corner
  shapes and stale dependency rejection; a dedicated raw-TCP corner assertion
  remains absent.
- Ordinary torches place as wall torches on horizontal conservative full-cube
  supports, remain standing on `UP`, and reject `DOWN` or known partial
  supports. Irregular sturdy-face parity and neighbour break cascades remain
  open.
- P47 real-client artifact
  `.analysis/real-client-runs/20260720T122329Z-real-client-playable-loop-Dbzfoj`
  passed stonecutter placement, menu open, normal take, close/reopen
  conservation, and shift-click conservation. The scenario exited 0; the outer
  runner was degraded by two startup slow-tick warnings. Setup used three
  `giveAndSelect` debug commands, so this proves the real-client menu/wire path,
  not earned survival. Earned setup and rejected invalid input remain open.
- P48 real-client artifact
  `.analysis/real-client-runs/20260720T124754Z-real-client-playable-loop-l8eWbc`
  passed earned wall-torch, stair, and slab building through a no-debug Gradle
  client with `server_op_users=NONE`; the scenario and driver exited 0. The
  outer validator remained degraded by slow-tick warnings, so do not call the
  combined gameplay/performance gate green.
- P04 artifact
  `.analysis/real-client-runs/20260720T143912Z-real-client-playable-loop-rjWZVp`
  passes natural gather/craft, 27 continued resource cycles, all `24,000`
  continuity ticks, clean server exit/restart, rejoin, placed-table
  persistence, and wooden-pickaxe persistence. Its generated config disables
  natural hostile spawning so bot tactics cannot invalidate the continuity
  proof; manual play and separate combat scenarios still enable monsters.
  Earlier real-client runs separately proved wooden-sword zombie and skeleton
  kills. The P04 run had 44 tick-budget warnings, maximum 412.302 ms, and is not
  broad performance evidence.
- The embedded client MCP provides reusable connection, observation, movement,
  interaction, and scenario tooling. Read `docs/AGENT_TOOLING.md` before
  changing it; protocol bots do not replace the real-client gate.

### Known Runtime Evidence

- Latest P44 artifact:
  `.analysis/real-client-runs/20260720T120018Z-real-client-playable-loop-UJtsgc`.
  Sheep and chicken passed, including chicken yaw. The selected cow moved 2.69
  blocks on flat terrain and did not satisfy the climb condition. The preceding
  P44 artifact observed a 1.0-block cow climb, so this is a nondeterministic
  candidate-selection gap rather than evidence that step physics regressed.
- The unrestricted run exposed a 3.47-second checkpoint stall caused by waiting
  for entity-journal replacement while holding the regional journal mutex. The
  checkpoint now acknowledges exact identities in memory after the durable
  world watermark; gameplay appends never queue behind a replacement `fsync`.
  Old replay-safe records are compacted on normal journal shutdown. Focused
  append-order, crash-replay, and shutdown-compaction regressions and the full
  workspace baseline are green.

## Active Risks

1. Run an owner-played survival session and fix its first common client-visible
   blocker before isolated parity or performance work.
2. If owner play finds no common blocker, advance the production Lua plugin API
   before returning to broad optimization work.
3. Only after those two priorities, continue reducing `simulation.rs` through explicit ownership boundaries;
   avoid moves that retain `use super::*` or duplicate authority.
4. Then advance regional ownership/ECS only with exact CAS, WAL, publication, and
   cross-region failure fences.
5. Broaden playable progression by the Pareto rule before polishing rare parity
   or save-error edges.

## Canonical Routes

| Need | Read |
| --- | --- |
| Playable/client behavior | `docs/playable/README.md`, then `docs/playable/ACTIVE.md` |
| Architecture/ownership | `docs/decisions/README.md`, then the exact ADR |
| Detailed core internals and pitfalls | `docs/CORE_INTERNALS_FOR_OWNER.md` |
| Current M100 milestone | `docs/milestones/M100.md` |
| Readiness claim | `docs/DEFINITION_OF_DONE.md` and `docs/VALIDATION_LEDGER.md` |
| Protocol | ADR 0002 and local protocol tools |
| Client MCP | `docs/AGENT_TOOLING.md` and the client-agent README |
| Server Lua API | `docs/PLUGINS.md` |

## Update Rules

- Replace stale current-head facts; do not append a wave-by-wave diary.
- Put architecture decisions in ADRs and playable observations in
  `docs/playable/ACTIVE.md`.
- Keep raw run output under `.analysis/` and out of commits.
- Use archives only to recover a specific old fact.
