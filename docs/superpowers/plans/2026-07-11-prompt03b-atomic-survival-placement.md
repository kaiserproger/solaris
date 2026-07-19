# Prompt 03B Atomic Survival Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:test-driven-development` and execute this plan task-by-task.

**Goal:** Commit survival placement block edits and the selected inventory-stack
debit as one session-fenced simulation-owner transaction.

**Architecture:** The connection keeps reach, clicked-face, item-to-block,
orientation, collision, and multi-block shape planning. It submits bounded edits,
all edit/support CAS preconditions, and the exact selected stack. The owner
validates the registered player snapshot, applies world edits and one-item debit
during one owner turn, publishes peer block deltas before responding, and
returns immutable block/inventory snapshots for requester wire follow-up.

**Tech Stack:** Rust 2024, Tokio bounded `mpsc`/`oneshot`, `WorldStorage`,
`SessionRegistry`, Prompt 02 replay/load and embedded Minecraft MCP gates.

## Global Constraints

- Add no dependency and keep `unsafe_code = "forbid"`.
- Reuse the existing four-attempt bounded `WorldBusy` policy.
- Stale block/support token, stale session, selected-stack mismatch, queue
  rejection, or unavailable world mutates neither world nor inventory.
- Preserve requester order: block delta, ack, inventory slot; peers receive the
  block delta owner-side before the response.
- Keep placement planning, lighting, sign editor state, hopper scheduling,
  generic block edits, and explicit save barriers outside this transaction.
- Do not commit, stage, or rewrite unrelated dirty-worktree changes.

## Tasks

- [x] Add RED exact-commit coverage for block placement plus one-item debit.
- [x] Add bounded command/result types and complete edit/support CAS validation.
- [x] Add stale CAS/session/held-stack, requester-loss, same-target one-winner,
  peer-event ordering, and transient/persistent `WorldBusy` tests.
- [x] Cut `handle_block_item_placement` over to owner snapshots and remove the
  second local decrement and peer rebroadcast.
- [x] Preserve TCP block/ack/slot ordering, multi-block doors, signs, hoppers,
  rejection resync, and existing same-target contention conservation.
- [x] Run all `mc-net` tests, 66 block-edit tests, checked replay, short 4+1 soak,
  and a no-debug MCP earned-block placement scenario.
- [x] Run full workspace tests/clippy/format/code-health and record exact staged
  debt without claiming Prompt 03B complete.

## Evidence

- `cargo test -p mc-net --lib --quiet`: 511 passed.
- `cargo test -p mc-test-harness --test block_edit`: 66 passed.
- `cargo test -p mc-test-harness --test physics_validation`: 15 passed. The
  observer now completes from the exact ack/block/entity/slot packets instead
  of a post-ack quiet-time guess.
- Same-target placement contention, checked replay, and short 4+1 client soak
  passed.
- Embedded client MCP scenario `playable-02b-natural-crafting-table-open`
  passed on a fresh seed-0 debug world: the client earned a birch log, crafted
  planks and a crafting table, placed the table, and opened `CraftingScreen`.
  The MCP endpoint reported 17 tools. The scenario used the default
  `run/mcp-artifacts` directory and required no screenshot argument or
  screenshot assertion.
- Client shutdown returned the expected signal status 130. Solaris flushed 107
  dirty chunks, saved 55 entities, and exited 0; ports 25565/39095 were clear.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo fmt --all -- --check`, and `cargo run -p xtask --
  code-health` passed. Workspace tests still contain explicitly ignored
  sidecar/oracle/performance gates; this is not a replacement-readiness claim.
- Prompt 03B still has staged crafting/container cursor, food/active-use, bow
  release/debit/durability, damage/death/respawn/pose, and save-barrier player
  transactions before the ECS shadow phase.
