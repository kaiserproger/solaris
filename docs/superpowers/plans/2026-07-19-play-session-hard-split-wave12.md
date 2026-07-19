# Play And Session Hard Split Wave 12

**Goal:** finish the synchronous crafting click domain and isolate session
registration/teardown lifecycle without changing inventory conservation,
connection fencing, wakeups, visibility order, or persistence.

## Global Constraints

- Preserve behavior and packet layouts; this wave adds no gameplay.
- Use explicit imports and concrete request/result values. Add no trait layer,
  parent wildcard import, sleep, polling, new lock, or direct channel send.
- Preserve every existing push notification and lock order.
- Keep async simulation commits and packet writes in `play.rs`; keep registry
  fields and lock helpers in `session.rs`.

## Task 1: Crafting Click Rules

**Files:** `crates/mc-net/src/play.rs`,
`crates/mc-net/src/play/containers/crafting.rs`,
`crates/mc-net/src/play/inventory.rs`

- [x] Move generic stack/slot click primitives needed by crafting beside
  `PlayerInventory`, with explicit item-registry inputs rather than a parent
  `InteractionState` dependency.
- [x] Move 2x2 and 3x3 crafting menu projection/mutation, pickup, swap, throw,
  quick-move, result taking, ingredient consumption, and container remainders
  into `containers::crafting` using concrete inputs/results.
- [x] Keep packet decoding, carried-hash/stale checks, active-window ownership,
  async owner commit/rollback, persistence, item-entity publication, window
  state IDs, packet writes, close/disconnect recovery, and logging in `play.rs`.
- [x] Preserve exact slot maps, transactional quick-move capacity behavior,
  repair consumption, bucket remainder, and item conservation.

## Task 2: Session Registration Lifecycle

**Files:** `crates/mc-net/src/play/session.rs`, new
`crates/mc-net/src/play/session/session_lifecycle.rs`

- [x] Move register/try-register, active-session queries, exact empty-session
  wait, unregister and unregister-preserving-player-state orchestration into
  `session_lifecycle.rs`.
- [x] Keep fields, lock helpers, generic entity lifecycle, persistence storage,
  and actual outbound delivery in their existing owners.
- [x] Preserve max/duplicate admission, player visibility ordering, prepared
  cache and container cleanup, sleep quorum recomputation, last-session pushed
  event, persistence choice, and all current lock ordering.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and boundary scans for both domains;
  update ADR 0006, memory, and append-only WAL.
- [x] Run focused crafting conservation/recovery and session register/unregister
  tests, full `mc-net`, strict Clippy, fmt, code-health, and independent reviews.
