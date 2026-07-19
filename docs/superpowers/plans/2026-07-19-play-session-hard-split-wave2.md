# Play And Session Hard Split Wave 2

**Goal:** continue shrinking the coordinators with one menu domain and one publication domain, preserving gameplay and authority boundaries.

## Task 1: Crafting Domain

**Files:** `crates/mc-net/src/play.rs`, `crates/mc-net/src/play/containers.rs`, new `crates/mc-net/src/play/containers/crafting.rs`

- [x] Move crafting window state, slot maps, recipe matching/repair/result rules, projections, and wire-item construction into `crafting.rs`.
- [x] Keep click application, packet I/O, `InteractionState`, active-container ownership, and simulation commits in `play.rs`.
- [x] Preserve current crafting behavior and public paths with narrow re-exports.

## Task 2: Visibility Publication

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/visibility.rs`; `outbound.rs` only if DTO ownership must move to avoid a cycle.

- [x] Move player/entity visibility mirror updates, recipient planning, snapshot publication, and movement publication helpers into `visibility.rs`.
- [x] Keep registration, view/ticket/cache orchestration, generic entity mutation, and lock ownership in `session.rs`.
- [x] Preserve release-before-fanout and current-state publication fences without adding a gameplay lock.

## Task 3: Fence And Validate

- [x] Add ownership and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused crafting/visibility tests, full `mc-net --lib`, strict Clippy, fmt, code-health, and independent review.
