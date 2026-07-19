# Play Hard Split Wave 21 Implementation Plan

**Goal:** move the complete use-item-on protocol adapter out of `play.rs` without changing interaction priority, gameplay ownership, block fences, inventory debit, resync or acknowledgement behavior.

## Task 1: Use-Item-On Adapter

**Files:** create `crates/mc-net/src/play/use_item_on_adapter.rs`; modify `play.rs` and direct consumers only.

- [x] Move use-item-on DTOs/preflight, interaction routing, TNT/hoe/bonemeal/plant/sign/placement adapters, loaded snapshot helpers and reject/no-op response projection.
- [x] Keep bed/toggle/bucket/campfire rules and commits in existing modules, placement planning in `block_placement`, plant rules in `plants`, simulation/storage mutation behind existing owners, and socket loop dispatch in `play.rs`.
- [x] Preserve dead/spectator/reach/world-border gates; interaction priority; block/token/inventory preconditions; creative debit rules; sign editor order; rejection resync/inventory/ack order; and no-op acknowledgement behavior.
- [x] Add no generic context trait, new lock/channel/task, sleep, polling, guessed tick wait, hidden retry or duplicate authority.

## Task 2: Boundaries And Checkpoint

- [x] Keep a coarse ownership/dependency tripwire for the adapter. Delete synthetic `xtask` unit tests that assert Rust source strings or line order; gameplay behavior is covered by real owner and wire tests.
- [x] Run focused use/placement/bonemeal tests, full `mc-net`, full workspace tests, workspace all-target strict Clippy, code-health, fmt, diff-check and independent review.
- [x] Update ADR, memory, append-only WAL, progress, exact line counts and skipped higher-level gates.
