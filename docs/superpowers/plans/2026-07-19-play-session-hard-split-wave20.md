# Play And Session Hard Split Wave 20 Implementation Plan

**Goal:** move campfire protocol/persistence orchestration out of `play.rs` and finish the generic entity-lifecycle registry adapter in `session.rs` without changing mutation authority, durable ordering, visibility, or packet behavior.

## Task 1: Campfire Adapter

**Files:** create `crates/mc-net/src/play/campfire_adapter.rs`; modify `play.rs` and direct campfire consumers only.

- [x] Move campfire use/tick/hydration/recovery orchestration, concrete reports and campfire block-entity packet projection.
- [x] Keep campfire rules/NBT in `campfire`, CAS in `session/campfire_authority`, simulation materialization in `simulation`, shared resident journal machinery in `play.rs`, and generic packet/inventory helpers with their owners.
- [x] Preserve success and stale response order, exact block/inventory/cooking fences, D1 -> entity -> D2 recovery order, resident-only tick/hydration behavior, prepared-chunk invalidation and no-debit rejection.
- [x] Add no context trait, new lock/channel/task, sleep, polling, guessed tick wait, hidden retry or duplicate authority.

## Task 2: Entity Lifecycle Adapter Completion

**Files:** modify `crates/mc-net/src/play/session.rs` and `crates/mc-net/src/play/session/entity_lifecycle.rs` only.

- [x] Move falling-block/command-entity spawn registry adapters and dying-entity tick adapter into the existing lifecycle owner.
- [x] Preserve public-in-Play signatures, one existing session/entity lock turn, spawn indexing/wire order, authoritative facts and death completion order.
- [x] Add no facade trait, new lock/channel/task, sleep, polling or gameplay rule.

## Task 3: Boundaries And Checkpoint

- [x] Add exact ownership anchors and explicit-boundary scans for the campfire adapter and lifecycle adapters.
- [x] Run focused tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check and independent reviews.
- [x] Update ADR, memory, append-only WAL, progress, line counts and skipped higher-level gates.
