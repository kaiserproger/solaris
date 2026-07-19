# Play And Session Hard Split Wave 14 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** isolate synchronous bed rules and simulation-owned player item commits from `play.rs` and `session.rs` without changing sleep, inventory conservation, lock order, or publication order.

**Architecture:** `play::beds` owns canonical bed geometry, occupancy/obstruction planning, wake candidates, respawn pose, and morning calculation through published read inputs. `play::session::player_item_action_authority` owns the existing food, bow, selected-drop, and TNT owner commits with their distinct lock scopes intact.

**Tech Stack:** Rust 2024 workspace, existing simulation owner, published world views, regional/world mutation APIs, and `xtask code-health`.

## Global Constraints

- No sleep, polling, guessed ticks, new locks, trait layer, parent wildcard import, direct sends, or packet writes.
- Preserve bed two-half canonicalization, ABA mutation-token fences, exact wake order, and published-view reads.
- Preserve food session→persistence; bow/drop entity→session→persistence; TNT world→entity→session→persistence lock order. Do not deduplicate these distinct lock scopes.
- Keep packet/session orchestration and async commit adapters in `play.rs`; keep registry fields and generic lock helpers in `session.rs`.
- Use `nice -n 15 taskset -c 0,1 env CARGO_BUILD_JOBS=2` for Cargo commands. Do not commit.

---

### Task 1: Bed Rules

**Files:**
- Create: `crates/mc-net/src/play/beds.rs`
- Modify: `crates/mc-net/src/play.rs`
- Modify: `crates/mc-net/src/play/session/sleep.rs` only for direct `next_morning_time` import
- Test: `crates/mc-net/src/play/tests.rs`

**Interfaces:**
- Consumes: block/light facts, `BlockRegistry`, published world read view, clicked/canonical positions, `PlayerPose`, `GameMode`, and hostile-nearby boolean.
- Produces: exact occupancy edit/precondition plans, canonical bed/respawn pose, obstruction result, safe wake pose, and morning time.

- [x] Move `plan_bed_occupied_edits`, monster/obstruction rules, loaded bed planning, `next_morning_time`, respawn/canonical geometry, wake offsets/support/yaw rules, and synchronous wake planning into `beds.rs` with explicit imports.
- [x] Keep `interact_with_bed`, `set_bed_occupied`, `wake_player_from_bed`, packet DTO construction/writes, hostile query, async commits, and publication in `play.rs`.
- [x] Preserve matching-half validation, mutation tokens, fail-closed malformed/unloaded behavior, the exact 12 wake candidates, support/fluid/campfire/collision checks, and saturating morning arithmetic.
- [x] Add direct mismatched-half and stale-token regressions, then run all focused bed/sleep tests.

### Task 2: Player Item Action Authority

**Files:**
- Create: `crates/mc-net/src/play/session/player_item_action_authority.rs`
- Modify: `crates/mc-net/src/play/session.rs`
- Test: existing owner tests in `crates/mc-net/src/play/simulation.rs`

**Interfaces:**
- Consumes: existing `SessionRegistry`, plans/results from `simulation.rs` and `explosions.rs`, and existing domain helpers.
- Produces: the unchanged `commit_tnt_ignition`, `commit_food_use`, `commit_bow_release`, and `commit_selected_item_drop` method surface.

- [x] Move the four commit methods as one concrete `impl SessionRegistry`; leave sheep test mutation in `session.rs`.
- [x] Keep every existing stale fence and each method's current lock scope; add no generic player-state lock helper.
- [x] Keep simulation dispatch after gameplay locks, TNT block delta before entity publication, requester-loss behavior, and inventory debit/entity creation atomicity.
- [x] Run focused food/bow/drop/TNT owner tests and exact lock/publication regressions.

### Task 3: Boundaries And Checkpoint

**Files:**
- Modify: `crates/xtask/src/main.rs`
- Modify: `docs/decisions/0006-mc-net-module-boundaries.md`
- Modify: `docs/MEMORY.md`
- Modify: `.superpowers/sdd/progress.md`
- Append only: `.analysis/junior-readonly-wal.md`

**Interfaces:**
- Consumes: final owner symbols from Tasks 1 and 2.
- Produces: multiple ownership anchors, explicit boundary scans, review evidence, and staged-migration records.

- [x] Reject coordinator/session/writer/lock/async/send/packet backedges according to each boundary and anchor every major rule/commit group.
- [x] Run focused tests, full `mc-net`, strict Clippy, fmt, diff-check, code-health, and independent reviews.
- [x] Record exact line counts, review fixes, passed evidence, and explicitly skipped higher-level gates.
