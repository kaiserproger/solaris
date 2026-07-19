# Wall Torch Placement Wave 25 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** place the ordinary torch item as a correctly faced wall torch on horizontal sturdy supports while retaining standing torch placement on top faces.

**Architecture:** keep item-to-standing-block resolution unchanged. The pure block-placement planner recognizes only `minecraft:torch`, selects `minecraft:wall_torch` for a horizontal clicked face, resolves its facing from the support direction, and rejects missing/non-sturdy support before the existing simulation commit.

**Tech Stack:** Rust, 26.1.2 block-state/collision tables, mc-net planner tests, raw-TCP harness.

## Global Constraints

- Do not touch `simulation.rs` or its child modules.
- Local oracle: `StandingAndWallBlockItem.getPlacementState` plus `WallTorchBlock.getStateForPlacement/canSurvive` from the decompiled 26.1.2 server.
- For a normal adjacent placement, wall-torch facing equals the clicked horizontal face; support is at `target.relative(facing.opposite())`.
- Preserve one authoritative inventory debit, conditional placement, resync-before-ack rejection, and standing torch placement on `UP`.
- No generic support framework, new dependency, sleep, polling, guessed tick wait, or source-string test.
- Ordinary torch only; redstone/soul/copper torches, neighbour break cascades, particles, and complete irregular-face support parity are out of scope.

---

### Task 1: Ordinary Wall Torch Selection

**Files:**
- Modify: `crates/mc-net/src/play/block_placement.rs`
- Modify: `crates/mc-net/src/play/tests.rs`
- Modify: `crates/mc-test-harness/tests/block_edit/furnace_and_chests.rs`
- Modify: `crates/mc-test-harness/tests/block_edit/placement_rejection.rs`

**Interfaces:**
- Consumes: resolved standing torch state, clicked face, loaded target/support snapshot, exact wall-torch block states, and existing placement commit contract.
- Produces: the existing `PlannedBlockPlacement` containing a standing or wall torch state.

- [x] Add failing registry-backed tests for all four wall facings, standing top placement, and unsupported wall rejection.
- [x] Select the wall state and validate a conservative full sturdy support face without changing unrelated block placement.
- [x] Add raw-TCP accepted and rejected coverage proving exact state, one/no debit, resync, and acknowledgement order.
- [x] Run focused tests, full `mc-net`, scoped strict Clippy, fmt, code-health, and diff-check.
