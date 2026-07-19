# Play And Session Hard Split

**Goal:** shrink the two remaining `mc-net` coordinator monoliths by moving two concrete domains behind explicit module boundaries without changing gameplay, packets, authority order, or lock order.

## Constraints

- No wall-clock sleeps, polling, new locks, worker percentages, or broad traits.
- No `use super::*` in extracted modules.
- Keep packet I/O and transaction coordination in `play.rs`.
- Keep registry locks and generic entity lifecycle ownership in `session.rs`.
- Preserve public paths with narrow re-exports where callers require them.

## Task 1: Campfire Rules And Persistence

**Files:** `crates/mc-net/src/play.rs`, new `crates/mc-net/src/play/campfire.rs`

- [x] Move campfire state transitions, recipes, NBT decode/encode, and block identity into `campfire.rs`.
- [x] Keep runtime ticking, crash-order coordination, storage writes, packet publication, and transaction authority in `play.rs`.
- [x] Run focused campfire unit tests. The existing wire-harness gate remains out of this bounded refactor pass.

## Task 2: Pickup Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/pickups.rs`

- [x] Move pickup result types, candidate planning, item/arrow/XP claims, spawn helpers, and pickup bookkeeping into `pickups.rs`.
- [x] Preserve entity-store -> session -> player-persistence lock order and fanout only after locks are released.
- [x] Keep generic entity cleanup, selected-item authority, and session lock ownership in `session.rs`.
- [x] Run focused atomicity, delay, owner-block, lock-release, and simulation-equivalence tests.

## Task 3: Architecture Fence And Validation

**Files:** `crates/xtask/src/main.rs`, `docs/decisions/0006-mc-net-module-boundaries.md`, `docs/MEMORY.md`, `.analysis/junior-readonly-wal.md`

- [x] Add ownership checks for both modules and forbidden coordinator dependencies.
- [x] Record the accepted boundaries and new line counts.
- [x] Run full `mc-net --lib`, strict `mc-net` clippy, fmt, code-health, and diff checks.
