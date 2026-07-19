# Prompt 03B Atomic Survival Break Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:test-driven-development` and execute this plan task-by-task.

**Goal:** Commit a timed survival block break, held-tool durability damage, and
all deterministic item-drop spawns as one session-fenced simulation-owner
transaction.

**Architecture:** The connection remains responsible for mining-time and reach
validation, deterministic edit/drop planning, direct socket writes, and its
temporary inventory mirror. A bounded `CommitSurvivalBreak` command validates
the complete block mutation-token set and expected held stack, then applies
world edits, the persisted inventory snapshot, and item-entity spawns during one
owner turn. The owner publishes peer block deltas and all drop visibility before
responding; the connection mirrors the committed inventory and performs only
requester wire/light follow-up.

**Tech Stack:** Rust 2024, Tokio bounded `mpsc`/`oneshot`, `WorldStorage`,
`SessionRegistry`, existing Prompt 02 replay/load and embedded Minecraft MCP
gates.

## Global Constraints

- Add no dependency and keep `unsafe_code = "forbid"`.
- Reuse the existing four-attempt bounded `WorldBusy` policy.
- A stale block token, stale session, held-stack mismatch, queue rejection, or
  unavailable world must mutate neither block, tool, nor drop entities.
- Preserve current requester ordering: block update, break ack, then queued drop
  spawn; peers must receive block removal before drop spawn.
- Keep creative breaking, mining-time validation, lighting, fluid scheduling,
  falling-block follow-up, campfire cleanup, and save barriers explicitly out of
  this transaction.
- Do not commit, stage, or rewrite unrelated dirty-worktree changes.

---

### Task 1: RED transaction conservation tests

**Files:**
- Modify: `crates/mc-net/src/play/simulation.rs`

- [x] Add owner tests for exact block/tool/drop commit, stale CAS rejection,
  requester loss after apply, stale-session rejection, and two-session
  same-target contention with one exact winner.
- [x] Run the focused tests and require failure because
  `CommitSurvivalBreak`/its response do not exist yet.

### Task 2: Owner transaction contract

**Files:**
- Modify: `crates/mc-net/src/play.rs`
- Modify: `crates/mc-net/src/play/session.rs`
- Modify: `crates/mc-net/src/play/simulation.rs`
- Modify: `crates/mc-net/src/play/survival.rs`

- [x] Add bounded command/result types carrying edits, complete CAS
  preconditions, expected selected stack, optional durability limit, and
  deterministic item drops.
- [x] Preflight every world position and player field before mutation; then
  apply edits, inventory replacement, and infallible entity spawns under one
  owner turn.
- [x] Dispatch peer `BlockDeltas` before item visibility and before the response;
  invalidate prepared chunks from the committed owner outcome.
- [x] Add the same bounded transient-`WorldBusy` retry behavior used by
  conditional block edits.
- [x] Run the focused owner tests to GREEN, then run all simulation tests.

### Task 3: Timed survival-break cutover

**Files:**
- Modify: `crates/mc-net/src/play.rs`
- Modify: `crates/mc-net/src/play/session.rs`
- Modify: `crates/mc-net/src/play/tests.rs`
- Modify: `crates/mc-test-harness/tests/block_edit/breaks_and_crafting.rs`

- [x] Extend the named legacy inventory sync to include selected hotbar slot.
- [x] Plan complete CAS preconditions and drops from one world snapshot, submit
  the owner command, mirror only its returned inventory, and remove the second
  local durability mutation/direct drop spawn from timed survival breaking.
- [x] Reuse requester block/light/falling/fluid follow-up without rebroadcasting
  owner-published peer block deltas.
- [x] Extend the real TCP break/drop/pickup regression to require exactly one
  durability update and preserve block-update/ack/drop ordering.

### Task 4: Slice validation and evidence

**Files:**
- Modify: `docs/decisions/0004-staged-single-writer-simulation.md`
- Modify: `docs/playable/ACTIVE.md`
- Modify: this plan

- [x] Run focused owner and TCP tests, all `mc-net` library tests, the 66-test
  block-edit harness, checked Prompt 02 replay, and short 4+1 slow-reader soak.
- [x] Run an MCP no-debug natural break/drop/pickup/craft scenario and require a
  clean final save and process exit.
- [x] Run `cargo test --workspace`, workspace clippy with warnings denied,
  `cargo fmt --all -- --check`, `xtask code-health`, source/diff audits, and
  process/port cleanup checks.
- [x] Record exact evidence and leave lighting/fluid/falling/campfire/save-barrier
  requester-loss gaps explicit; do not claim Prompt 03B complete.

## Evidence

- The first focused test failed because `SurvivalBreakPlan`, held/drop payloads,
  and `SimulationHandle::commit_survival_break` did not exist, then passed after
  the owner contract was implemented.
- Nine owner tests cover exact commit, stale block/session/held-stack rejection,
  requester loss after apply, two-session one-winner conservation, transient and
  bounded persistent `WorldBusy`, and peer block-before-drop ordering.
- `mc-net` passes 498/498 library tests; focused clippy passes with warnings
  denied. All 66 block-edit tests and the sugar-cane multi-edit/drop regression
  pass through the production owner pump.
- Prompt 02 checked replay passes on two fresh disk worlds. The short 4+1 soak
  passes with five transaction samples, 135 entity spawns, four bounded
  reliable retries, one maximum retry in flight, and a final 1-entity/28-chunk
  save.
- The embedded 17-tool MCP passed `playable-02a-natural-log-to-planks` on a
  fresh seed-0 debug world: a natural birch log became air, its visible drop was
  picked up, and one log produced four planks. Solaris flushed 84 chunks in its
  final save and exited 0; no client/server listener remained.
- The first workspace run exposed a pre-existing shield fixture that equated a
  350 ms wall sleep with five simulation ticks. The fixture now waits for five
  observed `ClientboundSetTime` ticks; its focused and six-test suite pass, and
  the complete workspace test was rerun from the start.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo fmt --all -- --check`, and `cargo run -p xtask --
  code-health` pass. Workspace tests marked ignored remain unpromoted.
