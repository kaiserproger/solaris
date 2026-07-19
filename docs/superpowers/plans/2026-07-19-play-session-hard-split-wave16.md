# Play And Session Hard Split Wave 16 Implementation Plan

**Goal:** remove command execution and player-pose lock orchestration from the two root files while keeping existing parser, authority, packet, and publication contracts intact.

## Task 1: Player Command Execution Adapter

**Files:** create `crates/mc-net/src/play/command_execution.rs`; modify `play.rs` and command-focused tests only.

- [x] Move `execute_player_command`, give/debug/survival/game-mode/client-command execution, command feedback/time packets, status/error text helpers, and their concrete DTO plumbing.
- [x] Keep parsing/tree/suggestions in `commands.rs`; keep the socket loop and packet dispatch in `play.rs`; keep state mutation in simulation/session/world owners.
- [x] Preserve permission/plugin routing, save/stop/status/time ordering, inventory limits, teleport fences, and exact packet responses.
- [x] Use explicit imports; add no lock helper, polling, sleep, or hidden retry.

## Task 2: Player Pose Adapter

**Files:** create `crates/mc-net/src/play/session/player_pose_adapter.rs`; modify `session.rs` and focused session tests only.

- [x] Move `commit_player_pose`, test `update_pose`, accepted-pose publication, and body-push orchestration as one concrete `impl SessionRegistry`.
- [x] Keep lock-free mutation/filter/publication helpers in `player_pose_authority.rs`; keep registry fields and generic lock acquisition in `session.rs`.
- [x] Preserve session -> persistence acceptance, entity ECS push, current-snapshot stale fence, prewarm update, and body-push -> player move -> pickup ordering.
- [x] Add no new lock, async wait, direct send, or packet write.

## Task 3: Boundaries And Checkpoint

- [x] Anchor command execution groups and pose adapter entry/publication groups; reject wildcard and forbidden backedges appropriate to each adapter.
- [x] Run focused tests, full `mc-net`, strict Clippy, xtask, code-health, fmt, diff-check, and independent reviews.
- [x] Update ADR, memory, WAL, progress, exact line counts, and skipped higher-level gates.
