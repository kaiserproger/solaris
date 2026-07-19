# Play And Session Hard Split Wave 7

**Goal:** move deterministic random-tick rules and hostile-mob session authority out of the remaining coordinators without changing random ordering, lock order, simulation ownership, or publication.

## Task 1: Random Tick Rules

**Files:** `crates/mc-net/src/play.rs`, new `crates/mc-net/src/play/random_ticks.rs`

- [x] Move random-tick section filtering/sampling, deterministic seed/rule planning, leaf decay/drops/distance, fire, grass, and farmland rules into `random_ticks.rs`.
- [x] Keep policy DTOs/reexports, candidate grouping, resident snapshot acquisition, async fanout, world commit, journal, relight, drop spawning, and publication in `play.rs`.
- [x] Reuse existing plant planners and concrete read-only inputs; add no configuration, writer, session, lock, async, packet, or trait dependency.

## Task 2: Hostile Mob Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/hostile_authority.rs`

- [x] Move hostile attack DTOs/planning, target refresh, bed-rest exclusion, scan/commit probes, melee/skeleton tick authority, and hostile goal diffing into `hostile_authority.rs`.
- [x] Keep registry/probe fields and initialization, generic entity lifecycle/indexes, projectile authority, simulation scheduling, and actual channel delivery in their current owners.
- [x] Preserve existing session/entity lock order, save barriers, constant-work candidate scan, target fences, and release-before-publication behavior; test-only probes remain push-driven, with no production await/send or new lock.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused random-tick/hostile tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
