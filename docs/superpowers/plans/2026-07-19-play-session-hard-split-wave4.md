# Play And Session Hard Split Wave 4

**Goal:** remove the remaining stonecutter rules from `play.rs` and shared-container registry/view state from `session.rs` without changing packet, authority, or lock order.

## Task 1: Stonecutter Domain

**Files:** `crates/mc-net/src/play.rs`, `crates/mc-net/src/play/containers.rs`, new `crates/mc-net/src/play/containers/stonecutter.rs`

- [x] Move `StonecutterWindow`, menu constants/title, input projection, recipe selection, slot mapping, result refresh, wire items, and pure click rules into `stonecutter.rs`.
- [x] Keep packet writes, open/click handlers, active-window ownership, stale fences, and simulation commits in `play.rs`.
- [x] Use concrete DTOs and registries; the child module must not import `InteractionState`.

## Task 2: Shared Container State

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/container_state.rs`, and narrow import updates in `container_views.rs`/`transactions.rs` if required.

- [x] Move container registry shards/guards, commit context/errors, viewer state, recipient planning, and container-specific test probes into `container_state.rs`.
- [x] Keep registry fields and initialization, unregister lifecycle order, generic inventory/drop authority, and actual dispatch in `session.rs`.
- [x] Preserve literal lock order: session to container for registration/unregister, container to player persistence for transactions, and no send under either lock.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused stonecutter/shared-container tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
