# Play And Session Hard Split Wave 8

**Goal:** move the complete synchronous scheduled-block/hopper subsystem and herd-spawn session authority out of the remaining coordinators without changing due-tick ownership, durability, lock order, or publication.

## Task 1: Scheduled Block And Hopper Domain

**Files:** `crates/mc-net/src/play.rs`, new `crates/mc-net/src/play/scheduled_blocks.rs`

- [x] Move scheduled-block planning DTOs/rules, comparator signal rules, hopper transfer planning/execution, furnace/campfire insertion, hopper geometry, placement ticks, and backfill scheduling into `scheduled_blocks.rs`.
- [x] Keep due-tick discovery/claim/requeue/fences, regional fanout and fallback, world lock acquisition, resident/global commit, journal, relight, invalidation, broadcasts, container dispatch, and publication in `play.rs`.
- [x] Preserve exact synchronous storage/session authority calls and transfer ordering; use explicit imports and add no async/await, packet delivery, direct send, lock acquisition, parent wildcard, sleep, or polling.

## Task 2: Herd Spawn Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/herd_spawn_authority.rs`

- [x] Move herd outcome/claim DTOs, claim probes, grouped herd admission, pending-hostile activation, owner commit/rollback, spawn candidate construction, distance/cap rules, and committed publication installation into `herd_spawn_authority.rs`.
- [x] Keep registry/probe fields and initialization, lock helper definitions, generic entity facts/indexes, simulation retry orchestration, sleep/world-time orchestration, and actual channel delivery in their current owners.
- [x] Preserve session/entity lock order, journal outcome handling, exact retry rules, UUID dedupe, stable batch publication, and push-driven test probes; add no new lock, production send/await, wildcard import, sleep, or polling.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused scheduled-block/hopper/herd tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
