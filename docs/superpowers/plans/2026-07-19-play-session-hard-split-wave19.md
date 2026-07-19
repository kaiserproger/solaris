# Play And Session Hard Split Wave 19 Implementation Plan

**Goal:** remove player damage orchestration from `play.rs` and pure interaction geometry from `session.rs` without changing combat rules, survival authority, entity ownership, or packet behavior.

## Task 1: Player Damage Adapter

**Files:** create `crates/mc-net/src/play/player_damage_adapter.rs`; modify `play.rs` only.

- [x] Move fall, campfire-contact and general player damage application, damage publication projection, melee knockback packet projection, and their concrete DTOs.
- [x] Keep damage formulas/shield rules in `combat`, survival commits in existing simulation/session owners, movement rules in `movement`, and socket dispatch in `play.rs`.
- [x] Preserve mode/death gates, armor and shield durability, exact owner retry bound, state/inventory/xp publication, death cleanup, packet order and no-state fallback behavior.
- [x] Use explicit imports and direct functions; add no generic context trait, new lock, sleep, polling, guessed tick wait, retry beyond the existing exact shield retry, or duplicate authority.

## Task 2: Interaction Geometry

**Files:** create `crates/mc-net/src/play/session/interaction_geometry.rs`; modify `session.rs` and direct geometry consumers only.

- [x] Move player-chunk proximity, distance/AABB/entity geometry, player eye/block center and block/entity reach calculations as one pure module.
- [x] Keep entity facts in `mc-data`, physics primitives in `mc-physics`, spawn chunk conversion in `spawn`, and gameplay/visibility decisions in their existing owners.
- [x] Preserve baby dimensions, eye-height fallback, creative/survival reach limits, box-distance math and simulation-distance clamping.
- [x] Add no registry/session access, lock, async work, packet write, channel, sleep, polling, or cached duplicate facts.

## Task 3: Boundaries And Checkpoint

- [x] Add ownership anchors and explicit-boundary scans for both modules.
- [x] Run focused tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check, and independent reviews.
- [x] Update ADR, memory, WAL, progress, exact line counts, and skipped higher-level gates.
