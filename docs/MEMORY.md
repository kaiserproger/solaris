# Solaris Durable Memory Index

This is the short continuity index for long `/goal` runs. It records current
head and routes detail to its canonical owner. Historical checkpoint prose is
kept in [`archive/status/2026-07-19-memory.md`](archive/status/2026-07-19-memory.md)
and is not startup context.

## Current Checkpoint

- Date: 2026-07-22.
- Branch: `dev/M100-client-agent`.
- Latest recorded checkpoint: the client fluid-contact blocker is fixed after
  the bounded protocol, pose, and water-plant commit `7ac0fa6`. Basic economy
  and land claims shipped in `1c2ba4a`. The prior mob death deadline checkpoint is
  `45ff133` (`fix(play): index mob death deadlines`).
  The reach checkpoint is `5146926`, creeper checkpoint is `9af1309`,
  dry-spawn checkpoint is `6b0dcec`, autoscale checkpoint is `5b7017a`, and
  water checkpoint is `547525e`.
  Delivery-order checkpoint `5e2908a` remains binding.
- The worktree may contain unrelated owner files and local artifacts. Inspect
  exact ownership before editing; never clean or stage them by accident.
- Fresh-player spawn now chooses the nearest non-hazardous collidable support
  with collision-free, non-fluid body space in the resident 11x11 spawn window.
  Focused tests cover water, transparent collision, and magma support. A new
  real 26.1.2 client on seed `20260721` changed the initial sampled cell below
  spawn from water to air; this observation alone does not prove settled
  landing. The final tested O3 binary is
  `6be274ad51f43129e4949ad2a5eea39444d50d580bd694f5340e300b59b105d9`.
- Creepers use server-owned 30-tick retained fuses, reverse fuse progress
  beyond seven blocks,
  stop navigation while swelling, never survive a prior lethal transition to
  explode, do not persist natural swell across restart, and explode with power
  3 through the same
  ordered authority path as TNT. The source-specific explosion contract keeps
  TNT at power 4 and resolves chained TNT from the canonical registry instead
  of the exploding entity type. Unit and real TCP gates cover prime/cancel,
  terminal removal, radius-3 explosion, and player damage. Exact 26.1.2
  swell/ignited wire indexes and line-of-sight cancellation are still pending;
  no manual-client gate was run.
- Block use/break, entity interaction, and default melee now use separate
  26.1.2 eye-to-AABB verification contracts. Block and entity interactions are
  strict at their buffered limits; attack is inclusive. Player and
  server-entity combat both use the authoritative held item's attack range;
  embedded and sidecar item facts cover the seven 26.1.2 spears. Player pose
  selects standing, crouching, or swimming eye height and target bounds, and
  non-finite inputs fail closed. Focused reach, mob damage/death, death timing,
  and skeleton tests plus the full `mc-net` suite pass. A manual client gate
  remains open.
- Mob death completion is indexed by exact retained deadline instead of an
  unconditional full entity scan. Lethal melee/projectile/effect paths and
  persisted restore populate the index. Cleanup drains at most four deaths per
  tick, keeping mass-death overload bounded. The explicit `-O3` 4,096-cow load
  measured idle p99 11 us, sustained four-kill p99 13,668 us, and bounded
  four-removal p99 24,367 us. Focused death/effect/arrow/restart tests pass;
  this does not prove real socket throughput or manual combat feel.
- `basic-economy` now owns durable virtual wallets and an inventory shop with
  atomic item/balance commit. `land-claims` owns a bounded durable whole-chunk
  index, waits for targeted zone-command results, and rolls storage back when
  a registry change is rejected. The production block-break and placement
  paths consult the shared zone adapter before mutation. Focused
  Lua and two-client TCP tests prove default balance, purchase, claim commit,
  owner/operator bypass, stranger rejection, and unchanged world blocks. The
  adapter convention is intentionally temporary and protects direct break and
  placement only; containers, fluids, pistons, explosions, fire, and entity
  interaction remain open. No manual-client gate has run.
- The agent-run real-client hostile-combat functional gate is closed on an
  isolated O3 server. Ordinary 26.1.2 client actions selected an iron sword,
  killed a zombie, observed and collected its rotten-flesh drop, observed a
  skeleton arrow and player damage, then observed creeper damage and removal
  while remaining `in_play=true`; this is consistent with the exact explosion
  path already proved by the TCP regression. The retained harness shows that
  operator commands only created the deterministic fixture. The server logged
  one `57.474 ms` tick after processing 62 simulation commands with 10 still
  queued and no reliable drop, retry, or disconnect warning. Evidence:
  `.analysis/mcp-combat-check.py`,
  `.analysis/codex-logs/mcp-hostile-combat-result-v2.json` and
  `.analysis/codex-logs/mcp-hostile-combat-server-v2.log`. Next: run a
  20-minute MCP survival session with subagent-made decisions, no deterministic
  scenario runner, and no operator setup; subjective combat feel remains open.
- Hostile attack planning no longer holds `SessionRegistry.inner` across
  regional entity-owner requests. It copies live target pose/visibility, runs
  creeper fuse CAS, skeleton arrow spawn, and melee attacker validation through
  regional owners. Final melee admission uses per-session immutable
  combat-target/visibility snapshots and accepts an ordered reservation only if
  their shared odd/even publication epoch stays unchanged. Disconnect publishes
  non-targetable before queue close. Final melee does not reacquire the global
  session registry. Focused races cover attacker/target death, movement,
  Spectator, unregister, and completion while that registry is held elsewhere;
  manual feel remains pending.
- The exact dense 5,132-cow O3 gate is closed. The final reproduced cause was
  an unanswered keepalive challenge while valid movement packets still proved
  the client alive. Solaris now preserves one pending challenge, requires both
  challenge and total inbound inactivity before timeout, uses vanilla's
  three-tick default movement interval, and rotates a 512-candidate movement
  shard under extreme load; arrows, items, and XP remain latency-sensitive.
  A real 26.1.2 MCP client completed 975 client ticks with 5,227 total server
  entities and remained `in_play=true`; the server logged no keepalive
  mismatch/timeout, reliable drop, or retry. Evidence:
  `.analysis/codex-logs/dense-5132-spawn.json`,
  `.analysis/codex-logs/dense-5132-release-build-v5.log`,
  `.analysis/codex-logs/dense-5132-keepalive-fixed-v5.json`, and
  `.analysis/codex-logs/dense-5132-fixed-v5-server.log`. The current
  autoscaler slice removes per-tick owner-lane reconfiguration on `Hold`, skips
  capacity-capped no-op actions, requires 20% recovery headroom, and coalesces
  continuous slow-tick warnings to the 100-tick metrics cadence. Focused and
  full workspace L2 gates pass. The
  current water slice adds vanilla swimming metadata and server-owned
  air/drowning/recovery. Aquatic entity physics uses fish drag without generic
  buoyancy, removing the force that held fish at the surface. Canonical
  `LivingAquatic` and `LivingAmphibious` contracts share that path. Focused
  tests and full workspace tests, strict Clippy, fmt, and code-health pass; all
  four prior reviewer findings were fixed. The follow-up sends the vanilla
  enabled-feature packet before known packs, makes water plants passable, and
  uses swimming/crouching body and eye heights for water/collision queries.
  The follow-up fixes the root cause of zero client-local fluid height: chunk
  encoding always published `fluid_count=0`, causing 26.1.2
  `LevelChunkSection.hasFluid()` to skip `EntityFluidInteraction`. The wire
  count now covers water, lava, water plants and waterlogged states. An O3 real
  client entering source water reports `in_water=true` and
  `water_fluid_height=0.8888889`; 81 chunks streamed with measured
  `chunk_data_ms=0`. The follow-up O3 MCP gate observed ascent, diving,
  swimming pose, air depletion, drowning damage and connection continuity.
  Evidence is `.analysis/codex-logs/deep-water-real-client-final.json`. The
  hostile-combat functional follow-up is recorded above.
- `7cdd917` fixes the ordinary active-game save path exposed by the natural
  furnace loop. A resident mutation during out-of-lock whole-region encoding
  now skips that Anvil region before filesystem installation and leaves it
  dirty for bounded replanning; stable independent regions continue. A real
  filesystem version mismatch remains `StaleRegion`, and exact barrier-save
  semantics are unchanged. Focused tests cover one-time and continuous
  resident conflict, whole-region skip, stable-region progress, cleanup, and
  the typed bounded failure. Full workspace tests, strict workspace Clippy,
  fmt, code-health `0 fail / KEEP`, and diff-check pass. The real-client
  artifact
  `.analysis/real-client-runs/20260721T112014Z-real-client-playable-loop-pURskM`
  completed the natural wood -> furnace -> charcoal scenario with runner exit
  0 and no dirty-flush degradation warning. This does not replace the pending
  owner-played 20-minute session or a vanilla oracle.
- `5e0d93b` adds bounded host-local Lua timers driven by pushed monotonic
  simulation ticks. Tick admission coalesces the newest tick under queue
  pressure without blocking the simulation thread; due callbacks run in
  deterministic deadline/id order, at most eight per pushed tick, and share
  one instruction and command budget with an optional `on_server_tick` handler.
  Replacement, cancellation, capacity/input rejection, handler rollback,
  same-tick cancellation, stale ticks, queue pressure, close/drain, and shared
  fuel failure have focused coverage. A real TCP/Lua gate proves command ->
  timer -> targeted client message without a `server.tick` subscription. Full
  workspace tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`,
  and diff-check pass. A `sol high` re-review found no remaining
  blocker/high/medium issue. No manual-client or vanilla-oracle gate was run
  for this plugin-only slice.
- `d59bd57` adds optional per-plugin `config.toml`, loaded and recursively
  bounded before plugin registration, plus a fresh-copy `solaris.config()` Lua
  API. The shipped currency catalog now reads currency, zone, and products from
  that file and validates its exact schema at load. A production TCP/Lua gate
  overrides the example with gold currency, a stone axe, and a moved zone, then
  proves menu content, buy, stale rejection, unchanged state, and refund. Full
  workspace tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`,
  and diff-check pass. A `sol high` re-review found no remaining
  blocker/high/medium issue. No manual-client or vanilla-oracle gate was run
  for this plugin-only slice.
- `9aee245` adds capability-gated Lua transactions over a connected player's
  main inventory and hotbar. The session endpoint plans every resource delta
  before replacing canonical persistence state and publishing one authoritative
  inventory snapshot. Unknown items, insufficient input, full inventory,
  absent/stale sessions, disconnect races, and worldless runtimes return exact
  targeted failures without partial mutation. The real TCP/Lua gate proves
  grant, exchange, failed overdraw, failed unknown-resource exchange, later
  clearing of the unchanged inventory, targeted isolation, and the worldless
  rejection. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found no remaining
  blocker/high/medium issue. No manual-client or vanilla-oracle gate was run for
  this plugin-only slice.
- `c82c344` adds capability-gated same-dimension Lua player teleports through
  the exact reliable session and authoritative simulation owner. Success
  survives cancellation after commit; missing/stale players, pending teleport
  confirmation, and runtime failure remain distinct. The real TCP/Lua gate
  proves the initial pending rejection, exact cross-chunk position sync and
  center replan, zone observation, targeted result isolation, repeated pending
  rejection, and authoritative follow-up pose. A direct queue test drops the
  session waiter after owner commit and still proves success plus the persisted
  pose. Full workspace tests, strict workspace Clippy, fmt, code-health `0 fail
  / KEEP`, and diff-check pass. A `sol high` re-review found no blocker. No
  manual-client or vanilla-oracle gate was run for this plugin-only slice.
- `d9c0804` derives the default 26.1.2 furnace contract from the complete
  resolved item-tag graph and carries a pinned 280-item fallback for embedded
  startup. Startup rejects a partial or drifted sidecar. Furnace, smoker,
  blast-furnace, container, and hopper paths share that immutable snapshot;
  specialized furnaces halve duration and non-flammable wood remains rejected.
  The local decompiled oracle and full sidecar match the fallback for all 280
  ids and durations. The real TCP container test smelts with oak stairs, and
  sad-path tests prove rejected menu/hopper transfers do not mutate state. Full
  workspace tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`,
  and diff-check pass. A `sol high` re-review found no remaining blocker. No
  manual Prism-client gate was run. Pre-existing entity-scale and local
  artifacts were not staged.
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

## Workflow Lock

- The persistent `/goal` is a north star. Execute one finite checkpoint using
  the Autonomous Goal Protocol in `AGENTS.md`; select only the explicit
  checkpoint route and never keyword-match injected goal/history text.
- Route exact surfaces through `.memory/MEMORY.md` and
  `docs/AGENT_ROUTES.md`; do not load the whole docs stack or raw session
  history after compaction.
- Finish the active request before accepting a later one unless the owner
  explicitly interrupts or replaces it. On retry, verify current process and
  worktree state before resuming.
- Keep implementation direct and local. Ask only about a material ambiguity;
  an explicit request does not need reconfirmation.
- Use the checkpoint's L0/L1/L2 tier. L2 runs only for a completed code commit,
  release, or milestone close and never repeats on an unchanged tree identity.
  Markdown/instruction-only work gets static/path/diff checks, not Cargo tests.
- Self-check every completed task and use exactly one independent read-only
  reviewer. Extra workers require an explicit owner request.
- Runtime event delivery, hard counters/fresh continuations, validation cache,
  compact subagent results, and conditional completion/blocked audits remain
  external Codex work described in `docs/GOAL_WRAPPER_V2.md`; repo prose must
  not pretend those mechanisms already exist.

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
- Production worldgen revision 4 consumes explicit `ChunkGeometry` for terrain,
  ores, structures, and biomes. `OverworldRouter` owns continuous continents,
  ridges, rivers, climate, and neighbour-connected tunnel caves behind a
  32-block surface shell. Trees require exact planned support and a stable 5x5
  footprint. Solaris worlds persist revision/seed/mode/geometry; unversioned
  Anvil worlds open without fallback generation. The playable profile uses
  `.analysis/test-world-v4`.
- Lua API 0.6 has bounded DTO/files/batches, optional bounded startup-only TOML
  configuration with fresh Lua copies, push-driven bounded simulation timers,
  one-shot host admission, an attested `mc-net` router, and durable plugin
  storage. Production adapters now
  cover menus, inventory/storage transactions, zones, same-dimension player
  teleports, colony records, ephemeral villager binding, owner-scoped
  `home`/`hold` orders through journaled regional goals, and required
  post-commit `player.block_broken` and `player.block_placed` events.
  `player.item_crafted` now covers committed 2x2,
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
  explosion, environmental, and non-player paths do not claim attribution.
  Accepted right-clicks now publish `player.entity_interacted` with an exact
  reachable living target, actor pose/mode, hand, and secondary-action
  snapshot. It is a gesture event, not proof of feeding, shearing, trading, or
  another vanilla side effect. The vanilla interaction and client writes finish
  before required Lua admission can wait; missing, nonliving, dying, far,
  Spectator, and dead-actor paths publish nothing. The death/kill
  owner-to-server outbox is unbounded to avoid waiting under owner locks;
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
  applied order only after the targeted owner result. The Lua API also has a
  capability-gated `list_online_players` query. It returns a
  targeted, sorted, bounded point-in-time identity/pose/dimension snapshot and
  marks truncation; closed session owners are excluded and no live handles are
  exposed. The shipped `online-roster` plugin consumes that result for `/who`
  and renders it through a server-owned inventory menu; its production TCP/Lua
  gate checks the connected player's exact name and dimension. Focused Lua
  coverage proves queue-rejection retry and the 128-byte menu-label bound.
  Plugin readiness and the combat-cooldown fixture are push-fenced by exact Lua
  messages and simulation ticks; timeouts only fail. General villager
  roles/work orders and durable entity handles remain absent.
- Production and test waits must remain event-driven. Timeouts only fail stuck
  work and never prove success.

### Playable And Client-Visible

- P02 real-client artifact
  `.analysis/real-client-runs/20260721T095305Z-real-client-playable-loop-hXlAv8`
  passed a no-debug natural birch loop: three block breaks with visible
  progress/drop/pickup, twelve planks, crafting table, sticks, wooden pickaxe,
  and table open/close. The server reported sub-500 ms tick-budget warnings in
  `animal_breeding`, with a 133 ms observed peak, but no client-visible failure.
  This is focused real-client evidence, not an owner-played 20-minute session or
  broad performance proof.

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

1. Run the requested 20-minute MCP survival session with subagent-made
   decisions, no deterministic scenario runner, and no operator setup. Fix its
   first common client-visible blocker before isolated parity or performance
   work; an owner-played subjective-feel gate remains separately pending.
2. Include the `d9c0804` fuel contract in that survival session; fix a concrete
   common client-visible regression if one appears.
3. If the session finds no common blocker, advance the production Lua plugin
   API before returning to broad optimization work.
4. Only after those priorities, continue reducing `simulation.rs` through
   explicit ownership boundaries; avoid moves that retain `use super::*` or
   duplicate authority.
5. Then advance regional ownership/ECS only with exact CAS, WAL, publication, and
   cross-region failure fences.
6. Broaden playable progression by the Pareto rule before polishing rare parity
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
