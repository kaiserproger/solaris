# Simulation Queue Split Wave 22 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** reduce `simulation.rs` by moving command admission, queue accounting, batching, and shutdown into a focused module without changing gameplay behavior or public call sites.

**Architecture:** `simulation.rs` keeps command and response types, the owner/handle structs, and all gameplay processing. New `simulation/queue.rs` owns only queue envelopes, metrics, admission methods, batch draining, owner shutdown, and channel constructors; it extends the parent types through private module-visible implementations.

**Tech Stack:** Rust, Tokio bounded MPSC and oneshot channels, existing `mc-net` tests.

## Global Constraints

- Preserve FIFO sequence ordering, the two-command background admission cap, herd coalescing, queue metrics, and existing external imports.
- Waiting remains push-driven through Tokio channels; no sleep, polling, quiet-period success, or guessed tick waits.
- Tests assert behavior and state, never Rust source strings or line order.
- Add sad-path coverage for invalid capacity, closed owner, blocked sender closure, zero drain budget, and shutdown rejection.
- Do not introduce a new runtime task, lock, trait, dependency, configuration option, or protocol behavior.

---

### Task 1: Extract Queue Ownership

**Files:**
- Create: `crates/mc-net/src/play/simulation/queue.rs`
- Modify: `crates/mc-net/src/play/simulation.rs`

**Interfaces:**
- Consumes: parent `SimulationHandle`, `SimulationOwner`, `SimulationCommand`, `SimulationOutcome`, `SimulationRequestError`, and existing regional/journal types.
- Produces: parent-visible queue constants, `SimulationQueueSnapshot`, queue methods on the existing handle/owner types, and unchanged `simulation_channel*` constructors.

- [x] Move queue constants, envelope ordering helpers, snapshots, metrics, herd enqueue claim/probe, atomic max accounting, queue-focused handle methods, queue-focused owner methods, owner `Drop`, and channel constructors into `simulation/queue.rs`.
- [x] Keep command/response enums, handle/owner struct definitions, gameplay processors, session fences, and direct consumers in `simulation.rs`.
- [x] Preserve existing queue tests and add behavioral sad-path tests for zero capacity, owner drop, waiting sender closure, zero budget, and complete shutdown draining.
- [x] Run focused queue tests and the existing full-channel regressions.

### Task 2: Review And Checkpoint

**Files:**
- Modify: `docs/decisions/0006-mc-net-module-boundaries.md`
- Modify: `docs/MEMORY.md`
- Modify: `docs/playable/ACTIVE.md`
- Append only: `.analysis/junior-readonly-wal.md`

- [x] Review the extraction for visibility leaks, duplicate queue logic, ordering changes, passive waiting, and unrelated churn.
- [x] Run full `mc-net`, workspace tests, workspace all-target strict Clippy, format check, code-health, and diff-check with bounded build resources.
- [ ] Record exact evidence, line-count reduction, skipped real-client/performance/soak gates, and commit the independently revertible slice.
