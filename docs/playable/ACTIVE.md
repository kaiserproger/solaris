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

The embedded client now confirms respawn only after an authoritative health
packet has been applied. The original persisted dead-player save reproduced
`health=0`; the corrected 26.1.2 client respawned with immediate movement held,
closed the death/loading screen, restored its inventory, and observed positive
health. `wait_for_health_below` also now uses strict numeric ordering: the real
client accepts exact zero below a `0.001` threshold without changing its
push-driven wait. Player collision now consumes the complete embedded vanilla
shape table before custom-registry fallbacks and supplies vanilla movement
context for leather boots, Shift descent, and long falls through powder snow.

## Current Queue

This queue is binding across context compaction: common vanilla gameplay first,
then production plugin API, then measured optimization, and only then rare
hardening. An already-open lower-priority diff does not override this order.

1. Run the requested 20-minute MCP survival session on the current build, with
   one fast subagent making the decisions and no deterministic scenario runner
   or operator setup. Record concrete client-visible failures; an owner-played
   subjective-feel gate remains separately pending. The focused precursor is
   closed: on a fresh world the decision agent found and mined a natural jungle
   log, crafted and placed a crafting table through ordinary client actions,
   and opened its crafting screen at full health. The complete 20-minute run is
   still required.
2. Treat failures from that session as the playable queue. Fix the first common
   player-visible blocker, then rerun the shortest real-client path that
   reproduces it.
   The exact dense-world failure is closed on an O3 server with 5,132 injected
   cows and 5,227 total entities. Solaris keeps one outstanding keepalive,
   treats other valid inbound packets as connection-liveness evidence, uses
   vanilla's three-tick default movement interval, and rotates a bounded
   movement-publication shard under extreme entity counts. A real 26.1.2 MCP
   client remained in play for 975 client ticks with no keepalive mismatch,
   timeout, reliable drop, or retry. The reported water gap now has
   server-owned air/drowning, vanilla swimming metadata, and aquatic
   physics that no longer pushes fish to the surface. The client now receives
   the vanilla enabled-feature packet and can cross kelp/seagrass instead of
   being corrected onto their former full-cube fallback. Chunk sections now
   publish the exact vanilla `fluid_count`, so the real client no longer skips
   entity/fluid overlap: entering source water reports `in_water=true` and a
   fluid height of `0.8888889`. The O3 deep-water client gate now covers
   ascent, diving, swimming pose, air depletion and drowning damage without a
   disconnect.
3. The agent-run 26.1.2 hostile-combat gate is closed. Ordinary client action
   packets killed a zombie with an iron sword and collected its rotten-flesh
   drop; a skeleton published a visible arrow and damaged the player; a
   creeper damaged the player and was removed, consistent with its explosion
   path already proved by the TCP regression. The retained harness shows that
   local operator commands only created the deterministic night-time fixture
   and summoned the three mobs. This proves the functional client/server
   paths, not subjective combat feel or natural survival progression.
4. Basic economy and whole-chunk land claims now run on the production Lua API.
   Close the remaining claim surfaces (containers, fluids, explosions and
   entity interaction) through a first-class zone protection policy after the
   ordinary break/place slice is client-verified.
5. Close this owner batch with a 20-minute MCP-driven survival session whose
   decisions are made by one fast subagent. Do not use the deterministic
   scenario runner or operator setup. Treat its visible failures as the next
   playable queue.
6. Keep the rare multi-region save recovery `fsync`/metrics issue documented
   but deferred. Do not resume it unless it becomes ordinary save corruption or
   blocks the playable or plugin path.

## Recent Evidence

- Repeated `minecraft_connect` calls no longer start competing login attempts.
  Calling it for the active address is an idempotent no-op; a different address
  is rejected until the caller explicitly disconnects. The original autonomous
  attempt had mistaken this duplicate-login lifecycle for respawn/navigation
  failure. On a fresh server, three same-address calls retained one session with
  no rejection or disconnect. A fast decision-agent rerun then found and mined
  a natural jungle log, crafted planks and a crafting table with normal
  container clicks, placed it on observed clear ground, and opened the crafting
  screen with health `20.0`.

- The autonomous survival run's apparent natural-zombie combat failure was in
  the MCP observation boundary, not server combat. The server log had committed
  health `20 -> 19`, but `minecraft_attack_entity_once` returned before the
  client applied the entity update. The tool now sends the player's exact
  rotation before the attack and waits on applied-state notifications for
  target damage or removal, with UUID/type and client-level fences. A fresh
  26.1.2 client attacked the same naturally spawned zombie without operator
  setup and returned `confirmed=true`, health `19 -> 18`. The dead-player path
  timed out as an error instead of producing a false success.

- Worldgen revision 8 passed an agent-run MCP route with a fresh 26.1.2 client,
  seed `918273645`, and `tellus_like` mode over forest, coast, ocean, and the
  representative high-relief range around `(-78080, -28928)`. The first route
  exposed low ridge masks becoming enormous
  flat gravel shelves. The corrected router keeps low shelves out of mountain
  biomes, strengthens the existing 720x280 rolling relief, adds anisotropic
  520x210 mountain detail, uses elevation-aware grass/gravel/snow surfaces, and
  explicitly keeps spawn dry. Numeric checks require visible local mountain
  relief without a five-block adjacent wall. A second agent-run MCP pass used
  the exact shipped `playable.toml` profile (`seed=0`, `tellus_like`) and found
  a dry, solid, moderately decorated forest spawn with raised tree crowns. Its
  focused harness then exposed leaf canopies being accepted as spawn support;
  spawn selection now rejects leaves, matching vanilla's no-leaves heightmap
  intent. The first run exposed a pending
  31-second far-travel chunk stream under dirty-cache flush pressure; that is a
  runtime latency item, not accepted worldgen performance evidence.

- Fresh and legacy Solaris chunks now serialize the mandatory vanilla Anvil
  metadata at the codec boundary rather than in one generator. `DataVersion`
  defaults to the pinned 26.1.2 value, nonzero production ticks become
  `LastUpdate`, imported `InhabitedTime` is preserved, and each field is emitted
  exactly once through a real region write/read. Runtime now uses vanilla's
  strict 128-block
  chunk-center range around non-spectator players, counts each spawning chunk
  once per game tick, applies those counts in 20-tick mutation batches, and
  flushes chunks as they leave that range. Missing residents retain their delta
  for retry and shutdown loads them without generation. The value survives a
  real Anvil flush/reopen without adding per-tick chunk publication pressure.

- Every exact vanilla state now reaches its embedded collision shape in player
  movement instead of being bypassed by the old campfire/passable-name lists.
  Focused tests prove empty torch collision, the campfire's 7/16-block body,
  leather-boots support on powder snow, Shift descent, the long-fall 0.9F shape,
  authoritative teleport correction, and conservative fallback after a state
  fingerprint mismatch. Independent review found the fingerprint and direct
  correction-path gaps; both were fixed before the full gates.

- Shift-click batching is client-verified. One real 26.1.2 inventory-menu
  quick-move consumed four logs and produced 16 planks, and one crafting-table
  quick-move did the same, raising the existing plank stack from 16 to 32. Both
  crafting grids were empty after their transactions, and the crafting table
  remained empty after reopening. A chest quick-move transferred one 61-stone
  stack from player slot 54 to storage slot 0 and back to player slot 27;
  reopening the chest preserved that complete stack and empty storage slot.
  Every quick-move was confirmed and the client remained connected.

- Placement is client-verified through both hands and a side face. A real
  26.1.2 client placed stone upward from the main hand (`64 -> 63`), used the
  ordinary vanilla input path to place upward from the offhand (`63 -> 62`),
  then placed to the east side (`x+1`) from the main hand (`62 -> 61`). Focused
  adapter regressions route ordinary blocks through all six clicked faces and
  independently cover an offhand packet against an east face. The run remained
  connected. It also exposed the now-closed direct-use limitation below:
  `minecraft_use_item_on` returned `ok` for the offhand-only stack without
  changing its count or the target, while ordinary `use` input placed it.

- The direct client-MCP offhand gap is closed. `minecraft_use_item_on` accepts
  `main_hand` or `off_hand`, defaults to main hand, forwards the exact vanilla
  interaction hand, and returns the local interaction result. In the focused
  26.1.2 gate, stone was present only in offhand slot `40`, the selected main
  hand was empty, and `hand=off_hand` returned vanilla `Success`, placed stone,
  consumed the stack from `1` to `0`, and left the client in play.

- A real 26.1.2 MCP client closed the intermittent survival-break gate. It mined
  eight prepared stone blocks at `x=12..19`, crossing both sides of the chunk
  boundary at `15/16`. Every first
  ordinary break became air and exposed a visible item entity; the final
  inventory contained exactly eight cobblestone, health stayed at 20, and the
  client remained connected. The run also exposed a client-MCP defect:
  `minecraft_break_block` missed pickup into a non-selected slot despite the
  authoritative inventory update; that tooling issue is closed below.

- The client-MCP pickup defect from that run is closed. `minecraft_break_block`
  snapshots the total expected-item count before mining and, after the block
  becomes air, reacts to applied client state events instead of polling ticks
  or the selected stack; only an observed inventory-count increase completes
  pickup. In the focused 26.1.2 client gate, a stone world drop was
  observed and one cobblestone entered non-selected slot `1` while the diamond
  pickaxe remained selected in slot `0`; the command returned
  `pickup_confirmed=true` with `initial_count=0`, `inventory_count=1`, and the
  client remained in play.

- Rapid sequential mining no longer strands the second valid early `STOP`
  behind an existing delayed break. The stop is retained as queued work, and
  completion or cancellation of the older delayed break promotes it into the
  single delayed slot for event-driven completion. Focused coverage proves
  both transitions alongside the existing chunk-edge precondition regression;
  owner/client confirmation remains pending.

- Survival block loot remains a server-owned world item before pickup rather
  than a direct inventory credit. The focused TCP gate observed the committed
  block update/ack and tool durability update before `AddEntity` plus item
  metadata, kept the drop visible for at least 100 ms, and only then accepted
  one pickup command and emitted take/remove plus inventory slot updates. The
  current run processed one block edit and one item pickup with queue depth
  returning to zero.

- The optional `examples/plugins/geological-mines` plugin declares the
  `geological_deposits` startup ore profile. Prepared plugin discovery runs once
  before world validation and is reused to start Lua later. The profile removes
  the vanilla ore rules and generates deterministic elongated deposits larger
  than 512 connected blocks across chunk boundaries. World contract schema 2
  persists the profile under worldgen revision 8 and rejects a later profile
  change; no declaration remains the vanilla default.

- Default ore generation now embeds 18 separate vanilla 26.1.2 placement passes
  instead of nine merged family approximations. It preserves raw height anchors
  before world clipping, so diamond and lower-redstone trapezoids peak at
  `Y=-64`; rarity filters and uniform `0..1` counts retain fractional attempt
  density. Emerald and extra gold use the exact vanilla biome lists rather than
  broad terrain groups. The local extracted oracle matches all embedded
  anchors, placement kinds, counts/rarities, sizes, discard chances, targets,
  and scoped biome lists. Generated 9x9 chunk evidence keeps each family inside
  its bands, makes diamond/redstone bottom-heavy, and retains iron at ordinary
  branch-mining heights. Vein shape remains Solaris-owned deterministic
  connected geometry, not vanilla RNG.

- Worldgen rolling relief now uses a rotated 720x280-block field with weaker
  190-block detail, while continent, erosion, mountain and river authorities
  remain at 610-3,600-block scales. A behavior gate requires 128-block regional
  height change to dominate eight-block change. Actual generated grassland,
  forest and jungle vegetation remains present but below 12.5% of eligible
  columns, with separate tree/grass/flower density per biome. The embedded
  collision oracle verifies every state of generated and growable plants is not
  a full cube, and the runtime sampler reproduces an exact partial pitcher-crop
  shape. This is measurable coherence/collision evidence; the bounded
  Tellus/Tectonic visual gate is now closed by the revision-8 route above.

- A normal 26.1.2 client is now fenced from combat between respawn and its
  `ServerboundPlayerLoaded` acknowledgement, matching vanilla's load gate.
  While unloaded it remains simulated but cannot be selected or damaged; the
  acknowledgement republishes it as a combat target and reconciles hostile
  goals. Focused coverage proves both sides of that transition. The embedded
  MCP can also combine respawn with immediate movement and perform an exact
  ordinary block break with drop/pickup confirmation. On a fresh non-operator
  world the real client navigated to a jungle tree, broke and collected four
  logs, then used ordinary inventory clicks to craft planks and a crafting
  table; it remained alive at full health. This closes the observed idle-agent
  tooling blocker and the first gather/craft survival slice, not the pending
  20-minute autonomous session.

- Embedded MCP launches now prefer the current run's environment token and
  port over stale JVM properties and check for an existing listener before
  starting another client. The earlier 401 was not reproduced under a fixed
  token, so this removes two launch ambiguities rather than claiming a proven
  server-side auth race. A patched real 26.1.2 client completed 200 fresh MCP
  sessions, 180 observations, and 20 one-tick forward inputs with zero failures
  or 401s, moved from z=0.598 to z=9.956, and remained in play at full health.
  The current-build 20-minute autonomous survival session remains pending.

- Worldgen revision 8 keeps the revision-6 cave stage and revises landforms:
  domain-warped continents, shaped branching mountain ranges, and warped
  river-valley contours that become river biomes only after substantial carving.
  Tests cover a dry walkable 193x193 spawn window across a seed grid, full
  sampled cave volumes, chunk-border slopes, isolated-crater rejection, surface
  shells, exact tree support, spawn resources, extreme vertical geometry, and
  wire-level generated world use.
  Fresh Solaris worlds persist revision/seed/mode/ore-profile/geometry in
  `solaris/world.json`; unversioned vanilla Anvil worlds open without Solaris
  fallback generation, so terrain authorities cannot mix. `playable.toml` now
  uses `.analysis/test-world-v8` with seed `0` and `tellus_like`; both that exact
  profile and seed `918273645` received bounded fresh-client inspection.

- Hostile melee now keeps a zero-speed target-facing goal while in reach, so a
  stationary zombie stops without freezing its body/head rotation and publishes
  the corrected facing to observers. Hostile ticks now read a dedicated active-
  hostile publication plus stable per-session pose/visibility snapshots and
  perform creeper, skeleton, and melee owner work on regional lanes without a
  global registry read. Final melee publication reads per-session
  immutable combat-target and visibility snapshots, rechecks target life, pose,
  vertical reach, and range, then reserves ordered output only while a shared
  target/visibility epoch remains unchanged and even. It never reacquires the
  global session registry. Focused tests cover an unmoving player, facing,
  attacker/target death, movement out of range, Spectator transition,
  unregister-after-snapshot, and completion while another thread deliberately
  holds the session registry; a whole ordinary melee tick is covered by the same
  lock exclusion. Regional selection publishes current loaded hostiles before
  attacks on each goal turn; unload and last-player disconnect clear that input
  without a later owner read. The existing TCP survival zombie damage/kill/drop
  test also remains green. A real-client feel check remains pending.

- A fresh isolated O3 server and real 26.1.2 client completed the hostile-mob
  functional gate through embedded MCP. The client selected an iron sword,
  approached zombie `1000079`, killed it, saw the rotten-flesh drop, and
  restored the pickup. Skeleton `1000082` published arrow `1000083` and reduced
  player health from `13.833334` to `10.333335`. Creeper `1000084` reduced
  health from `10.333335` to `9.333335` and disappeared, consistent with the
  explosion path already proved by the TCP regression; the client remained
  `in_play=true`. The server recorded one `57.474 ms` over-budget tick after
  processing 62 simulation commands with 10 still queued, with zero reliable
  drops, retries, or disconnect warnings. Evidence harness and results:
  `.analysis/mcp-combat-check.py`,
  `.analysis/codex-logs/mcp-hostile-combat-result-v2.json` and
  `.analysis/codex-logs/mcp-hostile-combat-server-v2.log`. This was an
  agent-run deterministic functional gate; subjective animation/feel and a
  natural no-operator survival run remain unproven.

- Water diagnosis now uses structured MCP state rather than screenshots. The
  26.1.2 client exposes fluid tags/type/height/collision, player water flags,
  pose bounds and loaded-chunk state. Solaris sends configuration packet `0x0c`
  with `minecraft:vanilla` before known-pack negotiation, matching the local
  26.1.2 protocol registration and server ordering. Kelp, kelp plants,
  seagrass, tall seagrass and bubble columns are passable instead of falling
  through the unknown-shape full-cube fallback. Swimming/crouching/standing
  body and eye heights now share one pose contract, so one-block water can
  submerge a swimming player's eyes. Focused Rust and client Gradle gates pass,
  and an O3 real client entered the ocean without the prior kelp correction.
  The zero-contact cause was the second section counter: Solaris encoded
  `fluid_count=0`, so 26.1.2 `LevelChunkSection.hasFluid()` prevented
  `EntityFluidInteraction` from scanning otherwise-correct water states. The
  encoder now counts water, lava, water plants and `waterlogged=true` states.
  In the O3 real-client rerun, entering a source block produced
  `in_water=true`, `water_fluid_height=0.8888889`, and no disconnect while 81
  chunks streamed with `chunk_data_ms=0`. Evidence is in
  `.analysis/codex-logs/fluid-count-real-client.json`. A second O3 real-client
  run observed ascent, diving, horizontal movement with the swimming pose, air
  depletion and drowning damage while remaining connected. Evidence is in
  `.analysis/codex-logs/deep-water-real-client-final.json`. Client-local fluid
  contact, movement and breathing are green for this representative survival
  path; broader aquatic mechanics remain normal parity work.

- The exact dense-world rerun reproduced the final disconnect as an unanswered
  keepalive challenge while the client was still sending valid movement. The
  tracker no longer replaces an unanswered challenge and only closes when both
  the challenge and all inbound client activity exceed the deadline. Ordinary
  entities use vanilla's default three-tick tracking interval; above 512
  candidates a rotating shard bounds each tracking turn, while arrows, items,
  and experience orbs remain latency-sensitive. On the O3 5,132-cow fixture
  (5,227 total entities), a real 26.1.2 client completed 720 movement ticks and
  255 additional ticks, remained `in_play=true`, and recorded zero keepalive
  mismatches, timeout closes, reliable drops, or retries. Evidence is
  `.analysis/codex-logs/dense-5132-spawn.json`,
  `.analysis/codex-logs/dense-5132-release-build-v5.log`,
  `.analysis/codex-logs/dense-5132-keepalive-fixed-v5.json`, and
  `.analysis/codex-logs/dense-5132-fixed-v5-server.log`.

- The shipped `basic-economy` plugin now provides durable virtual balances,
  operator self-grants, and an inventory shop whose item grant and balance CAS
  commit atomically. The shipped `land-claims` plugin stores a bounded whole-
  chunk claim index, waits for a targeted registry result, and rolls back the
  durable CAS when registration is rejected.
  Unit coverage proves owner/operator/stranger lookup semantics; real Lua and
  two-client TCP coverage proves a non-operator cannot break or place in the
  owner's claim and obtains its test item through the real economy menu. The
  temporary adapter covers only direct break/place in the configured
  Overworld range; container, fluid, piston, explosion, fire, and entity
  interaction protection remain open. No manual-client gate was run.

- Mob death completion no longer scans every server entity every tick. Lethal
  melee, projectile/effect damage, test ingress, and persisted restore enqueue
  exact retained deadlines; the tick path drains four due deaths per tick so a
  mass-death spike cannot monopolize one tick. An explicit `-O3` benchmark with
  4,096 cows measured idle death ticks at 7 us p50 / 11 us p99, sustained
  four-kill batches at 11,664 us p50 / 13,668 us p99, and four-removal batches
  at 10,147 us p50 / 24,367 us p99. Focused tests cover an empty index,
  multi-tick backlog, lethal effects, arrows, normal death timing, and restart
  reconstruction. This is in-process owner/publication evidence, not real
  socket throughput or manual combat feel.

- Primed TNT and creeper fuses now enter an exact deadline index instead of
  scanning every server entity each tick. Spawn, cancellation, rescheduling,
  persisted restore, and entity removal maintain that index without stale queue
  entries. The explosion owner claims at most one due explosion per world tick,
  including repeated owner calls, so ray planning, world edits, drops, entity
  impacts, and ordered publication cannot combine an arbitrary simultaneous
  batch under one world lock. The explicit O3 full-path benchmark used 4,096
  background cows, 64 due explosions, a fresh 27-block solid dirt volume per
  explosion, and one loaded observer. Idle fuse checks measured 0 us p50/p99;
  the 64 bounded explosion ticks measured 23,812 us p50, 37,943 us p95, and
  46,463 us p99/max. This is an in-process release-build authority/world/entity
  envelope, not publication or real socket throughput.

- Reach now uses the 26.1.2 server contracts instead of one shared
  center-distance rule. Block checks measure eye-to-block AABB at 5.5 survival
  or 6 creative with a strict boundary; entity use measures eye-to-entity AABB
  at 6 or 8 with a strict boundary; default main-hand melee uses the same 6 or
  8 packet envelope with the inclusive `AttackRange` boundary. Authoritative
  held spears use their 4.5/6.5 reach and 0.125 hitbox margin; crouching and
  swimming use their pose-specific eye height and target box. Non-finite
  coordinates fail closed. Focused tests cover exact boundaries, the
  previously rejected 5-to-6-block melee buffer, far rejection, direct mob
  damage/death, death timing, and skeleton owner requests; the full `mc-net`
  suite passes. A manual client gate remains pending.

- Creepers no longer enter generic melee authority. A visible survival target
  within three blocks starts one retained 30-tick fuse; leaving seven blocks
  away reverses its progress, and expiry removes the creeper and uses the shared ordered
  explosion path at power 3. Swelling stops navigation, dying creepers cannot
  reach fuse expiry, and natural swell is not persisted across restart.
  Source-specific explosion centers/power preserve
  TNT power 4, and chained TNT now always uses the canonical TNT entity type
  instead of inheriting the source type. Focused unit tests cover start,
  no-restart, cancellation, exclusive trigger boundary, retained air state,
  terminal removal, and packet
  planning. Real TCP tests prove creeper spawn -> removal -> radius-3 explosion
  -> player damage and preserve the existing skeleton-arrow and TNT paths.
  Client swell/ignited wire indexes and line-of-sight cancellation remain
  pending exact integration evidence; no manual-client gate was run for this
  slice.

- Fresh-player spawn now scans the already-resident 11x11 spawn window and
  chooses the nearest non-hazardous collidable support with collision-free,
  non-fluid body space. Missing world data still uses the previous origin
  fallback. Focused regressions cover origin water, transparent collidable body
  space, and magma support; all `mc-net` tests, fmt, and code-health pass. On
  the same generated seed `20260721` that previously reported
  `block_below_player=minecraft:water`, a new 26.1.2 client reached play with
  `block_below_player=minecraft:air` and 56 visible entities. That observation
  proves only that the initial sampled cell was no longer water; the focused
  server tests prove the selected support. The final tested O3 binary SHA-256
  is `6be274ad51f43129e4949ad2a5eea39444d50d580bd694f5340e300b59b105d9`.
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
  autoscaler churn; the later exact dense-world gate above closes the separate
  periodic disconnect.
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
  that earlier binary was superseded by the movement-backlog checkpoint above.
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
- Furnace cooking now changes the authoritative block state and block entity in
  one resident commit. Both resident and locked fallback paths retain the old
  baked light only as the base for immediate incremental relighting, while
  advancing the light-source token; this removes the 123-127 ms full chunk
  relight seen when a furnace toggled. The embedded 26.1.2 client opened a fresh
  furnace in 75 ms, observed `lit=true` and block light 13, then received
  `minecraft:cooked_porkchop` in output slot 2 through the new event-driven
  `minecraft_wait_for_container_slot` tool. No tick-budget warning occurred in
  the corrected rerun. This is focused furnace evidence, not a broad soak.
- Player melee reach and bare-hand damage are client-verified. A focused
  geometry regression accepts an ordinary survival attack against a sheep
  exactly two blocks away while the existing boundary regression rejects a far
  target. An embedded 26.1.2 client selected an empty hotbar slot, dispatched a
  full-strength attack, received the sheep's authoritative motion update, and
  observed health change from `8` to `7`; the sheep remained alive. The client
  stayed connected. This proves the requested common melee path, not broad
  combat balance across every item, enchantment, effect, or mob.
- Hostile melee now has a facing fence in both planning and final publication.
  A zombie cannot deal damage while its current head direction points away from
  the target, and a stale plan is rejected if either facing, range, visibility,
  attacker life, or player targetability changes before commit. Existing
  push-published target state handles stationary players and immediately fences
  dead players. Focused tests cover those ordinary and race paths. In an
  embedded 26.1.2 client run, the controller issued no movement input: a zombie
  spawned 1.5 blocks away at yaw `0`; the post-damage observation had yaw
  `-180`, player health `17` instead of `20`, and an unchanged player position.
  This is the requested zombie behavior gate, not broad hostile parity.
- Natural passive and hostile spawning now has a fresh-world client gate. An
  embedded 26.1.2 client saw seven naturally spawned pigs/sheep and consumed
  five pushed motion events from one sheep: horizontal deltas stayed smooth,
  yaw changed across events, vertical rise stayed zero, and the sheep travelled
  about `0.85` blocks. Changing only server-console time to night then exposed a
  naturally spawned moving zombie `20.3` blocks away; no summon command was
  used. Focused physics already proves full-block climbing for cows, sheep, and
  chickens, and session tests prove every-tick publication for bounded natural
  passive and hostile movement. This closes basic natural spawn, publication,
  and one-block stepping; longer visual movement quality remains open.
- The representative player water path is client-verified. The retained O3
  26.1.2 run proves ascent, diving, a `3.43`-block swimming pass, air depletion,
  `20 -> 18` drowning damage, and connection continuity. The missing aquatic
  client path was command spawning: `/summon` left water mobs in `Idle` even
  though natural spawns already received `AquaticWander`. Command-spawned water
  mobs now share one class-wide three-dimensional default goal with natural
  aquatic spawns and start off-ground; exact policy tests cover every supported
  aquatic and amphibious class, including hostile members. A corrected
  representative debug client gate summoned one previously absent tropical
  fish into an inspected deep source-water column, consumed eight pushed motion events,
  measured `0.36` blocks of horizontal travel, and observed it remain underwater
  at `y=62.50..62.57`; the client stayed connected. The fixture also exposed a
  separate scheduled-fluid backlog, recorded in the owner performance queue.

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
