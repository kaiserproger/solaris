# Play And Session Hard Split Wave 15 Implementation Plan

**Goal:** isolate synchronous falling-block rules and player persistence/state authority without changing world mutation, inventory conservation, save wakeups, or publication order.

## Task 1: Falling Block Rules

**Files:** create `crates/mc-net/src/play/falling_blocks.rs`; modify `play.rs`, direct imports in `simulation.rs`, and focused tests only.

- [x] Move start/landing DTOs, chunk collection, state/cell classification, start planning, and landing planning behind explicit imports.
- [x] Keep async start/landing orchestration, world locks, conditional commits, relight, entity mutation, drops, packet translation, and delivery in their existing owners.
- [x] Preserve stable input order, deduplicated chunk order, complete mutation-token preconditions, sequential multi-candidate landing projection, blocked-to-drop behavior, and unloaded/out-of-height fail-closed behavior.
- [x] Add no lock, async, sender, packet, session, or coordinator-state dependency.

## Task 2: Player State And Persistence Authority

**Files:** modify `crates/mc-net/src/play/session/player_state.rs`, `session.rs`, and existing focused tests only.

- [x] Move player persistence registration/recovery, active shield publication, container inventory commit, save snapshot/ack/generation/wait/notify methods into the existing player-state owner.
- [x] Preserve session/entity/persistence lock order, drop materialization atomicity, disconnected-generation ABA fence, spectator index updates, and exact notification-driven save waiting.
- [x] Keep registry fields and lock acquisition helpers in `session.rs`; add no generic lock wrapper, polling, guessed tick, or elapsed-time success condition.
- [x] Keep method signatures stable for Play, simulation, server, session lifecycle, and tests.

## Task 3: Boundaries And Checkpoint

- [x] Add multiple ownership anchors and reject wildcard/coordinator/async/send/packet backedges appropriate to each module.
- [x] Run focused rules and persistence/inventory/save tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check, and independent reviews.
- [x] Update ADR, memory, WAL, progress, exact line counts, and explicitly skipped higher-level gates.
