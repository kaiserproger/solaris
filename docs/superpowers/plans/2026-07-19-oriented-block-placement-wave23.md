# Oriented Block Placement Wave 23 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** make ordinary stair and slab placement choose the expected facing and top/bottom state instead of always using the registry default.

**Architecture:** keep state selection in the pure `block_placement` planner. The use-item-on adapter supplies the existing player yaw, clicked face, and cursor Y; simulation ownership, conditional edits, acknowledgement, resync, and inventory debit remain unchanged.

**Tech Stack:** Rust, existing block-state registry, mc-net placement tests, raw-TCP test harness.

## Global Constraints

- Do not touch `simulation.rs` or `simulation/queue.rs`.
- Preserve signs, doors, cactus, support checks, mutation fences, acknowledgement order, resync, and creative/survival debit semantics.
- Use exact registry properties; unsupported blocks retain their default state.
- The local 26.1.2 oracle is `StairBlock.getStateForPlacement` and
  `SlabBlock.getStateForPlacement`: facing is the player's horizontal
  direction; top is selected for `DOWN`, or for a horizontal face when the
  world hit Y relative to the placed cell is greater than `0.5`; `0.5`, `UP`,
  and lower horizontal hits select bottom.
- No sleeps, polling, guessed ticks, source-string assertions, new dependencies, or broad abstractions.
- Cover accepted placement and sad-path no-mutation/no-debit behavior.

---

### Task 1: Stair And Slab State Selection

**Files:**
- Modify: `crates/mc-net/src/play/block_placement.rs`
- Modify: `crates/mc-net/src/play/use_item_on_adapter.rs`
- Modify: `crates/mc-net/src/play/tests.rs`
- Modify if needed for wire coverage: `crates/mc-test-harness/tests/block_edit/placement_rejection.rs`

**Interfaces:**
- Consumes: clicked face, cursor Y, player yaw, item-to-block registry mapping, and existing loaded placement snapshot.
- Produces: the existing `PlannedBlockPlacement` with an oriented block state and unchanged edit/debit contract.

- [x] Add failing behavioral tests for four stair facings, stair halves, slab top/bottom, and default-state fallback.
- [x] Thread cursor Y into the planner and derive only properties present on the resolved registry state family.
- [x] Prove occupied target and invalid support reject without mutation or inventory debit and retain acknowledgement/resync behavior.
- [x] Run focused planner and wire tests, full `mc-net`, strict scoped Clippy, fmt, code-health, and diff-check.
