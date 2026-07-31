# Solaris Durable Memory Index

This is the short continuity index for long `/goal` runs. It records current
head and routes detail to its canonical owner. Historical checkpoint prose is
kept in [`archive/status/2026-07-19-memory.md`](archive/status/2026-07-19-memory.md)
and is not startup context.

## Current Checkpoint

- Date: 2026-07-31.
- Branch: `main`.
- A 2026-07-31 Pro static-review bundle is now tracked through
  `docs/CODE_REVIEW_REMEDIATION_2026-07-31.md`: 42 findings are grouped into
  parser/data, persistence, network, ownership, authority, and loader/architecture
  waves. Every item requires current-tree confirmation and focused evidence before
  it is treated as complete. `SOL-023` canonical `Identifier` serde, `SOL-024`
  validated resource paths, and `SOL-016` world-journal parser/file budgets are
  complete; `SOL-035` count-prefixed protocol allocation safety is next.
- The first owner-run public-alpha session remains the routing authority for the
  next stabilization release; its exact plan is `docs/PUBLIC_ALPHA_PLAN.md`.
- The future full Luau addon platform is frozen in
  `docs/LUAU_ADDON_API_1_0_SPEC.md`; its 145-task decomposition is in
  `docs/LUAU_ADDON_API_1_0_TASKS.md`. Only the two documentation tasks are
  complete and all 143 runtime tasks remain `BLOCKED-VP` until scoped vanilla
  parity and explicit owner activation.
- The interrupted colony checkpoint is already complete in `4902a3d`: colony
  identity, roles, orders, and persistence live in Luau/plugin storage, while
  Rust exposes only generic villager binding and movement/idle goal primitives.
- Periodic natural spawning is implemented with independent friendly/hostile
  cadences, bounded rotating loaded-chunk work, admission fences, refill after
  movement/despawn, common-biome rules, and metrics. The no-operator 20-minute
  client observation and restart-identity acceptance gates remain open.
- Plugin deployment requirements are now fully operator-visible. Discovery,
  startup logs, and `--check` derive deployment class, supported loaders,
  permissions, bundle identities, artifact sizes, and totals from validated
  manifests. Shipped examples carry explicit deployment labels, and a failed
  required-Loader handshake receives a Configuration disconnect naming the
  supported platforms and required bundles. The full graphical Loader matrix
  remains external and must not be claimed from this headless workspace.
- The strong baseline must be preserved: seed `712816`, VD16 and one local player
  pre-generated 225 chunks at 929.473 chunks/s, streamed every requested chunk
  without degradation or memory-pressure shedding, used roughly 300 MiB by owner
  observation, and shut down with zero dirty chunks.
- The P0 clock checkpoint is complete. The empty 26.1.2 clock-update map was
  replaced by a typed overworld update: monotonic simulation tick remains packet
  `game_time`, while persisted world time is the separate registry-id-0 clock at
  rate `1.0`. Exact protocol/package/server/command/sleep gates and the real-client
  day/sunset/night/dawn/restart graphical gate pass; evidence is in
  `docs/evidence/world-clock-26.1.2.md`.
- Worldgen revision 10 now removes the 384-block origin blend, forced stone/iron
  outcrops, and starter-tree anchor. A bounded 32-seed Tellus gate, including seed
  `712816`, finds distinct dry low-relief terrain; schema 3 persists the selected
  spawn and startup generation/light plus final player support search use it. The
  public default and `example.toml` are `tellus_like`, and the playable world moved
  to `.analysis/test-world-v10`. Unversioned Anvil imports skip Solaris spawn
  selection, retain the origin fallback, and do not gain a Solaris contract. The
  single read-only worldgen reviewer consumed the diff but timed out at 180 seconds
  without a verdict or actionable finding; no second reviewer was run. Final gates
  pass for `mc-world` 229/229, `mc-worldgen` 94/94 plus 12/12, `mc-net` 1828/1828,
  the complete `mc-server` package, and the five-seed real server/client spawn
  harness. `cargo test --workspace --quiet` printed every executable group green
  through the final worldgen group, then exceeded the 180-second wrapper limit;
  workspace doc-tests pass separately (3/3).
- Revision-10 vegetation is now seed-driven and regionally coherent instead of
  uniform per-column modulo noise. One 192-block density field combines regional
  noise with routed moisture, then biome thresholds create forest edges, grassland
  copses and sparse savanna cover. Savannas use acacia logs/leaves; deserts,
  snowy plains and ice spikes are treeless, while taiga and grove retain spruce.
  A 32-seed 8192x8192 metric requires distinct biome/feature fingerprints, at
  least 12 land biomes in aggregate and no more than eight sufficiently-landed
  seeds above 90% one land biome. `mc-worldgen` passes 110/110 and the external
  worldgen harness passes 5/5. The narrow single-thread debug probe measured
  23.2 chunks/s versus 23.0 before the non-vegetated-column optimization; it is
  not the owner i5-12600 release throughput gate. The single read-only vegetation
  reviewer inspected the production and external-test diff, then timed out at
  180 seconds without a verdict or actionable finding; no second reviewer was run.
  Drainage, rendered 2048x2048 mosaics, clean seed-`712816` owner playtest and
  release throughput remain active.
- The current P0 lock checkpoint removes the confirmed public-alpha defect.
  Production item-drop owner commits finish before short session publication.
  Item pickup plans from immutable snapshots, installs one runtime-only regional
  claim token, validates session and player state without nested waits, and resolves
  that token against the current entity snapshot so rollback preserves newer motion.
  The regional owner blocks competing stack/remove/merge/lifecycle/damage mutations
  during the claim while allowing kinematics. Claim install/rollback/finalize are
  checkpoint-only; the simulation `SaveBarrier`
  captures matching player and entity state, while the direct save path is only used
  after owner drain. Debug phase timings cover owner claim/finalize, session, player,
  and publication. Deterministic lock, checkpoint, and interleaving tests prove
  progress, conservation, and crash consistency. The reproducible 200-action
  raw-TCP break/drop/pickup gate is now green: session-registry max hold fell from
  5,266 us to 2,178 us, player-persistence max hold from 5,263 us to 285 us, and
  the final exact 1,200-sample tick window peaked at 9,643 us. `mc-net` passes
  1859/1859.
- The single independent lock-diff review session exhausted its 180-second limit
  without a verdict. It focused on the real risk that inverse full-snapshot rollback
  could lose an interleaving motion update. No second reviewer was run; the reviewed
  design was replaced by current-snapshot token resolution and checkpoint-only
  durability, with dedicated regressions for both findings.
- P1 natural spawning no longer relies on one-shot chunk materialization. The
  periodic scheduler and common-biome rules are implemented; only the real-client
  observation and restart-identity acceptance rows remain open.
- Plugin deployment reporting is complete without a duplicate manifest flag.
  Remaining Loader work is the external Fabric/NeoForge/Forge visual matrix and
  any future addon-platform scope explicitly activated by the owner.
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
- Primed TNT/creeper expiry uses an exact retained-deadline index populated by
  spawn, fuse updates, restore, and removal. Rescheduling removes the previous
  bucket entry, and repeated owner calls cannot exceed one explosion per world
  tick. An O3 full-path load with 4,096 background cows, 64 explosions, and a
  fresh 27-block solid volume per explosion measured idle fuse p99 0 us and
  explosion-tick p50/p95/p99 23,812/37,943/46,463 us. This is in-process
  authority/world/entity evidence, not publication or socket throughput.
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
- `basic-economy` now owns one configurable physical item currency, a
  zone-activated inventory shop, and a durable refund ledger. Purchase and
  refund use one inventory/storage transaction, so currency, product, and
  ledger never commit separately. The old virtual wallet and duplicate
  `currency-catalog` fixture were removed. Stable product ids preserve original
  refund terms across catalog edits and reject purchases until old terms are
  cleared. A production TCP/Lua gate proves a configured gold-ingot purchase,
  insufficient-funds rejection, refund, and zone-triggered menu refresh.
  `land-claims` owns a bounded durable whole-chunk index. Direct break/place,
  right-click block actions, containers, buckets, living-entity interaction,
  explosion block damage, and the bounded common-fuel fire path are protected.
  A direct lever/button can extend or retract one normal piston with one common
  full block; its atomic base/head/destination edits consume the ambient
  protection snapshot in direct and scheduled-button planning. No manual-client
  gate has run.
- Land-claim admission now covers right-click block actions, all halves of a
  chest window, exact filled-bucket destinations, living-entity target
  positions, and explosion block candidates in addition to direct break/place.
  Every chest/furnace click rechecks the backing positions. Player actions keep
  the bounded authoritative actor check. Explosion planning clones one
  immutable generic protection snapshot only when an explosion is due and before the world
  lock, so idle ticks do not copy zones and the candidate loop takes no zone
  mutex. Random fire planning now consumes the same immutable snapshot and
  skips protected burn targets while allowing the source fire to age. The
  baseline normal-piston mutation is atomic and rejects the whole group if any
  affected position is protected; sticky pistons, multi-block chains,
  slime/honey, and moving animation remain outside this slice. The fire slice
  does not yet reproduce the complete vanilla material/odds table, and the
  bounded actor lookup still needs the published policy index described by ADR
  0009. Rust contains no `land-claims` plugin-id or zone-id convention:
  `solaris.upsert_protected_zone` carries a typed actor-or-operator policy, and
  the Lua plugin owns claim identity, persistence, and lifecycle.
- Current production Lua mutations hide regional/session ownership completely:
  entity spawn and villager commands enter simulation/regional owners, while
  menus, teleports, and standalone player-inventory transactions enter the
  exact ordered session lane. Standalone inventory routing now waits for the
  session owner to plan from live inventory and update the durable mirror;
  dropped commands reject without mutation. The compound inventory/storage
  transaction shares the same internal session gate, so it cannot plan past an
  earlier owner command and its ledger and inventory cannot commit separately.
  No lock, region key,
  lease, epoch, or worker handle enters a Lua DTO.
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
  regional entity-owner requests. It reads a dedicated active-hostile ID
  publication, stable per-session target/visibility snapshots, and an atomic
  skeleton arrow type, then runs creeper fuse CAS, arrow spawn, and melee
  attacker validation through regional owners. Final melee admission uses
  per-session immutable
  combat-target/visibility snapshots and accepts an ordered reservation only if
  their shared odd/even publication epoch stays unchanged. Disconnect publishes
  non-targetable before queue close. Final melee does not reacquire the global
  session registry. Focused races cover attacker/target death, movement,
  Spectator, unregister, and completion while that registry is held elsewhere;
  an ordinary whole melee tick also completes under that held lock. Regional
  selection replaces the hostile publication before attacks on each goal turn;
  unload and zero-live-session paths clear it without later owner reads. Manual
  feel remains pending.
- Ordinary entity goal-input collection no longer enters
  `SessionRegistry.inner`. Active chunks, a 64-shard chunk-to-entity index,
  terrain-pathing IDs, and per-session combat-target poses are immutable
  publications. A revision fence prevents cross-shard moves from disappearing
  from concurrent snapshots. The duplicate chunk/entity maps were removed from
  `SessionRegistry.inner`; visibility, projectiles, grazing, lifecycle radius
  queries, and player-body relocation now use the same routing authority.
  Mutation paths remain centralized, so regional mutation ownership is still
  incomplete.
- Physics chunk crossings now release `SessionRegistry.inner`, update all
  routing moves under one generation fence with one clone per touched shard,
  then reacquire the session lock for visibility/tracker publication. Each move
  requires its expected old route, so a concurrent newer relocation/removal
  wins and the stale crossing is not published. Session membership can progress
  independently.
- Common physics publication updates only position, rotation, velocity, and
  on-ground fields in the existing wire snapshot; it no longer rebuilds the
  full entity DTO or repeats entity-type lookup under `SessionRegistry.inner`.
  Tracker admission uses one sharded get-or-insert instead of separate
  contains/owner-snapshot/insert/get operations. Visibility, arrow resolution,
  and the freshness recheck still intentionally serialize on the session lock.
- Ground wanderers retain independent deterministic 3-7-block targets until
  arrival, pause without drift, and turn body/head at bounded rates. Physics
  preserves goal-owned rotation when collision clips velocity; zero-speed
  hostile melee still faces immediately. Animals in love use a courtship
  follow goal and return to wandering after breeding. Exhausted wander paths
  retarget instead of retrying forever. Old persisted path JSON defaults the
  two added pause fields. Focused client evidence covers natural sheep, pig,
  and cow motion with non-zero yaw change and no vertical rise.
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
- The dense-world latency follow-up bounds work instead of treating every
  artificial cow as a 20 Hz obligation. Populations within the autoscaler
  budget remain full cadence; larger active sets rotate deterministic
  `256 * cpu_limit` simulation cohorts, with 512 goal and dense natural-movement
  limits. Sheep grazing uses a maintained sheep index, while breeding uses a
  lock-free index of babies and animals in love without shrinking its full
  active eligibility to the physics cohort. In the exact 5,227-entity O3
  975-client-tick gate, over-budget warnings fell from 223 to 8, entity-goal
  warning p50 from 78.237 ms to 17.473 ms, and grazing warning p50 from
  8.389 ms to 0.234 ms. The client remained in play with no disconnect,
  reliable-command loss, or runtime work-budget info spam. Representative
  interaction is also green: a fresh agent-run 26.1.2 `playable-12` gate on the
  optimized dev profile completed natural block breaks and drops, maximum-count
  crafting, crafting-table and chest placement/opening, natural pig combat and
  pickup, and a container transfer in 22 seconds with nine natural entities and
  active chunk streaming. The server emitted no tick-budget, packet-dispatch,
  reliable-command, or disconnect warning. This is representative ordinary-play
  smoke evidence, not a per-action latency SLO or broad overload soak. Evidence is
  `.analysis/real-client-runs/responsiveness-o3/20260723T103459Z-real-client-playable-loop-4vVxYV`.
- Scheduled-block planning uses autoscaler CPU admission and the blocking pool.
  The phase services pushed simulation commands while the job runs, then joins
  it before fluid or later phases. One shared admission fence covers every
  entry point. The deterministic 256-button regression proves owner command
  responsiveness under CPU pressure, duplicate rejection, and complete commit.
  Its optimized run took `1,666 us`; evidence is
  `.analysis/codex-logs/scheduled-background-owner-256-o3-final.log`.
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
  API. The then-separate currency catalog read currency, zone, and products
  from that file and validated its exact schema at load. A production TCP/Lua gate
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
- The exact release entity-scale gate now covers 40,000 active hostile entities,
  60 complete headless TCP clients, 16 regions, 200 warm-up ticks, and 1,200
  measured ticks. Tick p50/p95/p99/max was
  `37.289/41.932/43.886/52.863 ms`; goal and hostile-attack p99 were
  `19.838/9.766 ms`. All clients stayed connected with zero reliable drops,
  write timeouts, or pressure sheds, and the derived cohort rotated the full
  population within 28 ticks. The separate 1,500-vs-1,500 regional battle
  completed at `32.610 ms` tick p99 with no missing follow targets. This is
  focused release profiling, not a broad deployment or long-soak claim.

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
- Keep a checkpoint within 8 soft and 12 hard model roundtrips, six shell
  batches, one stateless subagent, one L2 run, and zero compactions. Continue
  the next checkpoint in a fresh session from a compact cursor.
- Never use a full-history subagent fork. Give the reviewer or worker only its
  bounded task, base commit, owned paths, acceptance checks, and relevant
  evidence, then close it after its single result.
- Batch independent calls. Treat repeated one-tool model rounds, broad
  truncated discovery, progress polling, and L2 before a commit candidate as
  workflow failures rather than normal execution.
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
- Production worldgen revision 9 consumes explicit `ChunkGeometry` for terrain,
  ores, structures, and biomes. Separate landform and cave stages provide
  domain-warped continents, branching mountain ranges, substantially carved
  river-valley contours, and bounded tunnel caves behind a 32-block surface shell.
  Trees require exact planned support and a stable 5x5 footprint.
  Solaris worlds persist revision/seed/mode/ore-profile/settlement-profile/geometry; unversioned
  Anvil worlds open without fallback generation and reject plugin worldgen
  profiles. The playable profile uses `.analysis/test-world-v9`. Revision 8
  adds visible anisotropic mountain detail, stronger long relief, elevation-
  aware mountain surfaces, and an explicit dry-spawn floor; the fresh-client
  forest/coast/ocean/high-relief route is complete. Revision 9 removes filled
  3x3 upper leaf boxes in favor of connected irregular oak/jungle crowns.
- The optional startup-only `plains_village_prototype` Lua manifest profile
  combines an extracted vanilla plains fountain, small house, and toolsmith at
  stable offsets and consumes extracted village placement facts. Seed zero
  fixes it near spawn; other seeds use deterministic grassland placement. The
  selection shares the persisted plugin worldgen profile fence and requires the
  vanilla sidecar. The bounded plan selects buildings, roles, named inhabitants,
  jobs, and owner-scoped extensions. Extracted vanilla villager jigsaw slots
  become persisted chunk markers; a dedicated simulation-owner command
  materializes each villager once with durable profession metadata and a claim
  separate from ambient herd spawning.
- Lua API 0.6 has bounded DTO/files/batches, optional bounded startup-only TOML
  configuration with fresh Lua copies, push-driven bounded simulation timers,
  one-shot host admission, an attested `mc-net` router, and durable plugin
  storage. Production adapters now cover menus, inventory/storage transactions,
  zones, same-dimension player teleports, opaque ephemeral villager bindings,
  bounded `idle`/`follow_position` goals through journaled regional ownership,
  and required post-commit `player.block_broken` and `player.block_placed`
  events. Colony records, homes, roles, order vocabulary, limits, and durable
  member intent are Luau/plugin-storage state rather than Rust runtime concepts.
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
  integer block coordinates. The consolidated item-currency economy now has a
  production wire gate for zone activation, buy, insufficient-funds rejection,
  unchanged ledger, and refund. The exact shipped colony scaffold now reads its
  complete bounded domain configuration, persists colony metadata plus member
  role/order intent in plugin storage, maps `home` to a configured movement goal
  and `hold` to idle, and has a production wire gate for durable recruit, later
  order application, and removed-villager recovery. It retains the opaque token
  only in Luau memory, retries one typed rejected/stale binding, and reports an
  applied order only after the targeted regional-owner result. The Lua API also
  has a
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

1. Complete worldgen revision 10 beyond the now-landed spawn, vegetation, and
   drainage work: rendered height/biome/vegetation mosaics, seed-`712816` owner
   playtest, restart, and release-host throughput evidence remain.
2. Run the no-operator 20-minute survival and restart gates for the landed
   periodic friendly/hostile spawn runtime; do not infer client visibility or
   retained identity from unit scheduling coverage.
3. Run the real Fabric/NeoForge/Forge Loader matrix for the landed deployment
   reporting and disconnect contract. Headless packet and harness gates do not
   replace graphical permission, artifact, screen, asset, and reconnect evidence.
4. Close `v0.0.2-alpha.1` only after the exact gates in
   `docs/PUBLIC_ALPHA_PLAN.md`; do not substitute warning suppression, unit-only
   seed checks or a merely uncommented `tellus_like` setting.

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
