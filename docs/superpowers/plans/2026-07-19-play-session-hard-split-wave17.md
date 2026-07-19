# Play And Session Hard Split Wave 17 Implementation Plan

**Goal:** remove the block-edit commit/publication pipeline from `play.rs` and player-state event/publication orchestration from `session.rs` without changing authority, lock order, packet order, or failure behavior.

## Task 1: Block Edit Commit Adapter

**Files:** create `crates/mc-net/src/play/block_edit_commit.rs`; modify `play.rs` only.

- [x] Move conditional storage edits, scheduled-tick admission, opaque block-entity fencing, light-change classification, loaded resyncs, visible outcome finalization, and player block acknowledgements as one concrete adapter.
- [x] Keep block planning in existing gameplay modules, simulation ownership in `simulation.rs`, world storage ownership in `mc-world`, and socket dispatch in `play.rs`.
- [x] Preserve precondition-before-mutation order, mutation tokens, scheduled ticks only for applied positions, baked-light handling, campfire cleanup, invalidation, block/light packet order, and acknowledgement behavior.
- [x] Use explicit imports and direct functions; add no generic context trait, new lock, sleep, polling, guessed tick wait, retry, or buffered duplicate authority.

## Task 2: Player State Adapter

**Files:** create `crates/mc-net/src/play/session/player_state_adapter.rs`; modify `session.rs` only.

- [x] Move `commit_player_state_event` and player animation/entity-data publication methods as one concrete `impl SessionRegistry`.
- [x] Keep persistence/inventory/survival authority in `player_state.rs`, sleep policy in `sleep.rs`, visibility selection in `visibility.rs`, registry fields and generic locks in `session.rs`, and actual delivery in `outbound.rs`.
- [x] Preserve session/persistence lock order, validation, spectator/sleep transition order, including-self behavior, and return-only dispatch semantics.
- [x] Add no async work, direct send, packet write, lock helper, sleep, polling, or hidden retry.

## Task 3: Boundaries And Checkpoint

- [x] Add ownership anchors and explicit-boundary scans for both adapters.
- [x] Run focused tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check, and independent reviews.
- [x] Update ADR, memory, WAL, progress, exact line counts, and skipped higher-level gates.
