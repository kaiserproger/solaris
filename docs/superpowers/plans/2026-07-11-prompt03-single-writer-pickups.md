# Prompt 03 Single-Writer Pickup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route authoritative item and XP entity claims through one bounded,
observable simulation owner before entity goals and physics run.

**Architecture:** Add an explicit `SimulationHandle`/`SimulationOwner` channel
pair. Network tasks enqueue typed claim commands; the existing entity ticker is
the sole command consumer and applies mutations synchronously through
`SessionRegistry`.

**Tech Stack:** Rust 2024, Tokio bounded `mpsc` and `oneshot`, existing
`SessionRegistry`, Prompt 02 replay/load/client gates.

## Global Constraints

- Keep `unsafe_code = "forbid"` and add no dependency.
- Queue capacity is 1024 and per-tick command budget is 256.
- Preserve all owner changes in the dirty worktree; do not commit or stage in
  this session unless the owner explicitly asks.
- Do not add ECS, migrate blocks/containers, or claim full single-writer status.
- Production network code must have no direct item/XP claim mutation call.

---

### Task 1: Bounded simulation channel

**Files:**
- Create: `crates/mc-net/src/play/simulation.rs`
- Modify: `crates/mc-net/src/play.rs`
- Test: `crates/mc-net/src/play/simulation.rs`

**Interfaces:**
- Produces: `simulation_channel() -> (SimulationHandle, SimulationOwner)`,
  `SimulationQueueSnapshot`, `SimulationRequestError`, and typed item/XP
  requests.

- [x] Add RED tests proving capacity 1024 is bounded, a full queue returns
  `SimulationRequestError::Full`, sequences increase, and cancellation is
  visible. Keep `SimulationOwner` non-`Clone` by API construction/review.
- [x] Run `cargo test -p mc-net --lib play::simulation -- --nocapture`; expect
  compilation/test failure because the channel contract does not exist.
- [x] Implement envelopes with `AtomicU64` sequence allocation, atomic queue
  telemetry, oneshot outcomes, and non-cloneable receiver ownership.
- [x] Rerun the focused command and require every simulation module test green.

### Task 2: Owner-only item and XP application

**Files:**
- Modify: `crates/mc-net/src/play/session.rs`
- Modify: `crates/mc-net/src/play/simulation.rs`
- Test: `crates/mc-net/src/play/session.rs`

**Interfaces:**
- Consumes: sequenced `SimulationCommand` envelopes.
- Produces: `SimulationOwner::process_tick(&SessionRegistry, 256)` and typed
  `Option<ClaimedPickup>` / `Option<ClaimedExperience>` outcomes.

- [x] Add a RED dual-path test: seed identical item and XP entities, execute the
  same ordered claims through test-only legacy helpers and through the owner,
  and compare winner count, item/XP totals, remaining entities, and dispatch
  kinds.
- [x] Add RED tests that a 256-command tick leaves later commands queued and
  that cancelled/shutdown commands do not mutate entities.
- [x] Move claim mutation behind one owner application method. Keep any direct
  helper available only to tests after production migration.
- [x] Run `cargo test -p mc-net --lib simulation -- --nocapture` and the existing
  `concurrent_item_pickup`/`concurrent_xp_pickup` session tests.

### Task 3: Server owner phase and telemetry

**Files:**
- Modify: `crates/mc-net/src/server.rs`
- Modify: `crates/mc-net/src/play.rs`
- Test: `crates/mc-net/src/server.rs`

**Interfaces:**
- Produces: one owner moved into `BoundServer::serve`, one handle cloned into
  each connection, and queue fields on `RuntimeTelemetrySnapshot`.

- [x] Add RED server telemetry coverage for queue capacity/depth/counters;
  combine focused owner shutdown/no-mutation coverage with the existing
  entity-ticker-drain-before-final-save regression.
- [x] Create the channel in `bind_internal`, pass the handle through
  `handle_connection` and `play::handle`, and process one bounded batch before
  `tick_entities_and_collect_physics_queries`.
- [x] Close/reject the owner queue on ticker shutdown and publish its metrics.
- [x] Run `cargo test -p mc-net --lib server::tests -- --nocapture`.

### Task 4: Production pickup authority transfer

**Files:**
- Modify: `crates/mc-net/src/play.rs`
- Modify: `crates/mc-net/src/play/tests.rs`
- Modify: `crates/mc-net/src/play/tests/inventory_and_survival.rs`

**Interfaces:**
- Consumes: `SimulationHandle::claim_item_pickup` and
  `claim_experience_pickup`.
- Produces: network pickup paths with no direct authoritative entity mutation.

- [x] Convert `concurrent_pickup_tasks_conserve_item_and_xp_entities` to run a
  real owner pump, then add a RED source/behavior guard proving queue rejection
  leaves inventory, XP, and entities unchanged.
- [x] Replace direct item/XP claim calls in production `play.rs`; treat full or
  closed queue as fail-closed retryable no-pickup and never apply local rewards.
- [x] Keep visibility dispatch and socket writes after the owner outcome.
- [x] Run `cargo test -p mc-net --lib concurrent_pickup -- --nocapture`, then all
  442+ `mc-net` library tests.

### Task 5: Slice closeout

**Files:**
- Modify: `docs/milestones/M90.md`
- Modify: `docs/milestones/M96.md`
- Modify: `docs/VALIDATION_LEDGER.md`
- Modify: this plan

**Interfaces:**
- Produces: exact evidence and explicit remaining legacy mutation inventory.

- [x] Run Prompt 02 checked concurrent replay and the short fallback soak.
- [x] Run focused clippy, format, code-health, and `git diff --check`.
- [x] Run `cargo test --workspace` and full workspace clippy after the slice.
- [ ] Run P4 and P42 through the fixed Gradle real-client adapter after all
  Prompt 03 slices, not as a substitute for this slice's focused tests.
- [x] Record exact queue metrics, replay equivalence, skipped gates, and all
  still-legacy spawn/combat/block/container/world mutation paths.
