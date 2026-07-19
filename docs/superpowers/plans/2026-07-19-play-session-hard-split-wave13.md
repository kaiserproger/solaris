# Play And Session Hard Split Wave 13 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** isolate player attack/shield rules and player-pose authority from the two remaining coordinators without changing combat, collision push, visibility, persistence, or publication behavior.

**Architecture:** `play::combat::player_actions` owns synchronous player attack cadence, held-weapon damage, shield state/rules, and inventory durability mutation through explicit inputs and results. `play::session::player_pose_authority` owns accepted-pose mutation, body-push authority, and pose publication planning while `session.rs` keeps fields, guard acquisition, persistence, prewarm orchestration, and actual delivery.

**Tech Stack:** Rust 2024 workspace, Tokio synchronization, existing `mc-data`, `mc-entity`, `mc-physics`, `mc-protocol`, and `xtask code-health` boundaries.

## Global Constraints

- Preserve behavior and packet layouts; this wave adds no gameplay.
- No sleep, polling, new lock, trait layer, wildcard parent import, direct channel send, or direct packet write in either new owner.
- Keep async work, simulation routing, guard acquisition, persistence, and delivery in the current coordinators.
- Preserve attack recharge, Sharpness, shield activation/front arc/durability, entity CAS, session/entity lock order, stale publication fences, stable dispatch order, pickup wakeups, and prewarm updates.
- Use the resource-light command prefix `nice -n 15 taskset -c 0,1 env CARGO_BUILD_JOBS=2`.
- Do not commit; this branch contains a large intentional working tree.

---

### Task 1: Player Action Rules

**Files:**
- Create: `crates/mc-net/src/play/combat/player_actions.rs`
- Modify: `crates/mc-net/src/play/combat/mod.rs`
- Modify: `crates/mc-net/src/play.rs`
- Modify: `crates/mc-net/src/play/survival.rs`
- Test: existing combat/shield tests in `crates/mc-net/src/play/tests.rs` and `crates/mc-net/src/play/tests/inventory_and_survival.rs`

**Interfaces:**
- Consumes: `ItemFactsTable`, `ItemRegistry`, `ItemStack` values/slices, `InteractionHand`, `GameMode`, `Vec3`, explicit tick and slot values.
- Produces: `ShieldUseState` plus concrete helpers for attack damage/recharge, shield creation/validation/metadata flags/blocking/durability mutation, and weapon-durability policy.

- [x] Move `ShieldUseState`, `held_attack_speed`, `held_attack_damage_at_tick`, `begin_player_attack_attempt`, `player_horizontal_look_direction`, `shield_hand_slot`, shield identity/state/flags/front-arc/durability rules, and `weapon_attacks_damage_held_item` into the new owner.
- [x] Narrow the former `survival::held_attack_damage` and Sharpness calculation to explicit inventory/registry inputs in the new owner; keep unrelated survival rules in `survival.rs`.
- [x] Leave `InteractionState` adaptation, pending-action clearing, world-time acquisition, `set_active_shield`, metadata dispatch, packet writes, simulation commits, persistence, and async durability publication in `play.rs`.
- [x] Add or retain direct unit coverage for attack recharge, spectator rejection, Sharpness, shield hand/flags, activation delay, front arc, finite/non-finite durability, breakage, and stale held-stack refresh.
- [x] Run `nice -n 15 taskset -c 0,1 env CARGO_BUILD_JOBS=2 cargo test -p mc-net play::tests:: -- --nocapture` with focused filters where practical; expected result is all selected tests pass.

### Task 2: Player Pose Authority

**Files:**
- Create: `crates/mc-net/src/play/session/player_pose_authority.rs`
- Modify: `crates/mc-net/src/play/session.rs`
- Test: `crates/mc-net/src/play/session/tests.rs`

**Interfaces:**
- Consumes: already-acquired `SessionRegistryInner`/`EntityStore` guards, `SessionId`, `PlayerPose`, candidate IDs, and current expected entity snapshots.
- Produces: `AcceptedPlayerPose` and ordered `VisibilityDispatch` plans for entity body pushes and accepted player movement.

- [x] Move `PLAYER_BODY_HALF_WIDTH`, `AcceptedPlayerPose`, accepted-pose mutation, body-candidate capture, entity body-push mutation, current-snapshot filtering, body-push publication, and accepted-pose completion into the new owner.
- [x] Keep registry/entity fields, `lock_inner`/`lock_entities`, player persistence mutation, prewarm frontier update, test pause probes, pickup candidate selection, and actual delivery in `session.rs`; pass guards or concrete callbacks/results instead of reacquiring through a facade.
- [x] Preserve session -> player-persistence order for pose commit, separate entity and session turns during body push, current-snapshot stale fence, chunk-index/visibility ordering, movement-before-pickup dispatch order, and test visit accounting.
- [x] Retain focused tests for same-chunk movement, chunk crossing, sparse candidate visits, body collision push, stale entity replacement, entity-index movement, pickup dispatch, and pose persistence.
- [x] Run focused `cargo test -p mc-net play::session::tests::...` commands under the resource-light prefix; expected result is all selected tests pass.

### Task 3: Boundaries And Checkpoint

**Files:**
- Modify: `crates/xtask/src/main.rs`
- Modify: `docs/decisions/0006-mc-net-module-boundaries.md`
- Modify: `docs/MEMORY.md`
- Modify: `.superpowers/sdd/progress.md`
- Append only: `.analysis/junior-readonly-wal.md`

**Interfaces:**
- Consumes: the final owner function names from Tasks 1 and 2.
- Produces: ownership anchors, forbidden-dependency scans, exact validation evidence, and durable staged-migration notes.

- [x] Add multiple ownership anchors for attack/shield and pose/body-push groups; reject parent globs, sleep/polling, lock acquisition, async, sender/direct dispatch, packet, persistence, and world-writer dependencies according to each boundary.
- [x] Run focused tests, full `cargo test -p mc-net`, strict all-target Clippy, `cargo fmt --all -- --check`, `git diff --check`, `cargo run -p xtask -- code-health`, and independent reviews.
- [x] Record exact line counts, review fixes, passed gates, and skipped workspace/performance/soak/wire/real-client gates in ADR, memory, progress, and append-only WAL.
