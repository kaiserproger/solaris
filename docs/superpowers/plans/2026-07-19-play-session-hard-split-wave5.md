# Play And Session Hard Split Wave 5

**Goal:** move deterministic fluid planning and campfire session authority out of the remaining coordinators without changing world ownership, durability order, or D1/entity/D2 recovery.

## Task 1: Fluid Rules

**Files:** `crates/mc-net/src/play.rs`, new `crates/mc-net/src/play/fluids.rs`, and narrow caller imports in `simulation.rs`/`item_blocks.rs` if required.

- [x] Move fluid delays, scheduled planning chunks/edits, flow/interaction rules, nearby-tick planning, horizontal neighbours, fluid identifiers, and state-level construction into `fluids.rs`.
- [x] Keep due-tick queue ordering, world/resident commit, journal, relight, publication, direct storage scheduling, and interaction dispatch in `play.rs`/`simulation.rs`.
- [x] Pass concrete normalized inputs instead of importing `ServerConfig`; add no lock, async path, packet I/O, or trait layer.

## Task 2: Campfire Session Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/campfire_authority.rs`

- [x] Move cooking-registry operations, legacy commit, conditional tick/ack/cooldown, recovery probes, and regional transaction preparation into `campfire_authority.rs`.
- [x] Keep the existing registry/probe fields and initialization, transaction commit implementation, simulation queue, storage writes, packet publication, and runtime recovery coordination in their current owners.
- [x] Preserve exact legacy/regional lock order and D1 -> entity materialization -> D2 semantics; no direct production send or new lock.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused fluid/campfire tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
