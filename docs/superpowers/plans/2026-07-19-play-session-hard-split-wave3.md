# Play And Session Hard Split Wave 3

**Goal:** remove the next two cohesive domains from the large coordinators without changing gameplay authority, packet order, or lock ownership.

## Task 1: Enchanting Domain

**Files:** `crates/mc-net/src/play.rs`, `crates/mc-net/src/play/containers.rs`, new `crates/mc-net/src/play/containers/enchanting.rs`

- [x] Move enchanting window state, offer rules, bookshelf counting, slot projections, supported-item rules, and wire-item construction into `enchanting.rs`.
- [x] Keep packet handling, active-container ownership, click application, and simulation commits in `play.rs`.
- [x] Preserve current menu behavior and paths with narrow re-exports and explicit imports.

## Task 2: Projectile Domain

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/projectiles.rs`

- [x] Move arrow spawn, expiry, entity-hit planning, knockback math, segment/AABB intersection, and projectile candidate scans into `projectiles.rs`.
- [x] Keep generic entity lifecycle, kill rewards, hostile combat coordination, and session/entity lock acquisition in `session.rs`.
- [x] Pass the existing concrete guards into projectile operations; do not add a lock or send from the module.

## Task 3: Fence And Validate

- [x] Add ownership and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused enchanting/projectile tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
