# Play And Session Hard Split Wave 18 Implementation Plan

**Goal:** remove bucket/cauldron interaction execution from `play.rs` and external/system outbound projection from `session.rs` while preserving existing simulation, inventory, visibility, and delivery owners.

## Task 1: Bucket Interaction Adapter

**Files:** create `crates/mc-net/src/play/bucket_interactions.rs`; modify `play.rs` and direct bucket-authority imports only.

- [x] Move bucket place/pickup, cauldron bucket handling, bucket inventory replacement, simulation commit, response, resync and animation as one concrete adapter.
- [x] Keep shared published-block preconditions and fluid scheduling in `play.rs`; keep fluid rules in `fluids`, storage commits in `block_edit_commit`, and mutation authority in simulation/session owners.
- [x] Preserve survival-only inventory debit, creative behavior, source-fluid checks, cauldron level rules, conditional tokens, reject resync-before-ack, success block/ack/inventory/animation order, and stack conservation.
- [x] Use explicit imports and direct sibling paths; add no generic context trait, new lock, sleep, polling, guessed tick wait, retry, or duplicate inventory authority.

## Task 2: Outbound Publication Adapter

**Files:** create `crates/mc-net/src/play/session/outbound_publication.rs`; modify `session.rs` only.

- [x] Move disconnect, custom payload, system/script chat, and debug pressure dispatch projection as one concrete `impl SessionRegistry`.
- [x] Keep channel lanes/backpressure/retry in `outbound.rs`, visibility recipient helpers in `visibility.rs`, registry fields and lock helpers in `session.rs`, and player state publication in `player_state_adapter.rs`.
- [x] Preserve short lock scopes, unordered direct-recipient semantics, all-session chat projection, dispatch-after-unlock, missing-session return values, and debug dispatch count.
- [x] Add no gameplay mutation, async work, packet write, raw channel operation, sleep, polling, or hidden retry.

## Task 3: Boundaries And Checkpoint

- [x] Add ownership anchors and explicit-boundary scans for both adapters.
- [x] Run focused tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check, and independent reviews.
- [x] Update ADR, memory, WAL, progress, exact line counts, and skipped higher-level gates.
