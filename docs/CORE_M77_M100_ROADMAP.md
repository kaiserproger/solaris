# Core MVP Roadmap: M77-M100

This document is the agent-facing roadmap for turning the current broad
draft implementation into a working, vanilla-near Solaris core MVP. It
is intentionally stricter than the earlier breadth-first milestone docs.

M77-M99 are preparation, hardening, and evidence milestones. M100 is the
full validation milestone. M100 must not become another feature bucket.

After the 2026-06-13 static review, the operating direction is a breadth freeze:
do not add new survival stations, mobs, or gameplay surface as the default next
step. Prioritize the M90/M91/M94/M95/M99 class of blockers instead: ownership,
performance, real-client evidence, vanilla oracles, soak, security, and
persistence. New breadth is justified only when the frozen ledger marks it as a
replacement blocker or the owner explicitly reopens scope.

The per-milestone prompts live in `docs/milestones/M77.md` through
`docs/milestones/M100.md`. This file is the high-level map; the
per-milestone docs are the files future agents should open for concrete
work.

## Target

The target is a scoped vanilla 26.1.2 overworld-survival server core:

- A normal vanilla client can join, play, disconnect, reconnect, and see
  stable behavior for the scoped mechanics.
- Scope is vanilla 26.1.2 client only. Fabric/Forge/NeoForge client
  mods, modpack replication, plugin APIs, custom datapacks beyond the
  scoped local sidecar, and resource-pack-specific behavior are non-goals
  unless a later ADR changes the M77-M100 scope.
- At least 80% of scoped client-observable overworld-survival mechanics
  have normal-path tests and vanilla/client-visible evidence.
- Remaining behavior is explicitly classified as accepted non-goal,
  deferred debt, or known Solaris divergence.
- Performance and concurrency claims are backed by reproducible load,
  lock, queue, tick, chunk, save, and memory evidence.
- Autoscale means stable operation on weak and strong machines: profiles,
  runtime backpressure, load shedding, health/drain controls, and clear
  limits. It does not imply transparent multi-writer shared-world
  horizontal sharding unless a later ADR explicitly designs that.

## Global Rules For M77-M100

- Read `AGENTS.md`, `docs/DEFINITION_OF_DONE.md`, this file, and the
  latest milestone closeout before work.
- Run the autonomous preflight from `docs/DEFINITION_OF_DONE.md` before
  code and before closeout.
- Use quality labels exactly: `draft`, `stabilization`, or
  `release-ready`.
- Default to `stabilization` after M94. New breadth after M94 is allowed
  only for a blocker found by evidence.
- Do not claim vanilla parity from Solaris-only tests.
- Protocol bots are harness evidence, not real-client evidence.
- Every M78-M100 closeout must update the validation ledger rows it
  touched, including unchanged debt.
- A draft feature does not enter the 80% scoped coverage numerator until
  stabilization evidence links focused runtime tests plus vanilla oracle
  or real-client evidence.
- After M77, implement new breadth only when the frozen ledger marks it
  as an M100 blocker. Otherwise classify it as accepted divergence,
  deferred debt, or M101+ work.
- Every closeout includes the evidence matrix from
  `docs/DEFINITION_OF_DONE.md`.
- Owner merges, tags, and pushes. Agents prepare branches and evidence.

## Hardware Profiles And Autoscale Targets

| Profile | Minimum target | Expected behavior |
|---|---|---|
| `low_end` | 2 vCPU / 4GB RAM, reduced view distance | Join, explore, edit blocks, use containers, fight a few mobs, save/restart without stalls or corruption; autoscale may shed chunk throughput/view distance. |
| `balanced` | 4 vCPU / 8GB RAM, view distance 8, 20 players, >18 TPS target | This is the Project Spec VPS target. M100 cannot claim release readiness if this profile is not green or explicitly downgraded to stabilization. |
| `high_end` | 8-16 vCPU / 16GB+ RAM | More workers/caches improve throughput or expose exact global-lock blockers; high-end must not collapse under contention. |

Autoscale gates must report TPS/tick p95/p99, chunk latency, memory,
lock wait/hold, queue depth, worker saturation, save pressure, slow
client behavior, and scale up/down decisions.

## Milestone Map

| Milestone | Label | Theme |
|---|---|---|
| M77 | stabilization | MVP scope freeze and validation ledger |
| M78 | draft | Real vanilla client automation MVP |
| M79 | stabilization | Vanilla oracle scenario suite |
| M80 | draft | Loot, recipes, and data-driven survival core |
| M81 | draft | Automation blocks and redstone-lite |
| M82 | stabilization | Block mechanics stabilization |
| M83 | draft | Vehicles and movement objects |
| M84 | draft | Time, weather, sleep, and dimension boundaries |
| M85 | draft | Entity ecosystem and pathing baseline |
| M86 | draft | Combat, equipment, and effects hardening |
| M87 | draft | Remaining survival stations decision/pass |
| M88 | stabilization | Persistence and crash recovery hardening |
| M89 | draft | Auth, chat, permissions, and anti-corruption MVP |
| M90 | stabilization | Concurrency ownership review II |
| M91 | stabilization | Performance budget pass |
| M92 | draft | Autoscale and runtime control plane |
| M93 | stabilization | Autoscale soak and failure recovery |
| M94 | stabilization | Full real-client regression pack |
| M95 | stabilization | Vanilla-observable coverage audit |
| M96 | stabilization | Multiplayer survival soak |
| M97 | stabilization | Evidence-driven parity bug bucket |
| M98 | stabilization | Operator/admin MVP docs and tooling |
| M99 | stabilization | Release-candidate validation rehearsal |
| M100 | release-ready or stabilization | Core validation decision |

## M77 - MVP Scope Freeze And Coverage Ledger

Quality label: `stabilization`.

Agent prompt:

> Build the canonical scope and evidence ledger for the vanilla-near
> core MVP. Do not add gameplay breadth. Read all M60-M76 closeouts,
> all earlier M0-M59 closeouts with known follow-ups, especially M8,
> M17-M24, M33, M36, M39-M49, M53, and M57-M64,
> `docs/REPLACEMENT_READINESS.md`, and `docs/DEFINITION_OF_DONE.md`.
> Produce a mechanic-by-mechanic matrix that future milestones update.

Scope:

- Create or update a tracked validation matrix document.
- Taxonomize mechanics: connection, config/play start, chunks, lighting,
  block edits, fluids, farming, loot, inventory, containers, block
  entities, entities, combat, death/respawn, persistence, multiplayer,
  performance, concurrency, ops/autoscale.
- Seed explicit sub-rows for legacy debt: configuration registries/tags
  and known-packs, login compression, offline/online-mode, spawn/respawn
  metadata and `SetDefaultSpawnPosition`, Anvil compression variants
  including LZ4, sidecar/test-world availability, worldgen/generated
  exploration, `.mca` light arrays, player water/swim feel,
  collision/fall parity, falling-block start and landing/removal, broad
  fluid spread/lava interaction, entity-vs-entity collision,
  frozen-world/manual regressions, runtime blocking/stall paths,
  M49 cache/snapshot/light-payload debt, and dependency/data drift.
- Seed explicit latest-debt sub-rows: regular vs hanging/waxed/styled
  signs, campfire visual/cooking/persistence/automation/effects,
  crop/plant growth/support/drop families, simple loot subset vs full
  loot execution, barrel/furnace-family/container click/automation/
  lit-state behavior, dropped-item lifecycle, recipe book/window sync,
  and common survival stations.
- For each row, record status: `ready`, `partial`, `not claimed`,
  `accepted divergence`, `blocked`, or `unknown`.
- Link existing focused tests, oracle evidence, manual/client evidence,
  performance evidence, and known gaps.
- A row may be marked `ready` only when it has normal-path runtime tests
  plus linked vanilla oracle or real-client evidence. Otherwise use
  `partial`, `degraded`, `draft debt`, or `unknown`.

DoD:

- No implementation breadth.
- Freeze the scoped M100 coverage denominator. Removals after M77 require
  owner-approved accepted non-goal/deferred entries and still appear in
  the denominator appendix.
- `docs/REPLACEMENT_READINESS.md` no longer overstates readiness.
- `docs/REPLACEMENT_READINESS.md` has no `Ready Scope` row contradicted
  by `Partial Scope`; ambiguous broad rows are split or demoted.
- Every future M78-M100 milestone has a row or target in the ledger.
- All deferred readiness updates from M53-M76 are reconciled into the
  ledger and readiness docs, including deferred oracle/manual gates.
- Plant/farming rows from M57-M64 record whether evidence is unit-only,
  Solaris harness, owner-run manual, agent-run real-client, vanilla
  oracle, or not run.
- Closeout states which rows block the 80% target.

## M78 - Real Client Automation MVP

Quality label: `draft`.

Agent prompt:

> Build the first approved real-client automation path. It may be an MCP
> server, wrapper, or harness, but it must drive a real vanilla 26.1.2
> client or PrismLauncher-launched client. Protocol-only bots do not
> satisfy this milestone.

Scope:

- Define the automation architecture and local-only artifacts.
- Launch or attach to a real client, connect to Solaris, and record at
  least one reproducible observation.
- Scenario v1: join, wait for chunks, move, break/place one block,
  open/close one simple container, disconnect.
- Capture client logs, server logs, screenshots or structured
  observations, commit id, toolchain, config, and scenario file.

DoD:

- Evidence is reproducible from a scenario file or exact manual command
  sequence.
- The closeout clearly labels the gate as `agent-run real-client`,
  `prepared owner-run`, or `blocked`.
- Any headless/mock limitation is explicitly not counted as client
  evidence.

## M79 - Vanilla Oracle Scenario Suite

Quality label: `stabilization`.

Agent prompt:

> Make vanilla oracle runs systematic. Build structured scenarios for
> Solaris vs vanilla captures and stop relying on one-off manual notes.

Scope:

- Harden `wire-probe` and parity harness scenario manifests.
- Add structured facts for block edits, inventory, containers, drops,
  death/respawn, save/restart, and two-client visibility where feasible.
- Convert ignored/oracle tests into explicit `full`, `degraded`, or
  `blocked` reporting based on `.analysis/server.jar` availability.
- Add a configuration-phase registry/tags scenario that verifies
  KnownPacks, RegistryData, UpdateTags, and FinishConfiguration against
  vanilla/client evidence.
- Inventory M43-M47 oracle artifacts and gaps. Mark each as usable,
  stale, missing, or tooling-only. Tooling-only captures must not count
  as parity evidence.
- Start the property/replay corpus: action seeds and replay manifests for
  block edits, inventory conservation, save/restart idempotence, and
  malformed packet fail-closed behavior.
- Store captures under `.analysis/`, never in git.

DoD:

- At least one non-flaky vanilla-vs-Solaris oracle scenario can be run
  locally with clear pass/diff output.
- Existing ignored oracle tests are documented as optional, degraded, or
  promoted.
- Missing `.analysis/server.jar`, test world, or sidecar reports cannot
  silently pass a stabilization gate; report `degraded` or `blocked`.
- The validation ledger links to the new oracle scenario classes.

## M80 - Loot, Recipes, And Data-Driven Survival Core

Quality label: `draft`.

Agent prompt:

> Extend the current simple loot/recipe sidecar support into a scoped
> survival executor for common gameplay. Do not attempt full vanilla
> datapack parity.

Scope:

- Loot: pools, rolls, `set_count`, simple random count ranges,
  `survives_explosion`, common tool predicates, fortune/silk-touch
  basics for scoped blocks, looting basics for scoped mobs if feasible.
- Recipes: verify shaped/shapeless/smelting/blasting/smoking/campfire
  paths against sidecar data and fallback data.
- Tags/facts: make unsupported features visible in logs/validation, not
  silently mistaken for parity.
- Dropped items: entity metadata, stack merge, pickup contention,
  overflow, despawn, save/restart, and two-client visibility for scoped
  drops.
- Reconcile deterministic crop/plant special-case drops with the sidecar
  loot executor: keep as accepted Solaris divergence, migrate scoped
  cases, or mark as blocker debt. Do not silently count deterministic
  crop drops as vanilla loot parity.

DoD:

- Common survival block and mob drops have tests and at least one oracle
  or data-sidecar citation.
- Unsupported loot/datapack features are classified, not ignored in
  readiness docs.
- Deterministic Solaris fallback remains explicit and does not claim
  vanilla parity.

## M81 - Automation Blocks And Redstone-Lite

Quality label: `draft`.

Agent prompt:

> Add the minimum automation/redstone-visible mechanics needed for a
> vanilla-near survival core. This is redstone-lite, not full redstone
> computer parity.

Scope:

- Decide and document MVP support for hoppers, comparators, buttons,
  levers, pressure plates, powered doors/trapdoors, and simple block
  power propagation.
- Implement high-value container/furnace hopper paths if in scope.
- Implement comparator output for supported containers if in scope.
- Explicitly defer complex redstone update order, quasi-connectivity,
  pistons, observers, and contraption parity unless a blocker appears.

DoD:

- Supported automation has focused runtime tests and client-visible
  evidence where it affects visuals.
- Automation debts from current readiness are named: hopper/comparator
  behavior for campfires, furnace-family blocks, barrels/chests, and
  supported containers.
- Deferred redstone is listed as accepted non-goal or post-MVP debt.
- No global tick or lock regression from automation scans.

## M82 - Block Mechanics Stabilization

Quality label: `stabilization`.

Agent prompt:

> Stabilize block mechanics after M80-M81. Fix regressions and desyncs;
> do not add a new broad subsystem.

Scope:

- Re-test block break/place/use ack paths, relight, neighbor updates,
  support cascades, falling blocks, fluids, containers, hoppers/redstone-
  lite, plants, signs, beds, campfires.
- Re-run the M40/M41 owner-facing regression route: water swim feel,
  survival visible drops/pickup, sugar cane support/cascade/drop scoped
  behavior, nearby mob/aquatic movement, and TPS/lock log capture.
- Retest crop random ticks, bonemeal consume/no-consume, mature/immature
  drops, sapling obstruction, sugar cane/cactus support and max height,
  stem age growth, berry/cocoa age growth, and support-break cascades.
- Add rejection/resync tests for invalid interactions.
- Reconcile `docs/REPLACEMENT_READINESS.md` and the validation ledger.

DoD:

- Focused block mechanics suite passes.
- Do not count M47 falling-block start as full falling-block parity;
  landing placement/removal requires its own oracle/test. Do not count
  narrow water/lava captures as broad fluid parity.
- Block mechanics closeout lists remaining partial gaps by name: hanging
  signs, waxed/styled signs, campfire persistence/effects/automation,
  barrel animation/events, furnace lit-state, and exact crop/vertical-
  plant support/survivability.
- Real-client or owner-run manual checklist covers at least the highest
  risk block interactions.
- Remaining block gaps are exact and linked to future milestones.

## M83 - Vehicles And Movement Objects

Quality label: `draft`.

Agent prompt:

> Add MVP boats and minecarts only to the extent needed for a normal
> vanilla client not to hit obvious missing survival mechanics.

Scope:

- Boats: item use spawn, mount/dismount, basic steering accepted from
  client, collision bounds sufficient for vanilla client, save/restart,
  two-client visibility.
- Minecarts: item/place on rail, mount/dismount, simple rail movement if
  scoped, save/restart, two-client visibility.
- Decide whether powered rails, chest/furnace minecarts, and boat
  variants are MVP, deferred, or non-goal.

DoD:

- Vehicle spawn/mount/move/dismount/remove paths have focused tests.
- Real-client evidence confirms no obvious visual/desync breakage for
  scoped paths.
- Complex rail physics is not implied solved unless explicitly tested.

## M84 - Time, Weather, Sleep, And Dimension Boundaries

Quality label: `draft`.

Agent prompt:

> Close high-visibility world-state gaps: day/night, weather, sleeping,
> and safe handling of portals/dimension boundaries.

Scope:

- Time: day/night progression and client sync.
- Weather: rain/thunder state sync if in MVP; otherwise explicit
  weather-disabled policy.
- Beds: sleep enter/exit, occupancy, wake position, single-player time
  skip; multiplayer quorum if feasible.
- Verify or revive `SetDefaultSpawnPosition` and spawn/respawn anchor
  packets from wire-probe, or document compass/spawn-anchor divergence.
- Portals/dimensions: either safe client-visible rejection/non-goal or a
  minimal scoped teleport boundary. Do not start full Nether/End parity
  unless the owner changes scope.

DoD:

- Bed and time/weather behavior is either implemented with client
  evidence or explicitly not claimed.
- Portal/dimension behavior cannot trap or corrupt a client session.
- Readiness docs no longer list sleeping/weather ambiguously.

## M85 - Entity Ecosystem And Pathing Baseline

Quality label: `draft`.

Agent prompt:

> Improve entities from simple presence/combat toward a stable survival
> ecosystem without unbounded AI complexity.

Scope:

- Spawn/despawn caps and predicates for scoped passive/hostile mobs.
- Loaded-terrain bounded pathing: flat movement, step-up, simple detour,
  stuck recovery, no chunk generation from AI ticks.
- Passive behavior: wander, panic/flee on damage if scoped.
- Hostile behavior: target acquire/forget, melee cadence/reach.
- Optional villager presence/trading placeholder only if required for
  MVP; otherwise explicitly defer villages/trading.

DoD:

- Entity behavior has harness tests and at least one two-client
  visibility path.
- Entity coverage table by type/family: spawn, sync, AI, damage,
  drops/XP, despawn, persistence, two-client evidence, and accepted gaps.
- AI budgets and lock behavior are measured or marked degraded.
- Entity readiness docs distinguish usable AI from vanilla parity.

## M86 - Combat, Equipment, And Effects Hardening

Quality label: `draft`.

Agent prompt:

> Harden player/mob combat and equipment enough for scoped survival.
> Avoid broad balance claims without oracle evidence.

Scope:

- Damage-source model for melee, projectile, fall, fire/lava, drowning,
  suffocation, starvation, and explicit explosion placeholder/non-goal.
- Armor mitigation and durability.
- Weapon, bow, and shield durability; shield axe-disable if scoped.
- Bow/arrow pickup, despawn, attribution, and visible entity behavior.
- Basic status effects/potions only if required by MVP scope.
- Explicitly reconcile M33/M54/M56 deferred gaps: projectiles, status
  effect packet sync, armor fact unification, bow attribution/damage
  causes, arrow physics/metadata/sticking, shield angle/timing,
  durability, axe disable, sounds/particles, and unsupported damage
  sources.

DoD:

- Combat scenarios cover player vs mob, mob vs player, projectile,
  shield, death, drops, XP, and two-client visibility.
- Existing bow/shield support remains partial unless oracle or
  real-client evidence proves the specific claim. Solaris-only tests may
  only justify usable draft/stabilization claims.
- XP curve/orb lifecycle is either vanilla-like and tested or explicitly
  scoped divergent.
- Effects not implemented are documented as post-MVP/non-goal.

## M87 - Remaining Survival Stations Decision/Pass

Quality label: `draft`.

Agent prompt:

> Decide and implement or explicitly defer remaining common survival
> stations. Do not leave them in ambiguous partial readiness.

Scope:

- Brewing stand, anvil, enchanting table, smithing table, grindstone,
  stonecutter, loom, cartography table, composter, and other common
  survival workstations.
- For each, choose: implement MVP, safe no-op/rejection, or non-goal.
- Implement only high-value station paths required for the core MVP.
- Recipe-book/display synchronization, window id/state id resync, cursor
  stack recovery, and non-player container close/reopen behavior are
  explicit rows, not implicit station work.

DoD:

- The validation ledger lists every common station with status and
  evidence/gap.
- Implemented stations have focused container/menu tests.
- Deferred stations are operator/player-visible non-goals, not hidden
  missing mechanics.

## M88 - Persistence And Crash Recovery Hardening

Quality label: `stabilization`.

Agent prompt:

> Make save/restart/crash recovery credible for the scoped MVP. This is
> about not losing or corrupting ordinary gameplay state.

Scope:

- Durable write primitives: temp file, rename, file fsync, parent-dir
  fsync where supported, tmp cleanup.
- Multi-file save consistency for players, world metadata, entities,
  chunks, block entities, scheduled ticks, containers, vehicles, weather,
  and time.
- Playerdata includes gamemode, abilities, selected slot,
  inventory/armor, health/food, spawn/death state, and safe discard or
  recovery of transient cursor/open-container state.
- Include campfire in-flight cooking slots, furnace/cooking runtime
  state, sign text/block-entity NBT, furnace-family kind, barrel
  identity/storage, plant/crop block states, dropped items, and
  scheduled/random-tick-relevant state where scoped.
- Support or explicitly reject all Anvil compression ids, including LZ4,
  with a real/sample fixture or blocked evidence note.
- Persist/load level.dat-relevant spawn, seed, generator policy, time,
  and weather; remove hard-coded test-world spawn assumptions from
  release scope.
- Chunk format fidelity: persisted light arrays in `.mca` are either
  round-tripped, explicitly regenerated with evidence, or listed as
  accepted persistence debt.
- Storage format inventory/versioning for Solaris-owned sidecars,
  backup/restore dry-run, and migration/compatibility stance.
- Dirty chunk autosave cadence and pressure-triggered flush.
- Startup recovery: corrupted playerdata/entities/world/chunks diagnose,
  quarantine, or fail loudly.
- `save-all` and shutdown behavior under load.

DoD:

- Save/restart scenario covers inventory, health/food/XP, spawn point,
  chunks, block entities, containers, entities/drops, weather/time.
- Fault-injection tests cover partial player/entity/world/chunk writes,
  interrupted save-all, and corrupted sidecar/chunk recovery.
- Crash/failure tests either recover last complete state or report a
  precise degradation.
- Operator docs explain backups and recovery limits.

## M89 - Auth, Chat, Permissions, And Anti-Corruption MVP

Quality label: `draft`.

Agent prompt:

> Harden public-facing session behavior enough that normal clients and
> simple malicious packets cannot corrupt state.

Scope:

- Decide online-mode scope: implement encryption/session auth or document
  offline-only as non-production/deferred.
- Duplicate-name/profile handling, whitelist/banlist/op persistence,
  permission levels, public bind warnings.
- Chat/command policy: basic chat if in MVP, or explicit commands-only
  scope with safe ignores/rejections.
- Movement/reach/container sanity checks, malformed packet fail-closed
  behavior, resource-pack status handling.

DoD:

- Invalid client actions cannot duplicate items, corrupt containers, or
  bypass obvious reach/mode constraints in scoped paths.
- Local-dev OP defaults are disabled or loudly marked unsafe outside dev;
  persisted ops/whitelist/banlist behavior has restart tests.
- M100 `release-ready` for a public replacement requires online-mode or
  session-auth evidence. Offline-only can still close as private/local
  scoped stabilization, but not public release readiness.
- Public operator risks are documented.
- Security claims remain scoped; no full anti-cheat claim.

## M90 - Concurrency Ownership Review II

Quality label: `stabilization`.

Agent prompt:

> Re-run the lock/runtime architecture review against the current feature
> set. The goal is to remove obvious global lock and blocking-path risks.

Scope:

- Inventory world/session/entity/storage/network locks and hot paths.
- Review block edits, fluid/random ticks, containers, AI, chunk
  streaming, save/autosave, lighting, worldgen, outbound lanes, slow
  clients.
- Import unresolved M39.c-M39.h and M49.b-M49.e items as checklist
  rows: world/session snapshots, harness buffering, slow-client/write
  blocking, world read-lock reduction, prepared chunk cache bounds,
  dirty/region cache pressure, and light payload sharing.
- Audit M48 residual blocking paths: login-time playerdata load, startup
  sidecar/world prep, save/shutdown drain, slow clients, chunk storms,
  and compression/network write paths.
- Audit M6/M7 paths: dirty eviction flush, Ctrl-C flush, generated chunk
  creation, relight, zlib/region IO, and compression must not run under
  long world/session locks.
- Import the M77 generated-world blocker as a first-class checklist item:
  a Solaris-generated `.analysis/test-world` streamed all 289 spawn chunks
  correctly, but the live debug-server probe produced repeated
  `chunk_prepare` lock waits, `save_all_flush` hold warnings, and tick
  budget violations. Audit chunk prepare disk commit/snapshot, light
  computation, random ticks over ticketed chunks, dirty flush planning, and
  outbound chunk write ownership before M91 measures budgets.
- Move blocking disk/worldgen/compression off forbidden paths.
- Add lock wait/hold regression gates where possible.

DoD:

- Updated lock inventory and ownership diagram/table.
- No known network write, sleep, disk IO, or worldgen work happens under
  long world/session locks in scoped hot paths.
- The M77 generated-world probe no longer emits unexplained
  `chunk_prepare`/`save_all_flush` lock budget warnings, or each remaining
  warning has a measured owner-approved budget and is carried as non-green
  M91/M100 debt.
- Remaining locks have budgets and are tracked in M91/M96.

## M91 - Performance Budget Pass

Quality label: `stabilization`.

Agent prompt:

> Establish reproducible performance budgets and optimize only measured
> bottlenecks. Debug builds are the dev gate; release numbers are optional
> unless the owner asks.

Scope:

- Workloads: solo join/explore, 2-client visibility, 20-client target
  where feasible, view distance 8, active entities, block edits,
  containers, fluids/random ticks, autosave, restart.
- Metrics: TPS, tick p50/p95/p99, chunk first/ring/full latency, memory,
  lock wait/hold, queue depth, worker saturation, disk IO, outbound
  retries/drops.
- Metrics include prepared chunk cache entries/bytes/evictions, region
  cache bytes, dirty chunk queue/flush pressure, snapshot clone/allocation
  counts, light payload allocation/reuse, and compression boundary costs.
- Workloads include interactive fairness while generating chunks: tick
  stalls, world-lock hold time during chunk prepare, stale generation
  cancellation, and movement-to-visible-chunk latency.
- Mandatory regression workload from M77: delete or isolate
  `.analysis/test-world`, let Solaris generate the empty `example.toml`
  world, restart, run a long `wire-probe` or real-client join at view
  distance 8, and record full-window chunk stream metrics. The M77
  baseline was functionally correct but not green: 289/289 chunks emitted,
  `elapsed_ms=17326`, `first_chunk_ms=177`, `ring1_complete_ms=793`,
  `ring2_complete_ms=1652`, `fetch_ms=27771`, `light_compute_ms=50489`,
  repeated 150-377 ms tick warnings, and `chunk_prepare` waits up to
  hundreds of ms.
- Include the M40/M41 manual-regression workload with water movement,
  visible drops, sugar cane edits, nearby mobs, autosave/save-all, and
  reconnect while recording tick/lock/queue metrics.
- Optimize measured hot paths only.

DoD:

- Budgets and actual numbers are recorded.
- The generated-world join/chunk-stream path is fixed or explicitly kept
  as non-green debt. A `release-ready` M100 path requires no unexplained
  tick-budget or `chunk_prepare`/`save_all_flush` lock warnings during the
  generated-world spawn-window stream, and full-window chunk latency must
  meet the budget recorded in this milestone.
- Low-end and high-end bottlenecks are separated.
- The `balanced` Project Spec profile is evaluated against 20 players,
  view distance 8, and >18 TPS target or the milestone remains
  `stabilization`.
- Any performance claim states workload, hardware, build mode, and commit.

## M92 - Autoscale And Runtime Control Plane

Quality label: `draft`.

Agent prompt:

> Add operational autoscale and runtime control-plane primitives so
> Solaris can adapt to weak and strong machines.

Scope:

- Profiles: `low_end`, `balanced`, `high_end`, and explicit defaults for
  workers, view/simulation distance, queues, caches, compression,
  chunk-send budgets, and save cadence.
- Runtime controller: dynamic chunk load/generate/send budgets based on
  tick time, queue depth, memory pressure, worker saturation, and first
  chunk SLA.
- Health/status endpoint or command output, graceful drain, load
  shedding, slow-client policy, safe config reload where feasible.
- Horizontal scaling stance: independent worlds/instances behind a
  proxy/lobby is allowed; shared-world transparent sharding is non-goal
  unless separately designed.

DoD:

- Autoscale decisions are observable and hysteresis prevents oscillation.
- Low-end profile degrades view/throughput predictably instead of
  corrupting or stalling.
- High-end profile shows improved throughput or reveals explicit lock
  blockers for M93/M97.

## M93 - Autoscale Soak And Failure Recovery

Quality label: `stabilization`.

Agent prompt:

> Soak the autoscale/control-plane work under failure and pressure. Fix
> correctness and operator visibility before adding more breadth.

Scope:

- Scenarios: slow disk, chunk-generation storm, reconnect storm, slow
  client, save during shutdown, drain/restart, memory pressure, queue
  saturation.
- Validate low-end, balanced, and high-end profiles.
- Record load shedding and recovery behavior.

DoD:

- No silent corruption, unbounded queue growth, or invisible data loss.
- Operator can see why the server degraded or drained.
- Remaining autoscale limits are explicit in docs.

## M94 - Full Real-Client Regression Pack

Quality label: `stabilization`.

Agent prompt:

> Expand M78 into the real-client evidence pack required by M100.
> Protocol harnesses are not enough.

Scope:

- Real-client scenarios: join/rejoin, movement, chunk load, block
  break/place/use, inventory, crafting, containers, signs, beds,
  campfires, fluids, farming, drops, combat, death/respawn,
  save/restart, two-client visibility.
- Capture screenshots or structured observations, client logs, server
  logs, scenario manifests, and commit ids.
- Mark manual-only steps if automation cannot drive them yet.

DoD:

- The real-client regression matrix covers every scoped checklist row in
  this milestone's Scope, or each missing row is marked `degraded`,
  `manual-pending`, or `blocked`. One scenario is enough only for M78,
  not for M94.
- This does not satisfy M100 complete manual/client coverage unless every
  scoped checklist row has scenario evidence.
- Client-visible blockers become M97 bugs or M100 blockers.
- The validation ledger distinguishes real-client evidence from harness
  evidence.

## M95 - Vanilla-Observable Coverage Audit

Quality label: `stabilization`.

Agent prompt:

> Calculate the actual 80% scoped coverage position. Do not rely on
> implementation existence; count only evidence-backed mechanics.

Scope:

- For each scoped mechanic, require focused runtime test plus vanilla
  oracle or real-client evidence.
- Classify all missing evidence as `draft debt`, `deferred`,
  `accepted divergence`, or `non-goal`.
- Update `docs/REPLACEMENT_READINESS.md` and the validation ledger.

DoD:

- Coverage accounting is reproducible and conservative.
- Mechanics with unit-only or Solaris-only coverage, especially M57-M64
  farming/plant mechanics, do not count toward the 80% until real-client
  or vanilla-observable evidence is linked.
- Campfire cooking, bows, shields, storage/cache/persistence behavior,
  worldgen/generated exploration, and each plant family are counted
  separately; broad categories cannot hide partial evidence.
- The project knows whether M100 can plausibly become `release-ready`.
- No unsupported parity wording remains in readiness docs.

## M96 - Multiplayer Survival Soak

Quality label: `stabilization`.

Agent prompt:

> Run long multiplayer survival soaks that combine mechanics instead of
> testing only isolated slices.

Scope:

- 2-4 hour debug-server soak where feasible. For `release-ready`, this is
  required or owner-waived as a non-green degradation; the degraded
  fallback is at least a 30-minute replayable harness soak with the same
  action mix.
- Participants: real clients and/or harness clients, with at least two
  clients for visibility and contention.
- Actions: exploration, block edits, containers, crafting, mobs/combat,
  death/respawn, drops/XP, fluids/plants, save/autosave, disconnect and
  reconnect, slow reader.

DoD:

- Record TPS/tick/memory/lock/queue/chunk latency and correctness
  outcomes.
- Required assertions include same-block concurrent edits, shared
  container read-modify-write, shared item/XP pickup, disconnect during
  pending chunk work, bounded outbound queue, and slow-reader behavior.
- Property/replay corpus is updated; failing seeds produce replay
  manifests.
- Bugs are classified for M97.
- If the soak is shortened or simulated, the closeout says exactly what
  it failed to prove.

## M97 - Evidence-Driven Parity Bug Bucket

Quality label: `stabilization`.

Agent prompt:

> Fix only bugs found by M94-M96 evidence or blockers in the validation
> ledger. No new breadth unless the owner explicitly reopens scope.

Scope:

- Packet ordering, container resync, ghost entities, chunk/light desync,
  item duplication/loss, persistence loss, invalid action corruption,
  real-client obvious lameness.
- If M90/M91/M94-M96 still carry the M77 generated-world blocker, fix it
  here as evidence-backed performance/concurrency debt: full view-distance-8
  spawn streaming from a Solaris-generated world must not retain
  unexplained 17s full-window latency, 150ms+ runtime ticks,
  `chunk_prepare` lock waits, or `save_all_flush` warnings.
- Add regression tests and update evidence links for every fix.

DoD:

- Every change maps to an evidence item.
- No broad new mechanic is introduced.
- M99 can run without known high-severity M94-M96 blockers.

## M98 - Operator/Admin MVP Docs And Tooling

Quality label: `stabilization`.

Agent prompt:

> Make the MVP operable. A working core also needs clear setup,
> diagnostics, backups, and safe defaults.

Scope:

- Admin guide: server jar, sidecar extraction, config, ports, ops,
  backups, restore, logs, metrics, autoscale profiles, drain/restart.
- Player guide: vanilla 26.1.2 client setup and known non-goals.
- Config validation: unsafe public defaults, missing sidecar, bad world
  dir, port conflicts, backup dir, operator security.
- Commands/docs for `/save-all`, `/stop`, status/health, and recovery.

DoD:

- A fresh operator can run the scoped MVP without reading milestone docs.
- Unsafe configs warn or fail loudly.
- Known non-goals are visible to operators and players.

## M99 - Release-Candidate Validation Rehearsal

Quality label: `stabilization`.

Agent prompt:

> Rehearse M100. Run the candidate validation suite, triage flakes, and
> freeze scenario manifests. No feature breadth.

Scope:

- Run cargo baseline, focused harness groups, vanilla oracle scenarios,
  real-client pack, performance matrix, concurrency matrix, crash/restart
  scenarios, autoscale soak, and readiness coverage accounting.
- Re-run the generated-world join/chunk-stream scenario from M91 and record
  artifact paths. Compare it against the M77 baseline: 289/289 chunks
  streamed correctly but full-window latency was 17.3s with repeated tick
  and `chunk_prepare` warnings.
- Produce a draft M100 report with blockers, degraded gates, and known
  accepted gaps.

DoD:

- M100 has a concrete runbook and expected evidence locations.
- All flakes are either fixed, quarantined with reason, or listed as
  M100 blockers.
- The owner can decide whether M100 is likely `release-ready` or only
  `stabilization`.

## M100 - Core Validation Decision

Quality label: `release-ready` only if every required row is green;
otherwise `stabilization`.

Agent prompt:

> Validate the full scoped core. Do not add features except tiny fixes for
> validation blockers. Run the full DoD and produce a brutally honest
> release-readiness report.
> M100 validates readiness; it does not create readiness by assertion.

Required evidence:

- Preflight: full autonomous preflight plus port, `.analysis/test-world`,
  `.analysis/server.jar`, vanilla sidecar, branch, and dirty status.
- Cargo baseline: `cargo fmt --all -- --check`,
  `cargo run -p xtask -- code-health`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` after final change.
- Focused harness: protocol/session, chunk streaming, block edits,
  lighting, fluids, farming, loot, inventory, containers, block entities,
  entities, combat, death/respawn, persistence, multiplayer, load.
- Vanilla oracle: structured side-by-side or captures for every mechanic
  counted toward vanilla-observable coverage.
- Real client: PrismLauncher/vanilla 26.1.2 automation or owner-run gate
  covering the complete manual checklist.
- Property/replay: seeded scenario corpus for cross-mechanic invariants;
  failing seeds emit replay manifests.
- Performance: low-end, balanced, and high-end profile metrics for TPS,
  tick time, chunk latency, memory, lock wait/hold, queue depth, worker
  saturation, disk IO, slow clients, autosave, and restart.
- Concurrency: no blocking disk/worldgen/network writes under long locks;
  slow reader cannot stall healthy clients; save/stop/reconnect/chunk work
  do not corrupt state.
- Generated-world join/chunk-stream: starting from an empty `example.toml`
  world or fresh generated fixture, Solaris generates, persists,
  restarts/opens, and streams the view-distance-8 spawn window with no
  missing/extra/duplicate chunks and with green budgets for first/ring/full
  chunk latency, tick p95/p99, lock wait/hold, `fetch_ms`,
  `light_compute_ms`, dirty flush pressure, and worker/queue saturation.
  The M77 17.3s full-window stream plus tick/`chunk_prepare` warnings is
  explicitly non-green.
- Autoscale: profiles and dynamic controls degrade predictably and log
  decisions without oscillation or silent corruption.
- Persistence/crash: save/restart and crash-window scenarios for players,
  chunks, entities, block entities, containers, scheduled ticks, time,
  and weather where scoped.
- Coverage: conservative 80% scoped client-observable coverage count.
- Docs: `docs/REPLACEMENT_READINESS.md`, operator docs, and validation
  matrix match the evidence.
- No M0-M76 deferred debt may be silently absent from the validation
  matrix. Each item must be green, accepted divergence, non-goal,
  deferred M101+ debt, or blocker.

Required rows are the M77 validation-ledger rows marked in-scope.
`degraded`, `manual-pending`, `oracle-missing`, and `not run` are
non-green.

M100 acceptance:

- `release-ready`: all required evidence rows for scoped claimed
  mechanics are green, scoped coverage >= 80%, no known high-severity
  real-client/manual blockers, and performance/concurrency/autoscale
  budgets are green. Owner-accepted budget misses are documented as
  non-blocking degradations and keep the affected evidence row non-green
  unless the report proves they do not affect release safety.
- The generated-world join/chunk-stream blocker cannot be waived silently;
  unexplained tick-budget, `chunk_prepare`, `save_all_flush`, or
  full-window latency misses keep M100 at `stabilization` unless the owner
  explicitly accepts the degradation as non-release-ready debt.
- Rows classified before M100 as accepted non-goal or accepted divergence
  are excluded from the 80% numerator and from required-green status, but
  must be visible in readiness/operator docs.
- `stabilization`: any required evidence is missing, degraded, or blocked.
  In this case M100 is still useful, but it must produce the M101+ blocker
  list instead of claiming readiness.
