# Session Module Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `play/session.rs` into focused child modules without changing server behavior.

**Architecture:** Keep `SessionRegistry` as the shared state owner. Move cohesive implementations into private child modules that use the existing types and locks; avoid new traits, wrappers, or configuration.

**Tech Stack:** Rust, Tokio, existing `mc-net` unit tests and workspace gates.

## Global Constraints

- No elapsed-time waits or polling; existing push-driven waits stay unchanged.
- No protocol or gameplay behavior changes in extraction commits.
- Preserve the current dirty worktree and do not revert unrelated changes.
- Run resource-constrained focused tests after every extraction.

---

### Task 1: Extract Pathing Support

**Files:**
- Create: `crates/mc-net/src/play/session/pathing.rs`
- Modify: `crates/mc-net/src/play/session.rs`

**Interfaces:**
- Consumes: `SessionRegistry` entity tick inputs and existing `PathingProbe` types.
- Produces: `LoadedChunkPathingProbe`, `LoadedTerrainPathingProbe`, terrain snapshot helpers, and `acquire_regional_worker_permits` with module-local implementation details.

- [ ] Move the pathing probe structs, implementations, snapshot helpers, and permit acquisition into the child module.
- [ ] Expose only symbols used by the parent module or its tests with `pub(super)`.
- [ ] Run pathing, entity tick, and collision-resolved rotation tests.

### Task 2: Extract Entity Simulation

**Files:**
- Create: `crates/mc-net/src/play/session/entity_simulation.rs`
- Modify: `crates/mc-net/src/play/session.rs`

**Interfaces:**
- Consumes: `SessionRegistry`, regional entity owners, pathing probes, physics queries, and visibility dispatch helpers.
- Produces: entity goal ticking, persistence restore/snapshot, physics application, and falling-block landing methods.

- [ ] Move the complete entity tick/restore/physics method range into one child-module `impl SessionRegistry`.
- [ ] Preserve caller-visible method visibility with `pub(in crate::play)` or `pub(crate)`.
- [ ] Run entity tick, pathing, persistence, movement, and physics tests.

### Task 3: Extract Prepared Chunk Cache

**Files:**
- Create: `crates/mc-net/src/play/session/prepared_chunks.rs`
- Modify: `crates/mc-net/src/play/session.rs`

**Interfaces:**
- Consumes: `PreparedChunkCache`, session tickets, and the existing generation notification.
- Produces: prepared frame claim, publication, invalidation, eviction, and push-driven wait methods.

- [ ] Move prepared-chunk methods and their private cache helpers together.
- [ ] Keep the notification path push-driven and preserve revision fencing.
- [ ] Run prepared-cache and chunk-stream tests.

### Task 4: Validate The Split

**Files:**
- Modify only files required by compiler diagnostics or documentation.

**Interfaces:**
- Consumes: all extracted modules.
- Produces: formatted, lint-clean `mc-net` with unchanged tests.

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo test -p mc-net` with constrained CPU/build jobs.
- [ ] Run strict `mc-net` Clippy and `cargo run -p xtask -- code-health`.
- [ ] Review the final diff for behavior changes, duplicate code, and excessive visibility.
