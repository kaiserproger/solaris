# Active Playable Task

Current target: post-P4 playable hardening. P4 real-client 20-minute loop gate
now has agent-run real-client evidence; remaining work is to broaden the
playable spike beyond the first wood -> tool -> restart loop and keep the gate
stable.

Story:

join -> see world -> move -> break dirt/log -> see drop -> pick up -> place
block -> relog -> still there

Current evidence:

- Online mode now performs the exact 26.1.2 RSA-1024 challenge, continuous
  AES-128-CFB8 transport, signed SHA-1 server hash, Mojang `hasJoined`
  verification, and only then encrypted compression/login success. The
  transport decrypts ciphertext already buffered behind Encryption Response
  and preserves fail-only socket write timeouts. Verified UUID and signed
  profile properties reach LoginSuccess and observer PlayerInfoUpdate. Local
  server/authlib bytecode supplies the oracle; packet codecs pass `16/16`,
  player-info property codecs `5/5`, auth foundation `10/10`, encrypted
  transport `3/3`, and the complete raw-TCP BoundServer login with a fake
  verifier passes `1/1` inside the full `mc-server` login suite `10/10`.
  Scoped all-target strict Clippy, fmt, no-sleep, diff-check, and code-health
  `0 fail / KEEP` pass. No real Mojang endpoint, paid-account real-client,
  auth load/concurrency, or proxy-IP configuration gate was run; those remain
  required before claiming public deployment readiness.
- The seed-zero `VanillaLike` playable profile now places one Solaris-owned
  cobblestone ruin in chunk `(4, 0)`, about 4.5 chunks east of spawn. Its typed
  chest contains one diamond, four lapis lazuli, and two bread; item protocol
  IDs come from the startup registry rather than constants or Mojang bytes.
  The ordinary chunk persistence path keeps an emptied chest empty after
  flush and reopen. Startup does not install this fixed structure for other
  seeds or Tellus mode, and short world geometries cannot create an orphaned
  chest block entity above the ceiling. The deterministic harness, disk
  persistence, startup-selection, and short-geometry focused gates pass. No
  fresh TCP chest interaction or real-client exploration gate was run, so the
  structure is playable content evidence rather than vanilla structure parity.
- Lua API `0.4.0` adds operator-only player command roots while preserving
  `0.3.0` ordinary command plugins. Non-operators neither receive nor get
  suggestions for restricted roots; a forged command is denied before the
  bounded Lua queue. Operators route to the owning plugin, and the raw-TCP
  fixture proves a disk-loaded `adminday` plugin can invoke its declared
  `time` capability and publish the result. Queue saturation now reports a
  claimed-but-dropped command instead of a false successful enqueue. The
  focused raw-TCP gate, script/runtime units, command tests, and independent
  ACL review pass; hot reload and direct world/entity/inventory plugin APIs
  remain open.
- Regional entity goal ticks no longer clone every entity from every owner
  lane twice. Prepare reads active entities plus exact referenced targets;
  apply validates only that union while retaining lease, migration, stale-CAS,
  and all-or-nothing rejection. Missing referenced IDs are also fenced against
  same-ID restoration before apply. Push-driven regressions prove active and
  target lanes are contacted, an unrelated third lane receives zero snapshot
  requests, and missing-target ID reuse rejects the batch. The focused owner
  tests and independent re-review pass. This removes an O(all entities)
  coordinator fan-in from the common goal path; a fresh dense benchmark and
  server load gate were not run.
- Survival block loot now preserves and executes the supported vanilla
  Silk Touch and Fortune context from local 26.1.2 loot tables. The typed
  `mc-data` model keeps the Silk Touch alternative plus regular branch and
  supports the vanilla `ore_drops` and `uniform_bonus_count` formulas.
  Survival break passes the full held stack, retains the existing correct-tool
  gate and pool order, and derives count and bonus rolls from the stable
  position/state/mutation-token seed. Focused runtime tests prove diamond ore
  self-drop with Silk Touch, a Fortune III count change, and exact level-zero
  behavior; `mc-data --lib` passes `127` tests. The deterministic playable
  enchanting path keeps its Efficiency I/II/III clues and now adds Fortune II
  to the second mining-tool offer and Silk Touch I to the third, so both loot
  branches are naturally obtainable without replacing the mining-speed
  progression. Three focused enchanting regressions pass. Scoped strict
  Clippy, fmt, no-sleep, and code-health pass. The typed loader also accepts
  the vanilla `binomial_with_bonus_count { extra, probability }` Fortune
  formula and samples its `level + extra` trials deterministically from the
  break seed; the level-zero `extra` trials match the inspected 26.1.2 oracle
  instead of incorrectly returning the raw count. Two parser/oracle tests and
  one runtime seed regression pass. Ordered `block_state_property` pools now
  select mature and fallback alternatives from the actual block state before
  the legacy plant fallback. The real local 26.1.2 wheat, carrot, potato, and
  beetroot tables all produce their expected young/mature item sets; a runtime
  wheat regression proves mature Fortune uses the sidecar binomial count.
  Entry-level `random_chance` is also typed and applied from the same stable
  break seed, so the mature potato table can emit its optional poisonous
  potato without bypassing the sidecar table. All 23 focused loot tests and 27
  focused runtime crop tests pass. Explosion context, exact vanilla RNG-stream
  parity, randomized enchanting parity, and a fresh real-client ore/crop gate
  remain open.
- A successful waiting sleep now atomically marks both halves of the canonical
  bed `occupied=true` through the conditional world-edit path before publishing
  the sleeping pose. Explicit wake atomically restores both halves to
  `occupied=false`; disconnect performs the same clear before session teardown.
  The raw-TCP regression proves both occupied states, rejects a second player
  using the other half, receives the exact wake position packet, and then
  proves both original unoccupied states. A second push-driven raw-TCP
  regression drops the sleeping client, waits for its exact entity and profile
  removal packets, proves both halves were cleared, and then proves the exact
  remaining player can sleep in the same bed without an occupied rejection.
  The focused sleep wire set is `5/5`. No fresh real-client bed gate was run.
- Living-entity collision now carries a typed top height instead of reducing
  every solid state to a full cube. All farmland states use the vanilla common
  `15/16` height, so livestock walks at `Y + 0.9375` without a false horizontal
  collision or full-block jump. A separate regression clears the blocked
  horizontal velocity after a real collision while preserving tangent motion
  and the jump arc, removing the repeated push-back that presented as circling
  or shaking in place. Moving from farmland onto a neighbouring full block now
  computes the exact `1/16` rise instead of moving the entity by the full `0.6`
  step allowance or starting a jump; the regression also pins the unchanged
  tangent drag/friction. Full `mc-physics --lib` is `20/20`; the focused sampled
  farmland runtime regression and strict scoped all-target Clippy pass. The
  contract intentionally covers rectangular full-footprint heights only;
  stairs, fences, and other irregular voxel shapes retain the conservative
  full-cube fallback. The combined post-slice short baseline is `mc-data`
  `131/131` and `mc-net --lib` `960 passed/1 explicit ignored`. No fresh visual
  client gate was run.
- The embedded client MCP now exposes `minecraft_wait_for_block_state`. It
  checks a loaded block immediately, then rechecks its exact ID and optional
  properties only after the existing `wait_state_change` producer reports an
  applied client event; timeout only fails a stuck wait. Protocol tests prove
  immediate completion for an already-matching state and blocking until an
  exact wheat age event. `minecraft_respawn` now exposes the existing vanilla
  respawn action and waits for confirmed active player state through the same
  client event producer. Executable facade tests prove exact timeout forwarding
  and fail closed when respawn is not confirmed. Parent-workspace
  `:bridge-core:test :java-agent:test --rerun-tasks` passes. No Minecraft client
  was launched for this transport slice.
- Standalone same-region kinematics CAS now uses the cached regional owner
  lane instead of the coordinator actor. The direct path revalidates the
  entity UUID and lease under mutation admission, preserves sequenced WAL,
  rollback, and finalize behavior, and is disabled for region crossings and
  vehicle-linked entities. Vehicle relationship changes invalidate cached
  standalone routes. A push-gated regression completes while the coordinator
  is deliberately held; concurrent identical snapshots apply exactly one
  update, safe journal failure rolls back, and checkpoint-only motion emits no
  durable decision. `mc-entity --lib` passes `165` tests with seven explicit
  benchmarks ignored; scoped strict Clippy, fmt, and no-sleep checks pass.
  `mc-net`, workspace, client, soak, and load gates were not run.
- `Chunk` now carries an explicit section-aligned `ChunkGeometry`; the existing
  constructor keeps the `-64..320` Overworld default, while
  `empty_with_geometry` sizes block, biome, and light-section storage and uses
  the geometry for block bounds, mutation tokens, scheduled ticks, and
  heightmap offsets. A `0..256` regression covers section count, boundary
  reads/writes, heightmaps, containment, and tokens; all 23 focused chunk tests
  and scoped strict Clippy pass. An existing real-world extras test also now
  ignores Solaris-owned `SolarisJournalLsn` before deciding whether its fixture
  is a vanilla oracle. In the combined short gate, the other 188 `mc-world`
  tests passed and the corrected extras test passed separately; the whole
  package was not rerun after that correction. Anvil decoding now derives the
  same geometry from `yPos` plus a complete contiguous section list, rejects
  empty/duplicate/gapped shapes, and preserves custom `yPos` on encode. Wire
  chunk data and baked-light masks use the actual section count; a `0..256`
  regression proves 16 data sections and 18 light slots. The five combined
  geometry tests, 22 focused Anvil tests including 115 vanilla chunks, 19 wire
  tests, and scoped strict Clippy pass. Full lighting computation and
  incremental block/sky relight now use the same geometry for Y coordinates,
  BFS bounds, extraction, and mixed-neighbourhood rejection. A `0..256`
  full-light regression plus ten incremental regressions pass. Server config
  propagates `min_y`/`height` through worldgen; Configuration patches the typed
  overworld dimension NBT (`min_y`, `height`, and `logical_height`) even when
  the client accepts the known pack, preserves vanilla payload bytes, and
  fails closed when the captured payload is missing. Startup also rejects a
  resident chunk whose geometry conflicts with config; two focused startup
  tests pass. Scoped all-target Clippy, fmt, no-sleep, diff-check, and
  code-health `0 fail / KEEP` pass. Solaris now writes a versioned
  `solaris/world-geometry.json` before opening the world. Restarts reject a
  different configured geometry even when no incompatible chunk is resident;
  legacy Overworld data receives the contract without decoding every chunk,
  while a legacy custom-height world with existing `.mca` data fails closed
  instead of guessing. Three focused contract tests pass. Sparse-section
  caller geometry remains open. Baked-light
  reconstruction now takes the owning `Chunk`, preserves its geometry
  `min_y`, and is used by the production resident-edit, chunk-stream,
  relight-cache, and startup checks; seven focused baked-light regressions
  include a `0..256` world-Y assertion. The legacy section-only constructor is
  retained for Overworld-compatible tests. No custom-map real-client gate was
  run.
- Adding food to a campfire no longer waits for the global `WorldStorage`
  mutex. The simulation owner prepares the player and campfire transaction
  under short registry locks, then commits the opaque block entity, cooking
  state, and held-item debit through the owning regional lane. The RED
  regression held the global writer and observed no owner completion; GREEN
  receives the exact campfire response before releasing that writer and proves
  the persisted bytes, runtime cooking state, and inventory debit. The three
  focused campfire-use tests, two queued parity/stale tests, and strict
  `mc-net --lib` Clippy pass. Full `mc-net`, workspace, client, VD8, soak, and
  performance gates were not run for this short slice.
- Passive livestock no longer climbs a full block by teleporting its feet
  directly to `Y + 1`. Local vanilla 26.1.2 bytecode shows default living
  `step_height=0.6`, `jump_strength=0.4199999869` blocks/tick, and
  `MoveControl -> JumpControl` for a full-block obstacle. Solaris now keeps the
  0.6 direct step, converts the vanilla jump strength to its blocks/second
  physics units, starts an airborne arc only when a one-block clearance probe
  succeeds, and stops upward motion at a solid ceiling. An upward climb
  collision no longer activates the wall-detour path, while a two-block wall
  still does. The complete 18-test `mc-physics` suite covers cow, sheep, and
  chicken AABBs reaching and landing on the upper block; focused server-step,
  pathing, and wire-velocity tests also pass. Scoped strict Clippy, fmt,
  no-sleep search, diff-check, and code-health `0 fail / KEEP` pass. A fresh
  visual client gate, full `mc-net`, workspace, VD8, and soak were not run for
  this short slice.
- Regional survival break, placement, and bucket commits no longer reacquire
  the global `WorldStorage` mutex to schedule their derived fluid ticks. The
  regional owner plans ticks from the published read view and inserts them
  through `WorldMutationView`, which updates the resident chunk and scheduled
  tick hint under the owning region lock. The break regression first timed out
  while the global writer was held after adding a real water neighbour; it now
  completes and proves the replacement water plus its fluid tick and relight
  were published. Placement and bucket held-writer paths also complete and
  prove their fluid ticks are present. The three focused tests, scoped strict
  `mc-net` Clippy, fmt, no-sleep search, and code-health `0 fail / KEEP` pass.
  Full `mc-net`, workspace, client, VD8, and soak gates were not run for this
  short slice.
- `wire-probe --hold-open` now provides a lightweight event-driven Play client
  for runtime gates. It completes Handshake/Login/Configuration/Play, confirms
  the initial teleport, sends `PlayerLoaded`, answers keepalives, and blocks on
  socket input or SIGINT without sleeps or duration-based success. Hold mode
  counts frames instead of dumping an unbounded movement stream. Against the
  persisted playable world, one active wire client triggered two exact
  periodic checkpoints: `195 entities/6 chunks` with a 1.335 s off-tick flush
  and `199 entities/2 chunks` with a 0.926 s off-tick flush. No runtime
  `>50 ms` tick warning was emitted across either checkpoint, providing direct
  evidence that the recent journal/save/session-lock fixes keep disk checkpoint
  work out of the tick path. The wire-probe CLI test, focused strict Clippy,
  fmt, no-sleep search, diff-check, and code-health `0 fail / KEEP` pass. This
  was a bounded protocol-client runtime check, not a real-client gameplay gate.
- Entity save snapshots no longer hold the global session registry while
  waiting for the regional owner `SaveBarrier`. The simulation owner first
  copies lifecycle, spawn-age, and pickup-delay metadata under a short session
  lock, releases it, and then captures the authoritative regional snapshot.
  This remains inside the single-writer simulation save command, so gameplay
  mutations cannot interleave between the two steps. A push-gated RED observed
  the session mutex held at the owner-barrier boundary; GREEN observes it
  available. Full `mc-net --lib` is `946 passed/1 ignored` in 8.16 seconds;
  scoped strict Clippy, fmt, no-sleep search, diff-check, and code-health `0
  fail / KEEP` pass. No additional client or long-running gate was run.
- Sheep-grazing start and finish no longer hold the global session registry
  while revalidating committed entities through the regional owner. The owner
  read now completes first; session publication then rechecks the grazing timer
  under the session mutex, preserving stale-publication rejection. A
  push-gated RED observed the session mutex held at the owner-read boundary;
  GREEN observes it available. Focused grazing tests are `8/8`; full `mc-net
  --lib` is `945 passed/1 ignored` in 8.14 seconds; scoped strict Clippy, fmt,
  no-sleep search, diff-check, and code-health `0 fail / KEEP` pass. A bounded
  idle runtime loaded 198 persisted entities and produced no post-startup
  `>50 ms` tick warning before clean shutdown, but it did not exercise periodic
  checkpoint because that trigger correctly requires an active session.
- Animal age/love countdown no longer performs a synchronous entity-journal
  commit every tick. The owner API now has an explicit checkpoint-only animal
  CAS used only when a breeding tick has no birth; birth ticks, player feeding,
  and grazing completion remain durable. RED observed one journal commit for a
  single baby-age decrement; GREEN observes zero while the authoritative state
  still advances. A second regression covers the cached same-lane regional
  path. Static tracing also confirmed that `animal_breeding_us` currently
  includes awaited sheep grazing, so it must not be interpreted as a pure
  breeding metric. Full `mc-entity` is `161 passed/7 ignored`; full `mc-net
  --lib` is `944 passed/1 ignored` in 8.11 seconds; scoped strict Clippy, fmt,
  no-sleep search, diff-check, and code-health `0 fail / KEEP` pass. No client
  or long-running world gate was spent on this slice.
- The dominant `entity_dispatch_us` session-lock scope no longer includes the
  regional kinematics commit. Physics now releases the session registry before
  applying the authoritative regional batch, then reacquires it and publishes
  only states that still exactly match the current owner snapshot. Push-gated
  regressions prove the session registry remains available while the owner
  apply is paused and that a delayed physics batch cannot overwrite or publish
  over a newer commit. Full `mc-net --lib` is `943 passed/1 ignored` in 8.06
  seconds; scoped strict Clippy, fmt, no-sleep search, diff-check, and
  code-health `0 fail / KEEP` pass. No client or long-running world gate was
  spent on this deterministic concurrency slice.
- Nighttime entity-goal stalls were traced to two durability barriers rather
  than pathfinding. Hostile `FollowPosition` refreshes changed whenever the
  player moved and synchronously journaled every target update; autonomous
  target refresh is now checkpoint-only while combat and external mutations
  remain immediately durable. After a successful entity save, journal phase
  cleanup also no longer runs inside the regional owner actor: the save task
  still waits for the exact journal rewrite completion, but checkpoint-only AI
  can continue through the actor while that I/O is in flight. RED regressions
  first observed one journal commit per hostile refresh and a goal command
  blocked behind journal cleanup; both are green. A constrained real-client
  diagnostic against the existing night world reached 54-59 entity queries per
  tick with ordinary `entity_goals_us` around 5-14 ms instead of the previous
  repeated 50-142 ms range. It also exposed the separate cleanup barrier before
  the second fix, including one 179 ms save-coincident goal tick; no second
  client run was spent after the deterministic concurrency regression closed
  that cause. Full `mc-entity` is `160 passed/7 ignored`; focused hostile tests
  are `20/20`, checkpoint-only tests are `3/3`, scoped strict Clippy, fmt,
  no-sleep search, and code-health `0 fail / KEEP` pass. Workspace tests and a
  complete 20-minute/restart client gate were not run for this slice.
- Regional chest and furnace commits: valid one- or two-block chest
  transactions and single-furnace client transactions whose complete footprint
  is resident in one 8x8 chunk region now run in the autoscaled regional
  mutation lanes. Narrow transaction handles validate the open viewer,
  container state id, player inventory/cursor, and resident block-entity state
  before atomically publishing world plus player state; drops, furnace XP, and
  viewer packets remain ordered on the owner after worker completion. Furnace
  commits compare only client-owned slots and recipe accounting, then merge
  them into the current resident furnace so a newer server burn/cook tick is
  preserved. Opaque block-entity updates used by sign editing also run in the
  regional lane: block state and mutation token are checked under the same
  region lock as the raw NBT COW write, so break-and-replace ABA is rejected.
  Held-writer regressions prove all three command families complete
  without the global `WorldStorage` mutex, stale chest/furnace CAS returns
  current authoritative state without partial mutation, and output-take awards
  XP once. `ContainerRegistry` metadata is now striped across 64 stable
  region-derived shards instead of one process-wide mutex. A two-region chest
  push gate blocks after each worker has acquired its actual metadata shard and
  proves both acquisitions complete before either is released; disconnect is
  the cold path that explicitly cleans every shard. Viewer-set pressure remains
  lock-free for observers and is maintained as atomic per-shard deltas, with a
  regression covering aggregation and removal across shards. Regional metrics
  count chest/furnace as container commits and opaque NBT as block-entity
  commits instead of misclassifying all three as block edits. Regional relight
  publication also no longer reacquires `WorldStorage`: one ordered
  multi-region CAS validates every immutable source snapshot and publishes all
  baked-light targets atomically under their resident region locks. A stale
  wave result writes no partial light, then recaptures and recomputes from the
  post-wave read view without sleeping or polling. A real survival-break test
  receives block, inventory, drop, and baked-light completion while the global
  writer is deliberately held. The deferred packet-owner relight path now uses
  the same CAS/recompute helper; its stale-source regression mutates the world
  during compute and still receives the published response while the global
  writer stays held. Full `mc-world` is 171/171 and full `mc-net` is 874 active
  passed with one explicit ignored filesystem latency benchmark;
  scoped strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep search, and
  diff-check pass. Hash collisions can still serialize a pair of regions inside
  one of the 64 metadata shards, while same-position operations intentionally
  serialize. Scheduled furnace ticks no longer use `WorldStorage`: an exact
  resident snapshot couples block state and furnace data, and an exact regional
  CAS publishes the tick or rejects it without overwriting a concurrent client
  change. On rejection the tick is recomputed from the new authoritative
  furnace before retry. Active and idle furnace regressions complete while the
  global writer is held; viewer data, later fuel clicks, and closed-window
  ticking remain intact. Campfire cooking ticks now couple the in-memory timer
  transition to a resident block-state/token plus opaque-NBT CAS. A stale,
  missing, broken, or replaced campfire leaves the timer untouched and cannot
  emit a completed-item drop; only an applied NBT commit advances/removes the
  slot. The cold-chunk test still avoids loading, and a resident tick completes
  while the global writer is held. Full `mc-world` is now 172/172; `mc-net` is
  875 active passed plus one ignored benchmark. The dominant hopper cooldown
  path is also resident-owned now: cooldown values above one are decremented
  with an exact block-state/hopper CAS that atomically schedules the next tick.
  Because hoppers wake every tick but transfer every eight ticks, seven of
  eight active-hopper wakeups avoid the second coordinator world lock. The
  `1 -> 0` tick deliberately remains on the full path because it can mutate a
  hopper plus chest/furnace/campfire endpoint. Same-region transfer ticks now
  plan against immutable resident chunk snapshots and conditionally commit the
  exact hopper plus chest/furnace endpoint, comparator wakeup, and next hopper
  tick under one region lock. A runtime-path counter proves hopper-to-chest
  uses this resident commit. Campfire transfers deliberately keep the existing
  coordinator/session transaction, and a hopper at block `x=127` feeding a
  chest at `x=128` proves the cross-region coordinator fallback still moves the
  item without a regional partial commit. All 14 hopper regressions pass. Full
  `mc-world` is now 174/174 and `mc-net` is 877 active passed plus one ignored
  benchmark; scoped strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep
  search, and diff-check pass. LRU/Anvil IO, journal, save, cross-region hopper
  transactions, and campfire-coupled transfers remain coordinator-owned. No
  workspace, client, VD8, or soak gate was run.
- Regional entity durability no longer rewrites the complete journal for every
  commit. Version 2 is an append-only stream of complete JSON decisions behind
  one bounded writer thread; callers wait on the exact write-and-`sync_all`
  completion channel, and one `record_commits` call is written and synced as a
  group. Startup accepts the old version-1 snapshot, migrates it atomically,
  replays complete version-2 records, and truncates only an incomplete final
  record. A finalized decision remains durable until a successful ordered
  `entities.dat` save checkpoints its exact phases. Recovered phase and
  sequence watermarks seed both the coordinator and every owner lane, so a
  restart cannot reuse phase 1 and accidentally delete the fresh restore
  decision while acknowledging the old one. Old recovery phases, including
  removal tombstones, now remain until that full save instead of being cleared
  during bind. First creation of `world_root/solaris` fsyncs `world_root` so the
  directory entry is durable. An append/fsync error is explicitly
  outcome-unknown: it is preserved through the file adapter, the applied owner
  phase is not rolled back, and the still-open phase rejects later mutation
  instead of risking a phantom replay. Journal round-trip, truncated-tail,
  group-commit, unknown-outcome fail-stop, tombstone restart, and phase-collision
  regressions pass. Full `mc-entity` is `136 passed/5 ignored`; full `mc-net` is
  `881 passed/1 ignored`; scoped strict Clippy, fmt, code-health
  `0 fail / KEEP`, no-sleep search, and diff-check pass. World block mutations
  still have no WAL, and workspace, client, VD8, and soak gates were not run.
- Client MCP: the embedded loopback-authenticated MCP server exposes 31 client
  tools, including event-driven inventory/item visibility waits, hotbar stack
  selection, selected-item drop, container quick-move/button actions, and
  `minecraft_run_scenario`. An agent-run gate against the updated debug core
  joined a fresh seed-0 world and passed
  `playable-02a-natural-log-to-planks` through MCP: approach a generated birch
  log, break it, observe and pick up the drop, then craft four birch planks.
  The post-world-owner-cutover rerun also passed the strict artifact validator:
  `.analysis/real-client-runs/20260712T021222Z-real-client-playable-loop`.
  Screenshots remain optional context rather than the control or assertion
  transport.
- Client MCP combat: `minecraft_wait_for_visible_entity` and
  `minecraft_wait_for_health_below` expose packet-driven combat assertions.
  A fresh embedded-client gate joined Solaris, summoned a skeleton through the
  normal command path, observed skeleton `1000049`, arrow `1000050`, and player
  health `20 -> 16`, while remaining in play. The first attempt exposed a real
  protocol bug: Solaris encoded boolean entity metadata with serializer `10`,
  which 26.1.2 reserves for `BlockPos`; the local Mojang jar confirms boolean
  serializer `8`, and the corrected packet now has a literal oracle-byte test.
- Client MCP: a direct two-client gate used no screenshots or elapsed-time
  success condition. `SolarisMcpA` selected and dropped one item through the
  real client, both clients observed entity `1000055`, `SolarisMcpB` received
  the item, the first inventory stayed debited, and both clients observed the
  entity removal. The gate passed first for birch log and again for diamond
  after the teleport correction below. A third emerald handoff passed after
  splitting state and tick notifications, including an explicit two-tick wait
  on each client before the packet-driven inventory/entity sequence.
- Simulation ownership: packet-authored owner commands are fenced by monotonic
  `SessionId`; disconnect/reconnect regressions prove stale item claims and
  conditional block edits are rejected before mutation. Prompt 03B authority is
  now partial: migrated transactions use the registered player aggregate, and
  non-empty crafting-table and enchanting inputs are projected there on every
  accepted click and persisted for restart recovery. Active use and the other
  transient container state remain staged.
- Simulation ownership: every accepted player pose now commits through the
  session-fenced `SimulationOwner`. The owner updates visibility and persisted
  coordinates in one turn and dispatches observer movement before replying;
  stale reconnect commands cannot mutate either copy. Movement, rotation,
  stance, input, respawn, admin teleport, and persistence all use this path.
  The atomic/stale owner regressions, all 570 `mc-net` library tests, the full
  workspace baseline, and strict Clippy pass. A fresh two-client P42 run passed
  opposite chunk crossings in
  `.analysis/real-client-runs/20260712T062405Z-real-client-playable-loop` with no
  server warning. A later clean-code repeat kept both clients in Play and had no
  slow server tick, but the secondary client moved only 4.52 blocks against
  terrain and did not cross its chunk; that repeat is not counted as a pass.
- Simulation ownership: window-0 and crafting-table clicks now commit the
  player inventory, carried cursor stack, and every resulting dropped item in
  one session-fenced owner turn. Atomic, duplicate, and stale-session owner tests,
  all 573 `mc-net` library tests, focused wire tests for normal/malformed clicks
  and both recipe paths, and the real-client natural log -> planks -> crafting
  table -> place/open scenario pass. The real-client artifact is
  `.analysis/real-client-runs/20260712T064355Z-real-client-playable-loop`.
  Closing either crafting grid returns its inputs through that same owner path.
  Disconnect settles the active 3x3 grid, inventory 2x2 grid, cursor, and all
  overflow drops in one command. An explicit owner rejection rebuilds the plan
  from the returned authoritative snapshot; owner loss fails closed and retains
  the last accepted owner snapshot. The non-empty 3x3 input now participates in
  the same owner compare-and-set as inventory, cursor, and drops. A regression
  drops the connection projection after two real click handlers and proves
  disconnect recovery from the owner copy; another proves stale clicks rebuild
  from that copy. The cursor, 3x3 input, and enchanting inputs now round-trip
  through Solaris-owned playerdata fields. Login returns recovered inputs and
  cursor to the normal inventory through one owner compare-and-set before
  sending initial inventory content; overflow uses the same atomic item-drop
  path. Connection cleanup, including task abort, moves the owner state into a
  generation-fenced pending map and pushes an immediate checkpoint request.
  The ordered periodic worker now saves players with entities and world
  metadata, removes only the successfully written pending generation, and lets
  an earlier reconnect recover directly from memory. Per-connection disk saves
  are gone. The playerdata round-trip, login recovery, cancellation checkpoint,
  disconnect, and stale-click regressions pass as part of all 821 `mc-net`
  tests and the fresh full workspace baseline. A versioned wire projection
  remains staged. No new real-client gate was run.
- Simulation ownership: per-loop persistence polling and inventory/XP revision
  reconciliation are gone. Selected hotbar slot, respawn pose, and game mode
  now commit as session-fenced owner events. `ServerboundPlaceRecipe` commits
  its inventory candidate before sending slot updates, and bonemeal commits
  all state/token-guarded world edits with the held-item debit in one existing
  owner transaction. Client hotbar selection also waits for an authoritative
  container state advance after a predicted `SWAP`; the selected target slot
  must contain the requested item before the action returns. P02b artifact
  `.analysis/real-client-runs/20260714T080044Z-real-client-playable-loop`
  exercises that exact main-inventory-to-hotbar path and then places/opens the
  table. Fresh P14
  `.analysis/real-client-runs/20260714T082057Z-real-client-playable-loop`
  passed three natural sheep/wool pickups, table recipe/place/open, bed
  recipe/place, natural night, and sleep-to-morning. The previous diagnostic
  P14 `.analysis/real-client-runs/20260714T081159Z-real-client-playable-loop`
  had failed table placement despite a persisted table; replaying its exact
  saved player, item, and target through MCP successfully placed the table at
  `(25,76,7)`. The old red status is no longer current, but its one-off cause is
  not claimed. Gradle `check`, all 735 `mc-net` tests, full workspace
  test/clippy, fmt, code-health, and diff-check pass.
- Multiplayer sleep: local 26.1.2 bytecode pins the quorum formula to
  `max(1, ceil(active * percentage / 100))`, excludes spectators from `active`,
  and requires 100 simulation ticks before a sleeper is deep enough to skip.
  Solaris now follows those rules. The narrow
  `/gamerule players_sleeping_percentage [value]` command supports query/set,
  defaults to 100, and persists in world metadata. Tick 100, disconnect,
  gamemode changes, gamerule changes, natural dawn, external time changes, and
  `StopSleeping` all recompute from their producer event and push exact
  `Sleeping`/`Standing` metadata plus next-morning time where required. Four
  coordinator regressions, legacy/default and save/rebind persistence, command
  feedback over TCP, the two-client raw-TCP sleep scenario, and all 809
  `mc-net` tests pass, as do the full workspace test, strict Clippy, fmt, and
  code-health gates. Nearby-monster rejection now follows the local 26.1.2
  `ServerPlayer.startSleepInBed` bytecode: non-creative players cannot sleep
  while a live supported hostile bounding box intersects the bed-centered
  `+/-8` X/Z, `+/-5` Y cuboid. Respawn is committed before this check, matching
  vanilla ordering; rejection sends an overlay and never publishes a sleeping
  pose. The cuboid/bounding-box unit regressions, survival-versus-creative
  regression, all 903 active `mc-net` tests, and all 79 block-edit wire tests
  pass on a two-CPU affinity. Strict `mc-net` Clippy, fmt, and code-health pass.
  Normal single-bed wake-up now keeps `{started_tick, bed}` in the sleeping
  state. Quorum completion, dawn, and manual `StopSleeping` push the exact bed
  to the sleeping connection. That connection checks the vanilla-ordered ten
  surrounding and two above-bed candidates against the published world,
  commits the selected player pose through the simulation owner, and sends one
  confirmable position sync; the all-blocked fallback is the block above the
  bed at `Y + 0.1`. The blocked-first-candidate unit regression and two-client
  raw-TCP quorum gate prove the wake teleport. A connected sleeper now also
  reserves the bed's canonical head position in `SessionRegistry`; using
  either the foot or head resolves to that same key, so a second player using
  the other half receives the occupied overlay and never publishes a sleeping
  pose. The opposite-half rejection gate uses a real north-facing two-block
  bed, and the quorum gate using two distinct beds also passes over raw TCP.
  On two-CPU affinity, all 908 active `mc-net`
  tests plus one explicit ignored benchmark and all 80 block-edit tests pass.
  Strict `mc-net` Clippy, fmt, code-health, no-sleep search, and diff-check
  pass. The stale `load_scenarios.rs` reference to the removed global
  `LockMetricsSnapshot::entity_store` has also been deleted; the focused
  harness compile/test and full workspace all-target Clippy now pass.
  Sleep obstruction now follows the inspected vanilla 26.1.2
  `ServerPlayer.bedBlocked`/`Player.freeAt` path: a suffocating block directly
  above either bed half rejects the interaction before the respawn-point owner
  commit, sends the vanilla overlay, and never publishes a sleeping pose.
  `block_light.json` now carries the exact per-state `isSuffocating` result;
  old three-field sidecars fail with a regeneration instruction instead of
  guessing from opacity. The state-oracle tests include stone, slab, soul sand,
  and the opacity-zero suffocating barrier case. The two-half unit regression,
  exact raw-TCP obstruction gate, all 124 `mc-data` tests, and all 909 active
  `mc-net` tests plus one ignored benchmark pass on two-CPU affinity. Scoped
  strict Clippy, fmt, code-health, no-sleep search, and diff-check pass. The
  persisted bed-block `occupied` property, villagers, bunk-bed and full
  `DismountHelper` surface rules, dimension semantics, and a fresh real-client
  multiplayer sleep gate remain open; the full workspace and complete
  block-edit suite were not rerun for this slice.
- Validation gate repair: the O2/VD8 load report no longer invents a lock
  metric for the removed global entity-store mutex. Entity coverage remains in
  the report through server-entity counts and tick latency, while regional
  owner behavior has its own focused tests. `load_scenarios` compiles and its
  one non-ignored percentile test passes; all 13 sidecar/load scenarios remain
  explicitly ignored. The complete `cargo test --workspace` baseline and full
  workspace all-target Clippy, fmt, code-health, the no-sleep API search, and
  diff-check pass under a two-CPU, lowered-priority run. This restores
  compile/test/lint coverage, not 20-client VD8, client, throughput, or soak
  evidence.
- Simulation ownership: item-entity claim plus inventory credit is now one
  owner transaction. Requester-loss, partial-capacity, full-inventory, stale
  session, 66-test wire, checked replay, short 4+1 soak, and fresh MCP
  break/drop/pickup/craft gates pass. Other inventory actions, XP, arrows, and
  active use remain staged.
- Simulation ownership: XP-orb removal plus XP credit is also one owner
  transaction. Requester-loss, stale-session, two-session contention,
  lethal-reward, real TCP `SetExperience`, replay/soak, and MCP join gates pass.
  Survival-command XP changes and death reset are now covered by the player
  survival transaction below.
- Simulation ownership: grounded-arrow removal plus inventory credit now uses
  the same owner transaction. Requester-loss, capacity, stale-session,
  concurrent-winner, connection slot-packet, replay/soak, and MCP join gates
  pass. Non-pickup arrow inventory changes remain staged outside the atomic bow
  release path below.
- Simulation ownership: timed survival break now commits its complete block CAS
  set, held-tool durability, and deterministic item-drop spawns in one owner
  turn. Nine owner failure/conservation tests, all 66 block-edit tests, the
  sugar-cane multi-edit cascade, checked replay, short 4+1 soak, and a fresh MCP
  natural birch break/drop/pickup/craft gate pass. Lighting, fluid/falling-block
  follow-up, campfire cleanup, and save barriers remain staged.
- Block destroy effects: confirmed breaks now send vanilla
  `ClientboundLevelEventPacket` event `2001` with the destroyed root block's
  old state id to other players that have its chunk loaded. The breaker is
  excluded because its client predicts the effect. The `0x2E` codec and field
  order match the local 26.1.2 source, and the two-client raw-TCP creative-break
  gate receives the exact position, event id, non-air state data, and
  `global=false`. All 78 `block_edit` and 787 `mc-net` tests pass. A real-client
  audiovisual check, a dedicated survival observer wire gate, and other
  gameplay sounds and particles remain open.
- World read contention: rejected block-edit and `UseItemOn` resyncs now read
  the producer-published immutable `WorldReadView`. They no longer wait for the
  shared `WorldStorage` writer, and a malformed/rejected action cannot load an
  absent chunk through that mutex. Two unit regressions hold the world writer
  and require each resync future to complete on its first poll; all five exact
  packet-order rejection scenarios pass. Mutations and tick work still use the
  shared world writer, so this closes only the failure-resync boundary.
- Scheduled hopper lock order: chest and furnace slot dispatches are now built
  after the scheduled-tick pass releases `WorldStorage`; the old path acquired
  container state while still holding the world writer. An event-driven probe
  reproduces the exact dispatch boundary and proves the writer is available.
  Hopper-to-campfire insertion now builds its candidate under the cooking-state
  lock, persists it to the world, and only then publishes runtime cooking. A
  failed persistence returns no transfer, so the hopper does not debit its
  source slot. The exact RED/GREEN failure regression, all 18 campfire tests,
  all 776 `mc-net` tests, and the full workspace test/Clippy, fmt, and
  code-health gates pass. Scheduled mutations still use the common world
  writer, and the final hopper block-entity write plus target persistence are
  not yet one general multi-block storage transaction.
- Outbound write stalls: the common packet writer now starts a fail-only
  timeout only after the socket write actually returns `Pending`. This covers
  direct handshake, login, configuration, and play responses instead of only
  queued play commands. The play loop turns a direct-response stall into one
  clean disconnect and one slow-client timeout observation. The low-level
  stalled writer, direct command-suggestion response, queued command, and chunk
  stream regressions pass; all 773 `mc-net` tests and the full workspace
  test/clippy, fmt, and code-health gates pass. A real TCP write stall,
  reconnect storm, and long soak were not run.
- Owner relight batches: a deterministic 16-edit same-chunk regression exposed
  16 identical 3x3 neighbourhood captures and 16 full light encodes for one
  final output chunk. The owner now caches that neighbourhood and defers the
  `ChunkLight` clone and wire encode until all edits finish, reducing both
  counts to one while matching full recompute and the exact wire oracle.
  Random ticks, scheduled block/fluid ticks, and falling-block landing now
  capture immutable source chunks under the world writer, release it for the
  incremental BFS and wire-light encode, then publish only if every source
  `Arc` is still current. A concurrent mutation forces a full current-world
  fallback instead of publishing stale light, and prepared-cache invalidation
  follows the actual updated light chunks. The exact event-driven lock/CAS
  regression and incremental wire oracle pass. Packet-authored break,
  placement, and bucket commands now also capture immutable light sources in
  their owner commit, release the world writer for incremental BFS and wire
  encoding, then briefly reacquire it for source-version validation and baked
  light publication. The owner waits for that exact completion before replying,
  so the actor still receives block deltas before light and peers keep the same
  ordered dispatch. A concurrent source mutation forces the existing full
  current-world fallback instead of publishing stale light. The event-driven
  writer-release/stale-source regression, requester-loss peer packet-order
  regression, all 822 `mc-net` tests, and the fresh full workspace
  test/Clippy/fmt/code-health baseline pass. A fresh ignored 20-client VD8 debug
  gate delivered all 289 chunks to every client with total tick p99/max
  `22.981/30.907 ms`, entity-physics p99/max `4.168/5.729 ms`, world-writer max
  wait/hold `11 us/2.040 ms`, successful `1.896 s` save, and zero dirty chunks
  after shutdown. That workload proves common-path non-regression, not a
  relight edit storm; the rare stale full-recompute fallback still runs under
  the writer. No new real-client or vanilla-oracle gate was run.
- Simulation ownership: survival placement now commits the complete block CAS
  set and selected-stack debit in one owner turn. Nine owner transaction tests,
  all 511 `mc-net` library tests, all 66 block-edit tests, same-target
  contention, checked replay, short 4+1 soak, and a fresh embedded-MCP
  earned-crafting-table place/open gate pass. Every cactus/door cascade edit
  now carries the state/token precondition captured by its placement plan.
  A newly opened sign editor also retains the exact resulting state/token and
  front side from that accepted placement. A break-and-replace ABA update from
  the old editor is rejected instead of editing the replacement sign; normal
  text and flush/reopen wire tests still pass. Lighting, hopper scheduling, and
  save barriers remain staged; no real-client sign gate was run for this slice.
- Simulation ownership: completed food use now commits the exact selected
  stack debit and hunger/saturation update in one session-fenced owner turn.
  Tick-duration, stale stack/state/session, requester-loss, cancellation, TCP,
  replay, short 4+1 soak, and full baseline gates pass. Use start/cancel remains
  connection-local.
- Animal breeding: cows and sheep use their vanilla food tags, and chickens now
  use the complete vanilla 26.1.2 `minecraft:chicken_food` tag instead of being
  rejected by the wheat-only interaction path. Feed target, exact held stack,
  debit, love state, and event dispatch commit in one owner turn. Chicken age
  and love state also survive the existing entity storage round-trip. Both cow
  and chicken focused exact-TCP breeding scenarios pass; the focused
  owner/persistence tests and strict package Clippy pass. Sheep children now
  use all nine two-dye mixes derived from the local vanilla 26.1.2 recipe
  sidecar. Pairs without a recipe select either parent color through a stable,
  order-independent 50/50 branch. The result replaces the generic white child
  state in the authoritative ECS and its spawn metadata. Exact recipe-oracle,
  parent-fallback, and owner/ECS tests pass with all 797 `mc-net` tests. No new
  real-client gate was run because the MCP surface has no generic entity-use
  action; the existing TCP gates exercise the same `ServerboundInteract`
  packet path, but colored sheep breeding has no fresh raw-TCP or client gate.
- Sheep shearing: color and sheared state now live in the authoritative ECS,
  round-trip through entity storage, and emit the vanilla 26.1.2 packed byte at
  metadata index 18. One session-fenced owner command validates reach and the
  exact held stack, damages or breaks the shears, marks the sheep, and spawns
  1-3 one-item drops of the matching wool color. Repeated shearing is a no-op;
  melee and arrow deaths keep meat but do not recreate wool from an already
  sheared sheep. Embedded item facts and a shaped iron recipe make shears
  available without a sidecar. ECS, persistence, codec, owner, loot, raw TCP,
  full workspace test/clippy, fmt, code-health, and diff-check gates pass. No
  new real-client gate was run. Sheep grazing now follows the inspected vanilla
  26.1.2 40-tick animation and eats at animation tick 4: event 10 stops wander,
  grass blocks become dirt, edible sheep plants become air, sheared wool grows
  back through authoritative ECS metadata, and babies gain 1,200 age ticks. The
  owner path reads the published world snapshot and only takes the writer for a
  verified edit; its writer-held regression and all 797 `mc-net` tests pass.
  Natural sheep spawns now select temperate, warm, or cold colors from the
  resolved vanilla 26.1.2 biome tags and exact `5/5/5/3/82` plus `499/1` pink
  weights. Solaris uses a stable chunk/slot hash instead of claiming vanilla's
  `RandomSource` sequence, then carries the selected color through the herd
  plan into authoritative ECS state and wire metadata. Embedded climate sets
  match every resolved local sidecar tag; `mc-data 119/119`, focused ECS/wire
  tests, and the existing raw-TCP shearing test pass. Dyeing, exact vanilla RNG
  sequences, grazing sound/particles, raw-TCP grazing, and fresh natural-color
  and colored-breeding real-client gates remain open.
- Simulation ownership: bow release now commits one arrow debit, bow durability
  change or break, and projectile spawn in one session-fenced owner turn. Five
  owner failure/conservation tests, the exact TCP debit/damage/ack/motion/
  despawn gate, all 521 `mc-net` library tests, all 66 block-edit tests, checked
  replay, short 4+1 soak, and the full baseline pass. The production split
  spawn/local-debit/local-durability path is gone. Draw start/cancel remains
  connection-local; no dedicated embedded-MCP bow scenario was run.
- Simulation ownership: selected-item drop now commits selected-slot debit,
  item-entity creation, pickup delay, and owner pickup block in one
  session-fenced owner turn. Six owner tests, exact TCP, checked replay, short
  4+1 soak, and the direct two-real-client MCP handoff pass. Window-0/crafting
  throws and disconnect settlement now use the shared inventory owner command.
- Simulation ownership: health/hunger transitions, survival-command XP changes,
  armor/shield durability coupled to incoming damage, death inventory/cursor
  drops, XP-orb creation/reset, and survival respawn now commit through one
  session-fenced player transaction. A duplicate lethal command creates one
  exact set of item/XP entities, and fall, campfire, projectile, hostile,
  starvation, admin, exhaustion, and respawn runtime paths mirror only the
  committed snapshot. The transient cursor is part of the registered aggregate.
  All 529 `mc-net` library tests pass. Active-use start/cancel remains staged;
  no new real-client gate was run for this slice.
- Simulation ownership: chest and furnace clicks now commit the expected block
  entity/viewer version, registered player inventory, cursor, and an optional
  thrown item entity in one session-fenced owner turn. Conflicts return both
  authoritative world and player snapshots. Peer container and item-spawn
  events are dispatched by the owner before it replies, so requester loss after
  application cannot hide the committed change. The conservation regression
  proves one accepted command and one stale duplicate produce exactly one item
  entity; all 529 `mc-net` library tests and strict `mc-net` clippy pass. This
  slice has no new real-client gate and does not cover window-0 or crafting
  clicks.
- Furnace experience: cooking recipes now read canonical vanilla 26.1.2
  `cookingtime` and `experience`. Each completed furnace recipe increments the
  persisted vanilla `RecipesUsed` map. Taking output clears that map and spawns
  the computed XP orb in the same owner turn as the output and player-inventory
  update; a stale duplicate cannot award it again. Focused recipe, NBT, owner,
  and furnace tests pass. MCP observation now exposes level, progress, and
  total XP. Local 26.1.2 bytecode inspection found and fixed the packet wire
  order from `progress, total, level` to vanilla `progress, level, total`.
  XP-orb creation now push-dispatches nearby pickup candidates, and exact TCP
  waits for the moved ingot, the orb spawn, and the resulting player XP packet.
  Fresh P23
  `.analysis/real-client-runs/20260714T142509Z-real-client-playable-loop`
  mined and smelted two raw iron and observed total XP increase from `0` to
  `2` in the real 26.1.2 client. Gradle check, `block_edit 75/75`, the full
  workspace test, workspace Clippy, fmt, code-health, and diff-check pass.
  Anvil, recipe unlock/category/group, orb splitting, and full furnace-family
  parity remain open.
- Bookshelf enchanting: the server now counts the exact vanilla 26.1.2 outer
  ring and clear midpoint geometry, capped at 15 shelves. Solaris exposes a
  deterministic playable progression: tools receive Efficiency I/II/III,
  swords receive Sharpness I/II/III, and armor receives Protection I/II/III at
  0/5/15 shelves, requiring player levels 1/10/30 and consuming 1/2/3 lapis and
  levels. Every button click rechecks the live world through the simulation
  owner, preserves total XP, advances the enchantment seed, and persists the
  item component. Efficiency affects mining with the vanilla `level^2 + 1`
  speed bonus. Sharpness affects melee with the local 26.1.2 enchantment JSON
  formula `1.0 + 0.5 * (level - 1)` damage. Protection uses the local vanilla
  `min(total protection points, 20)` cap and `1 - points / 25` damage factor.
  Player damage is now typed: mob, projectile, campfire, fall, starvation,
  generic, and generic-kill paths independently select armor, Protection,
  durability, and shield behavior from vanilla damage tags. Unit tests cover
  geometry, item-specific clues/offers, the Protection cap, typed reductions,
  and applying projectile damage to an enchanted chestplate. One raw-TCP
  scenario applies Efficiency III, Sharpness III, and Protection III in the
  same live window, proving all XP/lapis debits and returned wire components.
  The two live enchanting inputs now participate in the same owner
  compare-and-set as inventory and cursor. Accepted clicks publish the input
  projection before replying, enchanting commits XP and both changed inputs in
  one owner turn, and close/disconnect returns the owner copy even after the
  connection copy is lost. A stale-click regression proves the window is
  rebuilt from the owner projection. All 814 `mc-net` library tests, the seven
  focused enchanting tests, the eleven focused disconnect tests, strict
  package Clippy, and fmt pass for this slice; no new raw-TCP or real-client gate
  was run.
  All 787 `mc-net` tests, all 78 block-edit tests, strict focused Clippy, and
  the full workspace baseline pass. The
  existing real-client Efficiency first-offer scenario also passed at
  `.analysis/real-client-runs/20260714T173113Z-m94-regression-pack`.
  Its first post-change run passed gameplay but exposed a `10.25 ms`
  `chunk_prepare` wait in the read-only cache-pressure check. That check now
  reads an atomically published dirty-saturation state without entering the
  shared world mutex; final insertion still rechecks under the owner. The fresh
  rerun validated with no warning, slow tick, degraded delivery, or slow chunk.
  Randomized vanilla offers, Sharpness IV/V, Protection IV, specialized
  protections, Unbreaking and other enchantments, enchanted books, anvils, and
  a complete no-debug natural acquisition client gate remain open. No new
  real-client gate was run for Sharpness or Protection.
- Crafting-grid repair: both the inventory 2x2 grid and crafting-table 3x3 grid
  now accept exactly two matching count-one items with modelled max durability
  and an explicit damage component. The result combines their remaining
  durability with the vanilla 26.1.2 five-percent bonus, clamps at full
  durability, consumes both inputs, and returns one item. Ordinary
  enchantments are removed as vanilla does. Five direct formula, rejection,
  and container-path tests pass as part of all 763 `mc-net` tests. Solaris does
  not yet model the vanilla curse tag needed to preserve curses during this
  recipe; anvils, grindstones, and a dedicated real-client repair gate remain
  open.
- Entity loot progression: the vanilla 26.1.2 cow sidecar now loads both
  independent leather and beef pools with their inclusive `0..2` and `1..3`
  counts; the embedded fallback also yields both items. Melee and arrow deaths
  roll every supported simple entity pool deterministically from the entity ID.
  Partial sidecar entity tables retain their supported drops and add only
  missing fallback item IDs, so sheep mutton no longer masks the unsupported
  nested white-wool pool. Owner and exact-TCP tests prove multi-item death,
  pickup, despawn, and XP. Fresh real-client P14
  `.analysis/real-client-runs/20260714T165136Z-real-client-playable-loop`
  naturally collected wool from three sheep, crafted and placed a bed, slept
  to morning, and passed the artifact validator without a server warning.
  Exact vanilla RNG, nested/weighted entries, sheep color/shearing conditions,
  looting/burning context, and seeded block count ranges remain open.
- ECS shadow: standalone `bevy_ecs 0.18.1` runs beside the authoritative
  `EntityStore` with only its `std` feature. Stable IDs and moving-entity state
  cover transforms, motion, lifecycle, type, health/attributes, AI goals,
  items, XP, projectiles/falling blocks, vehicles/passengers, persistence, and
  visibility. Six single-threaded schedules consume typed input/AI, snapshot,
  physics, combat/lifecycle, persistence, and output-event work. Owner phases
  compare exact legacy/ECS state and ordered semantic events; telemetry exposes
  comparison counts, and the first mismatch is retained and written to
  `.analysis/entity-shadow-first-divergence.json`. A deterministic mixed replay
  covers all represented entity families, death/despawn, and persistence
  restart. Its explicit 72,000-tick accelerated gate passed with no divergence.
  The debug density report for 1,000 entities over 200 ticks measured
  `157 us/tick` legacy-only and `9,437 us/tick` legacy plus shadow on this host.
  ECS output is not sent to clients, and no new real-client gate was run for
  this slice.
- ECS authority: production item/XP, projectile/falling-block, passive/hostile
  mob, command-summoned entity, vehicle, and persistence-restore paths now
  write only ECS. `EntityStore` is outside `SessionRegistryInner`; AI/pathing
  and physics-query preparation no longer hold the session mutex, and runtime
  metrics report entity-lock pressure separately. All 534 `mc-net` tests, the
  six-test mob wire gate including two-client visibility, falling-block and
  bow/arrow wire gates, concurrency/persistence slices, and strict focused
  clippy pass. The optimized debug density result is `482 us/tick` for 1,000
  ECS entities versus `163 us/tick` for the retained test-only legacy SoA;
  Bevy schedule/query overhead remains measured debt, not a performance claim.
  Fresh real-client P21 passed natural zombie combat and drop pickup. P38
  passed all two-client item-handoff phases but exposed a repeatable 40 ms
  entity-physics wait behind busy chunk workers. Common batches up to 256
  entities stay inline; larger batches now run in one background stage that
  awaits the shared autoscaler CPU notification outside `SimulationOwner`.
  Completion wakes the owner directly, and the owner applies only results whose
  ECS kinematics and immutable chunk snapshot are still current. Per-tick
  physics sampling takes one immutable snapshot of the required chunks from
  `WorldReadView` instead of acquiring the shared world writer. Regressions hold
  both CPU admission and the world writer, prove owner command progress, and
  reject stale entity/world results. Fresh P39
  `.analysis/real-client-runs/20260714T132103Z-real-client-playable-loop`
  kept both real clients in Play, moved each `17.2` blocks across a chunk
  boundary, and passed the strict validator without server WARN/ERROR. This
  removes one hot reader; world mutation, loading, relight persistence, and
  save paths still use the shared writer. Goal pathing now captures ECS input
  under `EntityStore`, releases the mutex for bounded terrain probes, and
  applies the result only when position, rotation, velocity, on-ground state,
  and goal still match. An event-driven regression proves the mutex is
  available during compute and that a newer velocity survives stale-result
  rejection. All 823 `mc-net` tests and the fresh full workspace
  test/Clippy/fmt/code-health baseline pass. A fresh ignored 20-client VD8
  debug gate delivered all 289 chunks to every client with total tick p99/max
  `22.555/25.348 ms`, goal p99/max `6.306/7.709 ms`, entity-physics p99/max
  `3.968/5.006 ms`, and `EntityStore` max hold `5.636 ms`, down from the prior
  `8.434 ms` observation. The gate had 189 server entities, saved in `1.966 s`,
  and left zero dirty chunks after shutdown. It is a bounded protocol workload,
  not a dense AI soak. The common physics apply path now reads Copy-only motion
  state instead of cloning full ECS snapshots, and item expiry uses a dedicated
  item lifecycle index instead of scanning and type-checking every entity each
  tick. New/drop/restore/removal index regressions and all 824 `mc-net` tests
  pass with another fresh full workspace baseline. A second ignored 20-client
  VD8 run again delivered every chunk and reduced dispatch p99 from `7.018` to
  `4.851 ms`, session max hold from `5.634` to `3.939 ms`, and total
  `EntityStore` hold from `4.075` to `3.345 s`; its `EntityStore` max hold was
  `5.077 ms`, total tick p99/max `21.974/29.106 ms`, save `1.918 s`, and dirty
  chunks after shutdown zero. The explicit 1,000-entity debug density gate was
  `489 us/tick` versus the prior `482 us/tick`, with no measured regression.
  Entity apply now releases `EntityStore` before the session-only movement plan;
  an exact channel regression proves that `SessionRegistry` remains held while
  the entity mutex is already available. The first post-change VD8 run was flat,
  so this is a structural boundary fix rather than a claimed speedup. The next
  measured cause was three full `EntityView` passes in goal/physics preparation.
  Active selection now captures AABB and physics kind in its existing pass, and
  the final post-goal state comes from one narrow ECS kinematics query instead of
  another full view pass. Two repeated 20-client VD8 runs measured goal p99
  `4.993/5.316 ms` versus `7.491 ms`, total `EntityStore` hold
  `2.951/2.813 s` versus `3.355 s`, max hold `4.594/4.129 ms` versus
  `5.220 ms`, and total tick p99 `19.600/20.108 ms` versus `21.913 ms`.
  Both delivered 289 chunks to all clients, drained CPU/IO workers, and left
  zero dirty chunks. All 825 `mc-net` tests and a fresh full workspace
  test/strict Clippy/fmt/code-health baseline pass. The `...for_ids` ECS paths
  behind active goal/pathing preparation were still full-world Bevy queries.
  They now use deterministic `EntityId` index lookups while the active set is
  below half of the store and retain the linear query for dense sets. An
  explicit debug benchmark with 32 active entities out of 10,000 fell from
  `10,744 us/tick` to `285 us/tick`; the all-active 1,000-entity benchmark
  remained `475 us/tick`. Active hostile-target selection uses the same bounded
  view. At that checkpoint the separate due hostile-attack pass remained a
  global scan.
  One first 20-client VD8 run caught a real rare tail with tick p99/max
  `186.625/229.729 ms`: the first warning showed idle animal breeding holding
  both `SessionRegistry` and `EntityStore` for about `41 ms`, followed by
  simultaneous inflation of every ticker stage. The idle breeding snapshot now
  takes only `EntityStore`; an exact channel regression holds the session lock
  and proves planning still starts. The next VD8 run passed at tick p99/max
  `19.683/30.907 ms`; the post-boundary repeat passed at
  `20.310/29.740 ms`, delivered 289 chunks to all 20 clients, drained workers,
  saved 348 chunks, and left zero dirty chunks. Its total session-lock hold was
  `1.506 s` versus `1.864 s` in the adjacent green run. This removes the common
  idle breeding dual-lock, not all scheduler preemption risk: the mutation/
  birth commit still needs both state domains, and no long dense AI/fanout soak
  was run. `mc-entity` has 54 passed/4 ignored, `mc-net` has 830/830, and the
  fresh full workspace test/strict Clippy/fmt/code-health baseline passes.
  The first active-selection pass and the separate hostile-attack pass now take
  candidate IDs from the existing loaded-chunk entity index before entering
  `EntityStore`. Exact regressions with one loaded and two far entities fail at
  three visits on the old paths and pass at one visit; the existing position,
  player-distance, and commit-time visibility checks remain as stale-index
  guards. Three consecutive post-index 20-client VD8 runs passed at total tick
  p99/max `20.297/22.975`, `21.102/27.423`, and `20.653/23.599 ms`; hostile
  p99 stayed between `1.612` and `1.661 ms`, every client received 289 chunks,
  workers drained, and shutdown left zero dirty chunks. This proves the bounded
  common workload and removes sparse-world full scans; it is not a dense speedup
  or long soak claim. Idle adult animals also no longer enter the breeding tick:
  an ECS-owned active set tracks baby, cooldown, and love states through insert,
  state update, despawn, and removal. The real 60-tick cow birth and mixed-color
  sheep paths still pass. A post-index VD8 run reduced the combined
  animal-breeding plus sheep-grazing p99/max from the adjacent
  `2.619/3.080 ms` to `1.487/1.726 ms`, and total `EntityStore` hold from
  `2.939 s` to `2.683 s`; total tick p99/max was `20.423/22.158 ms`, all 20
  clients received 289 chunks, and shutdown again left zero dirty chunks. The
  common sheep-grazing plan now intersects an ECS-owned sheep index with the
  loaded-chunk entity index, snapshots sheep under `EntityStore`, then releases
  it before updating session-owned grazing state. Exact regressions fail on the
  old path because it holds both mutexes and visits all three test entities;
  they pass with the session mutex available during the entity snapshot and one
  loaded sheep visited. The real grazing animation, grass edit, and wool-regrow
  path still passes. Two post-change VD8 runs measured combined breeding plus
  grazing p99/max at `1.022/1.204` and `1.006/1.171 ms`, with total
  `EntityStore` hold at `2.435/2.422 s` versus `2.683 s`. Total tick p99/max was
  `20.161/26.331` and `19.136/25.583 ms`; all 20 clients received 289 chunks,
  workers drained, and shutdown left zero dirty chunks. Rare grazing start and
  finish mutations plus an actual breeding birth still need both state domains.
  `mc-entity` has 56 passed/4 ignored, `mc-net` has 833/833, and the fresh full
  workspace test/strict Clippy/fmt/code-health baseline passes. This is bounded
  debug evidence, not a long soak, dense speedup, client, or vanilla-oracle gate.
  Fresh agent-run P39 artifact
  `.analysis/real-client-runs/20260715T101949Z-real-client-playable-loop`
  passed with two real 26.1.2 clients continuously in Play: they moved
  `17.209/17.205` blocks and crossed `2/1` chunks. Its server log has no
  WARN/ERROR or tick-budget warning. One cold background crossing still
  reported max fetch/light `123/208 ms` and completed in `253 ms`; it did not
  stall the ticker, but remains cold-chunk latency debt. No fresh vanilla oracle
  or dense AI/fanout soak was run for these slices.
- World owner cutover: button release ticks now commit in the same owner turn
  as their mutation-token-checked block edits. Interaction-triggered fluid
  scheduling is a bounded detached simulation command that waits on channel
  capacity notification; connection tasks no longer lock `WorldStorage` for
  that step. Random ticks, scheduled block/fluid ticks, campfire cooking, and
  falling-block landing now enter through `SimulationOwner`; their
  implementation requires its private authority token, so the server loop has
  no free mutation entry point for those passes. Open furnaces are also ticked
  once by the owner instead of once by a selected connection; slot/data changes
  are pushed to every viewer, and click commits merge with newer cooking data
  without overwriting it. Window-0/crafting throws, crafting close overflow,
  disconnect cursor overflow, and plant-harvest drops now include item creation
  in their session-fenced owner transaction; the detached `SpawnItemDrop`
  command is gone. The owner capacity test, all 539
  `mc-net` library tests, strict focused clippy, and the real wire berry-harvest
  gate pass. Survival hopper placement also commits its initial scheduled tick
  with the block edit and inventory debit in the same owner turn; the old
  connection-side follow-up world write is gone. Version-checked relight
  persistence and broad immutable reader snapshots are still open. Survival
  mining now reads its target block state and mutation token through one
  `ReadBlockSnapshot` owner command instead of locking `WorldStorage` from the
  connection task. `WorldStorage` now also pushes immutable `Arc<Chunk>` block
  snapshots on chunk insertion, block edit, and eviction. Player collision,
  water overlap, farmland landing, and campfire contact read those snapshots
  without awaiting the async world writer. The concurrency regression stays
  ready while the writer mutex is held; all 150 `mc-world` tests, all 556
  `mc-net` tests, the full workspace baseline, and the fresh two-client P42
  opposite chunk crossing pass. The validated artifact is
  `.analysis/real-client-runs/20260712T023143Z-real-client-playable-loop`;
  broader immutable readers and the Prompt 06 world storage cutover remain
  incomplete.
- Interaction read contention: hand interaction planning for doors, trapdoors,
  fence gates, buttons, and levers now reads one immutable published 3x3 chunk
  snapshot instead of awaiting the shared world writer. Bed lookup and mature
  sweet-berry harvest planning now use published single-chunk snapshots too.
  Berry edits still carry exact state and mutation-token preconditions into
  the existing `SimulationOwner` compare-and-swap commit, while the bed pose
  keeps its existing owner commit. Three regressions hold the writer while
  planning a button press, bed use, and berry harvest; all 781 `mc-net` tests,
  both exact raw-TCP scenarios, all 78 block-edit tests, and the full workspace
  test, strict Clippy, fmt, and code-health gates pass. The owner commit, the
  shared storage writer, and planners for creative breaks and relight
  persistence remain open. No new real-client or performance gate was run for
  this contention slice.
- Bonemeal read contention: crop, mature-stem, and sapling growth planning now
  reads one immutable published snapshot covering every touched chunk. All
  edit states and mutation-token preconditions come from that same snapshot;
  the connection task no longer acquires the shared world writer before the
  existing atomic owner commit. A regression plans young-wheat growth while
  holding the writer. All 789 `mc-net` tests and all 78 raw-TCP `block_edit`
  tests pass, including crop consumption and oak-tree growth. Creative-break
  planning still uses the shared writer. No fresh real-client, vanilla-oracle,
  or performance gate was run for this contention slice.
- Random-tick contention: active-section and sampled-block filtering reads the
  producer-published immutable `WorldReadView`. Eligible passes now snapshot
  the needed 3x3 chunk neighbourhoods and run crop, farmland, grass, vertical
  plant, leaves, and sapling planning outside the global world writer. A small
  overlay preserves sequential semantics when the same position is sampled
  more than once. The commit takes the writer only to compare exact state and
  mutation-token preconditions for blocks actually read, apply the planned
  edits, and schedule nearby leaf ticks; a changed source rejects the whole
  stale plan. Scheduled fluid work now uses a short writer-held dequeue, then
  computes interactions, recursive source paths, and spread through the same
  snapshot overlay. Its commit uses the exact preconditions; a stale plan
  reapplies nothing and returns every drained due tick to the queue.
  Falling-block landing now reads candidate chunks through the ticker's
  published view and computes landing classification, ordered overlay edits,
  and loot outside the writer. Its short commit checks the exact state/token
  preconditions before placing blocks or despawning entities; a stale landing
  keeps both the replacement block and falling entity. Writer-free planning,
  repeated-sample, stale random/fluid/falling sources, 25 filtered random-tick,
  natural leaf-drop, both exact-TCP water/lava scenarios, all three focused
  falling-block physics harness tests, and all 803 `mc-net` tests pass. The
  earlier ignored 20-client VD8 gate delivered all 289 chunks
  per client with tick p99 `21.542 ms` and random-tick p99 `1.314 ms`; its
  world-lock max wait/hold was `6/24 us`. It was not rerun for these changes.
  Final random/fluid/falling edit and scheduling commits, scheduled block
  hopper/comparator fallback, furnace work, relight, the survival-break atomic
  falling-column scan, and the shared writer itself remain open; no new
  vanilla-oracle, real-client gameplay, or performance gate was run.
- Scheduled-block contention: hopper backfill and due dequeue use one short
  writer section. A due batch containing only buttons, leaves, or irrelevant
  stale ticks now plans from immutable 3x3 snapshots with an ordered overlay.
  Its short commit validates exact state and mutation-token preconditions;
  stale plans apply nothing and return every drained tick to the queue. Any
  batch containing a live hopper or comparator keeps the old writer-held
  ordered path. The writer-free button regression, ABA stale-requeue
  regression, all 23 focused scheduled/hopper tests, and all 804 `mc-net`
  tests pass, as does the full workspace baseline. No fresh real-client,
  vanilla-oracle, or performance gate was run.
- Falling-block start contention: connection-side post-edit column discovery
  now reads affected columns from the published immutable world snapshot and
  finishes before waiting for the shared writer. Its plan guards the edited
  support, every falling cell, and the terminal cell with exact state and
  mutation-token preconditions. The short conditional removal commit rejects
  a stale plan, preserving a concurrent replacement without spawning an
  entity. The survival-break owner path uses the same preconditions, but its
  short column scan remains inside the atomic break transaction. All five
  focused falling-block unit tests, all three physics harness scenarios, and
  all 805 `mc-net` tests pass, as do the full workspace test, strict Clippy,
  fmt, and code-health gates. No fresh real-client, vanilla-oracle, or
  performance gate was run.
- Furnace contention: `WorldStorage` now push-publishes per-chunk furnace
  snapshots with load, update, block-replacement, and eviction changes. Loaded
  furnace discovery and pure recipe/timer calculation run outside the global
  writer; an idle-furnace regression holds the writer and requires the whole
  pass to return zero on its first poll. A changing furnace rechecks block kind
  and current state under the writer, and recomputes on conflict before commit.
  The commit phase now scopes that writer to one independent furnace instead
  of holding it across the whole active set. An exact event-driven two-furnace
  regression stops after the first commit and proves the writer is available
  before the second. Four furnace-tick tests, eight hopper/furnace tests, all
  775 `mc-net` tests, and the full workspace test, Clippy, fmt, and code-health
  gates pass. The earlier 20-client VD8 gate delivered all 289 chunks per
  client with tick p99/max `20.326/27.405 ms` and world-lock max wait/hold
  `9/17 us`; it was not rerun for this change. Each furnace commit and the rare
  conflict recompute still use the shared writer, and no new vanilla-oracle or
  real-client gameplay gate was run.
- SIMD checkpoint: the production light extraction loop now packs nibble rows
  directly instead of calling per-cell setters. On the Ryzen 5 7535HS host,
  Criterion measured the old loop at about `318.7 us`, the new scalar backend
  at `34.674 us`, and the safe `wide` backend at `32.537 us`. The SIMD-specific
  gain is only about 6.2%, and full emissive recompute measured `10.314 ms`
  scalar versus `10.178 ms` SIMD, so scalar remains the default and
  `SOLARIS_SIMD_BACKEND=portable` is explicit. Both backends match the old
  implementation bit-for-bit on deterministic randomized light grids. Prompt
  07 remains open against its 10% gate; the next measured candidate is batched
  worldgen column evaluation or chunk encoding, not more tuning of this small
  light tail.
- Autoscale work ownership: manual simulation budget settings are removed and
  rejected by config parsing. Autoscale is enabled by default and selects bounded
  ECS pathfinding, random-tick chunk, and scheduled-tick budgets from
  `low_end`, `balanced`, or `high_end`, then rebalances them from the existing
  runtime p95 window. A saturated scheduled queue keeps its quota while random
  ticks yield first; deferred scheduled ticks remain queued. Entity physics is
  never dropped to satisfy a work budget: every active in-range entity is
  processed, while the shared adaptive CPU admission limit controls its worker
  parallelism. Runtime pressure,
  drain state, or a scale-down decision also suppresses speculative forward
  chunk prewarm while leaving visible chunk delivery untouched. The same
  controller now changes admission to the shared worldgen, lighting,
  compression, and entity-physics CPU pool. Scale-down and drain stop new work
  above the live limit; permit release and scale-up wake blocked work directly.
  Physics batching uses that live limit, while the physical maximum remains the
  automatically selected startup capacity. Focused pressure, recovery, drain,
  and wake regressions pass, as do all 558 `mc-net` tests, the full workspace
  test/clippy/fmt/code-health baseline, and fresh two-client P42 artifact
  `.analysis/real-client-runs/20260712T025347Z-real-client-playable-loop`.
  Both clients stayed in play and crossed opposite chunk boundaries; the
  server log has no slow-tick, chunk-lock, dirty-pressure, or degraded-delivery
  warning. Profile pressure/recovery soak and multi-instance scaling remain
  Prompt 08 debt.
- Simulation ownership: active `SaveHandle`, player/console save and stop, and
  startup checkpoint saves enqueue an ordered owner barrier. The barrier
  captures immutable player, entity, world-time, and simulation-tick snapshots;
  disk IO writes those snapshots even if later live state changes. Final
  shutdown save uses a separate post-drain path after the owner has stopped.
  Owner-order and post-snapshot mutation tests pass, and all 529 `mc-net` plus
  all `mc-server` tests pass. Dirty-world flush remains token-guarded and may
  include later world changes; this is not a global atomic disk transaction.
- Client MCP waits are push-driven. Applied inbound packets publish a client
  state event through the NeoForge mixin; login/logout publish lifecycle state,
  while client ticks publish a separate condition used only for tick-driven
  input, movement, and use duration. Inventory/entity/login waits never wake
  just because a tick elapsed. Production MCP/scenario code and the smoke
  contain no `Thread.sleep` or retry polling. A final new-player
  `playable-11-eat-passive-food` run earned beef, observed natural sprint
  exhaustion `20 -> 19`, then observed food `19 -> 20` and stack `1 -> 0`.
- Block edits: owner world commands now enter the Tokio mutex wait queue once.
  Unlock wakes the owner, which validates and applies the original CAS command;
  requesters do not retry on guessed ticks. The exact test holds the mutex,
  proves the owner future is pending, releases it, and observes one commit from
  one queued command.
- Teleport handling: movement arriving before the matching confirmation is now
  ignored without sending duplicate position-sync packets over reliable TCP.
  Collision correction also avoids teleporting an already-colliding
  authoritative pose back into the same solid overlap. Five pending-teleport
  tests, two collision tests, all 525 `mc-net` library tests, and the repeated
  real-client diamond handoff pass without `teleport confirmation id mismatch`.
  That live run still recorded one separate 230 ms entity-physics tick with 55
  persisted mobs, so performance is not claimed clean.
- Runtime shutdown: console input now uses one process-wide OS reader feeding
  async server instances, so shutdown no longer leaves a Tokio runtime worker
  blocked in stdin. The MCP-backed debug server completed its final save and
  exited with status 0 after `SIGINT`.
- Harness: `cargo test -p mc-test-harness --test block_edit` passes the
  survival break/drop/pickup/place and crafting-table paths.
- Harness: block-action waits are packet-driven. Ack plus every required block
  update completes simple actions; falling-block `AddEntity` and pickup
  `SetSlot` packets complete their respective actions. Timeouts only fail a
  stuck gate and are never a success condition.
- Harness: `cargo test -p mc-test-harness --test persistence_inventory`
  passes block persistence through disk flush/reopen.
- Runtime smoke: `timeout 15s cargo run --bin mc-server -- --config
  playable.toml` reaches `Solaris is listening` in debug mode; no real client
  connected in that smoke.
- Config: `parses_playable_profile_as_loopback_survival_spike` keeps
  `playable.toml` on loopback/offline, view-distance 4, seed 0, embedded
  no-sidecar data, no default ops/debug commands, and live autoscale bounded to
  view-distance 4.
- Runtime capacity: worker-percentage config fields are removed and rejected.
  Startup derives one IO limit and one shared chunk/entity CPU limit from
  available parallelism; the 12-core MCP run selected 3 IO and 6 shared CPU
  permits.
- Runtime autoscale gate: the default-on playable controller raised live chunk
  send/load/generate limits from `8/16/16` to `16/64/32` while keeping
  view-distance 4. The no-debug P10 client scenario found and killed a natural
  chicken, picked up its drop, and validated
  `.analysis/real-client-runs/20260712T054056Z-real-client-playable-loop` with no
  server warning.
- Harness: `embedded_playable_flat_move_jump_input_and_wall_collision_behave`
  proves the no-sidecar playable path accepts flat-ground movement and jump
  input without rubber-banding, and corrects an invalid move into a seeded
  wall.
- Playable recipe and item-component fallbacks now include the full
  wooden/stone basic hand-tool set used when `playable.toml` runs without
  local vanilla sidecars.
- Renewable food: generated surface decorations include
  `minecraft:short_grass`; the repo-owned survival loot now turns broken short
  grass into `minecraft:wheat_seeds`, and the final fallback recipe turns three
  harvested wheat into one bread without shifting any existing recipe display
  ID. The embedded survival wire gate
  `embedded_short_grass_break_delivers_wheat_seeds_over_wire` proves a TCP
  client receives the seed in inventory. Existing runtime and wire tests prove
  farmland placement, mature wheat returning wheat plus seeds, 3x3 bread
  crafting, and bread's hunger rule. The real 26.1.2 client run
  `.analysis/real-client-runs/20260712T043919Z-real-client-playable-loop`
  passed natural seed collection, three age-7 crops, client light `sky=15
  block=0`, three harvest pickups, and one crafted bread. The scenario no
  longer adds synthetic jumping or hunger drain. Its functional observations
  passed, but the enclosing gate remains degraded by one startup slow tick and
  two `chunk_prepare` lock-wait warnings. The later P10 run below proves those
  startup performance warnings are gone after the shared runtime fixes; P43
  itself was not repeated in that final performance pass.
- Passive livestock movement: cows, sheep, and chickens now use their embedded
  vanilla movement-speed attributes instead of the old shared `0.8 block/s`
  constant. Physics starts a real `0.42 block/tick` jump when a grounded living
  entity meets a full-block obstacle, and all entity motion packets convert the
  runtime's blocks-per-second velocity to the client's blocks-per-tick units.
  `mc-physics` passes 17 tests and `mc-net` passes 566 tests. A push-driven MCP
  run observed a cow, sheep, and chicken each move more than one block; the cow
  also rose `1.137` blocks while moving. Sheep/chicken movement direction and
  yaw differed by at most `1.5` degrees; a diagonal path is therefore treated
  as normal movement, not forbidden. The no-debug P10 client scenario passed
  on a natural chicken in the final artifact
  `.analysis/real-client-runs/20260712T052905Z-real-client-playable-loop`.
  That enclosing gate also passed after processing all 49 active entities: its
  server log contains no warning, slow runtime tick, or `chunk_prepare`
  lock-wait.
- Runtime hot-path cost: entity physics now shares immutable resident chunk
  snapshots and reads only blocks actually requested by collision resolution;
  it no longer builds thousands of block positions and three hash maps per
  tick. Normal batches through 64 entities run directly, while larger batches
  use the autoscaled shared CPU pool in groups of at least 64.
  A block edit whose old and new states have identical light behavior now
  preserves baked chunk light directly; crop age changes no longer clear and
  recopy every section light array or rescan the highest opaque column. The
  final P10 artifact above passed on the same fixed playable budgets that had
  produced a `53 ms` tick before these changes. The old nearest-entity physics
  cap is also gone, so autoscaling cannot freeze mobs outside an arbitrary
  query budget. `mc-world` passes 151 tests,
  `mc-net` passes 570 tests, focused Clippy passes for both crates, and fmt plus
  code-health pass. The full workspace test and strict Clippy baselines also
  pass. P42 passed with 49-entity physics and no slow-tick warning in
  `.analysis/real-client-runs/20260712T062405Z-real-client-playable-loop`. The
  block-edit gate is 71/71 after removing its stale 17-tick wheat
  wait: wheat is vanilla-style instant break and now validates drops from the
  `StartDestroyBlock` result directly.
- Tool progression: common stone and ore drops now require the matching
  pickaxe tier derived from the local 26.1 `mineable/pickaxe` and
  `incorrect_for_*_tool` oracle tags. Hand or the wrong tool still breaks the
  block but produces no progression drop; wooden/golden, stone/copper, iron,
  and diamond/netherite tiers are ordered explicitly. Embedded fallback
  recipes now add iron pickaxe, diamond pickaxe, and diamond sword after all
  existing display IDs at `61`, `62`, and `63`. Focused runtime tests prove
  wooden pickaxe cannot harvest iron and stone pickaxe cannot harvest diamond,
  while stone and iron pickaxes produce the expected drops.
- Mining data: the embedded `minecraft:mineable/pickaxe` fallback resolves to
  the same 482 unique blocks as the local vanilla 26.1.2 oracle. With
  `data/vanilla` enabled, the server now also loads exact hardness and
  requires-correct-tool facts for all 29,873 block states plus ordered tool
  rules for all 1,506 sidecar item facts. Survival mining applies the vanilla
  base divisor, underwater penalty, airborne penalty, and Efficiency I bonus.
  Status/effect modifiers and enchantments other than Efficiency remain open.
- Block loot counts: the vanilla sidecar loader now preserves bounded uniform
  block counts when `apply_bonus` has no Fortune level and `explosion_decay`
  has no explosion context. Break transactions sample those ranges from a
  stable position/state/mutation-token seed, so retries of one owner CAS cannot
  reroll the drop. Local 26.1.2 tables and tests pin lapis to `4..9` and
  redstone to `4..5`; the owner-planned break test verifies the sampled count
  on the spawned item entity. `mc-data 117/117`, `mc-net 788/788`, and
  `block_edit 78/78` pass. Fortune, Silk Touch, explosion decay, broader
  conditional loot, exact vanilla random sequences, and a fresh real-client ore
  gate remain open.
- Block loot pools: survival break now emits every independent block drop pool
  that the fail-closed sidecar loader preserved, instead of silently taking
  only the first pool. The local 26.1.2 table for
  `potted_oak_sapling` is pinned to both `flower_pot` and `oak_sapling`, and a
  runtime regression returns both stacks in table order. The sidecar contains
  56 multi-pool block tables, including potted plants and leaves; only their
  currently supported pools are evaluated. `mc-data` loot tests pass 16/16,
  `mc-net` passes 790/790, and `block_edit` passes 78/78. Weighted entries,
  block-state/tool conditions, Fortune, Silk Touch, explosion context, and a
  fresh real-client multi-drop gate remain open.
- Iron utility: the embedded playable recipe set now exposes the vanilla 26.1
  three-ingot bucket shape as the final stable display ID `64`, leaving all
  previous scenario IDs unchanged. The packet-driven
  `embedded_bucket_recipe_composes_with_water_pickup_and_placement` gate crafts
  the bucket at a real crafting-table container, moves it into the selected
  hotbar slot, removes a water source into `minecraft:water_bucket`, and places
  the source back while observing the exact block, slot, and acknowledgement
  packets. Together with the fresh P23 natural iron-ingot run this proves the
  composed survival route without adding new fluid mechanics. A single
  no-debug real-client iron-to-water journey was not run for this slice.
- Iron tier: the same embedded recipe set now exposes the vanilla 26.1 shapes
  for iron axe, shovel, hoe, helmet, leggings, and boots at final stable display
  IDs `65..70`. Runtime recipe matching produces all six items from real 3x3
  inputs. Equipping the four-piece iron set yields the expected combined 15
  armor points, reduces a 10-point hit to 6, and applies durability damage to
  every equipped piece. Full `mc-data` (`93/93`), `mc-net` (`549/549`), and
  block-edit (`69/69`) suites pass. Existing recipe IDs are unchanged; a fresh
  real-client full-set crafting/equip run was not performed for this slice.
- Diamond tier: append-only `zzz_playable_diamond_*` recipes complete the axe,
  shovel, hoe, helmet, chestplate, leggings, and boots at display IDs `71..77`;
  the earlier diamond pickaxe and sword remain at `62` and `63`. Actual 3x3
  matching produces all seven new items. The four-piece set contributes 20
  armor and 8 toughness, reduces a 10-point hit to 3, and takes durability on
  every equipped piece. Existing worldgen tests prove normal and deepslate
  diamond ore are reachable, and the common ore gate requires an iron pickaxe
  or better before the diamond drops. No fresh no-debug real-client mining and
  full-set crafting journey was run, so that end-to-end checkpoint remains
  open.
- Harness: `survival_hoe_use_tills_dirt_and_damages_tool` proves a basic
  crafted-tool action: main-hand hoe use turns dirt into farmland and applies
  tool durability damage.
- Harness: `embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table`
  proves the no-sidecar playable path can mine nearby oak logs, pick up drops,
  craft planks/table/sticks, place/open the crafted table, and craft a wooden
  pickaxe, then use it to mine cobblestone and craft a stone pickaxe.
- Worldgen: `default_seed_spawn_window_contains_basic_playable_resources`
  proves fresh seed-0 playable worlds generate a harvestable tree and shallow
  stone within 64 blocks of spawn, so the no-op survival profile is not relying
  on debug commands for the first wood -> tools loop.
- Harness: `embedded_generated_seed_survival_crafts_tool_and_persists_without_debug`
  proves a no-op raw client can use the actual generated seed-0 disk world to
  mine a nearby tree, craft planks/table/sticks, place/open the crafted table,
  craft a wooden pickaxe, then shutdown/restart with the table block and
  crafted tool still saved.
- Harness: `embedded_save_restart_rejoin_preserves_inventory_and_edited_block`
  proves the no-sidecar playable path can `/save-all`, restart on the same
  disk world, reload the edited block from storage, and rejoin with the saved
  hotbar stack.
- Harness: `embedded_non_op_shutdown_restart_preserves_survival_edit_and_inventory`
  proves a no-op playable-style survival client can mine/place without debug
  commands or `/save-all`, then shutdown/restart restores the edited block and
  remaining hotbar stack.
- Storage: `sync_dirty_flush_replans_when_competing_writer_creates_region`
  covers the save-path race where a pressure flush writes a region between a
  synchronous flush plan and write; `flush_dirty` now re-plans instead of
  surfacing a transient `StaleRegion` during playable save checks.
- Harness: `embedded_playable_short_session_soak_keeps_clients_responsive`
  proves four no-sidecar playable raw clients can stay connected through a
  short movement/input/liveness window without position corrections, slow
  outbound pressure, or active-session visibility drops, and that a fresh
  probe client can connect afterward.
- Harness: `inventory_recipe_rejects_three_by_three_tool_without_crafting_table`
  keeps 3x3 tool recipes gated behind an active crafting table instead of the
  2x2 inventory recipe path.
- Client gate: `tools/run-playable-client-gate.sh` now pins
  `docs/playable/real-client-playable-loop.json`, `playable.toml`, and the
  `playable-04-twenty-minute-survival-loop` agent scenario. The default client
  adapter is now the repo-native NeoForge ModDev runClient adapter under
  `client-mod/solaris-client-agent` with task `:fabric-agent:runClientAgent`;
  the runner classifies it as `gradle-runclient`, and primary client launch is
  not supplied by an env override.
- Client gate: agent-run real-client smoke exists for the repo-native Gradle
  adapter ignoring a bogus legacy command override. The run used
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-01-join-generated-spawn`,
  `SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=300`, and
  `SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4` with
  `bash tools/run-playable-client-gate.sh --run`; it launched
  `:fabric-agent:runClient`, connected `SolarisPrimary`, passed
  `playable-01-join-generated-spawn`, wrote a screenshot, and validated
  `.analysis/real-client-runs/20260706T032835Z-real-client-playable-loop`.
  This is a runner smoke, not a fresh 20-minute loop rerun.
- Client agent: `playable-02a-natural-log-to-planks` is a no-debug real-client
  subscenario for loaded natural log families within survival reach: find a
  supported generated log, approach through normal movement, break until drop
  visible, pick up the item, and place the matching planks inventory recipe
  using the embedded playable recipe id.
- Client agent: `playable-02b-natural-crafting-table-open` extends that path by
  crafting a table from earned planks, selecting the earned table from hotbar,
  placing it, opening the crafting screen, and closing it without debug setup.
- Client agent: `playable-02-natural-wood-to-tool` now drives the no-debug
  real-client wood -> basic tool path: collect three generated natural logs
  from the detected family, craft planks with the embedded playable recipe id,
  craft/place/open a table, craft sticks in the active table container, and
  craft a wooden pickaxe.
- Client runner: `playable-03-save-restart-rejoin` is a runner-managed
  real-client scenario. The before phase drives the no-debug wood -> wooden
  pickaxe path, places a crafted table, and writes the table marker; the runner
  cleanly stops and restarts Solaris on the same `playable.toml` world; the
  after phase rejoins and verifies the persisted crafting table marker plus the
  saved wooden pickaxe inventory.
- Client gate: agent-run real-client evidence exists for the smoke join path
  through the auto-selected `gradle-runclient` adapter.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-01-join-generated-spawn
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=300 bash tools/run-playable-client-gate.sh
  --run` launched the default Gradle `runClient` adapter, connected the
  NeoForge client to `playable.toml`, entered Play, captured a valid PNG, and
  validated
  `.analysis/real-client-runs/20260705T181604Z-real-client-playable-loop`
  with `automation-driver.txt` recording `client_kind=gradle-runclient` and
  `observations.json` result `passed`.
- Client gate: agent-run real-client evidence exists for earned log -> planks.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-02a-natural-log-to-planks
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=180
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  approached and broke a generated birch log, picked up the drop, crafted
  birch planks with recipe display id `2`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T152419Z-real-client-playable-loop`
  with `observations.json` result `passed`.
- Client gate: agent-run real-client evidence exists for earned crafting table
  placement/opening. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-02b-natural-crafting-table-open
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=240
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  approached and broke the generated birch log, crafted birch planks, crafted
  a crafting table, swapped the earned table into hotbar through the client
  container interaction path, placed/opened/closed the crafting table screen,
  captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T153803Z-real-client-playable-loop`
  with `observations.json` result `passed`.
- Client gate: agent-run real-client evidence exists for earned wood -> wooden
  pickaxe. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-02-natural-wood-to-tool
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=360
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected three generated birch logs through normal movement/break/drop/pickup,
  crafted birch planks with recipe display id `2`, crafted and opened an earned
  crafting table, crafted sticks in container id `1`, crafted a wooden pickaxe,
  captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T160402Z-real-client-playable-loop`
  with `observations.json` result `passed`.
- Client gate: agent-run real-client evidence exists for save/restart/rejoin.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-03-save-restart-rejoin
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran the
  before phase through natural birch logs -> planks -> crafted table -> sticks
  -> wooden pickaxe, placed the earned table marker at `7,80,5/up`, stopped
  Solaris cleanly, restarted on the same world, rejoined, verified the persisted
  table marker plus `wooden_pickaxe_count=1`, captured before/after valid PNGs,
  and validated
  `.analysis/real-client-runs/20260705T161340Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `server_restart_count=1`, before/after phase exit status `0`, and driver exit
  status `0`.
- Client gate: agent-run real-client evidence exists for the full P4
  20-minute survival loop. `SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, drove
  the no-debug natural birch log -> birch planks -> crafted table -> sticks ->
  wooden pickaxe path, placed the earned table marker at `11,78,8/up`, stayed
  connected through a `1200000ms` survival soak, stopped Solaris cleanly,
  restarted on the same world, rejoined, verified the persisted table marker
  plus `wooden_pickaxe_count=1`, captured valid P4/post-restart PNGs, and
  validated
  `.analysis/real-client-runs/20260705T162257Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `server_restart_count=1`,
  P4/after phase exit status `0`, and driver exit status `0`.
- Worldgen: `default_seed_spawn_window_contains_basic_playable_resources` now
  requires exposed mineable stone within 64 blocks of spawn, not only shallow
  covered stone. The seed-0 playable world has a small spawn-near stone outcrop
  so the real client can progress from wood tools to stone tools without debug
  setup or dig-down automation.
- Client gate: agent-run real-client evidence exists for earned stone tool
  progression. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-05-stone-tool-progression
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, drove
  natural birch logs -> planks -> crafted table -> sticks -> wooden pickaxe,
  mined exposed stone at `12,77,4/up`, `12,77,5/up`, and `12,77,6/up`, picked
  up `cobblestone_count=3`, reopened the earned crafting table, crafted a stone
  pickaxe with recipe display id `22`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T171035Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, P5 phase exit status `0`, and
  driver exit status `0`.
- Client gate: agent-run real-client evidence exists for stone-tool
  save/restart/rejoin. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-06-stone-tool-save-restart
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=840
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the before phase through natural birch logs -> planks -> crafted table ->
  sticks -> wooden pickaxe -> exposed stone -> stone pickaxe, explicitly walked
  back into reach of the earned crafting table before reopening it, placed the
  restart marker at `10,78,8/up`, stopped Solaris cleanly, restarted on the
  same world, rejoined, verified the persisted crafting table marker plus
  `stone_pickaxe_count=1`, captured before/after valid PNGs, and validated
  `.analysis/real-client-runs/20260705T172042Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `server_restart_count=1`,
  P6 before/after phase exit status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for earned furnace
  placement/opening. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-07-furnace-placement-open
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the no-debug natural birch log -> planks -> crafted table -> sticks ->
  wooden pickaxe path, mined eight exposed stone blocks into
  `cobblestone_count=8`, reopened the earned crafting table, crafted a furnace
  with recipe display id `11`, placed the earned furnace, opened the vanilla
  `FurnaceScreen`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T173235Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, P7 phase exit status `0`, and
  driver exit status `0`.
- Data/server: the embedded playable fallback now includes vanilla-shaped
  charcoal smelting from `minecraft:logs_that_burn`, a required
  `logs_that_burn` item tag covering generated birch logs, and basic generated
  wooden fuels for the furnace path. Focused tests:
  `cargo test -p mc-data embedded_required_recipes_cover_charcoal_from_logs -- --nocapture`,
  `cargo test -p mc-data solaris_required_item_tags_cover_recipe_baseline -- --nocapture`,
  and `cargo test -p mc-net generated_wood_items_are_playable_furnace_fuel -- --nocapture`.
- Client gate: agent-run real-client evidence exists for furnace charcoal
  smelting. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-08-furnace-charcoal-smelt
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=960
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the no-debug natural birch log -> planks -> crafted table -> sticks ->
  wooden pickaxe -> furnace path, handled an upper-log close-approach miss by
  still breaking the already reachable log at `8,82,6/down`, reopened the
  furnace, moved `minecraft:birch_log` into input slot `0`, moved
  `minecraft:birch_planks` into fuel slot `1`, observed `minecraft:charcoal`
  in output slot `2`, moved it into inventory, captured a valid PNG, and
  validated
  `.analysis/real-client-runs/20260705T175014Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, P8 phase exit status `0`, and
  driver exit status `0`.
- Client gate: agent-run real-client evidence exists for torch craft/place.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-09-torch-craft-place
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1500
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the no-debug natural birch log -> planks -> crafted table -> sticks ->
  wooden pickaxe -> furnace -> charcoal path, crafted torches with recipe
  display id `26`, consumed one cooked `minecraft:charcoal` and one earned
  `minecraft:stick`, observed `minecraft:torch` count `4`, placed a
  `minecraft:torch` world block at `8,79,4/up`, captured a valid 854x480 PNG,
  and validated
  `.analysis/real-client-runs/20260705T180206Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, P9 phase exit status `0`, and
  driver exit status `0`.
- Data/server: embedded biome spawn rules now use `minecraft:plains` as the
  default land creature/monster fallback and `minecraft:ocean` as the default
  aquatic fallback when generated seed-0 biomes are not explicitly listed in
  the minimal no-sidecar spawn table. Focused regression:
  `cargo test -p mc-test-harness embedded_playable_seed_spawns_food_mob_in_initial_window --test mob_presence -- --nocapture`.
- Client gate: agent-run real-client evidence exists for a natural passive food
  mob drop. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-10-passive-food-drop
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, scanned
  a loaded natural `minecraft:chicken`, approached through real client
  movement, attacked it in survival, observed entity removal and a visible drop,
  picked up `minecraft:chicken` with inventory count `0 -> 1`, captured a valid
  PNG, and validated
  `.analysis/real-client-runs/20260705T183407Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, P10 phase exit status `0`, and
  driver exit status `0`.
- Client gate: agent-run real-client evidence exists for eating earned passive
  food after natural hunger drain.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-11-eat-passive-food
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, scanned
  a loaded natural `minecraft:chicken`, approached and attacked it through real
  client movement, picked up `minecraft:chicken` with inventory count `0 -> 1`,
  drained hunger through normal sprint movement, selected and ate the earned
  food item, and captured a valid 854x480 PNG. Validated artifact:
  `.analysis/real-client-runs/20260705T185640Z-real-client-playable-loop` with
  `observations.json` result `passed`, `natural hunger drain: passed`, and
  `passive food eating: passed item=minecraft:chicken started=true
  food_before=19 food_after=20 item_count_before=1 item_count_after=0`. The
  artifact records `client_adapter_source=auto-gradle-runclient`, P11 phase
  exit status `0`, and driver exit status `0`.
- Data/server: the embedded playable fallback now includes the shaped
  `minecraft:chest` recipe from the `minecraft:planks` item tag. The hostile
  spawn planner no longer bypasses light/cover checks in the origin chunk, so
  the fresh playable spawn surface does not bootstrap hostile mobs into the
  real-client survival gate. Focused regressions:
  `cargo test -p mc-data required_recipes_cover -- --nocapture` and
  `cargo test -p mc-net hostile_spawn_planner -- --nocapture`.
- Client gate: agent-run real-client evidence exists for earned chest storage.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-12-earned-chest-storage
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected three generated birch logs, crafted 12 birch planks, crafted and
  opened an earned crafting table, crafted a chest with recipe display id `5`,
  collected an earned passive `minecraft:chicken` drop, placed/opened the
  earned chest, moved the chicken into chest slot `0`, captured a valid
  854x480 PNG, and validated
  `.analysis/real-client-runs/20260705T193107Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `earned chest storage: passed
  slot=0 item=minecraft:chicken moved=true slot_matched=true closed=true`, and
  server shutdown state `health=20.0 food=20`.
- Client gate: agent-run real-client evidence exists for chest storage
  save/restart/rejoin. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-13-chest-storage-save-restart
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the before phase through natural birch logs -> planks -> crafted table ->
  earned chest -> earned passive chicken storage in chest slot `0`, wrote the
  chest marker at `6,79,15/up`, stopped Solaris cleanly, restarted on the same
  world, rejoined, verified the persisted chest block, reopened the vanilla
  `ContainerScreen`, observed `minecraft:chicken` still in chest slot `0`,
  captured before/after valid 854x480 PNGs, and validated
  `.analysis/real-client-runs/20260705T194324Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `server_restart_count=1`,
  `chest storage persistence: passed slot=0 item=minecraft:chicken count=1
  slot_matched=true closed=true`, and server shutdown state `health=20.0
  food=20`.
- Data/server: the embedded playable fallback now includes a white-bed recipe
  appended after the existing recipe display ids, preserving `chest=5`,
  `crafting_table=10`, `furnace=13`, `torch=27`, and `wooden_pickaxe=31`.
  Sheep fallback loot is now `minecraft:white_wool`, while the passive food
  path scans cow/pig/chicken so it does not depend on sheep mutton. Focused
  regressions: `cargo test -p mc-data --lib -- --nocapture` and
  `cargo test -p mc-test-harness --test mob_presence
  embedded_playable_seed_spawns_food_mob_in_initial_window -- --nocapture`.
- Client gate: agent-run real-client evidence exists for earned bed sleep.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-14-earned-bed-sleep
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected two generated birch logs, crafted eight birch planks, killed three
  natural sheep for `minecraft:white_wool` counts `0 -> 3`, crafted and opened
  an earned crafting table, crafted `minecraft:white_bed` with recipe display
  id `34`, placed the bed at `33,73,-19/up`, waited for natural night, used the
  bed, observed `Respawn point set; skipped to morning`, captured a valid
  854x480 PNG, and validated
  `.analysis/real-client-runs/20260705T200048Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `natural night wait: passed`,
  `bed sleep skip: passed`, P14 phase exit status `0`, and driver exit status
  `0`.
- Client gate: agent-run real-client evidence exists for cooked passive food.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-15-cooked-passive-food
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the no-debug natural wood -> table -> wooden pickaxe path, collected a raw
  natural `minecraft:beef` drop before stone/furnace travel, smelted charcoal,
  cooked the raw beef with earned charcoal, drained hunger through normal
  movement, ate `minecraft:cooked_beef`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T213135Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, `passive food drop: passed`,
  `cooked passive food output: passed`, `natural hunger drain for cooked food:
  passed`, and driver exit status `0`.
- Data/server: the embedded playable fallback now includes matching wooden
  door recipes for generated overworld plank families, appended after existing
  playable recipes so earlier recipe display ids stay stable. Focused
  regression:
  `cargo test -p mc-data --lib embedded_required_recipes_cover_playable_wooden_doors_without_shifting_existing_display_ids -- --nocapture`.
- Client gate: agent-run real-client evidence exists for earned wooden door
  craft/place/toggle. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-16-earned-door-place-toggle
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected three natural birch logs, crafted 12 birch planks, crafted/opened
  an earned crafting table, crafted `minecraft:birch_door` with recipe display
  id `36`, placed the earned door at `8,79,5/up`, used it once to open and
  once to close, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T214510Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, dynamic bridge port `47073`,
  `door recipe: passed`, `door placement: passed`, `door toggle open: passed`,
  `door toggle close: passed`, P16 phase exit status `0`, and driver exit
  status `0`.
- Data/server: the embedded playable fallback now includes matching wooden
  sign recipes for generated overworld plank families, appended after the
  wooden door recipes so earlier recipe display ids stay stable. Focused
  regression:
  `cargo test -p mc-data --lib embedded_required_recipes_cover_playable_wooden_signs_without_shifting_existing_display_ids -- --nocapture`.
- Client gate: agent-run real-client evidence exists for earned wooden sign
  craft/place/edit. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-17-earned-sign-place-edit
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected three natural birch logs, crafted 12 birch planks, crafted/opened
  an earned crafting table, crafted sticks with recipe display id `21`, crafted
  `minecraft:birch_sign` with recipe display id `45`, placed the earned sign
  at `8,79,7/up`, observed the vanilla sign editor, wrote
  `Solaris|P17|NoDebug|OK`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T215902Z-real-client-playable-loop`
  with `observations.json` result `passed`. The artifact records
  `client_adapter_source=auto-gradle-runclient`, dynamic bridge port `33823`,
  `sign recipe: passed`, `sign placement: passed`, `sign editor: passed`,
  `sign text update: passed`, P17 phase exit status `0`, and driver exit
  status `0`.
- Data/server: the embedded playable fallback now includes a shaped
  `minecraft:campfire` recipe at display id `53` plus campfire-cooking recipes
  for beef/chicken/porkchop at display ids `54..56`, appended after the wooden
  sign recipes so earlier display ids stay stable. Focused regression:
  `cargo test -p mc-data --lib embedded_required_recipes_cover_playable_campfire_without_shifting_existing_display_ids -- --nocapture`.
- Client gate: agent-run real-client evidence exists for earned campfire
  craft/place/cook. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-18-earned-campfire-cooking
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1500
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, reserved
  four earned birch logs before the furnace route, returned the furnace input
  log remainder after smelting charcoal, reopened the earned crafting table,
  crafted `minecraft:campfire` with recipe display id `53`, placed it at
  `10,79,2/up`, used earned raw `minecraft:chicken` on the campfire, observed
  and collected `minecraft:cooked_chicken`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T223437Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `furnace input remainder clear: passed`, `campfire recipe: passed`,
  `campfire placement: passed`, `campfire cooking output: passed`, P18 phase
  exit status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for earned campfire
  death/respawn. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-19-earned-campfire-death-respawn
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  bash tools/run-playable-client-gate.sh` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the earned wood -> furnace -> charcoal -> campfire route, placed a crafted
  `minecraft:campfire` at `11,78,3/up`, used earned raw `minecraft:chicken`
  on the campfire, skipped making cooked pickup mandatory for the death path,
  moved onto the same lit campfire, reached the vanilla death screen from
  normal contact damage, performed respawn, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260705T233707Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `campfire cooking output: passed pickup_required=false`,
  `campfire hazard death: passed`, `campfire respawn: passed`, P19 phase exit
  status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for campfire death-drop
  recovery. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-20-campfire-death-drop-recovery
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  bash tools/run-playable-client-gate.sh` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, ran
  the earned wood -> furnace -> charcoal -> campfire route, placed a crafted
  `minecraft:campfire` at `10,79,3/up`, used earned raw `minecraft:chicken`
  on the campfire, reached the vanilla death screen from normal campfire
  contact damage, respawned, walked back to the death site, observed the
  dropped earned `minecraft:wooden_pickaxe`, picked it up, captured a valid
  PNG, and validated
  `.analysis/real-client-runs/20260705T234928Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `campfire hazard death: passed`, `campfire respawn: passed`,
  `campfire death-site return: passed`, `campfire death-drop recovery: passed
  item=minecraft:wooden_pickaxe visible=true pickup_restored=true
  expected_count=1`, P20 phase exit status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for earned tool zombie
  combat. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-21-earned-tool-zombie-combat
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  bash tools/run-playable-client-gate.sh` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected four generated birch logs through normal movement/break/drop/pickup,
  crafted birch planks, an earned crafting table, sticks, and a wooden pickaxe,
  waited for natural night, scanned a loaded natural `minecraft:zombie`,
  approached it through real client movement, killed it with the earned wooden
  pickaxe, observed entity removal and a visible drop, picked up
  `minecraft:rotten_flesh` with inventory count `0 -> 1`, verified the player
  survived the fight with `health_after=4.0`, captured a valid PNG, and
  validated
  `.analysis/real-client-runs/20260706T021520Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `zombie scan: passed`, `zombie approach: passed`, `zombie combat drop:
  passed`, `zombie combat survival: passed health_after=4.0`, P21 phase exit
  status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for earned stone-sword
  zombie combat. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-22-stone-sword-zombie-combat
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected four generated birch logs through normal movement/break/drop/pickup,
  crafted birch planks, an earned crafting table, sticks, and a wooden pickaxe,
  mined two natural stone blocks into cobblestone with the earned wooden
  pickaxe, reopened the earned crafting table, crafted `minecraft:stone_sword`
  with recipe display id `26`, waited for natural night, scanned a loaded
  natural `minecraft:zombie`, approached it through real client movement,
  killed it with the earned stone sword, observed entity removal and a visible
  drop, picked up `minecraft:rotten_flesh` with inventory count `0 -> 1`,
  verified the player survived the fight with `health_after=15.0`, captured a
  valid PNG, and validated
  `.analysis/real-client-runs/20260706T030241Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `stone sword recipe: passed`, `stone sword zombie scan: passed`,
  `stone sword zombie approach: passed`, `stone sword zombie combat: passed`,
  `stone sword zombie combat survival: passed health_after=15.0`, P22 phase
  exit status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for no-debug iron ingot
  progression. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-23-iron-ingot-progression
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world,
  collected four generated birch logs, crafted planks, an earned crafting
  table, sticks, and a wooden pickaxe, mined eleven natural stone blocks into
  cobblestone, crafted `minecraft:stone_pickaxe` with recipe display id `24`
  and `minecraft:furnace` with recipe display id `13`, placed/opened the earned
  furnace, mined generated `minecraft:iron_ore` at `13,77,7/west` with the
  earned stone pickaxe, collected `minecraft:raw_iron` with inventory count
  `0 -> 1`, smelted it with earned `minecraft:birch_planks` fuel, moved the
  output `minecraft:iron_ingot` into inventory, captured a valid PNG, and
  validated
  `.analysis/real-client-runs/20260711T222710Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `natural iron ore break/drop/pickup: passed`, `furnace iron ingot output:
  passed`, `furnace iron ingot inventory: passed`, P23 phase exit status `0`,
  and driver exit status `0`. This fresh rerun was made after pickaxe-tier drop
  enforcement and proves the no-debug wooden -> stone -> iron path still works;
  its server log has no `WARN`, `ERROR`, runtime slow-tick, lock-hold,
  teleport-mismatch, or `degraded_delivery=true` line. A preceding fresh run
  reached eleven cobblestone
  but exposed the client controller stuck in its own shallow mining trench;
  stalled `approachBlock` strafing now jumps as well, and the passing rerun
  proves recovery without teleport or a scenario-specific coordinate bypass.
- Client gate: agent-run real-client evidence exists for no-debug iron sword
  zombie combat. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-24-iron-sword-zombie-combat
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, mined
  eleven natural stone blocks, crafted `minecraft:stone_pickaxe` and
  `minecraft:furnace`, mined two generated `minecraft:iron_ore` blocks at
  `13,77,6/east` and `13,77,7/east`, smelted two earned raw iron drops into
  two `minecraft:iron_ingot` items, crafted extra sticks from earned
  `minecraft:birch_planks`, crafted `minecraft:iron_sword` with recipe display
  id `57`, waited for natural night, scanned a loaded natural
  `minecraft:zombie`, killed it with the earned iron sword, picked up
  `minecraft:rotten_flesh` with inventory count `0 -> 1`, verified the player
  survived with `health_after=15.0`, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260706T035247Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `recipe registry loaded entries=58`, `iron sword recipe: passed`, `iron sword
  zombie scan: passed`, `iron sword zombie approach: passed`, `iron sword
  zombie combat: passed`, P24 phase exit status `0`, and driver exit status
  `0`.
- Client gate: agent-run real-client evidence exists for no-debug iron sword
  save/restart persistence. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-25-iron-sword-save-restart
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClient` adapter, generated a fresh per-run `playable.toml` world, mined
  two generated `minecraft:iron_ore` blocks at `13,77,6/east` and
  `13,77,7/east`, smelted both earned raw iron drops into two
  `minecraft:iron_ingot` items, crafted `minecraft:iron_sword` with recipe
  display id `57`, wrote a restart marker for the earned crafting table at
  `7,80,5/up`, performed a runner-managed clean Solaris restart, rejoined the
  same world, verified the marker crafting table still existed, verified the
  earned iron sword remained in player inventory with `iron_sword_count=1`,
  captured valid PNG screenshots for both phases, and validated
  `.analysis/real-client-runs/20260706T040942Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClient`, `server_restart_count=1`,
  `restart marker placement: passed`, `restart marker persistence: passed`,
  `iron sword inventory persistence: passed`, P25 before/after phase exit
  status `0`, and driver exit status `0`.
- Client gate: agent-run real-client evidence exists for no-debug earned shield
  zombie blocking. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-26-earned-shield-zombie-block
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClientAgent` adapter, generated a fresh per-run `playable.toml` world,
  collected four natural generated birch logs, crafted an earned table, sticks,
  wooden pickaxe, stone pickaxe, furnace, one raw iron, and one iron ingot,
  crafted `minecraft:shield` with recipe display id `58`, waited for natural
  night, scanned and approached a loaded natural `minecraft:zombie`, held the
  shield use action, survived with `health_before=17.0` and `health_after=17.0`,
  captured a valid PNG, and validated
  `.analysis/real-client-runs/20260706T044106Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `recipe registry loaded
  entries=59`, `shield recipe: passed`, `natural night wait for shield zombie
  block: passed`, `shield zombie scan: passed`, `shield zombie approach:
  passed`, `shield zombie block: passed`, P26 phase exit status `0`, driver
  exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run real-client evidence exists for no-debug earned iron
  chestplate crafting and equip. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-27-earned-iron-chestplate-equip
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClientAgent` adapter, generated a fresh per-run `playable.toml` world,
  collected four natural generated birch logs, mined eleven natural starter
  stone blocks, crafted an earned table, wooden pickaxe, stone pickaxe, and
  furnace, mined eight natural generated `minecraft:iron_ore` blocks from a
  ten-block exposed starter reserve, smelted eight earned raw iron drops into
  eight `minecraft:iron_ingot` items using two earned plank fuel batches,
  crafted `minecraft:iron_chestplate` with recipe display id `59`, quick-moved
  it into the chest armor slot, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260706T054033Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `recipe registry loaded
  entries=60`, `stone inventory: passed cobblestone_count=11`, `iron chestplate
  fuel planks: passed`, `furnace iron ingot inventory: passed
  iron_ingot_expected_count=8`, `iron chestplate recipe: passed`, `iron
  chestplate equip: passed`, P27 phase exit status `0`, driver exit status `0`,
  and `server_restart_count=0`.
- Client gate: agent-run real-client evidence exists for no-debug earned iron
  chestplate zombie mitigation. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-28-earned-iron-chestplate-zombie-mitigation
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1800
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClientAgent` adapter, generated a fresh per-run `playable.toml` world,
  collected four natural generated birch logs, mined eleven natural starter
  stone blocks, crafted an earned table, wooden pickaxe, stone pickaxe, and
  furnace, mined eight natural generated `minecraft:iron_ore` blocks, smelted
  eight earned raw iron drops into eight `minecraft:iron_ingot` items, crafted
  and equipped `minecraft:iron_chestplate`, waited for natural night, scanned
  and approached a loaded natural `minecraft:zombie`, observed a real hostile
  hit, captured a valid PNG, and validated
  `.analysis/real-client-runs/20260706T060437Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `natural night wait for
  iron chestplate zombie mitigation: passed`, `iron chestplate zombie scan:
  passed`, `iron chestplate zombie approach: passed`, and `iron chestplate
  zombie mitigation: passed entity_id=1000017 health_before=17.54
  health_after=16.080002 damage_taken=1.4599991 max_expected_damage=2.75
  observed_hit=true survived=true mitigated=true`, P28 phase exit status `0`,
  driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run real-client evidence exists for no-debug earned iron
  chestplate persistence across a runner-managed server restart, followed by
  post-restart natural zombie mitigation. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-29-iron-chestplate-save-restart-mitigation
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=2400
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched the default Gradle
  `runClientAgent` adapter, generated a fresh per-run `playable.toml` world,
  crafted and equipped an earned `minecraft:iron_chestplate`, wrote the restart
  marker, stopped the server, restarted against the same world, rejoined with
  persisted player state, verified `armor_slot=chest item=minecraft:iron_chestplate
  count=1`, waited for natural night, scanned and approached a loaded natural
  `minecraft:zombie`, observed a real hostile hit, captured valid PNG
  screenshots for both phases, and validated
  `.analysis/real-client-runs/20260706T063955Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `restart marker placement:
  passed`, `restart marker persistence: passed`, `iron chestplate armor
  persistence: passed armor_slot=chest item=minecraft:iron_chestplate count=1`,
  and `iron chestplate restarted zombie mitigation: passed entity_id=1000020
  health_before=17.54 health_after=15.080001 damage_taken=2.46
  max_expected_damage=2.75 observed_hit=true survived=true mitigated=true`,
  P29 before/after phase exit status `0`, driver exit status `0`, and
  `server_restart_count=1`.
- Client gate: agent-run two-real-client evidence exists for shared visible
  item drops and pickup removal. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-30-two-client-shared-log-drop-pickup
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the primary break a natural generated log and
  leave the item drop visible, had the secondary observe that shared drop, had
  the primary collect it, then had the secondary observe the item entity removal.
  The run captured valid primary and secondary PNG screenshots and validated
  `.analysis/real-client-runs/20260706T070401Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, primary and
  secondary bridge waits ready, `primary bridge scenario=playable-30-two-client-shared-log-drop-break
  result=passed`, `secondary bridge scenario=playable-30-two-client-shared-log-drop-observe
  result=passed`, `primary bridge scenario=playable-30-two-client-shared-log-pickup-collect
  result=passed`, `secondary bridge scenario=playable-30-two-client-shared-log-pickup-gone-observe
  result=passed`, P30 phase exit status `0`, driver exit status `0`, and
  `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for a shared earned
  chest transfer. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-31-two-client-earned-shared-chest
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the primary mine four natural generated birch
  logs, craft birch planks, an earned crafting table, and an earned chest,
  place/open the chest, deposit one leftover earned `minecraft:birch_planks`
  item into slot 0, had the secondary walk to the same chest marker, open it,
  observe and withdraw that item, then had the primary walk back, reopen the
  chest, and observe slot 0 empty. The run captured valid primary and secondary
  PNG screenshots and validated
  `.analysis/real-client-runs/20260706T071908Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, primary and
  secondary bridge waits ready, `two-client shared chest deposit: passed slot=0
  item=minecraft:birch_planks moved=true slot_matched=true closed=true`,
  `two-client shared chest withdraw: passed target=12,78,10/up
  item=minecraft:birch_planks count=1 approached=true visible=true
  screen_matched=true slot_matched=true moved=true empty=true closed=true`,
  `two-client shared chest empty observe: passed target=12,78,10/up
  item=minecraft:birch_planks approached=true visible=true screen_matched=true
  empty=true closed=true`, P31 phase exit status `0`, driver exit status `0`,
  and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for shared earned
  block edit visibility. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-32-two-client-earned-torch-block-edit
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1200
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the primary earn wood, stone, charcoal, sticks,
  and torches, place an earned `minecraft:torch`, had the secondary approach
  the support marker and observe that torch block, then had the primary break
  and collect the torch drop and the secondary observe the block become air.
  The run captured valid primary and secondary PNG screenshots and validated
  `.analysis/real-client-runs/20260706T074232Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`,
  `two-client shared torch placement: passed target=9,78,8/up
  block=minecraft:torch item=minecraft:torch`, `two-client shared torch
  visibility: passed target=9,78,8/up block=minecraft:torch approached=true
  visible=true`, `two-client shared torch break: passed target=9,78,8/up
  block=minecraft:torch item=minecraft:torch approached=true visible=true
  break_started=true became_air=true saw_drop=true collected=true`, `two-client
  shared torch removal visibility: passed target=9,78,8/up block=minecraft:torch
  approached=true removed=true`, P32 phase exit status `0`, driver exit status
  `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for player entity
  visibility and movement broadcast. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-33-two-client-player-visibility-movement
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the secondary observe the primary client's
  client-visible player entity, moved the primary through the normal
  `move_forward` bridge command, and had the secondary observe the primary
  player's client-visible position change. The run captured valid primary and
  secondary PNG screenshots and validated
  `.analysis/real-client-runs/20260706T075919Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client player
  visibility: passed player=SolarisPrimary entity_id=1 position=0.5,81.0,0.5
  distance_squared=0.053603268849113615`, `primary bridge move_forward
  duration_millis=1000`, `two-client player movement visibility: passed
  player=SolarisPrimary before=0.5,81.0,0.5 after=0.5,81.0,5.555685399275042
  min_horizontal_delta=0.05 horizontal_delta=5.055685399275042`, P33 phase exit
  status `0`, driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for normal chat
  visibility. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-34-two-client-chat-message
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the primary send normal client chat, and had the
  secondary observe the formatted primary chat line in real client chat
  history. The run captured valid primary and secondary PNG screenshots and
  validated
  `.analysis/real-client-runs/20260706T082138Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client chat
  send: passed message=p34 hello from primary`, `two-client chat observe:
  passed expected=<SolarisPrimary> p34 hello from primary`, P34 phase exit
  status `0`, driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for player disconnect
  removal visibility. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-35-two-client-player-disconnect-removal
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the secondary observe the primary client-visible
  player entity, disconnected the primary through the real client bridge,
  observed server session release, and had the secondary observe that the
  primary player entity disappeared. The run captured valid pre-disconnect
  primary and post-removal secondary PNG screenshots and validated
  `.analysis/real-client-runs/20260706T083212Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client player
  pre-disconnect visibility: passed player=SolarisPrimary entity_id=1
  position=0.5,81.0,0.5 distance_squared=0.28712517544814137`, `primary bridge
  disconnect: sent`, `server session release: observed log=server.log`,
  `two-client player disconnect removal: passed player=SolarisPrimary`, P35
  phase exit status `0`, driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for player reconnect
  cleanup visibility. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-36-two-client-player-reconnect-cleanup
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the secondary record the primary client-visible
  player entity, disconnected the primary through the real client bridge,
  observed server session release, had the secondary observe old primary
  removal, reconnected the primary through the bridge, and had the secondary
  observe a replacement primary player entity. The run captured valid
  pre-disconnect primary, post-reconnect primary, and secondary PNG screenshots
  and validated
  `.analysis/real-client-runs/20260706T084608Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client player
  pre-reconnect visibility: passed player=SolarisPrimary entity_id=1
  position=0.5,81.0,0.5`, `two-client player reconnect removal: passed
  player=SolarisPrimary`, `primary bridge reconnect: reached Play state`,
  `two-client player reconnect visibility: passed player=SolarisPrimary
  old_entity_id=1 new_entity_id=3 old_position=0.5,81.0,0.5
  new_position=0.5,81.0,0.5`, P36 phase exit status `0`, driver exit status
  `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for player
  death/respawn visibility after a natural campfire death. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-37-two-client-player-death-respawn-visibility
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=1200
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the secondary record the primary player baseline,
  had the primary earn wood, cobblestone, charcoal, sticks, and a campfire
  without debug setup, placed the campfire at `9,78,7/up`, observed cooked food
  output, stood on the retained campfire target until the death screen,
  respawned, moved forward through the bridge, and had the secondary observe
  post-respawn primary movement. The run captured valid primary and secondary
  PNG screenshots and validated
  `.analysis/real-client-runs/20260706T091130Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `stone inventory:
  passed cobblestone_count=8 expected_at_least=8 mining_attempts=8`, `campfire
  hazard death: passed target=9,78,7/up timeout_seconds=45`, `campfire respawn:
  passed timeout_seconds=10`, `two-client campfire death/respawn: passed
  natural_hazard=campfire respawn=true`, `two-client player post-respawn
  movement visibility: passed player=SolarisPrimary before_death=0.5,81.0,0.5
  after_respawn_move=1.140769702670747,81.00000273800086,5.233838402381865
  min_horizontal_delta=0.05 horizontal_delta=4.77700866983995`, P37 phase exit
  status `0`, driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for an earned
  inventory item handoff using vanilla selected-item drop. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-38-two-client-inventory-drop-handoff
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=900
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, had the primary mine and pick up a natural
  `minecraft:birch_log`, drop it through the vanilla selected-item action,
  had the secondary observe the item entity, collect it by movement pickup,
  and had the primary observe that the item entity was gone. The run captured
  valid primary and secondary PNG screenshots and validated
  `.analysis/real-client-runs/20260706T094850Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client
  inventory drop: passed target=9,80,-3/up item=minecraft:birch_log
  selected=minecraft:birch_log x1 visible=true`, `two-client inventory drop
  visibility: passed target=9,80,-3/up item=minecraft:birch_log`, `two-client
  inventory drop secondary pickup: passed target=9,80,-3/up
  item=minecraft:birch_log saw_drop=true drop_gone=true pickup_restored=true
  held=minecraft:birch_log x1`, `two-client inventory drop removal: passed
  target=9,80,-3/up item=minecraft:birch_log`, P38 phase exit status `0`,
  driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for a short
  simultaneous movement/liveness soak. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-39-two-client-short-soak
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=600
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, kept both clients in Play state while each client ran
  six normal `move_forward` pulses through the bridge, recorded state after
  every pulse, and captured valid primary and secondary PNG screenshots. The
  run validated
  `.analysis/real-client-runs/20260706T100117Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client short
  soak: passed pulses=6 duration_millis=750 primary_horizontal_delta=17.200
  secondary_horizontal_delta=17.200 all_states_in_play=true`, P39 phase exit
  status `0`, driver exit status `0`, and `server_restart_count=0`.
- Client gate: agent-run two-real-client evidence exists for simultaneous
  movement across a chunk boundary with live chunk streaming. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-40-two-client-chunk-stream-crossing
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, kept both clients in Play state while each client ran
  twelve normal `move_forward` pulses through the bridge, recorded state after
  every pulse, required both clients to cross at least one chunk coordinate,
  and captured valid primary and secondary PNG screenshots. The run validated
  `.analysis/real-client-runs/20260706T101116Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client chunk
  crossing: passed pulses=12 min_chunk_delta=1 duration_millis=750
  primary_horizontal_delta=17.200 secondary_horizontal_delta=17.200
  primary_chunk_delta=1 secondary_chunk_delta=1 all_states_in_play=true`, P40
  phase exit status `0`, driver exit status `0`, and `server_restart_count=0`.
  `server.log` recorded view-distance flushes at `center_cz=1` for the crossed
  boundary with `degraded_delivery=false`; generated chunk load still reported
  slow fetch/light chunks, so this is a chunk-streaming liveness gate, not a
  no-slow-chunk performance claim.
- Client gate: agent-run two-real-client evidence exists for forward-edge
  chunk prewarm before a simultaneous chunk-boundary crossing.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-41-two-client-chunk-prewarm-crossing
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two default Gradle
  `runClientAgent` adapters with isolated game directories and usernames
  `SolarisPrimary` / `SolarisSecondary`, generated a fresh per-run
  `playable.toml` world, kept both clients in Play state while each client ran
  twelve normal `move_forward` pulses through the bridge, required both clients
  to cross at least one chunk coordinate, and captured valid primary and
  secondary PNG screenshots. The run validated
  `.analysis/real-client-runs/20260706T103617Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, `two-client chunk
  prewarm crossing: passed pulses=12 min_chunk_delta=1 duration_millis=750
  primary_horizontal_delta=17.200 secondary_horizontal_delta=17.200
  primary_chunk_delta=1 secondary_chunk_delta=1 all_states_in_play=true`, P41
  phase exit status `0`, driver exit status `0`, and `server_restart_count=0`.
  `server.log` recorded the initial primary view flush with
  `prewarm_dispatched=9`; the crossed `center_cz=1` windows recorded
  `fetch_ms=0`, `light_compute_ms=0`, `slow_fetch_chunks=0`,
  `slow_light_compute_chunks=0`, and `degraded_delivery=false`. This proves the
  first forward crossing used prepared cache for that path; it is not a broad
  no-stall or no-slow-chunk claim for every movement direction.
- Client gate: agent-run two-real-client evidence exists for suppressing
  duplicate same-spawn chunk preparation when a second client joins while the
  first client's initial view is still warming. The focused RED regression
  `later_same_spawn_client_waits_for_earlier_session_to_warm_chunk` failed on
  the old path because the later stream dispatched its own chunk prepare work
  before the earlier session warmed the chunk; it now passes by treating the
  lower `SessionId` as the chunk warm owner until that session marks the chunk
  loaded or disconnects. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-41-two-client-chunk-prewarm-crossing
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` reran the same two-client
  crossing gate and validated
  `.analysis/real-client-runs/20260706T105224Z-real-client-playable-loop` with
  `observations.json` result `passed`. The artifact records
  `client_kind=gradle-runclient`, `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`, `second_client_enabled=1`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, P41 phase exit
  status `0`, driver exit status `0`, and `server_restart_count=0`. In
  `server.log`, the second client's initial `center_cz=0` view flush now
  recorded `fetch_ms=62`, `max_fetch_ms=32`, `slow_fetch_chunks=0`,
  `slow_light_compute_chunks=0`, and `degraded_delivery=false`; the crossed
  `center_cz=1` windows still recorded `fetch_ms=0`, `light_compute_ms=0`, and
  no slow fetch/light chunks. This is evidence that same-spawn duplicate prepare
  no longer creates slow chunk work for that gate, not a broad no-fetch claim.
- Client gate: the P41/P42 two-client crossing path now keeps forward prewarm
  low-priority and cache-resident instead of trading second-client latency for
  post-crossing lock contention. Focused RED regressions covered the old failure
  modes: the then-current prewarm worker was made sequential after an
  uncontrolled eight-chunk generation burst;
  `forward_prewarm_releases_remaining_claims_when_new_session_joins` failed
  because a newer visible session could not reclaim background prewarm chunks;
  `forward_prewarm_skips_speculation_with_multiple_active_sessions` failed
  because a two-session crossing still dispatched the next speculative edge.
  The historical sequential policy has since been replaced by autoscaler-width
  ordered waves described in the current rare-tail evidence below.
  Startup chunk cache sizing now keeps a `view_distance + 2` radius for
  playable profiles (`view_distance=4` records `chunk_cache_capacity=169`) so
  the spawn window plus forward edge do not churn the warm storage cache.
  `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-41-two-client-chunk-prewarm-crossing
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  bash tools/run-playable-client-gate.sh --run` validated
  `.analysis/real-client-runs/20260706T111459Z-real-client-playable-loop` with
  `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`, phase exit status
  `0`, driver exit status `0`, and `server_restart_count=0`. Its `server.log`
  recorded the primary initial window with `prewarm_dispatched=9`; the second
  initial window recorded `fetch_ms=0`, `light_compute_ms=0`,
  `slow_fetch_chunks=0`, `slow_light_compute_chunks=0`, and
  `degraded_delivery=false`; both `center_cz=1` crossing windows recorded
  `fetch_ms=0`, `light_compute_ms=0`, no slow fetch/light chunks, and no
  `degraded_delivery`. A warning scan for `runtime tick exceeded`,
  `chunk_prepare` lock waits, dirty pressure warnings, and degraded delivery
  was empty. This is playable two-client evidence for the forward crossing
  gate, not replacement-readiness or broad movement/perf coverage.
- Client gate: `tools/run-real-client-regression.sh --run` now validates its
  own artifact directory before it can return success, and `--validate-run`
  fail-closes on server-side playable degradation warnings in `server.log`.
  The focused RED regression
  `validate_run_rejects_playable_server_log_degradation_warnings` failed on the
  old validator because a run with `runtime tick exceeded`,
  `chunk_prepare` lock wait, dirty chunk-cache pressure warning, and
  `degraded_delivery=true` still printed `validated`; it now fails the run and
  reports the matching `server.log` lines. The static runner regression
  `approved_real_client_runner_is_fail_closed` also requires
  `validate_run_dir "$run_dir"` in the `--run` path. The stricter validator was
  checked against the clean P41 artifact
  `.analysis/real-client-runs/20260706T111459Z-real-client-playable-loop`.
  A follow-up self-validating `--run` of the same P41 gate produced
  `.analysis/real-client-runs/20260706T112406Z-real-client-playable-loop` with
  both primary and secondary clients launched by the repo-native Gradle
  `runClient` adapter (`client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`,
  `second_client_adapter_task=:fabric-agent:runClientAgent`), bridge waits
  ready, phase exit status `0`, driver exit status `0`,
  `server_restart_count=0`, valid primary/secondary screenshots, and
  `observations.json` `result=passed`. Its `server.log` had
  `chunk_cache_capacity=169`, the primary initial window prewarmed 9 chunks,
  the secondary initial window and both `center_cz=1` crossing windows
  recorded `fetch_ms=0`, `light_compute_ms=0`, no slow fetch/light chunks, and
  `degraded_delivery=false`; the self-validation server-log degradation scan
  accepted the run.
- Client gate: default P4 was rerun through the repo-native Gradle
  `runClient` adapter with
  `SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4 bash tools/run-playable-client-gate.sh --run`
  and produced
  `.analysis/real-client-runs/20260706T115222Z-real-client-playable-loop`.
  The runner self-validation accepted the artifact (`bash
  tools/run-real-client-regression.sh --validate-run ...` exit `0`).
  `observations.json` has top-level `result=passed`, primary scenario
  `playable-04-twenty-minute-survival-loop` `result=passed`, restart scenario
  `playable-03-save-restart-after` `result=passed`, and valid 854x480 PNG
  screenshots for both phases. The primary observations include 3 natural log
  break/drop/pickups, birch planks, crafting table, stick and wooden pickaxe
  crafting, marker placement, and a 1,200,000 ms survival soak with the wooden
  pickaxe still present; the restart observations confirm marker and inventory
  persistence. `automation-driver.txt` records
  `client_adapter_source=auto-gradle-runclient`,
  `client_adapter_task=:fabric-agent:runClientAgent`,
  bridge wait ready, primary phase exit `0`, restart phase exit `0`, driver
  exit `0`, and `server_restart_count=1`.
- Client gate hardening: a stopped/stale diagnostic run
  `.analysis/real-client-runs/20260706T114225Z-real-client-playable-loop`
  exposed two validator holes: failed observations and server-side teleport
  mismatch spam could still print `validated`. The validator now requires
  top-level and per-scenario `result=passed`, rejects `teleport confirmation id
  mismatch` in `server.log`, and prints degradation matches without leaking a
  `pipefail`/`head` exit 141. Focused regressions
  `validate_run_rejects_failed_observations_result` and
  `validate_run_rejects_playable_server_log_degradation_warnings` cover this.
  The same stale artifact now fails `--validate-run` with the matching
  teleport mismatch lines.
- Client/runtime fixes behind the green P4 artifact: the client scenario no
  longer treats the selected stack as proof of a new pickup; pickup success for
  visible drops is based on inventory count increasing from the pre-break
  baseline. Server `/tp` commands no longer overwrite an unconfirmed pending
  teleport with a newer id, which stopped the real-client `received=N-1`
  confirmation loop. Single-client prewarm now claims a full outer edge ring
  and prioritizes the negative-Z spawn crossing before stale/default yaw can
  send the first P4 movement cold. The P4 `server.log` has no `WARN`, `ERROR`,
  `teleport confirmation id mismatch`, `chunk_prepare` lock waits, or
  `degraded_delivery=true`; the initial window recorded
  `prewarm_dispatched=40`, the first `center_cz=-1` window completed with
  `degraded_delivery=false`, and the return window recorded `fetch_ms=0`,
  `light_compute_ms=0`, no slow fetch/light chunks, and
  `degraded_delivery=false`.
- Client gate: agent-run two-real-client evidence exists for opposite-direction
  chunk-boundary movement. `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-42-two-client-opposite-chunk-crossing
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=720
  SOLARIS_REAL_CLIENT_SERVER_START_SECONDS=4
  bash tools/run-playable-client-gate.sh --run` launched two repo-native
  Gradle `runClientAgent` adapters, moved the primary with `move_forward` and
  the secondary with the new bridge-level `move_backward` command, and
  validated
  `.analysis/real-client-runs/20260706T122802Z-real-client-playable-loop`.
  `observations.json` has top-level `result=passed`; the scenario observation
  records `primary_horizontal_delta=17.200`, `secondary_horizontal_delta=28.200`,
  `primary_chunk_delta=1`, `secondary_chunk_delta=2`, and
  `all_states_in_play=true`. The first P42 diagnostic artifact
  `.analysis/real-client-runs/20260706T122417Z-real-client-playable-loop`
  exposed a cold second negative crossing at `center_cz=-2`; chunk prewarm is
  now center-aware, so two clients at the same center still suppress
  speculative contention while moved-apart clients can prewarm their next owned
  edge. In the green P42 `server.log`, `center_cz=-1`, `center_cz=1`, and the
  formerly cold `center_cz=-2` windows all recorded `fetch_ms=0`,
  `light_compute_ms=0`, no slow fetch/light chunks, and
  `degraded_delivery=false`; no `WARN`, `ERROR`, `runtime tick exceeded`,
  `chunk_prepare` lock wait, `teleport confirmation id mismatch`, or
  `degraded_delivery=true` line was present. The validator now treats P41/P42
  prewarm-crossing artifacts as fail-closed on any `view-distance window
  flushed` line with non-zero `slow_fetch_chunks` or
  `slow_light_compute_chunks`: the fixed P42 artifact still validates, while
  the first diagnostic P42 artifact now fails `--validate-run` on the
  `center_cz=-2` slow chunk line.
- Client gate hardening: `--validate-run` now also requires
  `automation-driver.txt` to record the repo-native Gradle adapter
  (`client_kind=gradle-runclient`,
  `client_adapter_source=auto-gradle-runclient`, and
  `client_adapter_task=:fabric-agent:runClientAgent`) and rejects legacy
  client-command metadata. Regression
  `validate_run_rejects_legacy_client_command_adapter_metadata` covers the
  old false-positive path where passed observations could validate without
  proving the adapter source. The fresh P4 artifact
  `.analysis/real-client-runs/20260706T115222Z-real-client-playable-loop`
  and the green P42 artifact
  `.analysis/real-client-runs/20260706T122802Z-real-client-playable-loop`
  still validate under this stricter gate.
- Client gate hardening: `automation-driver.txt` validation now also requires
  the primary in-client bridge wait to be `ready`, at least one
  `client_agent_phase_exit_status_...=0`, and
  `client_agent_driver_exit_status=0`; any non-zero `client_agent_*exit_status`
  or non-ready bridge wait status rejects the run. Regressions
  `validate_run_rejects_nonzero_agent_driver_exit_status` and
  `validate_run_rejects_timed_out_agent_bridge_wait` cover the forged
  passed-observations false positives. The fresh P4 and green P42 artifacts
  still validate; their Gradle client process exit status remains 143 because
  the runner terminates the GUI after the driver has already passed.
- Client gate hardening: when `automation-driver.txt` records
  `second_client_enabled=1`, validation now requires secondary Gradle adapter
  metadata, a redacted secondary bridge secret, and
  `client_agent_bridge_wait_status_secondary=ready`. Regression
  `validate_run_rejects_two_client_enabled_without_secondary_bridge_ready`
  covers the forged two-client false positive where passed observations did
  not prove that the second real client bridge ever came up. The P4 one-client
  artifact and green P42 two-client artifact still validate under this check.
- Client gate hardening: `--validate-run` now cross-checks
  `observations.json` against `automation-driver.txt`, so any observed
  `playable-30` through `playable-42` two-client scenario requires
  `second_client_enabled=1` in the runner evidence. Regression
  `validate_run_rejects_two_client_observations_without_second_client_enabled`
  covers the forged P42 artifact shape where observations claimed a passed
  two-client scenario but the runner metadata only proved a primary client.
  The fresh P4 one-client artifact and green P42 two-client artifact still
  validate, and the old cold P42 diagnostic artifact still fails on the slow
  chunk window.
- Client gate hardening: after-restart observations now require runner-managed
  restart evidence in `automation-driver.txt`: `server_restart_count>=1`, a
  graceful `server_stop_phase=... signal=INT`, and an after-restart
  `server_start_phase`. Regression
  `validate_run_rejects_restart_observations_without_runner_restart_evidence`
  covers the forged path where `observations.json` claimed a passed
  `playable-03-save-restart-after` phase without proving the server actually
  stopped and restarted. The fresh P4 restart artifact
  `.analysis/real-client-runs/20260706T115222Z-real-client-playable-loop` and
  green P42 artifact
  `.analysis/real-client-runs/20260706T122802Z-real-client-playable-loop` still
  validate; the old cold P42 diagnostic artifact still fails on its slow chunk
  window. Older pre-`runClientAgent` restart artifacts are intentionally not
  current evidence under the stricter adapter and server-log gates.
- Client gate hardening: any passed after-restart observation now also requires
  a paired passed before/primary scenario in the same `observations.json`.
  Regression
  `validate_run_rejects_after_restart_observations_without_before_phase` covers
  the forged artifact shape where `playable-03-save-restart-after` had restart
  metadata and phase status but no observed setup phase. The fresh P4 artifact
  still validates because it contains the primary
  `playable-04-twenty-minute-survival-loop` observation before the
  `playable-03-save-restart-after` observation; the green P42 artifact is
  unaffected, and the old cold P42 diagnostic artifact still fails on its slow
  chunk window.
- Client gate hardening: each passed observed scenario now requires an exact
  matching `client_agent_phase_exit_status_<scenario-id>=0` line in
  `automation-driver.txt`. Regression
  `validate_run_rejects_observed_scenario_without_matching_phase_exit_status`
  covers the forged path where observations claimed `playable-01` passed while
  the runner metadata only proved an unrelated phase. The fresh P4 restart
  artifact and green P42 artifact still validate because they record exact
  phase statuses for `playable-04-twenty-minute-survival-loop`,
  `playable-03-save-restart-after`, and
  `playable-42-two-client-opposite-chunk-crossing`.
- Client gate hardening: top-level `result=passed` now requires
  `observations.json` to contain at least one executed scenario. Regression
  `validate_run_rejects_passed_observations_without_scenarios` covers the
  empty-`scenarios` artifact shape that previously printed `validated`. The
  fresh P4 restart artifact and green P42 two-client artifact still validate;
  the old cold P42 diagnostic artifact still fails on its slow chunk window.
- Client gate hardening: required screenshot artifacts now must be structurally
  valid PNGs with at least a 320x180 IHDR size, so a 1x1 placeholder can no
  longer satisfy `screenshots_required=true`. Regression
  `validate_run_rejects_tiny_required_screenshot_png` covers the old false
  positive; `validate_run_accepts_valid_required_screenshot_png` now uses a
  generated 320x180 PNG fixture. The fresh P4 and green P42 artifacts still
  validate with their real 854x480 screenshots, and the old cold P42 diagnostic
  artifact still fails on its slow chunk window.
- Client gate hardening: every screenshot path listed in `observations.json`
  is now validated even when the manifest scenario has
  `screenshots_required=false`; the required flag only controls whether the
  list must be present and non-empty. Regression
  `validate_run_rejects_invalid_optional_screenshot_png` covers the old false
  positive where an optional screenshot entry with bogus bytes was ignored.
  The fresh P4 and green P42 artifacts still validate; the old cold P42
  diagnostic artifact still fails on its slow chunk window.
- Storage/streaming: pressure dirty flush now serializes the async
  plan/write/commit path so concurrent chunk prepare tasks do not stampede the
  same dirty region file. Focused regression
  `concurrent_pressure_flush_replans_after_stale_region_replace` fails on the
  old retry-only path with 75 write attempts for one dirty cache and now passes
  with a single pressure flush run. The P21 artifact above validates the
  real-client path with one `dirty pressure flush completed ... stale_retries=0`
  line and no `dirty pressure flush failed` / `region changed before replace`
  warnings in `server.log`.
- Entity scaling: dense loaded entity groups now batch cached-block physics
  sampling and cap per-tick physics queries while preserving the nearest
  hostile under budget pressure. Focused regressions
  `entity_physics_batch_sampling_deduplicates_overlapping_positions` and
  `entity_physics_budget_caps_dense_loaded_groups_but_keeps_nearest_hostile`
  cover the unit path. The P21 artifact above has no `runtime tick exceeded
  performance budget`, `entity physics sampling`, or `lock hold exceeded`
  warnings in `server.log`; the prior batch-only artifact
  `.analysis/real-client-runs/20260706T020236Z-real-client-playable-loop`
  still produced slow ticks at 41-44 entity physics queries, so the query cap
  is part of the playable performance fix.
- Worldgen: seed-0 terrain now has sparse deterministic surface cave mouths
  outside the 24-block spawn-safe radius. Mouth depth changes by at most one
  block per horizontal column, water surfaces are excluded, and the deepest
  column intersects normal cave carving instead of ending in a blind pit.
  `playable_seed_has_a_dry_surface_cave_mouth_outside_spawn` and
  `cave_mouth_mask_has_a_one_block_gradient_and_keeps_spawn_clear` pass; the
  full `mc-worldgen` result is 49 library tests plus 3 integration tests passed
  with one explicit performance probe ignored. The fresh real-client startup
  artifact `.analysis/real-client-runs/20260712T072037Z-real-client-playable-loop`
  passed without a server warning. This proves the generated path and client
  startup, not manual exploration of the cave interior.
- Hostile bootstrap: open surface chunks no longer seed zombies, skeletons, or
  spiders during daytime chunk preparation. Covered positions still seed
  hostile combat. The failed P02 artifact
  `.analysis/real-client-runs/20260712T072235Z-real-client-playable-loop`
  persisted a zombie at `(10.65,79.00,-3.96)` beside the dead player at
  `(9.70,79.00,-2.49)`; the spawn planner had only protected the origin chunk.
  The focused open/covered spawn regressions now pass. A clean rerun below
  ended at full health, so the diagnostic failure remains recorded but is no
  longer the current result.
- Item drops: generic item entities now start with vanilla-style vertical
  velocity `0.2` and deterministic horizontal scatter inside `[-0.1,0.1]`
  instead of stacking at exact block centers with velocity `(0,0.1,0)`. The
  ranges come from the local decompiled 26.1.2 `ItemEntity` constructor. The
  new scatter regression, all 7 item-pickup tests, all 8 survival-break tests,
  and all 574 `mc-net` library tests pass. The first post-hostile-fix P02
  artifact `.analysis/real-client-runs/20260712T073301Z-real-client-playable-loop`
  kept `health=20` but left seven generated log drops in center-aligned stacks;
  it is retained as the red diagnostic.
- Client gate: the final clean P02 rerun is
  `.analysis/real-client-runs/20260712T074342Z-real-client-playable-loop`.
  The real 26.1.2 Gradle client collected three generated birch logs, crafted
  12 planks, placed and opened a crafting table, then crafted sticks and a
  wooden pickaxe. `observations.json` reports `result=passed`, the matching
  phase and driver exit statuses are `0`, final saved health is `20`, and
  `server.log` has no warning. This is a focused wood-to-tool gate, not the
  full 20-minute/restart gate.
- Client gate: P44 now observes naturally spawned cows, sheep, and chickens
  through applied-packet notifications. It succeeds only after each exact
  entity moves at least one block; the cow must also rise by at least 0.8
  blocks. The probe records client velocity and movement yaw without moving
  the player or using debug commands. The validated fresh-world artifact is
  `.analysis/real-client-runs/20260712T082256Z-real-client-playable-loop`:
  cow/sheep/chicken speeds were `0.098/0.113/0.074` blocks per client tick,
  yaw deltas were at most `1.48` degrees in the accepted samples, and the cow
  climbed `0.95` blocks. The screenshot is real and `server.log` has no
  warning or error.
- Worldgen: generated tree leaves no longer use the vanilla registry default
  `distance=7,persistent=false`, which marks every new canopy as detached.
  They now start as connected `distance=1,persistent=false` leaves. Before
  this fix three otherwise passing P44 runs repeatedly triggered leaf decay,
  lighting work, and `chunk_prepare`/slow-tick warnings; the fresh P44 run
  above passed the complete validator without those warnings. The focused
  state-selection regression and the existing data-fed tree regression pass.
  Exact per-leaf distance propagation after later log removal remains outside
  this slice.
- Storage diagnostic: the explicit ignored
  `disk_backed_generated_world_startup_stream_budget` probe still leaves two
  dirty chunks after four checkpoint attempts while live world mutation is
  active. It is degraded diagnostic evidence and is not counted as a green
  baseline or fixed by the worldgen work above.
- Entity tick tail: repeated two-client P45 runs reproduced the rare join-time
  stall after the external CPU load was removed. Diagnostic artifact
  `.analysis/real-client-runs/20260712T115556Z-real-client-playable-loop`
  reached `50.037 ms`, and
  `.analysis/real-client-runs/20260712T120335Z-real-client-playable-loop`
  reached `54.270 ms` while chunk-herd materialization overlapped random world
  work. Duplicate herd requests are now coalesced, at most two background herd
  commands run per tick, herd entities enter ECS in one batch, and nearby
  pickup queries use the existing chunk index instead of cloning every entity
  snapshot.
- Client gate: final artifacts
  `.analysis/real-client-runs/20260712T120813Z-real-client-playable-loop` and
  `.analysis/real-client-runs/20260712T121017Z-real-client-playable-loop`
  both passed strict P45 with two real 26.1.2 clients, shared-chest exchange,
  save, restart, and persisted-state checks. Neither log contains a runtime
  tick budget warning. Connection tasks still occasionally waited for the
  single `EntityStore` mutex, with a maximum observed wait of `19.177 ms`;
  removing that remaining ECS-owner contention is follow-up work. This is
  repeated playable evidence, not a long soak or broad performance readiness.
- Pickup ownership: item, arrow, and experience pickup discovery is now pushed
  by an exact player-pose or pickup-capable physics event. Connection tasks no
  longer scan nearby entities once per tick. The focused owner-block regression
  and all 588 `mc-net` library tests pass. The latest two-real-client P38
  artifact
  `.analysis/real-client-runs/20260712T134643Z-real-client-playable-loop`
  passed natural birch-log mining, selected-item drop, secondary pickup, and
  primary removal observation. Its server log has no tick, lock, outbound, or
  backpressure warning.
- Rare save stall: the ignored 20-client VD8 gate exposed a real periodic tail,
  not external CPU noise. Before the fix it measured tick p99 `53.764 ms`, max
  `1.188 s`, and `entity_save` max `1.157 s` while the ticker waited behind an
  explicit save. The ticker now pushes into one coalescing save-worker channel;
  that worker obtains an ordered simulation-owner barrier and writes players,
  entities, and world metadata outside the tick. Four identical post-fix runs delivered
  all 289 chunks to all 20 clients with `entity_save max=0`; tick p99 was
  `36.675/35.669/34.861/36.163 ms`. A later cold-join run reproduced the earlier
  entity-physics scheduler tail at `230.465 ms`. Common 198-entity herd batches
  now stay inline instead of entering the blocking worker pool; three repeated
  runs then held entity-physics max to `8.491/8.606/7.891 ms` and tick max to
  `38.566/48.426/40.956 ms`. The later background-stage change removes the
  remaining busy-worker inline fallback for batches above 256: those batches
  wait off-owner and never consume a Tokio worker without autoscaler admission.
  All 818 `mc-net` tests and the full code-health/workspace test/strict
  Clippy/fmt checkpoint pass. A fresh post-change 20-client VD8 run delivered
  all 289 chunks to every client, with first-chunk p99 `1.525 s`, full-window
  p99 `25.164 s`, tick p99/max `24.435/37.064 ms`, entity-physics p99/max
  `4.218/5.875 ms`, and zero dirty chunks after shutdown. It had 189 server
  entities, below the 257-input background threshold, so it proves no regression
  in the common path but does not replace the focused concurrency proof or a
  dense background-stage load gate. No fresh real-client performance gate was
  run.
  The gate now rejects nonzero in-tick save time, tick p99 above `50 ms`, or an
  entity-physics sample above `50 ms`, and
  `periodic_checkpoint_persists_ordered_owner_snapshot` plus
  `periodic_checkpoint_persists_cancelled_connection_owner_state` cover the
  ordered checkpoint and cancellation recovery contents.
- Concurrent join tail: instrumentation showed every client reached Play in
  `29-430 ms`, one claim owner received the first chunk near `1.9 s`, and the
  other clients delayed their center behind the rest of the 289-chunk queue.
  Same-spawn waiters now keep the center first and are woken by the exact shared
  prepared-cache/claim event. Chunk streaming also runs only after its own
  progress or replan event, so unrelated entity movement no longer rescans an
  empty queue. The final 20-client gate measured first-chunk p99 `1.885 s`,
  full-window p99 `27.611 s`, `QueueEmpty` stops `5,580` instead of `766,114`,
  and session-registry lock acquisitions `30,372` instead of `772,763`. The
  gate now rejects first-chunk p99 above `2.5 s` and more than `100,000` empty
  queue stops.
- Rare chunk-crossing tail: tracing corrected the earlier light-edit diagnosis.
  Same-center clients disabled forward prewarm entirely, and a stale autoscale
  decision could make that one-time skip permanent. The latest same-center
  session now owns the shared prewarm while CPU admission remains controlled by
  the autoscaler. `latest_same_center_session_owns_shared_forward_prewarm` and
  `healthy_background_observation_does_not_drop_prewarm_after_tick_pressure`
  cover both failures. P41 artifact
  `.analysis/real-client-runs/20260712T140752Z-real-client-playable-loop`
  passed with one shared producer and both crossings at `0-1 ms`, with no fetch,
  light compute, or slow chunks.
- A first opposite-direction P42 repeat then caught a narrower race in
  `.analysis/real-client-runs/20260712T140930Z-real-client-playable-loop`: one
  prewarm was already computing when the client crossed again, but the center
  generation change discarded its result and forced a `126 ms` recompute. The
  current producer now finishes and publishes that one chunk to the exact
  waiting visible claim, then cancels the remaining old-center batch.
  `crossing_keeps_inflight_prewarm_result_for_new_visible_edge` reproduces the
  ordering without time-based success. Fresh P42 artifact
  `.analysis/real-client-runs/20260712T141557Z-real-client-playable-loop`
  passed opposite crossings through `center_cz=-2`; every crossed window had
  `fetch_ms=0`, `light_compute_ms=0`, no slow chunks, and `elapsed_ms=0-1`.
- A later fresh P23 exposed the remaining single-client first-crossing cause in
  `.analysis/real-client-runs/20260714T055903Z-real-client-playable-loop`:
  the background batch still used one worker while the autoscaler allowed six,
  so movement beat prewarm and `center_cz=-1` paid `1576 ms` fetch, `1596 ms`
  light compute, and nine slow chunks. Prewarm now runs ordered waves whose
  width is the current autoscaler CPU admission limit. Every wave completes
  before the next session/center check, retaining the exact cancellation and
  prepared-result handoff behavior. The RED concurrency event failed at live
  limit two before the change; all ten prewarm/ownership/cancellation tests now
  pass. Fresh P23 artifacts
  `.analysis/real-client-runs/20260714T061435Z-real-client-playable-loop` and
  `.analysis/real-client-runs/20260714T062449Z-real-client-playable-loop` both
  recorded `fetch_ms=0`, `light_compute_ms=0`, no slow chunks, and window time
  `192/75 ms`. Both strict validators passed with no server warning. This proves
  the normal first crossing on this route, not every pressure/drain or distant
  movement pattern and not a faster cold-light kernel.
- The first post-prewarm P23 also turned an intermittent raw-iron pickup into a
  concrete client-controller failure: the item remained visible `2.55` blocks
  away while the player reported `horizontal_collision=true`. The MCP movement
  adapter no longer guesses 12/18-tick strafe windows, cancels a detour after a
  one-tick twitch, or jumps continuously while approaching entities. It chooses
  forward/left/right from the current collision flag and swept player AABB,
  keeps a side until the direct path is geometrically clear, and jumps only for
  an actual height step. Pure direction tests and full Java-agent tests pass.
  The second fresh P23 picked up the first raw iron `0 -> 1`, completed the iron
  ingot route, and validated. This is reusable movement-controller evidence,
  not a general maze or unloaded-terrain pathfinding claim.
- Three post-fix 20-client VD8 runs delivered all 289 chunks to every client.
  The earlier two measured first-chunk p99 `1.876/1.867 s`, full-window p99
  `27.220/26.960 s`, entity-physics max `8.776/8.029 ms`, and tick max
  `47.809/44.158 ms`. The fresh post-contention run measured lower tails:
  first-chunk p99 `1.746 s`, full-window p99 `24.918 s`, entity-physics
  p99/max `5.527/6.113 ms`, and total tick p99/max `21.683/29.641 ms` across
  565 samples. World-lock max wait/hold was `9/22 us`; session and entity-store
  max hold were `5.362/7.809 ms`. Save completed in `1.902 s`, dominated by
  `1.766 s` of disk flush. The latest run had less external CPU load, so the
  lower numbers are measured improvement, not a causal claim for only these
  code changes. This remains bounded debug-load evidence, not a broad no-stall
  claim or duration soak.
- Client gate: fresh P5 artifact
  `.analysis/real-client-runs/20260714T130754Z-real-client-playable-loop`
  passed three natural birch logs, planks, table, sticks, wooden pickaxe, three
  natural stone blocks, and a stone pickaxe through the real 26.1.2 client.
  Full sidecar recipes keep the embedded display-ID prefix stable and append
  sidecar-only recipes, so enabling `data/vanilla` no longer shifts the IDs
  used by the playable scenarios. Solaris now sends typed 26.1.2 recipe-book
  settings and displays for the same executor vector. An agent-run embedded
  MCP smoke against the real client observed all 1,191 entries with one
  contiguous `0..1190` display-ID range, and P5 still crafted through those
  IDs without a decoder error. This playable-first pass exposes all supported
  recipes immediately; vanilla recipe unlock persistence and exact
  category/group/cooking-XP metadata remain open.
- Sapling growth: local 26.1.2 `SaplingBlock` bytecode pins natural growth to
  raw brightness at least 9, a one-in-seven roll, and `stage=0 -> stage=1`
  before tree placement. Solaris now applies the one-in-seven roll and both
  stages; successful bonemeal use follows the same staged transition instead
  of making a tree from a fresh sapling in one action. Oak trunks vary from
  four to six blocks and birch trunks from five to seven, with the three-layer
  blob crown read from the bundled configured-feature JSON. Planning still
  runs on published chunk snapshots and the owner commit checks every touched
  state and mutation token. Six focused sapling regressions, the two-action
  raw-TCP bonemeal scenario, all 809 `mc-net` tests, and the full workspace
  test, strict Clippy, fmt, and code-health gates pass. This is a common-tree
  subset, not full tree parity: the raw-brightness gate, vanilla RNG stream,
  45% bonemeal success roll, replaceable-block tags, flower/beehive variants,
  and species-specific spruce, jungle, acacia, dark-oak, cherry, pale-oak, and
  mangrove placement remain open. No fresh real-client gate was run.
- Full RegistryData fallback: clients that do not echo Solaris' advertised
  known pack can now enter Play when the local sidecar contains exact captured
  Network-NBT payloads. The first raw JSON-to-NBT implementation was rejected
  by the vanilla oracle because registry codecs intentionally remove or
  normalize fields; `minecraft:cat_variant/all_black` exposed the mismatch.
  `tools/extract-vanilla-data.sh` now drives a local vanilla server, declines
  Known Packs, captures its codec-produced payloads, verifies the complete
  registry/entry set, and installs the result under the gitignored sidecar.
  Startup validates and shares those immutable bytes; a missing, partial, or
  malformed capture remains fail-closed, while the matching-pack fast path
  still sends `has_data=false`. The synthetic raw-TCP gate covers both modes,
  all 122 `mc-data` tests pass, and the explicit vanilla-vs-Solaris oracle
  matched all 28 registries and 382 payloads as canonical NBT. The full
  workspace test, strict Clippy, fmt, and code-health gates pass at this
  checkpoint. This does not yet prove a real Java client with an intentionally
  empty Known Packs reply, and custom/mod registry overlays are not supported
  by this vanilla-only capture.
- Multiplayer status and pre-play bounds: server-list status now reads the
  exact producer-published Play-session count, so one fully joined client is
  reported as `online=1` without taking the session-registry mutex. The same
  register/unregister watch already wakes downstream consumers and now has a
  direct published-count regression. Public `solaris.health` now contains only
  `ready` and `state`; it reports not-ready for shutdown/drain, unsupported
  online-mode authentication, a missing world, or full player capacity.
  Autoscale limits, pressure, counters, and reasons remain available only
  through the operator `/status` command. Login profile properties, Known
  Packs, RegistryData entries, and all three nested UpdateTags collections
  reject oversized VarInt counts before allocating. Focused raw-TCP status,
  push register/unregister, seven status-state tests, strict focused Clippy,
  and four RED-to-green codec regressions pass. This does not implement online
  authentication or remove the global world mutex.
- Lua plugin runtime: the normal server can load `plugin.toml` plus `main.lua`
  packages from `[plugins].directory`. Each plugin gets its own 16 MiB Lua VM
  on one dedicated host thread; server events and returned commands cross
  bounded queues, and every handler has a fixed instruction and command budget.
  Filesystem, process, package, debug, and network libraries are not exposed.
  Lifecycle, player join/leave/chat, tick, targeted/broadcast chat, disconnect,
  and manifest-authorized console roots are wired through the real server path.
  A failed handler disables only its plugin, and duplicate plugin IDs are
  skipped. The full workspace tests, strict Clippy, fmt, and code-health gates
  pass; the disk-to-Lua-to-wire test also proves `time set day` reaches the
  client time packet. No Java-client plugin gate, plugin load soak, hot reload,
  or direct world/entity/inventory API was run or implemented in this slice.
- Rare chunk-prepare stalls: the failing P39 artifact
  `.analysis/real-client-runs/20260715T102905Z-real-client-playable-loop`
  showed repeated neighbour-snapshot acquisitions waiting about `14 ms` on the
  shared world writer. Missing 3x3 neighbours are now planned during one short
  storage acquisition and rechecked through the published `WorldReadView`
  before disk load or generation. A later P41 artifact
  `.analysis/real-client-runs/20260715T105854Z-real-client-playable-loop`
  exposed the remaining cross-client cause: each client batch could start its
  snapshot and disk planning before taking autoscaler CPU admission. The whole
  prepare request now takes a global request permit controlled by the same live
  autoscaler limit, with separate push notifications for CPU-stage and
  request-stage waiters. The prepare lifecycle, scale-down, release wakeup, and
  batched-neighbour paths have direct regressions; all 830 `mc-net` tests and
  focused strict Clippy pass.
- Prewarm breadth is now bounded to the forward, opposite, and nearest lateral
  edge: 27 chunks at playable view distance 4 instead of the full 40-chunk
  perimeter. This keeps the common forward and side crossings covered without
  reproducing the 163-chunk pressure flush and `1093 ms` write from
  `.analysis/real-client-runs/20260715T104246Z-real-client-playable-loop`.
  Prewarm-derived frames remain in the bounded LRU for a trailing subscriber;
  world revision invalidation and pressure shedding still remove them.
  Post-admission P41 artifacts
  `.analysis/real-client-runs/20260715T110701Z-real-client-playable-loop` and
  `.analysis/real-client-runs/20260715T111412Z-real-client-playable-loop` both
  passed two real 26.1.2 clients through chunk crossings with no server warning,
  lock warning, or pressure flush. The repeat moved the clients `17.672` and
  `17.761` blocks; the other crossing windows completed in `0-36 ms` without a
  slow stage, while one chunk still recomputed light in `134 ms` and completed
  in `138 ms`. This is bounded
  two-client evidence, not a long movement soak, general fairness proof, or a
  replacement for removing the global `WorldStorage` mutex.
- Persistence gate ownership: a full workspace retry caught
  `place_dirt_persists_through_flush_to_disk` calling synchronous
  `WorldStorage::flush_dirty` while the live server save coordinator still had
  an external region plan in flight. `StaleRegion` correctly fenced that
  competing write; increasing its fixed retry count would not establish an
  owner. The gate now requests the production listener shutdown, awaits exact
  `serve()` completion, asserts zero dirty chunks, then reopens the world and
  verifies the placed block. Two focused reruns and the full workspace pass.
  This strengthens the test path; it does not make direct synchronous flushes
  safe against arbitrary external writers or prove crash recovery.
- Remaining cold relight tail: P41 artifacts
  `.analysis/real-client-runs/20260715T144519Z-real-client-playable-loop` and
  `.analysis/real-client-runs/20260715T145200Z-real-client-playable-loop`
  isolated full relights at `(0,5)` revision 1, `(-5,-5)` revision 0, and
  `(-1,5)` revision 2, with maxima of `136-165 ms`. Light publication was
  incorrectly fenced by `Arc` identity, then by a generation advanced even by
  proven light-inert crop updates. It now compares an opaque source token that
  changes only when block light behavior can change; ordinary mutation tokens
  still advance for transaction fencing. The same 27-chunk prewarm batch now
  orders its forward, opposite, and nearest lateral edges by distance from the
  player to the corresponding chunk boundary, so a nearby side crossing is not
  queued behind the farther opposite edge. Exact source-token, light-only
  publication, edge-order, and in-flight handoff regressions pass. Fresh P41
  repeats `.analysis/real-client-runs/20260716T083358Z-real-client-playable-loop`
  and `.analysis/real-client-runs/20260716T083543Z-real-client-playable-loop`,
  plus opposite-direction P42
  `.analysis/real-client-runs/20260716T083712Z-real-client-playable-loop`, all
  passed: crossed windows took `0-5 ms`, performed no light computation, and
  logged no server warning. Expanding startup generation and light baking was
  tested and reverted because a clean world took about `29 s` to start. This
  closes the reproduced paths, not broad movement soak or general lighting
  performance.
- Animal commit lock split: sheep grazing start and finish plus animal breeding
  and birth previously held `SessionRegistry` and `EntityStore` together while
  mutating ECS state and publishing client state. Each path now completes its
  ECS mutation, releases the entity lock, and then projects captured state into
  session, spatial, and wire indexes. Exact RED-to-green boundary tests prove
  that neither lock is retained before publication; focused grazing, breeding,
  birth, and sheep-color behavior tests pass, as do all 838 `mc-net` tests and
  the full code-health, workspace test, strict Clippy, and fmt gates. The
  bounded 20-client VD8 debug gate also drained all clients and workers with no
  slow-client drop or timeout: total tick p99/max was `24.362/26.868 ms`, animal
  breeding p99/max `1.140/1.499 ms`, and `EntityStore` max wait/hold
  `10 us/5.659 ms`. This is non-regression evidence, not a measured speedup,
  soak, vanilla oracle, or fresh real-client gate. One global `EntityStore`,
  other dual-lock mutation paths, dense Bevy schedule/query work, and the
  global `WorldStorage` mutex remain.
- Hostile attack lock split: the periodic attack pass previously held
  `SessionRegistry` and `EntityStore` together while selecting player targets,
  spawning skeleton arrows, updating indexes, and building fanout. It now
  selects targets under session state, releases it, creates authoritative
  arrows under ECS state, releases that, and publishes captured snapshots and
  damage commands under session state. The exact boundary regression failed on
  the old lock pair and now proves both mutexes are free before publication.
  Melee behavior, real arrow spawning, and the embedded wire path from skeleton
  shot to player damage pass; all 839 `mc-net` tests and the full code-health,
  workspace test, strict Clippy, and fmt gates pass. A bounded 20-client VD8
  repeat delivered all 289 chunks per client, drained workers, saved with zero
  dirty chunks, and had no slow-client drop or timeout; total tick p99/max was
  `21.693/27.562 ms`, hostile attack p99/max `1.596/1.705 ms`, and
  `EntityStore` max wait/hold `7 us/4.872 ms`. This is non-regression evidence,
  not a speedup, soak, vanilla oracle, or fresh real-client gate. Other
  dual-lock interaction and physics commits plus the single `EntityStore`
  remain.
- Player body-push candidate bound: every accepted pose previously scanned all
  ECS entities while holding `SessionRegistry` and `EntityStore`, although only
  nearby living entities could overlap the player. The path now asks the
  existing chunk index for candidates within the player width plus the widest
  registered entity AABB, then keeps the previous lifecycle, category, AABB,
  and exact overlap checks. The RED regression visited three entities across
  distant chunks; the fixed path visits only the one nearby candidate and still
  pushes it. The follow-up commit split now accepts session and persisted pose
  together under session ownership, applies exact body pushes under
  `EntityStore` alone, releases it, and only then updates published snapshots,
  spatial indexes, visibility, and wire state under session ownership. The
  event-driven RED-to-green boundary test proves both mutexes are free before
  publication, persisted pose is already current, and an observer unloaded at
  that boundary does not receive a stale move. Boundary-chunk,
  projectile-ignore, pose, stale-session, and pickup-dispatch tests pass, as do
  all 841 `mc-net` tests and the full code-health, workspace test, strict
  Clippy, fmt, and diff-check gates. Two fully captured 20-client VD8 repeats
  delivered all 289 chunks per client, drained workers, and saved with zero
  dirty chunks and no slow-client drop or timeout. Total tick p99/max was
  `22.790/25.734 ms` and `20.557/23.870 ms`; SessionRegistry max wait/hold was
  `10.239/9.203 ms` and `3.560/4.187 ms`. The first repeat's single
  `cache prepared chunk` wait warning did not recur in the confirmation run.
  VD8 does not continuously drive player poses, so this proves non-regression
  rather than a movement speedup. Accepted-pose pickup collection now also
  uses three non-overlapping phases: a session-owned spatial plan, an
  `EntityStore` snapshot with exact lifecycle/type/distance checks, and a
  session-owned eligibility and recipient publication pass. Its event-driven
  RED-to-green regression proves the session mutex is free while the ECS mutex
  is held. Item delay and owner block are rechecked at publication and claim;
  XP, grounded-arrow, disconnect, and deterministic-order behavior were also
  reviewed. All 32 focused pickup tests, all 842 `mc-net` tests, and the full
  code-health, workspace test, strict Clippy, fmt, and diff-check gates pass.
  A bounded 20-client VD8 repeat delivered 289 chunks to every client, drained
  workers, saved with zero dirty chunks, and had no slow-client drop or
  timeout; total tick p99/max was `19.066/21.054 ms`, SessionRegistry max
  wait/hold was `3.728/3.811 ms`, and EntityStore max wait/hold was
  `8 us/4.071 ms`. This workload is not pose-heavy, so it proves
  non-regression rather than a movement or pickup speedup. Physics-triggered
  pickup now finishes ECS mutation, releases `EntityStore`, selects affected
  sessions under session state alone, releases that state, and uses the same
  batched three-phase snapshot/publication path. Its event-driven regression
  proves session state is free during the ECS pickup snapshot; the existing
  entity-apply boundary proves ECS is free during the session-only physics
  plan. Pickup publication also rechecks the player's current position after
  the unlocked snapshot. Authoritative item, XP, and grounded-arrow claims now
  reject a disconnected or out-of-radius collector under the final combined
  state check, closing the race after candidate publication. The physics wire
  regression now drains the spawn-time candidate first and proves the new
  physics candidate is sent before movement. Focused pickup tests pass `35/35`,
  all `mc-net` tests pass `845/845`, and code-health, strict `mc-net` Clippy,
  fmt, and diff-check pass with four build jobs. A fresh VD8, workspace test,
  and full workspace Clippy were deliberately not run under the current
  reduced CPU budget, so this slice has no new performance or full-baseline
  claim.
  XP-spawn pickup fanout no longer uses the old combined-lock helper. Direct
  spawn, player death, melee kill, arrow kill, and furnace XP now finish their
  mutation, release the guard, and use the same three-phase pickup pipeline.
  A RED-to-green channel regression proves the session mutex is free while the
  XP pickup ECS snapshot is active; the arrow-kill regression also preserves
  immediate XP pickup publication. Existing player-death and melee-kill tests
  now also assert their real outbound pickup push. The obsolete helper was
  deleted. Focused XP/arrow/death/melee tests, full `mc-net` (`848 passed`),
  strict `mc-net` Clippy, fmt, code-health, and diff-check pass with four build
  jobs. One global
  `EntityStore`, other dual-lock mutation paths, dense Bevy work, the global
  `WorldStorage` mutex, movement soak, a fresh vanilla oracle, and a fresh
  real-client gate remain. Proposed ADR 0005 defines
  fixed 8-by-8-chunk region ownership, epoch fencing, autoscaled worker lanes,
  and push-driven phase barriers as the path past the global ECS serialization
  ceiling. Pre-R1 now has a pure `mc-entity` ownership model: region keys use
  Euclidean chunk boundaries, every lease carries an epoch and lane, ownership
  changes are rejected during an active phase, stale leases and stale phase
  completions fail validation, and leases are exposed in deterministic key
  order. The first R1 scaffold now assigns single-owner entity and herd spawn
  commands `(sequence, RegionLease)` metadata on lane 0. Each lease is checked
  in release builds inside an exact phase; route preparation failures reject
  the batch without mutation instead of silently falling back. The original
  batch still executes in global sequence through the current authority. One
  regression proves two cross-region spawns keep order and persisted output;
  another proves an occupied phase rejects a spawn without creating it. Four
  focused regional tests, full `mc-net` (`848 passed`), strict scoped Clippy,
  fmt, code-health, and diff-check pass with four build jobs. The next R2
  scaffold now keeps physically separate regional `EntityStore` instances
  behind global entity-id, UUID, and location indexes. Stale leases, wrong
  spawn regions, duplicate UUIDs, and cross-region follow/passenger references
  fail without partial insertion or consumed ids. Exact lane acknowledgements
  prevent phase completion while an assigned lane is outstanding. Full
  `mc-entity` validation passes with 63 tests passed and 4 ignored; the same
  full `mc-net` and scoped quality gates remain green after this change. This
  is still scaffold, not production regional execution: `SessionRegistry`
  continues to own the live global store, lane acknowledgements are not lane
  worker threads, and entity-targeted commands, player movement, multi-owner
  transactions, full command coverage, migration, multicore execution, and a
  speedup remain absent. The first R3 migration primitive now gives each move
  a stable `(tick, source region, source epoch, entity id)` `TransferId` and
  idempotent prepare/decision/apply state. Source remains authoritative before
  commit; commit moves the same id/UUID exactly once; reject and an absent
  boundary decision leave it at source. Phase completion first requires exact
  lane acknowledgements, rejects undecided transfers deterministically, and
  forbids work submitted after a lane acknowledgement. Incoming and outgoing
  cross-region entity references fail closed. Independent review found and
  the regressions cover the undecided-phase deadlock, late-after-ack mutation,
  and incoming-reference holes. At that checkpoint, twelve regional tests and
  full `mc-entity` with 68 passed/4 ignored passed; full `mc-net` remained
  848/848, and strict scoped
  Clippy, fmt, code-health, and diff-check pass with four build jobs. This
  state machine is still coordinator-local: no durable recovery journal,
  production `SessionRegistry` cutover, lane workers, migration wire fanout,
  concurrency evidence, or speedup exists yet.
  Transfer snapshots now carry the complete physics result: position,
  rotation, velocity, and on-ground state. Regional kinematics apply mutates a
  same-region entity immediately, but boundary motion only prepares the
  transfer and keeps source authority unchanged until coordinator commit. A
  pending transfer also fences later source physics. Thirteen regional tests
  pass; full `mc-entity` now has 69 passed/4 ignored, `mc-net` remains 848/848,
  and the same scoped quality gates pass. Production physics still targets the
  global `SessionRegistry` store, so this is migration-path evidence rather
  than a production cutover or performance result.
  The regional authority facade now covers global len/id/UUID/motion lookup,
  deterministic EntityId-ordered snapshots and simulation visitors, indexed
  breeding/sheep visitors, and phase-fenced velocity, animal-state, goal, and
  damage point mutations. A cross-region `FollowTarget` mutation fails before
  changing either store. Fourteen regional tests pass; full `mc-entity` now
  has 70 passed/4 ignored, `mc-net` remains 848/848, and strict scoped Clippy,
  fmt, code-health, and diff-check pass with four build jobs. Batch
  spawn/restore, aggregate goal prepare/apply, aggregate shadow telemetry, and
  the actual `SessionRegistry` type swap remain. These facade reads are
  correctness evidence, not concurrency or performance evidence.
  Cross-region batch spawn and persisted restore now preflight every ID, UUID,
  lease, location, entity reference, and vehicle graph before publishing any
  global index. Physical stores receive region-grouped batches; all snapshots
  are inserted before passenger links are restored, preserving forward links.
  Duplicate UUIDs do not consume IDs, invalid/cyclic or duplicate-passenger
  graphs fail atomically, and a new follower cannot attach to an entity with a
  transfer in flight. Independent review found the transfer-reference and
  silent vehicle-sanitization holes; both now have regressions. Seventeen
  regional tests pass; full `mc-entity` has 73 passed/4 ignored, `mc-net`
  remains 848/848, and strict scoped Clippy, fmt, code-health, and diff-check
  pass with four build jobs. Aggregate goal prepare/apply, aggregate shadow
  telemetry, and the production type swap remain.
  Workspace tests, full workspace Clippy, VD8, and a real-client gate remain
  deliberately deferred under the reduced CPU budget.
  Regional AI goals now use one prepare/resolve/apply aggregate across
  physical region stores. Pathing resolves after store access is released,
  apply returns summed `GoalTickStats`, and authority, phase, and pending-lane
  provenance reject foreign, stale, or post-ack batches before mutation.
  Independent review found the post-ack and foreign-store holes; five focused
  regressions cover both plus cross-region apply and stale phases. Twenty-two
  regional tests pass; full `mc-entity` now has 78 passed/4 ignored, and strict
  `mc-entity` Clippy, fmt, and code-health pass with four build jobs. The
  convenience pathing resolver is still sequential, and aggregate shadow
  telemetry, production `SessionRegistry` wiring, lane-dispatch execution,
  concurrency evidence, and speedup remain. `mc-net`, workspace tests, full
  workspace Clippy, VD8, and the real-client gate were not rerun for this slice
  under the reduced CPU budget.
  Regional shadow telemetry now has a lane-local batch path: each worker can
  compare its owned `EntityStore`, send the closed result, and let the
  coordinator merge reports in `RegionKey` order without reopening stores.
  One aggregate call increments one logical comparison, sums current
  entity/event coverage, and preserves the first divergence across later
  calls. Coverage is carried by match and divergence results, so saturated
  child counters cannot produce a false zero. Independent review identified
  the global comparison serialization and saturation holes; four regressions
  cover lane-local merge, deterministic repeated divergence, aggregate stats,
  and saturated counters. Twenty-six regional tests pass; full `mc-entity`
  has 82 passed/4 ignored, the focused `mc-net` shadow-artifact test passes,
  and strict scoped Clippy, fmt, and code-health pass with four build jobs.
  Production `SessionRegistry` wiring is now the next single-lane type-swap
  blocker. Goal lane dispatch, real lane workers, durable migration recovery,
  concurrency evidence, and speedup remain. Full `mc-net`, workspace tests,
  full workspace Clippy, VD8, and the real-client gate were not rerun under the
  reduced CPU budget.
  Production `SessionRegistry` now owns `RegionalEntityAuthority`, so live
  server entities are stored in physical region stores instead of one global
  `EntityStore`. The compatibility adapter preserves server entity ids, uses
  fenced lane-0 phases, commits repeated boundary crossings, rejects non-finite
  kinematics, and clears completed transfer records. Persisted restore now
  inserts the full batch before publishing indexes, so forward passenger links
  survive and invalid batches cannot leave a partial restore. Phase cleanup is
  unwind-safe, and regional goal apply checks lease epochs before mutation.
  Independent review found the restore, panic cleanup, and goal provenance
  holes; focused regressions cover them. Full `mc-entity` passes with 84 passed
  and 4 ignored; full `mc-net` passes 850/850. Strict scoped Clippy, fmt, and
  code-health pass with four build jobs. This is a production authority cutover,
  not a multicore result: one global authority mutex and lane 0 still serialize
  simulation. A connected vehicle/passenger or follow-target group also cannot
  yet migrate atomically across a region boundary. Real lane workers, group
  migration, goal lane dispatch, durable recovery, migration wire fanout, and
  measured parallel speedup remain. Workspace tests, full workspace Clippy,
  VD8, and the real-client gate remain deliberately deferred under the reduced
  CPU budget.
  Boundary migration now treats a vehicle/passenger chain as one transaction.
  One `TransferId` reserves every member, commit removes and inserts the saved
  snapshots as batches, reject unlocks the full group, and rollback restores
  the original batch. The top-level vehicle is the deterministic movement
  leader, so passenger-first or stale passenger physics input cannot select a
  different delta; repeated crossings preserve the passenger graph. A group
  that would split across destinations is rejected before mutation. A
  `FollowTarget` is no longer treated as a physical co-location edge: the
  follower stays in its region and receives the target position captured for
  the goal batch. Complete target and follower snapshots fence apply, so
  target movement/migration, identity replacement, or a changed follower goal
  rejects stale work. Two independent review passes found physics-order and
  provenance holes; RED-to-green regressions cover both input orders, reject
  cleanup, repeated crossing, remote follow, and prepare-to-apply races. Full
  `mc-entity` passes with 94 passed and 4 ignored; full `mc-net` passes 850/850.
  Strict scoped Clippy, fmt, and code-health pass with four build jobs. The
  global regional-authority mutex and lane 0 still serialize production, so
  this is boundary-correctness progress, not multicore or speedup evidence.
  Real lane workers, goal lane dispatch, durable recovery, migration wire and
  cross-region interaction fanout, concurrency measurement, workspace gates,
  VD8, and the real-client gate remain.

  Regional goal pathfinding now has a production multicore slice. The ticker
  releases the regional-authority mutex, resolves one busy region inline, and
  sends only additional busy regions to Rayon's persistent pool. Every extra
  task holds a shared autoscaler CPU permit; scale-down to one CPU returns to
  the serial path, and idle regions create no worker tasks. An event-driven
  regression proves two regions enter pathfinding before either is released.
  Full `mc-entity` passes with 96 passed and 4 ignored; full `mc-net` passes
  851/851. Strict scoped Clippy, fmt, code-health, and diff-check pass with four
  build jobs. This is concurrent read-only goal compute, not full regional
  simulation: store mutation is still behind the global authority mutex, and
  no throughput or p99 improvement is claimed. Workspace tests, full workspace
  Clippy, VD8, and the real-client gate were deferred under the reduced CPU
  budget.

  Dense production physics apply now mutates independent physical region
  stores concurrently in the same fenced phase. Same-region kinematics use
  one ticker lane plus permit-backed Rayon tasks; boundary transfers stay on
  the coordinator, including atomic vehicle/passenger migration and stale
  passenger suppression. Production keeps batches below 257 states inline,
  and the shared autoscaler can reduce the path to one CPU without a manual
  subsystem percentage. Event-driven concurrency, dense-admission,
  small-batch fallback, and mixed local-plus-boundary regressions pass.
  Independent review found two stale-result gaps. Goal batches now exact-fence
  complete snapshots for every active goal type, not only FollowTarget, and
  physics publication consumes accepted authoritative kinematics instead of
  speculative worker steps. A vehicle crossing therefore publishes the
  coordinator-corrected passenger position, while rejected results do not
  update chunk indexes or packets. Full `mc-entity` passes with 101 passed and
  4 ignored; full `mc-net` passes 852/852. Strict scoped Clippy, fmt,
  code-health, and diff-check pass with four build jobs. The global authority
  mutex still blocks unrelated entity mutations during the phase, and no
  throughput/p99 gain is claimed before a profile. Workspace tests, full
  workspace Clippy, VD8, and the real-client gate remain deferred under the
  reduced CPU budget.

  Regional goal apply now reuses the same autoscaler permits as parallel
  resolve. Authority, leases, remote sources, and complete goal inputs are
  validated for every region before any mutation; accepted batches then mutate
  disjoint stores concurrently and merge `GoalTickStats` after the exact scope
  barrier. An event-driven regression proves two regions enter apply before
  either is released, and a two-region stale-input regression proves one stale
  region prevents mutation in both. Full `mc-entity` passes with 103 passed and
  4 ignored; full `mc-net` passes 852/852. Strict scoped Clippy, fmt,
  code-health, and diff-check pass with four build jobs. This closes the common
  goal mutation serialization inside the phase; the global authority mutex
  still excludes unrelated point mutations, and throughput/p99 is not yet
  measured. Workspace tests, full workspace Clippy, VD8, and real-client gates
  remain deferred under the reduced CPU budget.

  Animal breeding commit is now conditional on the complete snapshots used
  by its unlocked plan. If either parent changes identity, lifecycle, motion,
  position, goal, vehicle state, or breeding state before commit, the whole
  parent batch is rejected and no child is spawned or published. The
  event-driven session regression changes one parent at the exact
  post-snapshot boundary and proves zero births, preservation of the newer
  parent state, and no cooldown mutation on the other parent. A direct
  regional-authority regression also proves that an unrelated motion change
  rejects both parent updates without partial mutation. Full `mc-entity`
  passes with 106 passed and 4 ignored; full `mc-net` passes 854/854. Strict
  scoped Clippy, fmt, code-health, and diff-check pass with four build jobs.
  This is transaction-correctness evidence, not throughput or vanilla-oracle
  evidence. Workspace tests, full workspace Clippy, VD8, and the real-client
  gate remain deferred under the reduced CPU budget.

  The persistent regional owner runtime now moves physical stores out of the
  coordinator in focused tests. The coordinator retains leases, global
  indexes, and transfer metadata; bounded push-driven workers own the actual
  `EntityStore` values. Exact snapshot replies and a
  prepare/commit/finalize-or-rollback phase protocol preserve deterministic
  `(RegionKey, sequence)` order. A global sequence watermark fences replay
  across phases and lane reassignment. Stale peer prepare aborts already-ready
  lanes with zero mutation, unfinalized commits roll back, startup validation
  returns its physical stores, and shutdown joins all lanes before returning
  full or explicitly partial recovered state. Seven focused owner tests cover
  these paths. This remains migration scaffolding, not production multicore
  evidence: `SessionRegistry` has not handed over its live stores, and the
  global authority mutex still exists. The full scoped baseline is `mc-entity`
  111 passed/4 ignored and
  `mc-net` 854/854 with strict Clippy, fmt, code-health, and diff-check green.
  A second independent review confirmed that ordinary reject/rollback paths
  are fenced, while worker loss between commit and finalization still needs a
  durable decision journal. A dead lane's physical store cannot be recreated
  from local undo, so that failure remains a production-cutover blocker.
  Expected cutover/shutdown errors no longer panic or return inconsistent
  indexes: partial recovery retains only leases, locations, and UUIDs backed
  by stores that were actually returned.
  Workspace tests, full workspace Clippy, VD8, and the real-client gate remain
  deferred under the reduced CPU budget.

  Owner lanes now start from an empty world and accept live region installation
  plus authoritative entity spawn. Region assignment chooses the lane with the
  fewest owned regions and a stable lane-ID tie break. Spawn is an undoable
  owner mutation; coordinator ID, UUID, and location indexes publish only after
  every participating lane finalizes. An empty-world regression spawns into two
  regions, proves 1/1 lane distribution, exact snapshots, stable IDs, and
  shutdown round-trip. Remove now follows the same phase protocol: rollback
  restores the complete snapshot and successful finalize removes UUID/location
  indexes. Independent review found that insert rollback retained the physical
  store's ID watermark and that one lane batch could duplicate inserted IDs or
  UUIDs across regions. Both are fixed with direct regressions, including an
  `i32::MAX` rollback followed by allocation of ID 1. Full `mc-entity` passes
  `115 passed/4 ignored`; strict scoped Clippy, fmt, code-health, and diff-check
  pass with four build jobs.
  This removes the empty-world cutover blocker, but production still stores
  `RegionalEntityAuthority` behind the global `SessionRegistry` mutex. Batch
  reads, migration, full mutation coverage, autoscaled reassignment, durable
  crash recovery, and throughput/p99 evidence remain before a true multicore
  claim. Workspace tests, full workspace Clippy, VD8, and the real-client gate
  remain deferred under the reduced CPU budget.

  Owner snapshot reads now fan requests to all lanes before waiting for exact
  replies, merge by entity ID, and reject any batch inconsistent with
  coordinator ID/UUID/location indexes. Cross-lane animal CAS also runs through
  owners with complete expected snapshots: a stale parent aborts both lanes
  with no cooldown change, while a fresh retry commits both. Full `mc-entity`
  passes `116 passed/4 ignored`; strict scoped Clippy, fmt, code-health, and
  diff-check remain green with four build jobs. Goal, damage, kinematics and
  migration commands plus the save barrier remain before `SessionRegistry`
  can surrender its global entity mutex. `mc-net` was unchanged and not rerun;
  its latest baseline remains 854/854. Workspace tests, full workspace Clippy,
  VD8, and the real-client gate remain deferred under the reduced CPU budget.

  Owner kinematics now use complete prepared snapshots. Stale input on one lane
  aborts same-region movement on every lane; fresh input commits all accepted
  states. Standalone boundary movement conditionally removes the source snapshot
  and inserts its updated state at the target owner in one phase. A regression
  proves stale source aborts a prepared target without movement or location
  change, then a fresh retry migrates lanes and survives shutdown restoration.
  Vehicle/passenger and referenced-goal crossings remain rejected pending group
  migration. Full `mc-entity` passes `118 passed/4 ignored`; strict scoped
  Clippy, fmt, code-health, and diff-check pass with four build jobs. `mc-net`
  was unchanged and not rerun; its latest baseline remains 854/854. Damage,
  goals, vehicle groups, save barrier, SessionRegistry handoff, durable crash
  recovery, and throughput evidence remain before a true multicore claim.
  Workspace tests, full workspace Clippy, VD8, and the real-client gate remain
  deferred under the reduced CPU budget.

  Owner rollback now includes semantic event queues, not only entity snapshots.
  Each touched store checkpoints pending, published, and legacy-expected event
  lengths; rollback restores state and truncates speculative spawn/remove/damage
  events to that exact boundary. Direct regressions cover insert rollback and
  lethal damage rollback. Coordinator damage uses complete-snapshot CAS,
  returns authoritative post-finalize health/lifecycle, rejects stale retries,
  and reports lethal despawn. Full `mc-entity` passes `121 passed/4 ignored`;
  strict scoped Clippy, fmt, code-health, and diff-check pass with four build
  jobs. `mc-net` was unchanged and not rerun; its latest baseline remains
  854/854. Goal execution, vehicle-group migration, save barrier,
  `SessionRegistry` handoff, durable crash recovery, and throughput evidence
  remain before a true multicore claim. Workspace tests, full workspace Clippy,
  VD8, and the real-client gate remain deferred under the reduced CPU budget.

  Owner goal ticks now prepare directly on persistent regional lanes. Requests
  are sent to every participating owner before the coordinator waits, remote
  follow targets carry complete source snapshots, and apply uses the same
  prepare/commit/finalize protocol as other owner mutations. A changed follower
  or target makes the whole multi-lane batch stale; committed peers roll back
  kinematics and speculative semantic events instead of publishing partial AI
  movement. Fresh batches return aggregate `GoalTickStats`. Full `mc-entity`
  passes `122 passed/4 ignored`; strict scoped Clippy, fmt, code-health, and
  diff-check pass with four build jobs. `mc-net` was unchanged and not rerun;
  its latest baseline remains 854/854. Vehicle-group migration, save barrier,
  `SessionRegistry` handoff, durable crash recovery, autoscaled reassignment,
  and throughput/p99 evidence remain before a true multicore claim. Workspace
  tests, full workspace Clippy, VD8, and the real-client gate remain deferred
  under the reduced CPU budget.

  Persistent owners now expose an exact save barrier: every non-empty lane
  verifies the same finalized global sequence watermark and its complete lease
  set before returning immutable, ID-ordered snapshots. Batch restore preserves
  IDs, UUIDs, allocation watermarks, and local vehicle graphs; invalid
  cross-region graphs are rejected before any insert. Vehicle/passenger region
  crossing now removes the complete exact-snapshot group at the source and
  inserts it at the target in one owner phase. The leader's accepted delta
  determines passenger position, so speculative passenger physics cannot split
  the mount. Global locations publish only after finalize, while rollback
  restores the full source graph and semantic-event checkpoints. Full
  `mc-entity` passes `125 passed/4 ignored`; strict scoped Clippy, fmt,
  code-health, and diff-check pass with four build jobs. `mc-net` was unchanged
  and not rerun; its latest baseline remains 854/854. `SessionRegistry`
  handoff, autoscaled reassignment, durable crash recovery, and measured
  throughput/p99/multiplayer/client evidence remain before a true multicore
  claim. Workspace tests, full workspace Clippy, VD8, and the real-client gate
  remain deferred under the reduced CPU budget.

  The owner coordinator now also runs behind a bounded actor handle instead of
  requiring callers to hold a store mutex. The actor owns coordinator metadata,
  coordinator commands use exact per-request reply channels, and physical
  stores remain on persistent regional lanes. The initial typed surface covers
  spawn, point and batch reads, batch restore, remove, animal CAS, velocity
  batches, conditional physics, damage, point and batch goal changes, item-stack
  changes, position changes, atomic herd spawn, goal prepare/apply, exact save
  barriers, and joined shutdown with recovered regional state. Goal pathing
  resolves outside the actor and returns only the fenced result for owner apply. Actor
  startup transfers the coordinator only
  after its thread is created, so a thread-spawn failure returns the original
  world instead of dropping it. Full `mc-entity` passes `128 passed/4 ignored`;
  strict scoped Clippy, fmt, code-health, and diff-check pass with four build
  jobs. This is the transport needed for `SessionRegistry` cutover, not the
  cutover itself: its direct and dual entity guards still use the old global
  authority mutex until owned snapshot visitors, pickup/combat orchestration,
  and shadow telemetry are routed through the handle. `mc-net` was unchanged
  and not rerun; its latest baseline remains 854/854. Workspace tests, full
  workspace Clippy, VD8, and the
  real-client gate remain deferred under the reduced CPU budget.

  The owner handle now exposes the remaining basic cutover reads without
  lending callers a store reference: ID-filtered snapshots and a scalar status
  projection containing entity count, lane count, and shadow counters. Runtime
  construction also preserves the server entity-ID watermark. Conditional
  remove compares the complete expected snapshot on the physical owner before
  mutation, so a stale pickup or despawn attempt is rejected without changing
  coordinator ID/UUID/location indexes. Owner-native shadow comparison fans out
  to every physical lane and records one aggregate comparison. Selected reads
  validate coordinator location/UUID indexes, and removing an attached
  passenger remains an explicit vehicle-graph error rather than a false stale
  result. Full `mc-entity` passes `131 passed/4 ignored`; strict scoped Clippy,
  fmt, code-health, and diff-check pass with four build jobs.
  `SessionRegistry` now owns `RegionalOwnerRuntime` directly; the global
  `Mutex<RegionalEntityAuthority>` and its fake lock metric are gone. Gameplay
  reads and mutations route through typed actor commands while physical stores
  remain on persistent owner lanes. Partial item pickup and all full
  item/XP/arrow removals use complete-snapshot CAS. Split owner/session
  publication for player push, breeding, grazing, and hostile arrows rechecks
  exact current snapshots, so a later physics or shear commit cannot be
  overwritten by delayed publication. Selected reads fan out once per lane,
  breeding uses its owner index, UUID lookup uses the coordinator index, and
  physics reuses one batch-prefetched snapshot set instead of issuing a request
  per entity. Owner lanes can now be reconfigured live at an actor-command
  boundary. Region handoff moves the physical store, advances its lease epoch,
  rejects the old lease, and joins emptied scale-down lanes. The existing
  runtime control-plane pushes each changed CPU admission limit to both chunk
  resources and `SessionRegistry`; startup uses that automatic limit as well,
  with no entity worker percentage setting. Scale `1 -> 2 -> 1` preserves
  entities in coordinator and runtime tests, and the server control-plane test
  proves a memory-pressure decision changes chunk admission and owner lanes from
  8 to 4. Full `mc-net` passes `855/855`; strict scoped `mc-net`/`mc-entity`
  Clippy and `mc-entity` `133 passed/4 ignored` pass with four build jobs. This proves live
  reassignment correctness, not a multicore speedup. Actor failure still reaches
  the adapter as fail-fast panic; owner lanes are not yet admitted through the
  chunk semaphore, so combined throughput/p99 and oversubscription remain to be
  measured. Fmt, code-health `0 fail / KEEP`, and diff-check pass. Workspace,
  VD8, and real-client evidence are pending for this slice.

  Durable owner decisions now have a tested coordinator boundary. The journal
  receives a compact delta of touched complete snapshots and removed entity IDs
  only after every lane commits and before any lane finalizes. A journal write
  failure rolls all applied lanes back; a successful finalize clears the phase.
  The no-op backend is explicitly disabled so ordinary tests and non-persistent
  worlds do not pay extra owner reads. Full `mc-entity` passes `135 passed/4
  ignored`; strict scoped Clippy and fmt pass.

  Persistent worlds now use a versioned owner-decision JSON file that preserves
  complete entity snapshots rather than the intentionally lossy vanilla entity
  save projection. Writes use a distinct temp file, flush plus `sync_all`, atomic
  rename, and directory sync. Bind startup overlays pending upserts/removals on
  the last entity save, preserves age/pickup-delay metadata, restores the merged
  owner state, and acknowledges journal phases only afterward. The disk codec
  test round-trips custom attributes, follow-position goal, vehicle link, and
  animal state; the bind-restart test restores that pending state and proves the
  journal is then empty. Full `mc-entity` passes `135 passed/4 ignored`, full
  `mc-net` passes `857/857`, and strict scoped Clippy passes. This closes the
  process-restart journal gap, not the performance/fault-handling gate:
  synchronous durability needs p99 measurement and journal/worker errors still
  reach the session adapter through its fail-fast path.

  Explicit debug multicore evidence now separates regional CPU work from the
  durable commit boundary. On this host, 2,048 entities spread over eight
  regions and moved for 80 exact-CAS iterations measured one owner lane at
  p50/p99 `44.1/47.7 ms` and four persistent owner lanes at `22.6/28.2 ms`,
  with identical final snapshots. The regional entity path therefore has a
  measured multicore gain, although it is not yet a full-server throughput
  claim. The filesystem journal's 40-cycle local benchmark measured durable
  record p50/p99 `5.8/8.4 ms`, durable clear `2.7/4.9 ms`, and combined
  `8.5/12.0 ms`. The dominant remaining architecture bottleneck is the global
  `WorldHandle = Arc<tokio::sync::Mutex<WorldStorage>>`; journal serialization
  is a second fixed boundary. Next scaling work should partition world
  ownership by region and replace per-phase JSON rewrite/delete with an
  ordered append/group-commit design whose completion is notification-driven.
  These ignored report gates are evidence tools, not default-suite latency
  assertions; no release, soak, workspace, VD8, or real-client claim changed.

  The first world-regionalization slice removes another real shared-lock path.
  `WorldReadView` now stripes published chunks and furnace projections by the
  existing 8-by-8 chunk region boundary instead of putting every resident
  chunk behind one `RwLock<HashMap<...>>`. Each stripe keeps copy-on-write
  snapshots, eviction publication, and exact resident/dirty pressure counters.
  A push-gated concurrency regression holds one region's writer and proves an
  unrelated region read completes before that writer is released. Simulation
  `ReadBlockSnapshot` commands still pass through the ordered owner queue, but
  resident hits now capture block state plus mutation token atomically from the
  regional read stripe without acquiring `WorldHandle`; missing resident data
  falls back to the storage owner for disk/generator access. A second regression
  holds the global storage writer and proves the queued resident query still
  responds. Full `mc-world` passes `161/161`; full `mc-net` passes `858` with
  one explicit journal benchmark ignored. Strict scoped Clippy, fmt,
  code-health `0 fail / KEEP`, and focused diff-check pass. This removes the
  global lock from hot resident reads, not from block mutation, LRU, Anvil IO,
  relight publication, or save. Regional mutation ownership plus a separate
  disk/flush coordinator remains the next world-scaling boundary.

  The second world-regionalization slice removes `WorldStorage` as the
  canonical owner of resident chunks. `ResidentChunkStore` now owns each
  resident `Arc<Chunk>` behind the same 8-by-8 chunk region boundary, while
  `WorldStorage` retains LRU, generation, Anvil, and flush coordination.
  `WorldMutationView` provides a cloneable conditional resident mutation path;
  a channel-gated regression holds one region and proves a mutation in an
  unrelated region completes before release, then verifies the coordinator
  observes that exact canonical result. COW snapshots, dirty generations,
  clean-only eviction, runtime chunk identity, scheduled-work projections, and
  dirty-flush compare-and-swap behavior remain covered. Full `mc-world` passes
  `162/162`, and strict scoped Clippy passes. Simulation routing now classifies
  `ApplyBlockEdits` as regional only when every edit, precondition, and
  scheduled tick has one owner; cross-region commands retain the coordinator
  fallback. This is ownership and fencing groundwork, not a production
  world-write multicore claim: the simulation loop still executes mutations
  sequentially and assigns its scaffold routes to lane 0. The next slice needs
  one atomic regional batch API for the complete effect footprint, real
  autoscaled lane jobs, notification-driven completion, and ordered owner-side
  publication of responses and light updates.

  The third world-regionalization slice enables the first production
  multicore write path. `WorldMutationView` now commits a complete
  single-region conditional block batch under one regional lock: all
  preconditions are checked before mutation, cross-region and missing
  footprints fail closed, resulting mutation tokens are returned, requested
  scheduled ticks are added only for applied positions, adjacent leaf ticks
  are scheduled in the same commit, and light-inert edits preserve baked
  light. Five focused world regressions cover stale zero-mutation behavior,
  cross-region fencing, scheduled ticks, leaf ticks, and highest-opaque
  maintenance. Production simulation passes resident light-inert
  `ApplyBlockEdits` through the shared autoscaler CPU admission instead of the
  global world mutex. Commands are grouped by stable regional lane, execute in
  sequence within a lane, execute concurrently across lanes, and publish
  responses and block deltas in original command order. A channel-release
  regression proves two regions enter different jobs before either is
  released; another proves same-region CAS ordering yields one commit followed
  by one stale rejection. A production-like test holds the global world writer
  while a light-table-classified inert edit completes. Full `mc-world` passes
  `167/167`; full `mc-net` passes `862` with one explicit benchmark ignored;
  strict scoped Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass.
  This is a real but deliberately bounded write result: light-changing edits,
  mixed command runs, survival/container mutation families, relight
  persistence, LRU/Anvil IO, and save still use the coordinator. The next
  scaling slice is regional light-source capture and conditional baked-light
  publication, then contiguous regional waves around coordinator barriers.

  The fourth world-regionalization slice admits ordinary light-changing
  `ApplyBlockEdits`, including break/place opacity changes, to those autoscaled
  regional jobs. Each worker commits through `WorldMutationView`, captures the
  immutable 3x3 light-source neighborhood, and computes incremental baked-light
  updates without holding `WorldHandle` or blocking the simulation ticker. The
  owner then verifies source chunk identities before a short serialized
  publication; a stale source falls back to recomputing against the current
  world instead of publishing obsolete light. The distinct-region regression
  now uses real stone-to-air opacity changes, proves both regional jobs enter
  before either is released, and verifies baked light reaches both canonical
  chunks. Full `mc-world` remains `167/167`; full `mc-net` remains `862` active
  tests passed with one explicit benchmark ignored; strict scoped Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. Heavy relight compute is now
  multicore, but baked-light publication, mixed command runs,
  survival/container and scheduled mutation families, LRU/Anvil IO, journal,
  and save are still coordinator-serialized. The next Pareto slice is survival
  break/place routing and contiguous regional waves around coordinator
  barriers, not a wholesale rewrite of already immutable read paths.

  Contiguous regional waves now survive mixed world-command batches. Routing
  is decided per envelope before execution, adjacent eligible block edits are
  grouped into one autoscaled regional wave, and each non-regional command is
  retained as an ordered coordinator barrier. A push-gated regression queues
  two distinct regions followed by a chest-read barrier, holds the global
  world writer, proves both regional lanes enter before either is released,
  observes both edits, and only then releases the writer for the barrier. The
  test probe was also made deterministic: worker identity arrives on the
  entered channel while release uses interchangeable tokens, avoiding a race
  where workers consumed each other's region IDs. Full `mc-net` now passes
  `863` active tests with one explicit benchmark ignored; strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass. Survival
  break/place remains the next high-value mutation family; this slice removes
  the batch-wide fallback that previously hid available regional parallelism.

  Survival placement now participates in those autoscaled regional waves.
  Routing requires the complete edit, precondition, and scheduled-block-tick
  footprint to have one resident owner. Before worker dispatch the owner
  captures a narrow cloneable handle to that player's persistence record;
  inside the worker one player mutex protects inventory validation/debit while
  `WorldMutationView` performs the conditional regional block commit. No
  global session/entity mutex or `WorldHandle` is held for the transaction.
  Leaf and requested block ticks remain part of the regional commit. Dry
  placements skip global world access entirely; placements touching fluid use
  a short owner-side scheduling pass, while light-changing placements reuse
  worker-side immutable light capture/compute and conditional baked-light
  publication. One regression holds the global writer and receives a complete
  dry block+inventory transaction. Another uses real air-to-opaque changes for
  two players in distinct regions, proves both lanes enter before either is
  released, and verifies inventory debits plus precomputed light publication.
  Full `mc-net` passes `865` active tests with one explicit benchmark ignored;
  strict scoped Clippy, fmt, code-health `0 fail / KEEP`, no-sleep search, and
  diff-check pass. Survival break still uses the coordinator because its root
  transaction additionally owns drops, falling-block follow-up, campfire
  state, and fluid scheduling; that is the next regional mutation slice.

  Survival break now uses the same autoscaled regional transaction path for
  both prepared plans and the real `commit_survival_block_break` request used
  by gameplay. The break planner no longer requires mutable `WorldStorage`;
  door halves, cactus/sugar-cane/bamboo cascades, fluid replacement, mutation
  tokens, and deterministic loot are planned from one immutable regional
  snapshot. A narrow player handle keeps tool validation/damage atomic with
  the conditional regional world commit. The worker then plans and commits
  falling-block removal against the post-commit read view, calculates relight,
  and reports deterministic follow-up data. The owner preserves publication
  order: block/light deltas first, then item drops and falling entities;
  campfire cooking is cleared before the response. Fluid ticks and baked-light
  persistence acquire the global world only when those effects actually
  exist. A held-writer regression covers the real block-break request with
  tool damage and drop. A two-player opacity-changing regression proves
  distinct regions enter concurrently and verifies both inventories, drops,
  and precomputed light. The regional requester-loss regression additionally
  covers adjacent water scheduling, falling sand, baked light, and drop
  durability; a stale-root regression proves zero tool/drop mutation; the
  campfire regression also runs through the regional API. Full `mc-net` passes
  `867` active tests with one explicit benchmark ignored; strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep search, and diff-check
  pass. Survival block edits are no longer a coordinator-only family;
  containers, other scheduled mutation families, baked-light publication,
  LRU/Anvil IO, journal, and save remain.

  Bucket use and detached fluid scheduling now avoid coordinator world
  mutation. Bucket workers use a narrow player handle to atomically validate
  and replace the held bucket with the one-block regional CAS. Fluid ticks are
  published after a successful commit, and opacity/emission changes reuse the
  regional relight pipeline. A two-player regression proves distinct bucket
  regions enter concurrently before either worker is released, then verifies
  both inventory replacements, scheduled fluid ticks, and precomputed light;
  requester-loss and stale-token regressions also run through the regional
  API. The generic `ScheduleFluidTicksNearApplied` command now plans ticks from
  `WorldReadView` and writes them through `WorldMutationView` without a global
  world lock. Its scheduler batches by region and chunk, producing one COW
  update and scheduled-work publication per touched chunk. A held-writer
  regression proves the detached command completes and the canonical tick is
  visible afterward. Full `mc-world` passes `167/167`; full `mc-net` passes
  `869` active tests with one explicit benchmark ignored; strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep search, and diff-check
  pass. Shared containers and block entities are now the next broad gameplay
  mutation family; baked-light persistence, LRU/Anvil IO, journal, and save
  remain coordinator-owned.

  Autonomous regional `ApplyBlockEdits` now has a crash-safe chunk WAL without
  reintroducing the global world lock. Decision IDs are durably reserved before
  workers start and never reused after restart. Journaled mutations stamp the
  exact direct, scheduled, and derived chunk footprint under the resident
  region lock. Those chunks remain fenced out of dirty flush until the ordered
  group append reaches `sync_all`; append failure leaves them dirty but
  unflushable and requests controlled shutdown. Startup replay applies only an
  image newer than the resident/disk chunk LSN, so an old pending image cannot
  overwrite a later saved non-WAL update. Corrupt CRC tails fail closed, while
  only incomplete final writes are truncated. All CPU permits are acquired
  before any lane starts, and reservation, append, and checkpoint filesystem
  waits run off the async owner. Full `mc-world` passes `181/181`; full `mc-net`
  passes `898` active tests with one explicit benchmark ignored; strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search, and diff-check
  pass. This durability contract currently covers autonomous chunk edits, not
  compound survival/container player+world transactions; those need one
  combined recovery record rather than a chunk-only snapshot.

  Entity authority is already backed by persistent regional owner lanes, not
  by one caller-held global entity mutex. The remaining shared cost is the
  coordinator actor and the tick phases that still make repeated round trips
  through it. The breeding path now caches its batch snapshot, commits parent
  state in one request, spawns children in one batch, and revalidates parent
  and child publication together after releasing the session mutex. Its
  cross-region regression records `6` owner requests instead of `8`. The
  hostile goal/physics path now reuses the active-entity prefetch and no longer
  asks for a redundant owner status. Goal apply also returns the committed
  active snapshots in the same coordinator command instead of requiring a
  second synchronous coordinator round trip; its regression now records `4`
  requests instead of `7`, with deterministic entity order retained. Skeleton arrows
  are now spawned and fetched as one volley rather than one coordinator round
  trip per arrow: the two-skeleton regression records a constant `4` owner
  requests instead of `6`, and larger volleys no longer add spawn/snapshot
  requests per arrow. Sheep grazing now also commits a whole group at once:
  both movement stop and wool/age mutation use one conditional regional batch,
  deduplicate input IDs, and fetch the committed group once. The two-sheep
  start and finish regressions each record a constant `4` owner requests
  instead of `7`. Full `mc-entity` passes `140` active tests with five explicit
  long/benchmark gates ignored. Full `mc-net` passes `907` active tests with one explicit
  benchmark ignored; strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search, and diff-check
  pass. This proves less coordinator traffic, not linear multicore scaling:
  coordinator-mediated cross-region phases and compound transactions remain,
  and no client, soak, or throughput gate was run for this slice.

  Regional mutation barriers now include only lanes with real mutations.
  Local work no longer pushes empty prepare/commit/finalize messages through
  every configured owner lane. The coordinator sends work to all selected
  lanes before receiving any completion, so one cross-region batch retains
  concurrent lane execution and atomic rollback. Per-lane sequence watermarks
  may now trail the global coordinator sequence; save accepts that finalized
  sparse history while rejecting any impossible ahead-of-coordinator lane. A
  real two-lane coordinator regression proves that a west-only mutation sends
  one prepare to west and zero to idle east, and the existing stable-save
  regression covers a later save after local-only work. Full `mc-entity`
  passes `138` active tests with five explicit long/benchmark gates ignored;
  full `mc-net` passes `901` active tests with one explicit benchmark ignored.
  Strict scoped Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search,
  and diff-check pass. Independent commands still serialize through one
  coordinator actor, and no client, soak, or throughput gate was run.

  Two remaining `O(world)` coordinator scans are removed from common entity
  work. The coordinator now tracks vehicle-to-passenger topology explicitly.
  Conditional kinematics reads only the connected vehicle components touched
  by the request; when the active set already covers the whole world it keeps
  the old dense all-snapshot path. Plain arrow, animal, and other non-mounted
  batch spawns skip existing-world graph scans entirely, while mounted batches
  retain full graph validation. Exact two-lane regressions prove that west-only
  kinematics sends zero batch reads east and that a plain west spawn batch
  reads neither existing lane. Full `mc-entity` passes `140` active tests with
  five explicit long/benchmark gates ignored; full `mc-net` passes `901`
  active tests with one explicit benchmark ignored. Strict scoped Clippy, fmt,
  code-health `0 fail / KEEP`, no-sleep API search, and diff-check pass. The
  bounded two-CPU debug benchmark now covers both modes. With `2,048` resident
  entities, moving all `2,048` measured p50 `45.1/40.3 ms` for one/two lanes;
  moving `512` entities in two of eight regions measured `11.2/10.1 ms` and
  p99 `11.8/11.2 ms`. This is current-head evidence that work follows the
  active set, not a production throughput or before/after speedup claim. Client,
  soak, workspace, and VD8 were not run.

  Warm point and ID-filtered entity reads now bypass the coordinator actor and
  fan directly to the persistent owner lanes through coordinator-published
  `(entity, UUID, lease, lane)` routes. Lane reads reject pending or committed
  but unfinalized phases. An actor-wide even/odd entity-state version rejects a
  multi-lane result when any mutation overlaps it, forcing an authoritative
  coordinator retry instead of returning snapshots from opposite sides of one
  transaction. Region migration falls back and refreshes the route; live lane
  reconfiguration clears old channel generations. Direct admission is capped
  at 16 concurrent batches and uses non-blocking lane enqueue, reserving at
  least 48 of each 64-slot lane queue for owner protocol traffic. Push-gated
  regressions prove reads from two lanes complete while the actor is held,
  overlapping two-lane mutation returns one coherent post-mutation set,
  unfinalized state is hidden, migration refreshes the route, and scale-up/down
  clears it. A cold point read now publishes the same route, and a push-gated
  regression proves its next `snapshot(entity)` completes while the actor is
  held. Full `mc-entity` passes 146 active tests with five explicit
  ignored gates; full `mc-net` passes 908 active tests with one explicit
  ignored benchmark. Strict scoped Clippy, fmt, code-health `0 fail / KEEP`,
  no-sleep API search, and diff-check pass. The final two-CPU debug run measured
  dense p50 `45.6/40.5 ms`, p99 `49.4/45.3 ms`, and 512-active p50
  `11.5/10.3 ms`, p99 `12.3/11.1 ms` for one/two lanes. Mutation commands,
  first/invalidated reads, and cross-region commits still serialize through
  the coordinator; client, VD8, soak, and production throughput were not run.

  Common standalone entity movement now avoids a second coordinator-side
  snapshot read and sends one exact-CAS mutation/sequence per touched region,
  rather than one mutation per entity. The owner lane still validates the
  complete regional batch before applying it, and prepare, durable journal,
  rollback, commit, and finalize behavior is unchanged. Mounted entities,
  passengers, and region crossings retain the full topology path. A regression
  with two entities proves zero repeated owner-lane snapshot reads and one
  sequence for the region; stale cross-lane CAS, standalone migration, and
  vehicle-group migration regressions also pass. Full `mc-entity` passes 147
  active tests with five explicit ignored gates; full `mc-net` passes 909
  active tests with one ignored benchmark. Strict scoped Clippy passes. The
  bounded SMT-sibling debug benchmark improved from dense p50/p99
  `45.6/49.4 ms` and `40.5/45.3 ms` to `19.052/20.926 ms` and
  `16.614/18.067 ms` for one/two lanes. The 512-active case improved from
  `11.5/12.3 ms` and `10.3/11.1 ms` to `4.755/5.131 ms` and
  `4.340/4.883 ms`. This proves a roughly 58% common-batch latency reduction on
  the same topology, not multicore scaling. The old affinity `0,1` is two SMT
  threads of one physical core; the ignored benchmark now rejects that setup
  when testing two lanes. On physical cores `0,2`, dense one/two-lane p50/p99
  is `19.388/21.276 ms` and `11.996/19.823 ms`; the 512-active case is
  `4.901/5.716 ms` and `3.503/4.769 ms`. This proves lane-level multicore gain
  for batched entity movement. Parallel p99 remains noisy while a game competes
  for CPU under `nice`; client, soak, workspace, quiet-host p99, and production
  throughput gates were not run for this slice.

  Campfire multi-slot audit against local 26.1.2 bytecode confirmed that the
  current state, NBT, ticking, and drop paths already model four parallel slots
  in index order; the old single-slot review finding was stale. Two remaining
  visible divergences are fixed. An unlit campfire now reduces each slot's
  cooking progress by two per simulation tick, clamps at zero, and persists the
  cooled `CookingTimes` through the same resident block-state/token CAS while
  retaining every input. A valid campfire food interaction against four full
  slots is consumed without debiting inventory or emitting block/entity data;
  it sends only the sequence acknowledgement instead of falling through to
  generic placement/use. Unit regressions cover independent cooldown across all
  four slots and exact full-interaction wire output. The raw-TCP unlit test now
  proves `CookingTimes[0] == 0` after its simulation-tick fence while the raw
  porkchop remains in `Items`. All 21 focused campfire tests, full `mc-net`
  `911 active/1 ignored`, and all 81 block-edit wire tests pass. Scoped strict
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search, and diff-check
  pass. Recipe re-resolution after datapack reload and exact retained timing
  arrays for completed empty slots remain parity details; workspace, real
  client, soak, and production load gates were not run.

  Vertical-plant random ticks no longer let every lower segment grow the
  column top. Only the selected top segment can grow, removing a height-times
  amplification of cactus, sugar-cane, and bamboo growth chances. The bamboo
  path was checked against the bundled 26.1.2 `BambooStalkBlock` and
  `BambooSaplingBlock` bytecode and now applies the one-in-three chance,
  16-block cap, age cascade, small/large leaf crown, terminal stage, and
  two-block sapling transition as one conditional edit set. The wire gate now
  accepts every registry state belonging to `minecraft:bamboo`, because a new
  vanilla-shaped segment has `leaves=small` rather than the default state ID.
  Full `mc-net` passes `916 active/1 ignored`; the focused visible vertical
  plant wire test passes. Scoped strict Clippy, fmt, code-health `0 fail /
  KEEP`, no-sleep API search, and diff-check pass. Brightness gating and exact
  Java RNG sequence parity remain open; this slice preserves the checked
  probabilities but does not claim oracle-identical random sequences. The full
  block-edit suite, workspace, real client, soak, and production load gates
  were not run.

  Regional mutation profiling rejects direct actor bypass as the next scaling
  move. A retained ignored benchmark compares the same 512-entity batch through
  raw ECS, a directly called coordinator, and the production actor. On physical
  cores `0,2`, current-head debug p50 is `1.418/4.017/3.420 ms`; actor scheduling
  is inside host-load noise and is not the dominant transaction cost. Local
  mutations touching one owner lane now fuse preflight and apply into one
  owner message, while durable journal recording remains before finalize and
  both safe journal failure rollback and unknown-outcome fail-stop behavior are
  unchanged. Multi-lane and cross-region transactions keep separate prepare
  and commit barriers. On physical CPU `0`, the fused 512-entity actor path
  measured p50/p95/p99 `4.491/4.560/4.885 ms`; this is current-head debug
  evidence, not a quiet-host before/after or production throughput claim. Full
  `mc-entity` passes `147 active/6 ignored`; full `mc-net` passes `916 active/1
  ignored`. Scoped strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API
  search, and diff-check pass. Workspace, client, VD8, soak, and production load
  gates were not run.

  Random-tick planning now reads immutable published chunk snapshots and the
  common one-region commit uses exact state/token CAS against the resident
  regional store. With persistence enabled, the changed chunk image is written
  to the world journal before block deltas, light updates, or leaf drops are
  published; an incomplete or failed append requests controlled shutdown
  instead of falling back after an uncertain mutation. A regression holds the
  global `WorldStorage` writer for the entire operation and proves that the
  resident commit still completes, while a second test reopens the journal and
  recovers the post-growth block state. Exact stale-CAS, repeated-sample order,
  and deterministic leaf-drop tests remain green. Full `mc-net` passes `917
  active/1 ignored`; strict scoped Clippy, fmt, and code-health `0 fail / KEEP`
  pass. A plan whose read or edit footprint crosses an 8x8 region still uses
  the old coordinator fallback. Grouped multi-region commit, workspace,
  real-client, soak, and production-load gates were not run.

  Sheep grazing now uses the same durable resident block transaction for its
  common one-region action path. Planning reads the published snapshot,
  duplicate sheep targeting one food block retain first-candidate order, and
  only the entity whose exact block edit was applied completes grazing and
  regrows wool. The action regression holds the global `WorldStorage` writer
  while the block and sheep state change, so an accidental coordinator fallback
  fails immediately instead of being hidden by timing. Cross-region action
  batches retain the old ordered fallback. Full `mc-net` remains `917 active/1
  ignored`; strict scoped Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API
  search, and diff-check pass. Workspace, client, soak, and load gates were not
  rerun.

  Scheduled fluid ticks no longer dequeue or commit their common one-region
  path through the global world writer. Due ticks are selected as an exact
  prefix from immutable loaded-chunk snapshots. One resident transaction
  verifies that prefix and every block state/token, removes the consumed ticks,
  applies flow edits, schedules deterministic follow-up fluid and adjacent leaf
  ticks, and journals the complete post-mutation chunk images before publishing
  effects. A stale plan leaves the original due tick untouched, so no guessed
  retry or drain/requeue window exists. Held-writer and WAL-reopen regressions
  cover the common path; a region-edge regression covers exact coordinator
  fallback without tick loss or duplication. Full `mc-world` passes `183/183`;
  full `mc-net` passes `919 active/1 ignored`. Strict scoped Clippy, fmt,
  code-health `0 fail / KEEP`, no-sleep API search, and diff-check pass.
  Multi-region parallel fluid batches, workspace, client, soak, and production
  load gates were not run.

  Scheduled block ticks now use the same resident transaction for the common
  one-region button, leaf, and stale-tick path. Due ticks are selected as an
  exact immutable-snapshot prefix; one regional CAS verifies the queue prefix
  and block state/tokens, consumes the ticks, applies edits, schedules adjacent
  leaf ticks, and journals the complete post-mutation chunks before publishing
  effects. A stale plan leaves the due queue unchanged. Hopper tick backfill
  now updates resident chunks directly. A held-global-writer regression proves
  that a button release completes without coordinator fallback, and a WAL
  reopen regression recovers both the released state and consumed queue entry.
  A hopper-only same-region batch now consumes each exact due tick together
  with its hopper, chest, and furnace changes and next scheduled ticks in one
  resident transaction. The complete changed chunk set receives one journal
  decision and flush fence before container updates are dispatched. All
  independent hopper commits in one scheduled pass share that decision and one
  WAL append instead of fsyncing per hopper. Held-writer WAL-reopen regressions
  cover both cooldown and a hopper-to-chest transfer; a two-hopper regression
  proves one pending decision for the pass.
  Comparator-containing batches and hopper transfers crossing an 8x8 region
  boundary still exact-claim through the global coordinator. Reserved journal
  IDs rejected by stale, missing, or cross-region preflight now append durable
  empty decisions, so one rejected CAS cannot make the next valid append fail
  with a reservation gap. All 15 focused hopper tests, full `mc-world`
  `186/186`, and full `mc-net` `924 active/1 ignored` pass. Strict scoped
  Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search, and diff-check
  pass. Workspace, client, VD8, soak, and production-load gates were not run.

  Active furnace ticks keep their existing exact resident block-state and full
  furnace CAS, but now also participate in the world journal. Every successful
  furnace in one simulation pass receives one shared decision LSN; the final
  unique changed-chunk snapshots are appended once before viewer slot/data
  updates are dispatched. A stale furnace closes the first wave durably,
  replans from the current resident pair, and retries in the next wave without
  double-decrementing burn progress. Held-global-writer WAL reopen tests cover
  one active furnace and a two-furnace pass; the latter proves one pending WAL
  decision and recovery of both `burn_remaining` values. A stale no-op plus
  retry test recovers exact `9 -> 8` progress through two ordered decisions.
  Six focused furnace tests, full `mc-world` `186/186`, and full `mc-net` `924
  active/1 ignored` pass. Strict scoped Clippy, fmt, code-health `0 fail / KEEP`,
  no-sleep API search, and diff-check pass. Workspace, client, VD8, soak, and
  production-load gates were not run.

  Active campfire cooking now uses the same resident journal wave. Each lit or
  unlit campfire keeps its exact block-state/token CAS under the campfire
  session lock, stamps the changed resident chunk with one decision shared by
  the simulation pass, and appends the final unique chunk images once before
  publishing block-entity updates or cooked item drops. Cold campfire chunks
  remain unloaded. A two-campfire WAL-reopen regression proves one pending
  decision and recovery of both exact cooking states; all 21 focused campfire
  tests pass. A process crash after the world WAL append but before the cooked
  item entity spawn can still lose that completed output, so exactly-once item
  drop recovery is not claimed. Full `mc-world` passes `186/186`; full `mc-net`
  passes `924 active/1 ignored`. Strict scoped Clippy, fmt, code-health `0 fail /
  KEEP`, no-sleep API search, and diff-check pass. Workspace, client, VD8, soak,
  and production-load gates were not run.

  Ordinary scheduled block ticks spanning several independent 8 by 8 regions
  no longer fall back as one cross-region batch to the global `WorldStorage`
  writer. Due-order is split into contiguous regional waves. Every accepted
  group consumes its exact queue prefix and applies its block edits under the
  resident region lock; all groups before the next coordinator barrier share
  one world-journal decision and append. A group whose actual edit footprint
  crosses a region boundary durably finishes the preceding resident wave,
  commits through the existing coordinator path, and only then allows the next
  resident wave to start. Tests prove two independent button regions complete
  while the global writer is held, recover both chunks from one WAL decision,
  and preserve exact `resident A -> coordinator boundary -> resident B` order.
  Distinct groups now execute concurrently under autoscaler CPU admission while
  sharing the wave's one WAL decision; a one-CPU limit keeps execution inline.
  Worker-entry events prove two regional jobs overlap before either is released.
  Repeated order such as `A, B, A` remains sequential but replans each group from
  current resident state, so no due tick slips to the next server tick. Boundary
  coordinator commits now use ordered decision waiting, pre-stamping, exact
  flush fences, and post-mutation snapshots; the recovery test restores the
  button and both door blocks from one decision. Focused scheduled tests pass
  `33/33`; full `mc-net` passes `935 active/1 ignored`. Strict scoped Clippy and
  fmt pass. Workspace, client, VD8, soak, performance, and production load
  remain unrun.

  Random ticks no longer submit every active region as one resident batch that
  immediately becomes `CrossRegion` and falls back wholesale to the global
  `WorldStorage` writer. Contiguous groups preserve global sample order and
  original RNG indexes; each group is planned from the read view published by
  the previous commit. A conservative four-block boundary belt keeps known
  neighbour scans out of interior batches, and exact edit/precondition
  preflight catches any wider footprint. Accepted interior groups share one
  resident journal wave. A real boundary footprint durably closes that wave,
  then commits and records its stamped coordinator snapshots under one new WAL
  decision before block or drop publication. Leaf drops are retained only from
  the exact plan whose source edit applied. Tests prove `interior -> boundary ->
  interior` planning order, sequential `age 0 -> 1 -> 2`, two independent crop
  regions recovered from one WAL decision, and one exact-boundary coordinator
  crop recovered from one decision. Full `mc-world` passes `186/186`; full
  `mc-net` passes `934 active/1 ignored`. Coordinator snapshots now install an
  exact prospective flush fence before mutation without decreasing an existing
  LSN. Reserved decisions may append in ordered prefixes. If pre-stamp observes
  a newer decision, the old empty decision is closed, the global writer is
  released, and the retry waits for its exact append turn through a journal
  notification before locking and revalidating again; no coordinator edit has
  happened yet. Checkpoint poison and writer closure wake those waiters. Strict
  scoped Clippy, fmt, code-health `0 fail / KEEP`, no-sleep API search, and
  diff-check pass. Distinct owner regions now fan out through autoscaler CPU
  permits and share one WAL decision; a one-CPU limit remains inline. Exact
  worker-entry events prove two crop regions overlap before either release.
  Candidate indexes preserve RNG order, while repeated regions and boundary
  groups remain sequential and replan from current state. Fanout pre-stamps and
  fences the complete loaded edit/leaf footprint before mutation, drains all
  lanes on failure, appends every stamped post-state, and publishes drops only
  after durability. Exact owner-preflight red/green fails when sample ownership
  is weakened and passes with the current check. Focused random-tick tests pass
  `34/34`; full `mc-world` passes `188/188`; full `mc-net` passes `936 active/1
  ignored`. Strict scoped Clippy and final read-only review are clean. Workspace,
  client, VD8, soak, performance, production load, and exactly-once leaf-drop
  entity recovery were not run or claimed.

  The first ordinary entity mutation no longer waits behind the global
  coordinator actor. A warm `set_item_stack_if_current` route commits directly
  to its persistent regional owner lane with shared phase/sequence allocation,
  exact post-state journaling, finalize, and safe rollback. Same-lane admission
  preserves command order; distinct direct lanes do not share that lock.
  Save/reconfigure/shutdown exclude in-flight direct commits, and selected
  reads reject every overlapping writer through an active-writer count plus
  version check. Focused direct-CAS tests cover actor bypass, durable post-state
  and save-phase visibility, and safe journal rollback. Full `mc-entity` passes
  `150 active/6 ignored`; strict `mc-entity` Clippy, fmt, and code-health `0
  fail / KEEP` pass. The existing two-region 512-entity debug benchmark on
  physical CPUs `0,2` reports raw ECS p99 `1.655 ms`, direct coordinator p99
  `5.243 ms`, and actor p99 `4.461 ms` across 80 iterations. This benchmark
  exercises the older batch transaction path, not the new point CAS, and does
  not justify blaming the actor hop for its tail. Other entity mutations,
  point-mutation load p99, workspace gates, soak, and client validation remain
  pending. Final read-only review found and then verified fixes for speculative
  coordinator reads and selected-read version ordering; no high/medium finding
  remains.

  Cached same-lane animal CAS batches now use the same direct durable owner-lane
  protocol as item stacks. This covers per-tick age/love timer updates when a
  batch belongs to one owner, plus singleton feeding and shearing updates;
  cross-lane breeding batches keep coordinator atomicity. Exact route
  validation runs under the mutation gate, duplicate IDs fail before lane
  mutation, safe journal failure rolls back the whole batch, and one successful
  batch produces one decision containing every post-state. Actor-hold,
  two-entity durability, and two-entity rollback regressions pass. The bounded
  two-physical-CPU diagnostic with two simultaneous 64-entity lane batches
  measured direct/actor p50 `1.669/2.664 ms` and p99 `3.182/4.428 ms`; a single
  128-entity batch measured p50 `3.082/2.540 ms` and p99 `3.374/3.430 ms` after
  exact post-state capture became mandatory. The gain comes from independent
  lane overlap; one isolated lane does not become cheaper.
  Full `mc-entity` passes `154 active/7 ignored`; full `mc-net --lib` passes
  `936 active/1 ignored`; strict `mc-entity` Clippy and fmt pass. Final review
  found and verified the stale-route gate fix, with no remaining high/medium
  finding. Workspace/client/VD8/soak/production load remain unrun.

  Hostile targeting no longer sends unchanged goals through the entity actor.
  The current `EntityView.goal` travels with each hostile candidate; wander,
  follow-position, and in-range idle results are diffed before `set_goals`, and
  an empty diff emits no owner command. The in-range melee regression now
  proves the same idle behavior with `3` owner requests instead of `4`, while a
  focused helper regression covers equal and changed goals. Full `mc-net --lib`
  passes `937 active/1 ignored`; strict `mc-net` all-target Clippy and fmt pass.
  This removes redundant writes but does not yet bypass the coordinator for
  changed referenced goals. Workspace/client/VD8/soak/load remain unrun.

  Combat damage now uses the cached snapshot-CAS lane path instead of the
  ID-based actor command. The session preserves its existing invulnerability
  check, then submits that exact snapshot; the durable helper returns the exact
  post-hit snapshot captured before finalize, including lethal `Despawning`
  state. Safe journal failure restores health, while entity removal and global
  indexes remain coordinator-owned. Actor-hold, safe rollback, lethal result,
  hurt-invulnerability, and concurrent same-snapshot regressions pass; two
  simultaneous hits apply exactly one CAS and return that exact post-state.
  Full `mc-entity` passes `158 active/7 ignored`; full `mc-net --lib` passes
  `937 active/1 ignored`; strict
  scoped Clippy and fmt pass. Workspace/client/VD8/soak/load remain unrun.

  Changed same-lane goals that do not reference another entity now bypass the
  coordinator actor after a full snapshot scan has published exact cached
  routes. The direct batch revalidates snapshots and leases under entity
  mutation admission, commits one owner lane, journals exact post-state, and
  preserves rollback/fail-stop behavior. Duplicate IDs, cache misses,
  cross-lane batches, and `FollowTarget` retain the coordinator path. An
  actor-hold regression proves that the production-shaped full snapshot scan
  warms this route. Full `mc-entity` passes `159 active/7 ignored`; full
  `mc-net --lib` remains `937 active/1 ignored`; fmt passes. Scoped Clippy,
  code-health, workspace/client/VD8/soak/load are pending for this slice.

  Autonomous entity goals, kinematics, and random block ticks now avoid a
  synchronous per-tick WAL fsync and converge at the ordered periodic save
  barrier. Random leaf decay and the item drops it creates share that same
  checkpoint boundary, so a crash cannot restore the leaf while replaying its
  drop. Player-driven, combat, scheduled, and other crash-critical mutations
  remain immediately journaled. Periodic checkpoints now capture and flush
  dirty world chunks through the existing simulation-owner snapshot path;
  the disk-reopen regression proves the flushed block state, entities, and
  metadata come from one ordered barrier. Full `mc-entity` passes `159 active/7
  ignored`; full `mc-net --lib` passes `939 active/1 ignored`; strict scoped
  Clippy and fmt pass. A constrained real-client run completed the natural
  wood-to-tool path and 20 minutes without a server/client error; `20/21,600`
  observed ticks exceeded 50 ms (`99.91%` within budget), with the rare bursts
  inflating all CPU stages under external contention. The gate itself failed
  because its AFK outdoor player was killed by a skeleton/zombie/spider group
  and the scenario mislabeled the resulting stopped player tick counter as
  `tick_progress=false`; restart/rejoin therefore did not run. A focused
  wheat run then crossed repeated periodic world flushes without save-induced
  tick stalls and reached natural crop growth. Its overly strict `sky == 15`
  assertion was replaced by the vanilla crop-light threshold
  `max(sky, block) >= 9`; Java-agent tests pass. The repeat remained stable
  through ten minutes of periodic flushes but timed out before all three crops
  naturally matured, so a corrected bounded active-survival gate remains
  pending.

- Client automation now accepts a crafting-table target using the same
  vanilla full-block predicate as 26.1.2: the state must survive and
  `Level.isUnobstructed(..., CollisionContext.placementContext(player))` must
  pass. The former three-open-sides and neighbouring-fluid filters no longer
  reject legal table placement, while an intersecting sheep still rejects the
  target. The scenario waits for the exact observed table block even when the
  immediate client use result says `Fail`, and never sends the open interaction
  when that block is absent. Focused Java tests, full bridge/java-agent tests,
  and Fabric compilation pass. One constrained 180-second P14 retry used the
  verified compiled-classes client adapter and progressed into the natural
  night wait, but timed out with world time near `3004`, below the required
  `12542`; the 12-minute natural-night proof was intentionally not weakened or
  rerun. Artifact:
  `.analysis/real-client-runs/20260717T185230Z-real-client-playable-loop`.
- Campfire death recovery now binds the dropped wooden pickaxe to its exact
  client entity ID and UUID, requires the local-player take packet, exact
  disappearance, and inventory `+1`, and rejects entity-ID reuse or unrelated
  pickups. The take-event store is bounded to 64 entries and clears on
  disconnect. Focused Java tests and Fabric compilation pass; no fresh P20
  real-client run was spent.
- The embedded fallback recipe set appends the inspected vanilla shapeless
  recipe `1 bone -> 3 bone_meal` without shifting existing display IDs.
  `mc-data --lib` passes `135/135`, including the focused recipe test. A full
  sapling/bonemeal survival client path remains pending.
- Chunk herd creation and pending-hostile activation no longer hold the global
  session registry across the regional owner journal commit. They claim and
  snapshot under the session lock, commit one deduplicated authoritative batch
  outside it, then publish the exact committed snapshots after reacquiring the
  current sessions. Focused owner/pending/herd tests pass; the pre-collision
  checkpoint had `mc-entity 168 passed/7 ignored` and `mc-net 970 passed/1
  ignored`.
- Entity checkpoint acknowledgement now records the decision sequence for each
  pending journal phase and acknowledges only phases at or below the captured
  entity snapshot watermark. A push-gated concurrent append regression proves
  the newer `FollowPosition` decision is absent from the older checkpoint and
  remains in the append-only journal after compaction; the matching disk
  retention regression also passes.
- Dirty world persistence now has a per-server push coordinator instead of a
  process-global save mutex. Successful published `dirty_generation` changes
  notify the coalescing worker at the common world-storage boundary; rejected
  or unchanged mutations do not notify. Weak producer handles avoid lifecycle
  retention, and the mutex-protected drain processes a mutation arriving during
  an active flush before stopping. Focused world notification, real fluid-tick,
  stale-edit, coordinator, independent-server, and shutdown-trigger tests pass.
- Oracle extraction from the bundled vanilla 26.1.2 server now provides 5,436
  state mappings for farmland, slabs, fences, and stairs, deduplicated to 44
  collision shapes. Extractor `--check` is byte-identical. `mc-physics 25/25`
  proves farmland height, bottom slabs, over-height fences rooted below the
  body, and composed stair traversal. Production entity sampling and AI pathing
  now consume those exact boxes. Runtime regressions prove movement beside an
  isolated fence, collision with its over-height center, exact fence-top
  support, bottom-slab and directional-stair support, and preserved fluid
  behavior at `MIN_Y`. Final `mc-data --lib` passes `136/136`; final
  `mc-net --lib` passes `986 active/1 ignored`.
- Oak growth now accepts the exact 53 unique blocks flattened from vanilla
  26.1.2 `replaceable_by_trees`, including leaves, flowers, vines, and water,
  while a missing canopy chunk or solid obstruction rejects the whole edit
  batch. The raw-TCP survival test observes the actual `/give` inventory slot,
  moves bone meal through the normal container-click path when needed, waits
  for its exact consumption packet, and proves an existing canopy leaf is
  replaced. Sapling unit tests pass `10/10`; the exact harness test passes
  `1/1`. Scoped all-target strict Clippy, fmt, code-health `0 fail / KEEP`,
  and diff-check pass. No fresh real-client, full-workspace, VD8, or soak gate
  was run for this short delegated wave.
- Chunk-stream biome lookup now derives section and local Y from the owning
  `ChunkGeometry` instead of Overworld constants. A `0..256` regression
  proves both boundaries and the top biome section, while the default
  `-64..320` mapping remains unchanged. The focused lookup tests pass `2/2`;
  the worker's complete chunk-stream set passed `64/64`.
- A narrow, reachable explosion prerequisite now exists. Survival main-hand
  and creative offhand flint-and-steel use can atomically replace placed TNT
  with air and publish a primed `minecraft:tnt` entity. A dedicated owner
  tick expires it after exactly 80 ticks, removes the entity, and conditionally
  destroys one captured adjacent dirt block without loot. Unsupported or
  changed targets fail closed; generalized blast radius, resistance, damage,
  chain reactions, fire, creepers, and explosion loot remain out of scope.
  Primed TNT is explicitly transient until a complete fuse persistence format
  exists, preventing an inert permanent entity after restart. Unit tests prove
  tick 79/80, survival durability, creative offhand immutability, and transient
  save behavior `2/2`; the push-driven raw-TCP lifecycle passes `1/1`.
  Final `mc-net --lib` passes `990 active/1 ignored`; scoped all-target
  strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep, and diff-check
  pass.
- Lua plugins can now own bounded player command roots through script API
  `0.3.0`. Non-operators receive those roots in the real command tree without
  admin commands, and matching raw arguments route only to the owning plugin.
  Registration is atomic, rejects built-in/conflicting roots, and caps roots at
  64 bytes, 128 active roots, and 256 added tree nodes. Legacy `0.2.x`
  manifests remain valid only when they do not declare `player_commands`.
  `mc-script --features lua-runtime` passes `33/33` plus its doctest; focused
  `mc-net` routing passes `2/2`; the real login/configuration wire gate passes
  `1/1`. Independent re-review is clean after the version gate fix.
- TNT expiry now sends the exact verified 26.1.2 `ClientboundExplode` packet
  (`0x24`) through the reliable outbound lane. Due TNT entities are claimed and
  removed before awaiting the world edit, preserving each actual entity
  position and nearby recipients. Each fuse keeps its own conditional edit
  result, so `block_count` is `1` only when that target was removed. Recipients
  use the strict squared-distance `< 4096` rule, and removal is queued before
  the explosion packet. The raw wire RED observed every previous TNT result
  except `0x24`; GREEN decodes the full packet body and verifies center, radius,
  block count, particles, sound, no knockback, and removal order in `4.4s`.
  Focused fuse unit and wire tests pass `1/1` each; scoped strict Clippy, fmt,
  code-health `0 fail / KEEP`, no-sleep, and diff-check pass. Multi-TNT,
  zero-block, range-boundary, and reliable-backpressure cases remain direct
  test gaps; runtime review found no implementation defect.
- TNT block destruction now uses the exact vanilla 26.1.2 1,352-ray candidate
  algorithm instead of one dirt block captured at ignition. A Java
  `LegacyRandomSource` implementation preserves float/double operation order;
  an independent Java oracle pins the complete radius-4 candidate set with
  1,152 positions and SHA-256
  `96c6d197ca9eb6116ff9e470877c3fae7d1b9a972090f1f4f1b5a0f7788d989d`.
  The generated 29,873-state table uses
  `max(block resistance, fluid resistance)`, including water, lava, and 10,488
  waterlogged states, rejects registry gaps and truncated 26.1.2 reports, and
  is byte-identical across extractor runs. Fuse expiry samples live world
  state after the exact entity-physics completion event, applies one
  untruncated conditional batch under the world writer, and reports the
  vanilla candidate-set count rather than the number of changed blocks. Air
  keeps vanilla's absent-resistance behavior; a missing table disables block
  damage instead of treating bedrock as zero resistance. Focused `mc-data`
  tests pass `8/8`, planner tests pass `8/8`, the live-state fuse regression
  passes `1/1`, and the exact wire gate passes `1/1` in `4.45s` with candidate
  count `671` for its deterministic embedded scene. Scoped four-package
  all-target strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep, and
  diff-check pass. Entity damage, chain TNT, fire, loot, and rollback for an
  unexpected mid-batch storage failure remain open; full workspace,
  real-client, VD8, and soak gates were not run.
- Explosion-destroyed TNT now primes a second live TNT entity instead of being
  deleted as an ordinary block. The block is removed by the same conditional
  explosion batch, then the entity spawns at block center with vanilla's
  constructor velocity (`nextDouble` angle, horizontal speed `0.02`, vertical
  speed `0.2`) and shortened fuse `nextInt(20) + 10`. Candidate blocks are
  shuffled with the same Java RNG implementation before interaction; exact
  `nextDouble` and non-power-of-two `nextInt` vectors are pinned against local
  Java 25. The unit RED removed the second TNT but left the fuse map empty;
  GREEN retains exactly one chained fuse expiring on tick `90..=109`. A new
  raw-TCP gate proves the client order `first RemoveEntities -> first
  ClientboundExplode -> second AddEntity`, observes the second TNT block become
  air, and then requires the second removal and explosion; it passes `1/1` in
  `5.00s`. The previous generalized TNT wire gate remains `1/1` in `4.45s`.
  Planner/RNG tests pass `9/9`; scoped `mc-net/mc-test-harness --all-targets`
  strict Clippy, fmt, code-health `0 fail / KEEP`, no-sleep, and diff-check
  pass. Shared vanilla world-RNG-history parity, owner attribution, damage,
  fire, and explosion loot remain open; no full workspace, real-client, VD8,
  or soak gate was run.
- TNT now damages and knocks back nearby survival players. The runtime samples
  the pre-destruction world with vanilla's player AABB grid, uses exact
  extracted collision boxes for covered states and collision-height fallback
  for other solids, and treats unavailable geometry as occluded. Segment tests
  include over-height fence-style boxes rooted one block below the ray. Damage
  uses the inspected 26.1.2 formula
  `((power^2 + power) / 2) * 7 * (radius * 2) + 1`, feet position for distance,
  eye position for direction, existing armor/protection durability handling,
  and per-recipient knockback in `ClientboundExplode`; non-survival connections
  suppress packet knockback and ignore the damage command. Unit tests cover
  full exposure, a complete wall's base-one damage/zero knockback, the strict
  double-radius boundary, and over-height occlusion. The existing raw-TCP TNT
  gate first failed on its old `knockback=None` assertion; GREEN observes a
  health decrease before the explosion packet and a finite non-zero knockback
  vector in `4.47s`. Chain TNT remains green in `5.00s`; explosion tests pass
  `13/13`. Scoped `mc-net/mc-test-harness --all-targets` strict Clippy, fmt,
  code-health `0 fail / KEEP`, no-sleep, and diff-check pass. Entity damage,
  explosion-knockback-resistance attributes, creative-flying versus adventure
  distinctions, and fresh real-client evidence remain open; full workspace,
  VD8, and soak gates were not run.
- TNT now damages living server entities through the existing authoritative
  hurt, death, loot, XP, visibility, and hurt-invulnerability path. Target
  snapshots use entity dimensions and eye height, exposure samples each
  entity AABB against the pre-destruction world, and surviving entities receive
  a hurt event followed by the exact explosion velocity delta after the world
  lock is released. Unit
  coverage includes open and fully occluded mob AABBs, radius rejection,
  target geometry, damage, and velocity publication. The new push-driven
  raw-TCP gate changed from a live chicken surviving the blast to observing
  exact TNT and chicken removals plus raw-chicken and XP entity spawns; it
  passes `1/1` in `4.87s`. Existing player-damage and chained-TNT gates remain
  green in `4.46s` and `5.02s`; explosion geometry/planner tests pass `16/16`.
  Integrated four-package strict Clippy, fmt, code-health `0 fail / KEEP`,
  no-sleep, and diff-check pass. Explosion knockback-resistance attributes,
  block loot, fire, fresh client, workspace, VD8, and soak evidence remain
  open.
- Living mobs now follow the inspected vanilla 26.1.2 death lifecycle. Lethal
  damage drops loot and XP immediately, sends entity event `3`, and keeps the
  authoritative entity in `Despawning`; tick 20 sends event `60` before the
  final removal. Duplicate lethal attacks cannot produce duplicate rewards,
  and dying entities are excluded from persistence. Focused lethal coverage
  passes `9/9`. The TNT wire path reaches explosion, loot, and XP correctly;
  its old 5-second fail guard exactly equalled 80 fuse ticks plus 20 death
  ticks, so its fail-only guard is now 6 seconds while success remains tied to
  the exact removal packet.
- Adult and baby livestock geometry now comes from one state-aware helper.
  Exact 26.1.2 dimensions are pinned for chicken, cow, pig, and sheep; the
  previously wrong adult pig/sheep facts and cow fallback half-width are
  corrected. Explosion exposure, physics narrow phase, melee/feed/shear reach,
  arrow hits, and player body push use per-entity age state while the adult
  type cache remains only a conservative broad phase. A same-type adult+baby
  regression proves distinct explosion eye/AABB geometry and physics queries.
- TNT explosions now create common block drops only for edits that actually
  committed. Repo-owned loot preserves `survives_explosion` and
  `explosion_decay`; the vanilla-default `tntExplosionDropDecay=false` path
  uses `DESTROY`, while unsupported complex tables fail closed. Drops are
  published after block deltas and before the explosion packet, and chained
  TNT never also becomes an item. Current-head wire gates pass for dirt plus
  oak-log loot (`4.55s`), chained TNT (`5.90s`), and mob death (`5.50s`);
  loot parser tests pass `25/25`. Oracle review found and a RED fixed a
  hardcoded dirt/stone-only
  fallback: every fixed repo-owned `block_drops` entry, including logs and
  ores, now participates in default `DESTROY`. Simultaneous fuse expiry now
  publishes each explosion's block result and drops before that TNT's removal
  and explosion packet, then moves to the next stable entity ID; a focused
  two-TNT regression proves the second drop cannot overtake the first packet.
  The non-default decay gamerule, its named RNG stream, and crash atomicity
  between the world commit and item-entity journal remain open.
- NBT strings now use strict Java Modified UTF-8. Embedded `NUL`, BMP boundary
  values, and supplementary characters match Java 25 `writeUTF`; malformed,
  overlong, truncated, four-byte UTF-8, and invalid surrogate sequences fail
  closed. The exact 65,535-byte wire limit is enforced. `mc-nbt` passes `25/25`.
- Cached `FollowTarget` goal batches can now use the existing direct owner-lane
  path. The versioned selected read includes follower and target; lease, lane
  CAS, mutation gate, and global-version fencing reject a migrated target and
  use the coordinator fallback without a partial goal update. Two channel-
  driven regressions pass `2/2`, including completion while the coordinator is
  held and migration fallback.
- Hopper transfers across the resident-region boundary no longer return
  `CrossRegion`. Participating region locks are acquired in stable order, all
  states and consumed ticks are preflighted before mutation, and the existing
  journal decision covers both touched chunks. Atomic transfer and stale
  rollback regressions pass `2/2`; the broader resident set passes `25/25`.
- Dark oak bonemeal now requires a complete 2x2 square, resolves every clicked
  sapling to the same northwest anchor, and commits all four trunks and canopy
  as one conditional edit set. Single/incomplete squares do not consume bone
  meal; unloaded canopy and stale tokens produce no partial tree. Spruce and
  jungle 2x2 squares now use the same northwest-anchor and atomic four-sapling
  replacement contract with deterministic playable 2x2 trunks and species-
  specific crowns. Focused 2x2 coverage passes `6/6`; the dark-oak wire gate
  remains `1/1` in `0.49s`. Mega spruce and jungle crown shape, branches,
  vines, podzol decoration, and vanilla RNG selection remain parity work;
  these templates and the dark-oak crown are playable approximations, not
  exact shape claims.
- Campfire completion now persists `SolarisPendingCampfireOutputs` with a
  deterministic UUID and full item stack in the same D1 world decision that
  removes the cooking slot. UUID-deduplicated entity materialization commits E,
  then an exact-CAS D2 clears the intent before publication. Startup hydrates
  pending-only campfires after world/entity replay and completes E/D2 before
  `bind` returns. Push-gated crash cuts prove D1 before E, E before D2, and two
  consecutive restarts with exactly one item and no intent. A real periodic
  checkpoint regression now compacts D1, E, and D2, restarts from disk, and
  proves the cleared intent does not resurrect while the one UUID-stable item
  remains. Campfire recovery coverage passes `6/6`; no-output ticks no longer
  emit an empty D2 WAL decision. A fresh wire/client gate was not run in this
  short validation wave.
- Melee attacks now resolve an observed player entity ID through the fenced
  attacker session before falling back to server entities. Self, stale,
  out-of-range, creative-target, and forged-pose attacks fail closed; a valid
  Survival target receives `PlayerDamageKind::PlayerAttack` through its own
  session, reusing shield, armor, protection, health, and death handling while
  observers receive the target hurt event. Empty hand and ordinary items now
  use the vanilla base attack damage `1.0` instead of the old `2.0` fallback.
  The target session now emits base melee knockback only after positive damage
  commits: the local 26.1.2 oracle strength is `0.4000000059604645`, grounded
  vertical motion is `0.4`, and `SetEntityMotion` goes only to the target as
  vanilla does for `ServerPlayer`. Shielded and rejected damage cannot
  authorize motion. Seven focused melee tests and two shield gates pass; the
  two-client raw-TCP gate observes Bob `SetHealth(19.0)`, Alice's exact Bob
  hurt event, target-only wire motion quantized to `0.39998779222364655`, and
  no attacker damage in `1.03s`. Adventure targets, exact attack-speed
  scaling, sprint/enchantment extra knockback, resistance, zero-direction RNG,
  and suppression of the observer event when damage is blocked remain parity
  work; no fresh real-client gate ran.
- Generated ruin loot is now exercised through both the raw protocol path and
  the real Java client without operator privileges. The raw chest-withdrawal
  restart gate passes `1/1`. Real-client artifact
  `.analysis/real-client-runs/20260718T062557Z-m94-regression-pack-cMCVPt`
  withdrew the exact diamond/lapis/bread cache, restarted cleanly, and observed
  every chest slot still empty; its gameplay observations passed, while the
  overall runner correctly failed on 226 slow-tick warnings, max `141135us`.
  Lua player events now carry an owned, verified server snapshot of UUID,
  username, operator state, and last accepted position; unverified legacy
  contexts cannot authorize operator roots. Lua API `0.5.0` also adds exact
  manifest-allow-listed `solaris.spawn_entity`. The adapter reparses and resolves
  the namespaced type, submits one session-fenced owner command, and exposes no
  entity or simulation handle. `mc-script` passes `41/41` plus its doc test,
  focused visibility/save/stale-session and unknown-registry tests pass, and the
  raw-TCP disk-plugin gate observes the declared pig `AddEntity`. Independent
  review is approved after the focused visibility test was corrected to spawn
  inside its loaded chunk.
- Entity pathing no longer snapshots every ticketed chunk when only a few
  candidate footprints can be read. `PreparedGoalTick` publishes the exact
  bounded probe positions and the session snapshots only active chunks touched
  by each terrain entity AABB; a 289-chunk regression reduces the required set
  to the two boundary chunks. Goal preparation can also consume the existing
  owner-scoped versioned active snapshot batch. The owner accepts it only for
  the same authority and global version; mutation, restart, incomplete input,
  or cross-owner input falls back to a fresh coordinator read. Regional goal
  apply still performs exact lane CAS and referenced `FollowTarget` validation.
  Full `mc-entity` passes `176` tests with 7 explicit ignores; focused terrain,
  stale-input, inactive-lane, and cross-owner regressions pass, and independent
  review found no Critical or Important issue. The latest real-client P44
  artifact `.analysis/real-client-runs/20260718T095004Z-m94-regression-pack-kEg1Zl`
  passes cow, sheep, and chicken motion/climbing observations. Its overall gate
  remains performance-red with 124 slow ticks: slow-tick medians were about
  `24.5ms` for goals, `9.6ms` for physics, and `27.6ms` for dispatch, with a
  `432ms` maximum tick. The next measured targets are per-tick goal work and
  movement publication; this is not performance-green evidence.
- The embedded client MCP now exposes `minecraft_navigate_to_block` through the
  normal serialized command registry. A bounded event-driven loop observes one
  client tick notification at a time, succeeds only after grounded collision-
  free arrival, rejects unloaded/invalid targets and corridor escape, and uses
  raised-forward collision clearance for a local one-block step instead of a
  guessed jump duration. Start/target and the three-block lateral corridor are
  explicit, and every success/failure/timeout path clears movement keys. Fresh
  `--rerun-tasks` bridge-core and java-agent tests pass; direct behavioral tests
  cover tick progression, step input, corridor escape, blocked/invalid/unloaded
  observations, timeout-as-failure, arrival, and cleanup. Independent re-review
  is approved. No real Minecraft client gate has yet proved the step and
  corridor behavior, so scenario reliability remains manual/client-pending.
- Stonecutters now use the oracle-derived 26.1.2 menu type `24` and
  `ClientboundUpdateRecipes` packet `0x85`. Supported simple id/count offers are
  sent once during initial play sync before the recipe-book packets; air,
  unresolved, component-bearing, zero/overflow, and above-max-stack results are
  omitted by the same predicate used for selection and execution. Output
  quick-move plans the maximum craft count allowed by input and exact inventory
  capacity, then commits the complete debit and credit in one session-fenced
  owner transaction. Handler/owner tests cover normal take, max quick-move,
  stale snapshot/session rejection, close/reopen, disconnect, and rejoin
  conservation; focused stonecutter tests pass `15/15`, protocol/data tests
  pass, and independent re-review is approved. A real 26.1.2 client menu gate
  has not run, and complex component-bearing outputs remain deliberately
  unsupported.
- Detached herd commands now preserve exact owner order while batching up to
  the existing two-command background admission cap. Same-chunk producers are
  completed by one push-driven claim; definitely-uncommitted journal failures
  restore retryable claims, unknown outcomes consume them and fail-stop the
  owner, and `TimeSet` cannot overtake an earlier deferred herd command. Full
  `mc-net` passes `1096/1096` active tests with one explicit ignore; scoped
  strict Clippy, workspace fmt, code-health, diff-check, and no-sleep search
  pass. Final static re-review approved the production pushed path. No full
  workspace, real-client, soak, or post-batching performance gate has run.
- Committed herd publication now installs the complete batch before visibility
  planning, groups entities by chunk, and scans the session table once. This
  reduces structural work from `entities x sessions` to `sessions x unique
  batch chunks + actual dispatches` while preserving entity-major packet order,
  published indexes, visible sets, and dispatch metrics. The characterization
  test plus 24 herd tests, 6 pending-hostile tests, strict `mc-net` Clippy,
  fmt/diff checks, and independent static review pass. Bounded two-CPU P44
  artifact `.analysis/real-client-runs/20260718T095004Z-m94-regression-pack-kEg1Zl`
  again passes cow, sheep, and chicken movement/climbing, but remains strongly
  performance-red: 124 slow ticks with median total `98.1ms`, goals `24.5ms`,
  physics `9.6ms`, and dispatch `27.6ms`. The next measured targets are the
  per-tick goal and movement-publication paths, not more herd admission work.
- Physics publication no longer performs its initial owner CAS prefetch while
  holding the session registry lock, and its empty fast path uses the published
  entity mirror instead of an owner status request. A focused regression shows
  two counted reads instead of three. The post-commit authoritative snapshot
  and current-state recheck remain: review demonstrated that deleting them can
  publish stale motion after a newer commit and loses authoritative passenger
  delta on cross-region vehicle movement. The vehicle regression, 22 focused
  physics tests, full `mc-net` (`1096` active, one ignored), strict Clippy,
  fmt/diff checks, and final static review pass. Runtime effect is not yet
  measured; the more aggressive hack was removed rather than weakening the
  publication fence.
- Regional goal apply now returns a sorted typed kinematics projection instead
  of full snapshots for the active physics set after a successful CAS. A stale
  or empty batch uses a rare current full-state fallback that rechecks
  lifecycle, active membership, AABB, and physics kind. Regional preflight now
  rejects a prepared entity moved across regions instead of silently omitting
  it from expected inputs. Grazing sheep excluded from goal CAS always take the
  current full-state path and merge back in stable id order. The stale-motion,
  moved-out-of-simulation-area, and mixed goal/grazing regressions pass. Full
  `mc-entity` passes `177` active tests with 7 ignored and full `mc-net` passes
  `1100` active tests with one ignored; final independent re-review approved.
- Runtime work control now includes movement dispatch in entity pressure and
  receives completed percentile windows through a push channel from the async
  metrics worker instead of polling a stale snapshot every 100 ticks.
  Scheduled exhaustion is retained until a window is accepted, and healthy
  recovery moves halfway toward the ceiling instead of immediately restoring
  maximum work. Focused tests and full `mc-net` (`1100` active, one ignored)
  pass; independent review is still running for the push refinement.
- Bounded two-CPU P44 artifact
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`
  passed cow and sheep motion/climbing. Chicken moved 2.45 blocks at max client
  speed `0.0735`, but failed smooth turning with a `79.2` degree minimum yaw
  delta, so the gameplay gate is red. Server diagnostics improved from 124 to
  24 slow ticks; their averages were total `69.7ms`, goals `7.18ms`, physics
  `1.87ms`, dispatch `11.26ms`, max tick `102.7ms`. The run preceded the push
  autoscaler refinement and is not performance-green evidence; no retry ran.
- Each current-head persistence-pressure pass drains no more than 64 dirty chunks
  after the dirty high-water mark, then converges through pushed tail work.
  Interval, disconnect, shutdown, and explicit saves retain full checkpoints.
  Focused tests pass; P44 has not been rerun.
- Current-head movement fanout selects its adaptive path only when `S * M >
  2E`; `M = 0` sends no fanout work. Its three focused tests pass. The runtime
  performance rerun remains pending.
- P47 stonecutter coverage now has 134 focused tests and approved review; the
  real 26.1.2 client menu gate remains pending. Spruce and jungle growth have
  the current three-wire-test coverage, without a vanilla shape-parity claim.
- The `wide` SIMD experiment remains non-promoted: `7.86%` kernel-median and
  `0.72%` full-median gains are below the 10% promotion gate.
- Stair placement now selects all four horizontal facings plus vanilla top or
  bottom half, and slab placement selects top/bottom from the clicked face and
  world hit Y relative to the placed cell. The rule comes from the local
  26.1.2 `StairBlock`/`SlabBlock` source; exactly `0.5` remains bottom.
  Registry-backed planner tests cover every facing, face overrides, the height
  boundary and incomplete-family fallback. Rejected support and occupied
  target paths preserve world/inventory state and resync before acknowledgement.
  Slab merging/double state, stair neighbour shapes and a real-client building
  gate remain open.
- Ordinary torches now place as the exact wall-torch facing on horizontal
  conservative full-cube supports, remain standing on `UP`, and reject `DOWN`
  or known partial supports. Raw TCP proves accepted update-before-ack with one
  debit and rejected clicked/target plus unchanged-held-stack resync before ack
  with no debit. Complete irregular-face support parity, neighbour break
  cascades and a real-client building check remain open.
- The regional block/container mutation worker now lives behind an
  explicit-import child module and ownership tripwire. This is a coordinator
  cleanup only; it does not itself prove more throughput, regional completion,
  ECS completion, or client-visible behavior.

Likely code paths:

- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-net/src/play/session.rs`
- `crates/mc-net/src/play/survival.rs`
- `crates/mc-net/src/play/inventory.rs`
- `crates/mc-net/src/play/persistence.rs`
- `crates/mc-world/src/storage.rs`
- `crates/mc-test-harness/tests/block_edit/`
- `crates/mc-test-harness/tests/persistence_inventory.rs`

Validation:

- Focused harness test for the touched path.
- `cargo fmt --all -- --check`.
- `cargo run -p xtask -- code-health`.
- Manual/client check prepared or run with
  `cargo run --bin mc-server -- --config playable.toml`.

Stop condition:

- Do not promote beyond playable-spike evidence without the broader DoD
  matrix. The P4 agent-run real-client gate is green for the recorded artifact;
  replacement-readiness, parity, soak/perf, and broad gameplay claims still
  need their own evidence.
