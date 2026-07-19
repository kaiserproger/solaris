# Prompt 04 Shadow ECS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` task by task. Keep each task independently
> testable and review the dirty worktree before any commit.

**Goal:** Run standalone `bevy_ecs` beside the authoritative `EntityStore`,
compare normalized entity state and semantic events, and move no client-visible
authority.

**Architecture:** `mc-entity` owns the shadow ECS data model and deterministic
single-threaded schedules. The existing `EntityStore` remains authoritative and
feeds the shadow the same typed operations. `mc-net` compares both runtimes at
owner phase boundaries and records the first divergence as a replay artifact;
connections, chunks, block entities, and static registries stay outside ECS.

**Tech Stack:** Rust 1.94, edition 2024, `bevy_ecs 0.18.1` with only the `std`
feature, existing `mc-entity` types, existing core replay DTOs.

## Global Constraints

- Never use wall-clock sleep or polling; tests wait on exact commands/events.
- Keep legacy `EntityStore` authoritative throughout Prompt 04.
- Keep ECS schedules single-threaded and deterministic.
- Do not put chunks, block entities, connections, Tokio handles, or static
  registries into ECS.
- Keep Solaris crates under `unsafe_code = "forbid"`.
- The `Cargo.lock` change is justified only by the standalone ECS dependency.
- Do not stage or commit unrelated dirty-worktree paths.

---

### Task 1: ECS Component Model And Exact Snapshot Round-Trip

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/mc-entity/Cargo.toml`
- Create: `crates/mc-entity/src/shadow.rs`
- Modify: `crates/mc-entity/src/lib.rs`
- Test: `crates/mc-entity/src/shadow.rs`

**Interfaces:**
- Produces: `ShadowEntityRuntime::new()`,
  `insert_snapshot(EntitySnapshot) -> bool`,
  `remove(EntityId) -> Option<EntitySnapshot>`,
  `snapshot(EntityId) -> Option<EntitySnapshot>`, and
  `normalized_snapshots() -> Vec<EntitySnapshot>`.
- Produces components for stable identity, type, transform, motion, lifecycle,
  health/attributes, AI goal, item stack, XP value, block/projectile state,
  vehicle/passenger state, persistence, and visibility state.

- [ ] Write `all_supported_components_round_trip_exactly`, using one item, XP
  orb, hostile mob, arrow, falling block, boat with passenger, and minecart.
- [ ] Run `cargo test -p mc-entity shadow -- --nocapture`; verify RED because
  `ShadowEntityRuntime` does not exist.
- [ ] Add `bevy_ecs = { version = "0.18.1", default-features = false,
  features = ["std"] }` to workspace dependencies and `mc-entity`.
- [ ] Implement component bundles, stable-id index, exact insertion/removal,
  deterministic sort by runtime `EntityId`, and snapshot reconstruction.
- [ ] Run focused tests and strict `mc-entity` clippy.

### Task 2: Typed Shadow Operations And Explicit Schedules

**Files:**
- Modify: `crates/mc-entity/src/shadow.rs`
- Modify: `crates/mc-entity/src/lib.rs`
- Test: `crates/mc-entity/src/shadow.rs`

**Interfaces:**
- Consumes: Task 1 `ShadowEntityRuntime`.
- Produces: `ShadowEntityCommand` variants for spawn/restore, transform and
  motion updates, goal updates, item updates, damage/lifecycle, vehicle state,
  and removal.
- Produces: ordered `ShadowStage::{InputAi, SnapshotRequest, PhysicsApply,
  CombatLifecycle, PersistenceExtract, OutputEvents}` schedules and
  `run_stage(stage)`.
- Produces: normalized `ShadowSemanticEvent` and persistence extraction output.

- [ ] Write a mixed-operation test that enqueues commands in a known order and
  proves no stage applies work owned by a later stage.
- [ ] Write damage/death, vehicle/passenger, projectile/falling-block, item/XP,
  removal, and persistence/restart tests.
- [ ] Implement one queue resource per stage and a real system that drains each
  queue; do not add empty marker systems.
- [ ] Run the same operation corpus twice and assert byte-for-byte normalized
  snapshots and events.
- [ ] Run focused tests and strict `mc-entity` clippy.

### Task 3: Dual Execution And Tick-Boundary Comparison

**Files:**
- Modify: `crates/mc-entity/src/lib.rs`
- Modify: `crates/mc-net/src/play/session.rs`
- Modify: `crates/mc-net/src/play/simulation.rs`
- Modify: `crates/mc-net/src/server.rs`
- Test: `crates/mc-entity/src/shadow.rs`
- Test: `crates/mc-net/src/play/simulation.rs`
- Test: `crates/mc-net/src/play/session.rs`

**Interfaces:**
- Consumes: Task 2 typed operations and schedules.
- Produces: `ShadowComparison { tick, snapshots, events }` and
  `ShadowDivergence { tick, stage, entity_id, legacy, shadow }`.
- Produces: read-only counters for compared ticks, compared entities, and first
  divergence; no client packet path consumes ECS output.

- [ ] Add a failing session test for spawn -> AI -> physics -> damage -> death
  where legacy and ECS snapshots/events compare at every owner phase.
- [ ] Feed every authoritative `EntityStore` operation to shadow in the same
  owner turn; compare after command drain, AI snapshot, physics apply,
  combat/lifecycle, and persistence extraction.
- [ ] Deliberately perturb only the test shadow and prove comparison reports the
  exact first tick/stage/entity without changing legacy output.
- [ ] Expose comparison counters in existing runtime telemetry.
- [ ] Run all `mc-entity` and `mc-net` tests plus strict clippy.

### Task 4: Replay Artifact, Mixed Soak, And Benchmarks

**Files:**
- Create: `crates/mc-entity/benches/shadow_schedule.rs`
- Modify: `crates/mc-test-harness/src/replay.rs`
- Modify: `crates/mc-test-harness/src/bin/core_replay_validate.rs`
- Create: `tools/core-replay-scenarios/prompt04-shadow-mixed.json`
- Modify: `docs/playable/ACTIVE.md`
- Modify: `docs/decisions/0004-staged-single-writer-simulation.md`

**Interfaces:**
- Consumes: Task 3 comparison and divergence DTOs.
- Produces: first-divergence replay JSON under `.analysis/` during runtime,
  checked mixed replay input, and legacy-vs-shadow benchmark reports.

- [ ] Build a checked replay covering item, XP, passive/hostile mob, arrow,
  falling block, implemented boat/minecart behavior, death, despawn,
  persistence, and restart.
- [ ] Run an accelerated one-hour-equivalent simulation by advancing explicit
  simulation events; elapsed wall time is never an assertion.
- [ ] Record legacy-only and legacy-plus-shadow entity density benchmarks.
- [ ] Run `cargo run -p xtask -- code-health`, workspace tests, workspace
  clippy, and fmt check. Run the existing real-client smoke only to prove that
  authority did not move; do not claim client-visible ECS behavior.

## Self-Review

- Spec coverage: all required components, six schedules, shadow comparison,
  first-divergence replay, mixed families, persistence/restart, and benchmarks
  map to Tasks 1-4.
- Scope boundary: chunks, block entities, connections, and registries remain
  outside ECS; authority transfer is reserved for Prompt 05.
- Pareto boundary: Task 1 proves the data model, Task 2 proves execution, Task 3
  proves integration, and Task 4 supplies the expensive exit evidence. Do not
  harden rare component combinations before the mixed corpus exposes them.
