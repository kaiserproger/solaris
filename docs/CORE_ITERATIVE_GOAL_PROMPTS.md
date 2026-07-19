# Core Readiness: Iterative `/goal` Prompt Series

Status: execution-ready planning snapshot, quality label `stabilization`.

Snapshot date: 2026-07-11. Repository state at the time of the audit:
branch `dev/M100-client-agent`, heavily modified worktree. This document does
not claim that the branch is ready to merge.

## Purpose

This is an ordered program for taking the Solaris core from its current
playable stabilization state toward:

- reliable multiplayer gameplay;
- a single-writer simulation runtime and ECS-owned moving entities;
- measured portable SIMD acceleration;
- vertical and horizontal autoscaling;
- evidence-backed vanilla-observable parity;
- survival progression with several hours of meaningful play.

Run one prompt at a time. Do not start the next prompt until the current goal's
exit gate is green or the goal has produced a concrete blocker report after the
repeated attempts required by `AGENTS.md`.

## Audited State

### What is working

- `cargo run -p xtask -- code-health` reports `0 fail`, `KEEP`.
- Focused library tests pass: `mc-entity` 29, `mc-physics` 16, and `mc-net`
  473 tests, for 518/518 total.
- Prompt 00 restored the full Cargo baseline after correcting the reduced
  worldgen fixture to include required iron ore. The 2026-07-10 checkpoint
  passed formatting, code-health, workspace tests, and clippy.
- Prompt 01 now has a bounded 1200-sample tick-stage window with nearest-rank
  p50/p95/p99/max reporting plus a read-only runtime telemetry handle.
- The focused 20-client VD8 protocol workload now emits
  `solaris.prompt01.workload.result.v1` with tick and chunk percentiles, queue
  depth, worker saturation, lock pressure, RSS/limit, world/save pressure,
  outbound pressure, entity/session counts, provenance, and explicit skipped
  evidence classes. The 2026-07-11 debug run passed with 540 tick samples,
  total-tick p95/p99 `20.261`/`40.163 ms`, first-chunk p95 `1.575 s`, full-window
  p95 `26.729 s`, and dirty chunks `361 -> 0`.
- Prompt 01 also has strict `solaris.core_replay.scenario.v1` and
  `solaris.core_replay.result.v1` DTOs plus a checked seed/action fixture. The
  checked fixture now runs deterministically through the Solaris protocol
  adapter, passes one local vanilla-vs-Solaris normalized diff, and executes
  through the fixed Gradle real-client adapter on a fresh per-run world. The
  real-client result deliberately degrades its single-run determinism
  invariant rather than borrowing the protocol lane's evidence.
- `cargo test -p mc-test-harness --test real_client_manifest` passes 35/35.
- The repo-owned Gradle client adapter launches
  `:fabric-agent:runClientAgent`; live runner code does not require an
  injectable client command.
- Prompt 02 now has deterministic checked concurrent placement/chest replay,
  save/restart conservation, one 30-minute four-active-plus-one-paused-reader
  protocol workload, and fresh two-real-client shared pickup/chest/block-edit
  evidence. Its final Cargo and client-agent Gradle baselines pass.
- Prompt 03 has begun with an explicit bounded simulation owner. Production
  item/XP/arrow claims, player melee plus kill rewards, `/summon`, and bow
  projectile spawn now cross a 1024-entry typed queue and apply in a 256-command
  pre-goals tick phase. Passive herd spawn, lifecycle time, goals/physics,
  arrow hits, and startup restore are owner-controlled too. Mutation-token-
  protected survival break/place commits now use the owner and fail closed on
  world-lock pressure. Network chest/furnace click commits use expected/new
  snapshots and atomically pair world writes with viewer state ids. Other
  visible packet-authored block states are owner-ordered with fail-closed
  resync, but do not yet carry survival's CAS tokens. Sign text and player
  campfire insertion now use block-state/mutation-token/snapshot-checked owner
  commands. Generic/player drops, player inventory, server-origin campfire and
  furnace ticks, hopper transfers, scheduled world results, broader world
  state, and persistence writes remain legacy.
- Recorded real-client evidence includes a green 20-minute P4 survival loop
  with restart and a green two-client P42 opposite chunk crossing. These are
  playable-spike artifacts, not broad readiness evidence.
- Chunk work already uses bounded queues, semaphores, `spawn_blocking`, cached
  prepared frames, pressure shedding, and focused runtime-control inputs.
- `EntityStore` is data-oriented SoA storage, and entity physics can be sampled
  once and computed in bounded worker batches.

### What is not green

- Prompt 01 has not yet produced a profile matrix, repeated real-client
  determinism, or broad side-by-side oracle coverage beyond the checked
  core-action scenario. The 20-client metrics run also exposed a `1.149 s` max
  tick dominated by save contention and first-chunk p95 above the configured
  `1.5 s` target. One shared mutable-world real-client attempt separately
  exposed a `52.940 ms` tick before the reproducible fresh-world run passed.
- The M100 coverage audit reports 46 in-scope rows, 0 conservative `ready`
  rows, and 0.00% evidence-backed coverage. This is partly stale accounting,
  but it is still the canonical result until exact rows are reconciled.
- M96 now has one 30-minute four-active-plus-one-paused-reader protocol soak
  with repeated checked placement/chest races and final persistence. It still
  has no 2-4 hour multiplayer survival soak or natural TCP slow-reader proof.
- Low-end, balanced, and high-end profile matrices are not green. The 20-player
  view-distance-8 gate is complete focused protocol-workload measurement, not a
  hardware profile matrix, real-client performance result, or soak.

### Architecture gap

`PROJECT_SPEC.md` chose standalone `bevy_ecs`, a synchronous single-writer game
tick, and snapshot-based compute workers. The current runtime has not reached
that design:

- `WorldHandle` is `Arc<tokio::sync::Mutex<WorldStorage>>`.
- Per-connection `InteractionState` owns inventory, cursor, container, pending
  interaction, and combat state and directly performs many world locks.
- `SessionRegistryInner` is one `std::sync::Mutex` containing sessions, chunk
  tickets, prepared chunks, all entities, visibility indexes, container
  viewers, player persistence, and world/entity clocks.
- Goal/path selection runs while holding the session mutex; physics runs in
  worker batches; application, collision resolution, visibility, and dispatch
  planning return to the same mutex.
- `mc-entity` has a custom SoA `EntityStore`, not an ECS dependency or schedule.

ADR 0003 intentionally accepted this as transitional architecture. ECS work
must therefore use replay/shadow comparison and staged authority transfer, not
a big-bang rewrite.

### SIMD and benchmark gap

- No Solaris runtime source uses an explicit SIMD API, target-feature dispatch,
  or a SIMD backend. The only exact SIMD package match is transitive
  `simd-adler32` from compression dependencies; that is not Solaris SIMD
  support.
- The workspace forbids unsafe code.
- The only Criterion bench is `mc-world/benches/light_engine.rs`. There is no
  committed benchmark suite for palette packing, worldgen, entity physics,
  pathing, packet encoding, or whole-tick throughput.

### Autoscale gap

The local `RuntimeControlPlane` is real but narrow. It classifies tick, first
chunk, queue, worker, memory, and first-chunk pressure and adjusts view distance
plus chunk send/load/generate rates with hysteresis. It has focused chunk-stream
wiring, memory-pressure shedding, health snapshots, drain hooks, and slow-client
closures.

It does not yet control entity/pathing/light/save/compression budgets, prove
recovery under profile soaks, expose a production load-balancer contract, or
coordinate more than one server process. Shared-world horizontal sharding is
explicitly a current non-goal; the later prompts below reopen it as a separate
architecture program rather than pretending local throttling is full
autoscaling.

### Gameplay-depth gap

The playable client path reaches wood, stone, furnace/charcoal/torch, food,
chest, bed, door, sign, campfire, zombie combat, iron sword, shield, and iron
chestplate, plus several two-client interactions. The default P4 loop is still
20 minutes.

Embedded fallback data is deliberately narrow: 60 recipes, 28 block-drop
mappings, 12 entity-drop mappings, one food entry, and spawn rules for plains
and ocean. Vanilla sidecars supply much broader data, but many systems that
make those data meaningful remain partial or absent: broad AI/pathfinding,
enchanting, brewing, trading, breeding, structures and exploration rewards,
vehicles, full stations, progression goals, dimensions, and endgame.

## Strategy Decision

Three routes were considered:

1. **Big-bang ECS first.** It matches the target architecture directly, but it
   removes the only working runtime as an oracle and can leave the client gate
   broken for months.
2. **Content first.** It quickly adds things to do, but every new mechanic
   becomes coupled to the existing world and session mutexes and makes the
   eventual migration larger.
3. **Evidence-first strangler migration.** Build replay/oracle/load gates,
   establish a single-writer command boundary, run legacy and new simulation in
   shadow, transfer authority in bounded domains, then optimize and add breadth.

Use route 3. It is the only route that keeps an executable, real-client-visible
checkpoint after each architecture slice.

## Shared Execution Contract

Every prompt below inherits these rules:

1. Read `AGENTS.md` first and then only the documents named by the prompt.
2. Inspect branch/status and preserve all owner changes. Never reset, discard,
   stage, or rewrite unrelated work. Never push, merge, or tag.
3. Start by reproducing the relevant baseline. Use TDD for behavior changes:
   observe RED, implement the smallest defensible slice, observe GREEN.
4. Work in independently reviewable slices. Keep moving across checkpoints and
   continue independent work when one optional gate is degraded.
5. Packet IDs/layouts come from local `wire-probe`/`javap`; gameplay parity
   comes from a vanilla capture, legal source inspection, or side-by-side
   harness evidence. Do not guess and do not copy Mojang source.
6. Keep `unsafe_code = "forbid"`. A SIMD dependency may internally use unsafe,
   but Solaris code must expose a safe scalar-equivalent API and justify the
   dependency and lockfile change.
7. Keep local evidence in `.analysis/`; never stage Mojang bytes, vanilla data,
   worlds, screenshots, or other local-only paths listed in `AGENTS.md`.
8. For client-visible changes, use the repo-owned Gradle adapter through
   `tools/run-playable-client-gate.sh` or the approved M94 runner. Do not
   reintroduce an environment-supplied client launcher.
9. Before a checkpoint closeout, run focused gates and the full cargo baseline
   from `AGENTS.md`. If a higher-level gate cannot run, state exactly why and do
   not turn skipped coverage into a green claim.
10. Use `draft`, `stabilization`, and `release-ready` exactly as defined in
    `docs/DEFINITION_OF_DONE.md`. Do not mark a goal complete merely because an
    implementation exists; its exit gate must be satisfied.
11. Commit only coherent checkpoints whose staged paths were reviewed. Use
    Conventional Commits and never include unrelated or local-only artifacts.

## Dependency Order

| Prompt | Depends on | Result |
|---|---|---|
| 00 | current branch | honest green baseline |
| 01 | 00 | measurement and replay laboratory |
| 02 | 01 | multiplayer transaction invariants |
| 03 | 01, 02 | single-writer simulation boundary |
| 03B | 03 | simulation-owned player transaction state |
| 04 | 03B | shadow ECS |
| 05 | 04 | authoritative ECS |
| 06 | 05 | world single-writer cutover |
| 07 | 01, 05, 06 | measured SIMD backends |
| 08 | 01, 06, 07 | complete local autoscale |
| 09 | 08 | multi-instance cluster autoscale |
| 10 | 06, 09 | shared-world region ownership |
| 11 | 10 | elastic split/merge and failure recovery |
| 12 | 08; also 11 when shared-world scaling is in scope | integrated multiplayer soak |
| 13 | 06, 08, 12 | 0-2 hour survival progression |
| 14 | 13 | 2-8 hour overworld progression |
| 15 | 01-14, including 03B | scoped M100 parity campaign |
| 16 | 15 | broad overworld parity expansion |
| 17 | 16 | dimensions and endgame |
| 18 | all prior required goals | release-candidate decision |

Prompts 09-11 define "full autoscaling" to include production process scaling
and transparent shared-world spatial scaling. If the product target only needs
one process or independent worlds, stop after Prompt 08 or 09 and explicitly
record shared-world scaling as a non-goal.

---

## Prompt 00 - Restore Baseline And Evidence Truth

```text
/goal
Bring the current Solaris branch to an honest, reproducible stabilization
baseline. Read AGENTS.md, docs/DEFINITION_OF_DONE.md,
docs/playable/README.md, docs/playable/ACTIVE.md,
docs/VALIDATION_LEDGER.md, and docs/milestones/M100.md. Follow the Shared
Execution Contract in
docs/CORE_ITERATIVE_GOAL_PROMPTS.md.

First inspect the dirty worktree and separate coherent owner work from local
artifacts. Reproduce the known workspace failure in
mc_worldgen::terrain::tests::try_with_rules_allows_missing_optional_blocks_with_fallbacks.
Resolve the contract mismatch between required iron ore and optional worldgen
fallbacks without weakening the intended no-debug playable resource path. Add
or adjust the smallest regression that proves the chosen contract.

Then reconcile current client-agent/playable evidence with the canonical
ledger. Validate the recorded green P4 and P42 artifacts with the current
runner, but do not promote broad rows from artifact-shape checks alone. Remove
stale claims and record exact evidence legs for any row changed.

Exit gate:
- cargo fmt --all -- --check passes;
- cargo run -p xtask -- code-health passes;
- cargo test --workspace passes;
- cargo clippy --workspace --all-targets -- -D warnings passes;
- client-agent Gradle tests and real_client_manifest tests pass;
- current P4 and P42 artifacts validate, or their exact stale/degraded reason
  is recorded;
- git diff contains no accidental local artifacts or unrelated cleanup.

Do not add gameplay breadth, ECS, SIMD, or autoscale behavior in this goal.
Do not stop after documenting the worldgen failure; fix and verify it.
```

## Prompt 01 - Build The Measurement, Oracle, And Replay Laboratory

```text
/goal
Build a reproducible measurement and deterministic replay laboratory for the
Solaris core. Start only from Prompt 00's green baseline. Read AGENTS.md,
docs/DEFINITION_OF_DONE.md, docs/milestones/M79.md,
docs/milestones/M90.md, docs/milestones/M91.md,
docs/milestones/M96.md, the load and parity harnesses, and the Shared Execution
Contract.

Create one versioned scenario/result schema that can drive protocol clients,
real-client phases where supported, and Solaris-vs-vanilla oracle runs. Record
seed, action order, expected invariants, commit, config fingerprint, hardware,
build profile, sidecar version, and all degraded/skipped gates. A failed seed
must be replayable from a small checked-in manifest without checking in world or
Mojang data.

Add real tick histograms and p50/p95/p99 reporting for total tick and major
stages, plus chunk first/ring/full latency, lock wait/hold, queue depth, worker
saturation, RSS/limit, dirty/save pressure, outbound pressure, and entity
counts. Counter snapshots alone are insufficient for percentiles.

Define reproducible solo, two-client, 20-client VD8, entity-density,
save/restart, slow-reader, reconnect, and generated-world workloads. Run the
smallest useful baseline now; do not optimize hot paths in this goal.

Exit gate:
- schema and parsers fail closed on incomplete evidence;
- deterministic replay reproduces the same normalized Solaris state twice;
- at least one local vanilla-vs-Solaris scenario emits a real diff/pass result;
- a 20-client protocol workload emits complete metrics or a concrete measured
  blocker rather than silently skipping;
- reports distinguish unit, harness, oracle, real-client, performance, and
  soak evidence;
- full cargo baseline passes.
```

## Prompt 02 - Multiplayer Transaction Correctness

```text
/goal
Make shared multiplayer actions linearizable and replayable before changing
runtime ownership. Read AGENTS.md, docs/milestones/M96.md and ledger rows B1,
L2, K1, G4, S2, and Q3 in docs/VALIDATION_LEDGER.md, the replay laboratory from
Prompt 01, and the Shared Execution Contract.

Drive real concurrent actions, not sequential surrogates: same-block
break/place races, stale block action sequences, shared chest/furnace/hopper
RMW, simultaneous item and XP pickup, damage/death/drop contention, duplicate
login, disconnect during pending chunk work, reconnect, and one paused reader.
Define explicit server-authoritative versions/epochs or command ordering for
each mutable aggregate. Invalid or stale actions must fail closed and resync
without duplication, loss, ghost state, or trust in client deltas.

Preserve failing seeds and add state conservation assertions for blocks,
inventories, cursors, containers, drops, XP, entities, and persisted state.
Avoid a broad ECS or world-actor rewrite in this goal; produce invariants that
the later migration must preserve.

Exit gate:
- repeated concurrent replay seeds are deterministic;
- each named race has a RED/GREEN regression through the real session path;
- four active clients plus one slow reader run for at least 30 minutes with
  bounded queues and no lost/duplicated authoritative state;
- save/restart after the contention mix preserves the final state;
- focused two-real-client shared edit/container/pickup evidence is recorded;
- full cargo baseline passes.
```

## Prompt 03 - Establish The Single-Writer Simulation Boundary

```text
/goal
Create the single-writer simulation boundary required by PROJECT_SPEC without
yet replacing EntityStore with ECS. Read AGENTS.md, PROJECT_SPEC sections 3-4,
ADR 0003, M90, Prompts 01-02 outputs, hot play/session/server code, and the
Shared Execution Contract.

Define bounded typed network-to-simulation commands and simulation-to-network
events. Packet tasks may decode, validate packet shape, and enqueue commands;
they must not directly own authoritative world/entity/container mutation.
Define deterministic tick phases, command ordering, per-tick budgets,
cancellation, backpressure, and shutdown/drain semantics. Heavy IO and compute
must use immutable versioned snapshots and return validated diffs.

Migrate one vertical slice at a time: begin with entity lifecycle and item/XP
pickup, then block edits and shared containers. Keep an adapter for untouched
legacy paths. For each migrated slice, replay the same action log against legacy
and new paths and require identical normalized outcomes before authority moves.
Write an ADR that explicitly stages or supersedes ADR 0003.

Exit gate:
- one synchronous simulation owner controls migrated authoritative state;
- network tasks cannot mutate migrated domains directly;
- command queues are bounded and observable;
- dual-path replay is equivalent for Prompt 02 contention cases;
- P4 and P42 real-client gates remain green;
- lock metrics show no new global contention regression;
- full cargo baseline passes.
```

## Prompt 03B - Move Player Transactions Behind The Simulation Boundary

```text
/goal
Move authoritative player gameplay state out of per-connection packet tasks and
behind the Prompt 03 simulation owner. Read AGENTS.md, Prompt 02 contention
seeds, Prompt 03 ADR/source audit, player persistence and InteractionState code,
and the Shared Execution Contract. Do not start ECS or world-owner replacement
in this goal.

Introduce a simulation-owned player state aggregate for inventory, selected
slot, cursor, XP, health/hunger, active item use, combat/death state, spawn and
respawn state, authoritative pose, and container transaction generation. Keep
TCP framing, compression, keepalive, teleport acknowledgements, and outbound
socket ownership in the connection task. A disconnected or replaced session
must be fenced by a session generation; stale commands cannot mutate the new
session.

Make composite actions one owner transaction: item/XP pickup plus credit,
break/tool damage/drop, placement plus inventory debit, crafting and shared
container clicks, food use, bow release, damage/death/inventory drops, and
respawn. Return immutable player snapshots and semantic network events. Remove
the owner-apply/requester-cancel window instead of masking it with timeouts.
Persist only committed owner snapshots through an explicit save barrier.

Exit gate:
- packet tasks cannot directly mutate authoritative player gameplay fields;
- every Prompt 02 player/container/drop/XP conservation seed remains exact;
- disconnect, duplicate login, reconnect, cancellation, death, and save/restart
  cannot lose or duplicate inventory, cursor, XP, or drops;
- player state and world/entity/container effects commit or reject atomically;
- P4 plus two-client P37/P38/P42 or stricter real-client gates pass;
- queue latency and lock metrics show no global contention regression;
- full cargo baseline passes.
```

## Prompt 04 - Introduce ECS In Shadow Mode

```text
/goal
Introduce standalone bevy_ecs as a shadow runtime for moving entities while
the legacy runtime remains authoritative. Read AGENTS.md, PROJECT_SPEC ECS
sections, Prompt 03/03B simulation contracts, mc-entity, entity persistence and
wire adapters, and the Shared Execution Contract.

Justify the dependency and Cargo.lock change. Model stable entity identity and
components for transform, velocity, lifecycle, type, health/attributes, AI
goal, item stack, XP value, projectile, vehicle/passenger, persistence, and
visibility-relevant state. Keep chunks, block entities, connections, and static
registries outside ECS as PROJECT_SPEC requires.

Build explicit schedules for input/AI, snapshot requests, physics result apply,
combat/lifecycle, persistence extraction, and output event production. Run ECS
in shadow for the same commands as legacy EntityStore. At every tick compare
normalized snapshots and emitted semantic events; save the first divergence as
a replay seed. Do not send ECS output to clients yet.

Exit gate:
- all current entity unit and session tests pass;
- shadow replay covers items, XP, passive/hostile mobs, arrows, falling blocks,
  boats/minecarts where implemented, death, despawn, persistence, and restart;
- at least a one-hour accelerated mixed replay has zero unexplained divergence;
- ECS schedule and legacy baseline benchmarks are recorded;
- no client-visible authority has moved and full cargo baseline passes.
```

## Prompt 05 - Cut Entity Authority Over To ECS

```text
/goal
Make ECS authoritative for moving server entities and remove entity ownership
from the monolithic SessionRegistry mutex. Read AGENTS.md, Prompt 04 artifacts,
mc-net session/entity/persistence/wire code, M85-M86, and the Shared Execution
Contract.

Transfer authority by entity family in reversible slices: item/XP entities,
projectiles/falling blocks, passive mobs, hostile mobs, then vehicles. Keep
stable runtime ids and UUIDs, protocol ordering, persistence NBT, chunk
visibility, pickup/damage races, and replay determinism. Replace SessionRegistry
entity mutation with simulation commands and immutable/query snapshots. Split
visibility indexes and connection routing from ECS storage; do not put TCP
handles or chunk storage into ECS.

Remove legacy dual writes after each family passes shadow comparison. Delete
the old EntityStore authority only after every supported family is migrated;
avoid maintaining two permanent entity models.

Exit gate:
- SessionRegistryInner no longer owns EntityStore or serializes entity tick;
- ECS is the sole authority for all supported moving entities;
- Prompt 02 contention and Prompt 04 shadow corpus pass on ECS only;
- persistence/restart and two-client visibility remain exact;
- real-client P21/P28/P37/P39/P42 or stricter equivalents pass;
- entity density benchmark improves or records the precise remaining blocker;
- full cargo baseline passes.
```

## Prompt 06 - Cut World Mutation Over To A Single Writer

```text
/goal
Finish the single-writer runtime by removing direct hot-path mutation through
Arc<tokio::sync::Mutex<WorldStorage>>. Read AGENTS.md, PROJECT_SPEC threading
and snapshot sections, ADR 0003 plus Prompt 03 ADR, M88-M91, world/storage/play
mutation code, and the Shared Execution Contract.

Introduce an authoritative world simulation owner with bounded commands for
block edits, block entities and containers, scheduled block/fluid ticks,
random ticks, time/weather, and save barriers. Readers use immutable versioned
chunk snapshots. Worldgen, lighting, pathfinding, compression, and region IO
run outside the simulation owner and return version-checked results. Define
stale-result rejection, cross-chunk atomicity, save epoch, crash window, drain,
and restart semantics.

Migrate vertical slices with legacy/new replay equivalence. Do not hide a
global mutex behind a differently named handle. Keep storage format and unknown
NBT preservation compatible unless an explicit migration is tested.

Exit gate:
- network/session tasks do not hold or await the authoritative world writer;
- no disk, network write, compression, worldgen, relight, or sleep occurs in a
  world critical section;
- Prompt 02 races and generated-world/save/restart gates pass;
- P4/P42 real-client gates pass;
- 20-client VD8 shows bounded command queues and materially lower world/session
  lock pressure, with p95/p99 recorded;
- full cargo baseline passes.
```

## Prompt 07 - Add Portable, Measured SIMD Backends

```text
/goal
Add portable SIMD support only to measured Solaris hot kernels, with exact
scalar fallbacks. Read AGENTS.md, Prompt 01 profiles, Prompt 05-06 architecture,
workspace lint/dependency policy, existing light benchmark, and the Shared
Execution Contract.

First add Criterion/replay benchmarks for light propagation, section palette
pack/rebit/non-air scans, worldgen noise/column evaluation, entity integration
and broad-phase candidates, and packet/chunk encoding. Profile real workloads
to rank candidates. Verify what stable Rust 1.94 supports; compare compiler
autovectorization with a safe portable SIMD crate or safe multiversion wrapper.
Keep unsafe forbidden in Solaris code and justify any dependency.

For selected kernels expose one semantic API with scalar and SIMD backends,
runtime or build-time feature selection appropriate to supported platforms,
and a forced-scalar test mode. Integer outputs must be bit-identical. Floating
outputs require a documented deterministic tolerance and must not change
client-visible simulation decisions. Unsupported CPUs always use scalar.

Exit gate:
- benchmark corpus is reproducible and stores baseline/comparison metadata;
- at least one proven production hot kernel has a maintained SIMD backend and
  >=10% median improvement without p95 regression, or the goal remains open
  with measured evidence identifying the next candidate;
- randomized scalar-vs-SIMD property tests pass on every supported backend;
- forced-scalar and auto-selected full gameplay replays are equivalent;
- P4/P42 and full cargo baseline pass;
- CI guards correctness and reports performance drift without assuming AVX2.
```

## Prompt 08 - Complete Local Adaptive Autoscaling

```text
/goal
Turn RuntimeControlPlane into a complete single-instance adaptive resource
controller. Read AGENTS.md, docs/milestones/M91.md,
docs/milestones/M92.md, docs/milestones/M93.md,
docs/M92_AUTOSCALE_CONTROL_PLANE.md, Prompts 01 and 05-07 evidence, and the
Shared Execution Contract.

Use measured p95/p99 tick latency, queue age/depth, worker saturation, first
chunk SLA, memory/dirty pressure, slow clients, and save pressure. Allocate
bounded budgets across chunk load/generate/send, ECS entity/AI/pathing work,
lighting, scheduled/random ticks, persistence, compression, and background
prewarm. Preserve correctness and fairness: degradation may reduce view or
throughput and defer work, but may not silently skip authoritative simulation.

Implement observable hysteresis, cooldown, recovery, admission/load shedding,
OOM-safe memory policy, slow-client recovery/rejoin policy, safe reload for
allowed bounds, authenticated status, unauthenticated readiness/liveness, and a
load-balancer drain contract. Validate cgroup limits where available.

Exit gate:
- low_end (2 vCPU/4GB), balanced (4 vCPU/8GB, 20 players, VD8, >18 TPS), and
  high_end (8-16 vCPU/16GB+) profiles have reproducible reports;
- pressure and recovery scenarios prove no oscillation, starvation, corruption,
  invisible data loss, or unbounded queues;
- high_end either improves throughput or names an exact remaining serial owner;
- health/drain semantics are socket-tested and documented;
- full cargo, replay, load, persistence, and relevant real-client gates pass.
```

## Prompt 09 - Add Multi-Instance Cluster Autoscaling

```text
/goal
Add production horizontal autoscaling for independent Solaris instances and
worlds. Read AGENTS.md, Prompt 08 outputs, operator/security docs, extension API
boundaries, and the Shared Execution Contract. Write and approve an ADR before
runtime implementation.

Define instance identity, world identity, protocol/version compatibility,
capacity advertisement, readiness/liveness, admission, proxy routing, graceful
drain, player transfer/reconnect semantics, config/secret boundaries, metrics,
and persistence fencing. Keep the game server independent from a particular
orchestrator; provide a narrow control API and one local reference controller
that can scale 1 -> N -> 1 based on observed demand.

This goal is for independent instances, lobby/minigame/world routing, and safe
process replacement. It must not claim transparent shared-world scaling. Do not
place Mojang auth/session secrets in logs or artifacts.

Exit gate:
- a local cluster scenario starts one instance, scales out under synthetic
  admission pressure, routes new sessions, drains one instance, and scales in;
- no new session enters a draining instance;
- save/restart and instance crash do not create two writers for one world;
- proxy/client behavior and failure reasons are observable;
- config/API/security tests and full cargo baseline pass;
- limitations leading into Prompt 10 are explicit.
```

## Prompt 10 - Establish Shared-World Region Ownership

```text
/goal
Establish the correctness foundation for transparent shared-world spatial
scaling. Read AGENTS.md, PROJECT_SPEC snapshot rules, Prompt 06 single-writer
runtime, Prompt 09 cluster contract, persistence code,
`docs/decisions/0005-regional-simulation.md`, and the Shared Execution Contract.
Resolve that proposed ADR before runtime implementation. This is a new
architecture scope: write a deterministic model before networking multiple
processes.

Partition authoritative world state into spatial region actors with one writer
per region epoch. Define ownership leases/terms, fencing tokens, chunk and
entity ownership, player and entity border handoff, ghost/read snapshots,
cross-region commands, atomic multi-region transactions, scheduled tick
ownership, visibility, save layout, and recovery. Prefer deterministic local
multi-region simulation first; a distributed transport is only an adapter.

No chunk, entity, container, or scheduled action may have two authorities.
Stale owners and stale worker results must be rejected by epoch. Preserve
vanilla-visible ordering at borders and save compatibility.

Exit gate:
- deterministic model/property tests cover split ownership, stale writers,
  border edits, entity/player crossing, projectile crossing, item pickup,
  container access, disconnect, and save/restart;
- a local two-region runtime passes the same tests without global world/session
  mutation locks;
- two real clients can stand across a border and observe consistent actions;
- crash/replay cannot duplicate or lose authoritative state;
- full cargo and relevant client gates pass.
```

## Prompt 11 - Elastic Region Split, Merge, And Recovery

```text
/goal
Make shared-world region ownership elastic across processes. Read AGENTS.md,
Prompt 10 ADR/evidence, Prompt 09 cluster control plane, Prompt 08 pressure
signals, persistence/recovery docs, and the Shared Execution Contract.

Implement measured hot-region split, cold-region merge, placement, bounded
state transfer, ownership handoff, routing updates, rollback/fencing, and
failure recovery. Handoff must quiesce or log commands at a precise epoch,
transfer a verified snapshot plus tail, atomically publish the new owner, and
reject old-owner writes. Define player behavior during handoff and hard limits
for repeated failure.

Add chaos scenarios for process kill before/after each handoff phase, network
delay/partition, slow disk, stale routing, reconnect storms, and scale-in during
save. Never trade consistency for availability silently.

Exit gate:
- one shared world scales 1 -> 2+ workers -> 1 under measured regional load;
- connected players cross and interact across ownership boundaries without
  duplicate entities, ghost blocks, inventory loss, or unexplained disconnect;
- every injected handoff crash recovers to one fenced owner with replayable
  evidence;
- scale decisions and degraded modes are operator-visible;
- persistence, chaos, multiplayer, full cargo, and real-client gates pass.
```

## Prompt 12 - Run The Integrated Multiplayer And Chaos Soak

```text
/goal
Prove the migrated runtime, ECS, SIMD, and autoscale stack under sustained
multiplayer gameplay. Read AGENTS.md, docs/milestones/M96.md, Prompts 01-08
reports, docs/VALIDATION_LEDGER.md, and the Shared Execution Contract. If
shared-world scaling is in scope, also read and exercise Prompts 09-11;
otherwise record that branch as an explicit non-goal. Optimize only measured
blockers found by this goal.

Run a replayable 2-4 hour debug-server soak with 20 active clients at VD8 on
balanced hardware limits, including at least two real clients for visual and
shared-state observations. Mix exploration, concurrent block edits, crafting,
shared containers, hoppers, mobs/combat, death/respawn, item/XP contention,
fluids/plants, autosave, disconnect/reconnect, slow reader, and region crossing
if Prompt 10-11 are in scope.

Inject slow disk, worker saturation, queue pressure, memory pressure, reconnect
storm, instance drain/restart, and process failure at controlled points. Record
failing seeds and state hashes before/after recovery.

Exit gate:
- >18 TPS balanced target and tick/chunk/lock/queue/memory budgets are green or
  exact non-green blockers remain open;
- no crash, deadlock, unbounded growth, corruption, duplication, invisible data
  loss, or healthy-client starvation;
- final save/restart/rejoin state matches authoritative pre-stop state;
- real-client observations show no high-severity desync;
- full cargo baseline passes after the final fix.
```

## Prompt 13 - Deliver A Real 0-2 Hour Survival Arc

```text
/goal
Extend the playable spike into a no-debug 0-2 hour survival arc on the migrated
core. Read AGENTS.md, docs/playable/README.md, docs/playable/ACTIVE.md, current
sidecar loaders, Prompt 12 evidence, and the Shared Execution Contract. Preserve
architecture and performance budgets; do not add one-off spawn cheats as the
primary path.

Define a concrete player journey with checkpoints: join and orient; gather wood
and food; shelter and night survival; stone and furnace; coal/charcoal and
lighting; iron mining/smelting; tools, shield, and armor; farming or renewable
food; combat, death/drop recovery; exploration; save/restart/rejoin. Generated
terrain must naturally provide reachable resources across seeds, with explicit
fallback profile behavior rather than hardcoded per-scenario coordinates.

Use vanilla sidecar recipes/loot/tags/components as authoritative input and
expand execution semantics instead of duplicating recipes in Rust. Every
checkpoint needs harness coverage and real-client observation. Automate active
actions; an idle timer is not playability evidence.

Exit gate:
- one real vanilla client completes at least two hours of scripted active
  survival without debug commands, starter grants, crash, disconnect, or
  catastrophic stall;
- two clients can cooperate through the same progression and shared storage;
- restart preserves world, players, inventories, entities, containers, crops,
  scheduled work, time, and weather in scope;
- a concise player-visible gap list drives Prompt 14;
- full cargo and performance regression gates pass.
```

## Prompt 14 - Deliver A 2-8 Hour Overworld Progression Arc

```text
/goal
Create meaningful overworld progression from hour 2 through at least hour 8.
Read AGENTS.md, Prompt 13 evidence, ledger rows K2/V1/N1/N2/E3/G1-G4 in
docs/VALIDATION_LEDGER.md, worldgen structure/data loaders, and the Shared
Execution Contract.

Prioritize systems that create decisions and goals rather than decorative
breadth: reliable caves/resource distribution through diamond tier; XP and
enchanting; repair/anvil and advanced furnace/station paths; farming and animal
breeding; broader hostile/passive ecosystems and AI; exploration structures
with data-driven loot; vehicles; weather/time/sleep pressure; and a bounded
automation loop. Select exact vanilla-observable semantics with oracle evidence
and explicitly defer lower-value variants.

Add progression telemetry that records checkpoint completion without altering
gameplay. Build replayable real-client scenarios for each vertical slice and a
long combined journey. Keep worldgen Solaris-owned where accepted, but make
resource availability and exploration rewards deterministic enough to test.

Exit gate:
- a fresh no-debug real-client world sustains at least eight hours of active
  progression with multiple independent goals and no forced admin intervention;
- progression works cooperatively for at least two clients;
- resource, loot, XP, station, AI, persistence, and performance invariants have
  focused tests plus vanilla/client evidence;
- no new global lock or unbounded scan is introduced;
- full cargo, soak, and relevant client gates pass.
```

## Prompt 15 - Reach The Scoped M100 Vanilla-Parity Gate

```text
/goal
Run an evidence-driven campaign to reach the existing scoped M100 core target.
Read AGENTS.md, docs/DEFINITION_OF_DONE.md,
docs/CORE_M77_M100_ROADMAP.md, docs/milestones/M77.md through
docs/milestones/M100.md, docs/VALIDATION_LEDGER.md, Prompts 01-14 including
03B evidence, and the Shared Execution Contract. Do not create readiness by
editing statuses first.

Re-audit all 46 frozen rows. For every row, identify the smallest missing leg:
runtime behavior, vanilla oracle, real-client observation, performance,
concurrency, persistence, or accepted scope decision. Work row-by-row in
independently reviewable slices, fixing observed divergences and adding exact
evidence. Expand the M94 pack and oracle suite so broad scenarios are composed
from passing focused phases rather than blocked umbrella ids.

Target at least 37/46 conservative ready rows and no required-green O1/O2/O3,
S2, Q1, or Q2 blocker. Remaining rows must be owner-accepted non-goals or exact
documented divergences; do not relabel missing evidence as a divergence.

Exit gate:
- coverage-audit reports >=80% under its existing conservative rule;
- complete scoped real-client matrix is green with current Gradle adapter;
- vanilla oracle links exist for every counted row;
- low/balanced/high performance, multiplayer soak, autoscale, persistence, and
  security/operator gates are green;
- full cargo baseline passes;
- M100 decision remains stabilization if any hard DoD row is degraded.
```

## Prompt 16 - Expand To Broad Overworld Vanilla Parity

```text
/goal
After scoped M100 is genuinely green, expand the denominator toward broad
vanilla-observable overworld parity. Read AGENTS.md, Prompt 15 closeout, current
accepted divergences/non-goals, and the Shared Execution Contract. Create a new
versioned denominator; never rewrite M100 history.

Cover the highest-value omitted systems in vertical waves: full common
stations and inventories; broader redstone/update behavior; boats/minecarts;
entity collision, breeding, taming and richer AI; villages, villagers and
trading; structures and loot; enchanting, effects and brewing; advancements,
statistics and game events where client-visible; raids/patrols and other
overworld progression. Keep bit-perfect worldgen/RNG out unless explicitly
chosen, but match observable contracts.

Each wave requires runtime tests plus vanilla oracle and real-client evidence,
then multiplayer/performance/persistence regression. Do not start five broad
systems at once; finish one playable vertical slice before the next.

Exit gate:
- new denominator and exclusions are explicit and reproducible;
- >=90% of the chosen broad-overworld denominator is evidence-backed ready;
- every remaining divergence is observable, justified, and non-blocking for
  normal multiplayer survival;
- eight-hour progression and integrated soak remain green;
- full cargo baseline passes.
```

## Prompt 17 - Add Dimensions And Vanilla Endgame

```text
/goal
Add the missing dimension and endgame progression as evidence-backed vertical
slices. Read AGENTS.md, Prompt 16 denominator, protocol/data decisions, world
ownership/sharding design, and the Shared Execution Contract. Define a new
scope document before implementation.

Implement Nether portal creation/travel/return and dimension-scoped storage,
chunks, entities, tickets, time rules, coordinates, respawn, persistence, and
autoscale ownership. Then add the resource/progression systems needed for
brewing and Eyes of Ender, stronghold discovery, End portal travel, dragon
fight/reset semantics, credits/return, and durable endgame state. Use vanilla
data and legal oracle evidence; Solaris worldgen may remain non-bit-perfect but
must provide a testable route to required structures/resources.

Build each stage as a real-client journey with multiplayer, death/rejoin,
save/restart, cross-region ownership, and failure recovery. Preserve earlier
overworld playability and budgets.

Exit gate:
- two real clients can complete an end-to-end fresh-world progression through
  Nether and End without admin/debug intervention;
- portal/entity/player transfers are single-owner and crash-safe;
- dimension persistence, autoscale, oracle, performance, and soak gates pass;
- broad parity denominator is updated honestly;
- full cargo baseline passes.
```

## Prompt 18 - Final Core Release-Candidate Decision

```text
/goal
Perform the final Solaris core release-candidate validation. Read AGENTS.md,
docs/DEFINITION_OF_DONE.md, every active denominator/ADR, Prompts 00-17
closeouts, and operator/security docs. Validate readiness; do not implement
unrelated breadth or manufacture green rows.

Run full preflight and clean-room reproducibility. Run full cargo/Gradle/CI
baselines, complete vanilla oracle and real-client matrices, property/replay
corpus, low/balanced/high profiles, 20-player VD8, 2-4 hour chaos soak, 8+ hour
progression, save/restart/crash recovery, autoscale 1->N->1, region handoff if
in scope, dependency/license/security audit, backups/restore, and operator
install/upgrade/drain procedures.

For every failure, either fix it through a focused RED/GREEN slice and rerun the
affected matrix, or leave the goal at stabilization with an ordered blocker
list. Do not waive correctness, data safety, auth/security, or client desync.

Exit gate:
- every required evidence row is current and linked to reproducible artifacts;
- no high-severity client, corruption, concurrency, autoscale, security, or
  persistence blocker remains;
- docs, defaults, health/readiness, and operator procedures match runtime;
- all required baselines pass after the final change;
- use release-ready language only if the hard DoD and chosen parity denominator
  are actually satisfied; otherwise publish the next exact /goal blocker set.
```

## Review Notes

- The sequence deliberately postpones SIMD implementation until hot kernels and
  stable ownership boundaries are measurable. SIMD before Prompt 05-06 would
  optimize code likely to be deleted during ECS/single-writer migration.
- ECS does not own chunks, block entities, connections, or registries. Moving
  those into ECS would contradict the target architecture without a new ADR.
- Player connections remain outside ECS, but connection ownership does not
  justify mutating inventory, XP, health, pose, or container transactions in
  packet tasks; Prompt 03B moves that gameplay state to the simulation owner.
- Local adaptive throttling, independent-instance orchestration, and
  shared-world sharding are three different products. Prompts 08-11 keep their
  claims separate.
- Playability goals come after runtime migration but every architecture prompt
  retains P4/P42 or stricter real-client gates, so the server must remain
  playable throughout the program.
- "Vanilla parity" means evidence-backed client-observable behavior. It does
  not mean copied Mojang algorithms or bit-identical worldgen/RNG.
