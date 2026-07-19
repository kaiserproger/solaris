# Play And Session Hard Split Wave 6

**Goal:** move deterministic toggle/power rules and primed-TNT explosion authority out of the remaining coordinators without changing world commits, entity ownership, lock order, or publication order.

## Task 1: Toggle And Power Rules

**Files:** `crates/mc-net/src/play.rs`, new `crates/mc-net/src/play/toggles.rs`

- [x] Move toggle plan state, door/trapdoor/fence-gate/button/lever rules, button release delay, adjacent power propagation, and pure block-state construction into `toggles.rs`.
- [x] Keep loaded snapshot acquisition, scheduled-tick classification, world mutation, journal, relight, packet handling, and publication in `play.rs`.
- [x] Use concrete registry/state/planning inputs; add no world writer, session dependency, lock, async path, packet I/O, or trait layer.

## Task 2: Explosion Session Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/explosion_authority.rs`

- [x] Move expired-TNT and explosion target DTOs, target planning, entity impact application, due-fuse claim, chained-TNT spawn, knockback, and pure dispatch planning into `explosion_authority.rs`.
- [x] Keep player ignition transaction, registry fields, generic entity cleanup, simulation/world explosion commit, journal, relight, drops, and actual channel delivery in their current owners.
- [x] Preserve the existing session/entity guard order, `SimulationAuthority` fences, stable entity ordering, and reserve-before-deliver publication; add no await, direct send, new lock, or parent wildcard import.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused toggle/explosion tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
