# Simulation Regional Mutation Split Wave 24 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** move the regional block/container mutation worker lane out of `simulation.rs` after the queue split, reducing the coordinator by about 1,100 more lines without changing authority, journal, or publication behavior.

**Architecture:** new `simulation/regional_mutation.rs` extends the existing `SimulationOwner` with regional job preparation and execution. The parent keeps command classification, queueing, batch orchestration, shared routing DTOs, world access, lighting, and publication helpers.

**Tech Stack:** Rust, existing regional owners and WAL transactions, Tokio owner lanes, mc-net behavior tests.

## Global Constraints

- Execute only after Wave 22 is reviewed and committed.
- Preserve sorted lane admission, leases, mutation-token fences, WAL reservation and decision order, all-or-nothing transaction behavior, response order, and post-commit publication.
- Do not move or duplicate queue logic, command routing predicates, lighting/publication helpers, or `SimulationOwner` itself.
- No new lock, task, trait, dependency, config, sleep, polling, source-string test, or protocol behavior.
- Failures must leave world, inventory, drops, and publication unchanged unless the existing durable outcome proves commit.

---

### Task 1: Extract Regional Mutation Lane

**Files:**
- Create: `crates/mc-net/src/play/simulation/regional_mutation.rs`
- Modify: `crates/mc-net/src/play/simulation.rs`

**Interfaces:**
- Consumes: parent regional routing DTOs, command envelopes, world access, journal transactions, lighting/publication helpers, and existing authority handles.
- Produces: the existing `SimulationOwner::process_regional_block_edit_run` behavior and test-only regional probe through parent-visible methods/types.

- [ ] Move regional job/probe/result types and `process_regional_block_edit_run` into the child module with the narrowest visibility that compiles.
- [ ] Retain accepted, stale-token, requester-loss, cross-region, journal-failure, closed-response, and partial-failure behavioral coverage.
- [ ] Run focused regional mutation tests, full `mc-net`, strict scoped Clippy, fmt, code-health, and diff-check.

### Task 2: Review And Checkpoint

**Files:**
- Modify: `docs/decisions/0006-mc-net-module-boundaries.md`
- Modify: `docs/MEMORY.md`
- Modify: `docs/playable/ACTIVE.md`
- Append only: `.analysis/junior-readonly-wal.md`

- [ ] Review authority, journal, ordering, visibility, and negative-code boundaries; fix all important findings.
- [ ] Record exact line reduction and validation evidence, then commit the independently revertible slice.
