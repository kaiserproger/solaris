# Restart checkpoint - 2026-07-18

## Repository state

- Branch: `dev/M100-client-agent`
- HEAD: `65c5843dd040dffc2e22228f67f0a9a3bc6b3a47`
- Persistent `/goal` remains active. Do not mark it complete.
- Shared worktree is intentionally very dirty. Do not revert or rewrite unrelated changes.
- No commit, push, merge, or tag was made.
- All three subagents were stopped before reboot.
- The Gradle 9.6 daemon used by `client-mod/solaris-client-agent` was stopped.
- No repo test/client process was left running. The long-lived CodeGraph MCP process was not touched.

## Last integrated checkpoint

The previous verified package contains:

1. Deterministic playable ruin in chunk `(4, 0)` with registry-resolved chest loot and disk persistence coverage.
2. Lua API 0.4 operator-only command roots, permission filtering, and correct bounded-queue admission reporting.
3. Narrow regional goal snapshots with referenced-target inclusion and ID-reuse fencing.

Focused tests, scoped strict Clippy, fmt, diff/no-sleep checks, and `xtask code-health` were green for that package. No full workspace test, full workspace Clippy, real-client gate, benchmark, VD8, or soak was run.

Append-only WAL state after that package:

- `.analysis/junior-readonly-wal.md` size: `3151075` bytes
- SHA-256: `3bb7dbbd1cf15ee6154bd2560fa06cb4035e1244aacfaba516467e72b10d55eb`

Do not append another WAL checkpoint until the current package is reviewed and verified. Never remove or rewrite existing WAL content.

## Current package

### Verified: raw-TCP generated ruin withdrawal/restart

`crates/mc-test-harness/tests/worldgen.rs` contains
`generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart`.

The test uses the real server/wire path on a fresh temporary world: joins, moves near the ruin, makes chunk `(4, 0)` resident, opens the generated chest, verifies exact loot, quick-moves it using current state IDs, sends `save-all` and waits for exact feedback, cleanly restarts the same world, rejoins the same player, then verifies persisted inventory and an empty chest.

Focused result before reboot: `1/1` passed in about `1.39s`.

### Unverified after interruption: Java real-client P46

The stopped worker left P46 implementation in the shared worktree. Relevant evidence is present in:

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`
- `tools/real-client-agent-driver.py`
- `tools/run-real-client-regression.sh`

Scenario IDs include `playable-46-generated-ruin-cache`, `-before`, and `-after`. The runner contains forced isolated-world handling and a clean server restart between phases. The intended path is input-driven travel to the fixed ruin, exact chest discovery from client state, quick-move of fixed loot, restart, then client-state proof that inventory persisted and the chest is empty. Screenshots are optional evidence and must not be a success condition.

This slice was stopped mid-integration. Inspect the diff and run its focused Gradle/Python/Rust manifest tests before trusting it. The actual real-client P46 gate has not been run.

### Unverified after interruption: Lua player context

The stopped worker left implementation and tests in:

- `crates/mc-script/src/lib.rs`
- `crates/mc-script/src/lua.rs`
- `crates/mc-net/src/play.rs`
- `crates/mc-server/tests/play.rs`
- `docs/PLUGINS.md`

Search anchors: `ScriptPlayerContext`, `set_player_context`, `script_player_context`, and `lua_player_command_context_distinguishes_operator_and_exposes_identity_and_position`.

The intended contract is an immutable server-authoritative Lua event snapshot with verified UUID, username, operator flag, and last accepted pose `(x, y, z)` for joined/chat/command events; no peer IP; old constructors remain compatible. The raw-TCP fixture is present but was not re-run after the worker stopped. Review and validate before treating this slice as complete.

## Resume sequence

1. Read this file, then run `git status --short --branch`; do not blanket-read the repository.
2. Inspect the P46 and Lua-context diffs around the search anchors above. Preserve unrelated shared-worktree edits.
3. Run `cargo fmt --all -- --check` before making cleanup edits.
4. Re-run the focused raw-TCP worldgen test with the owner's resource envelope:
   `nice -n 15 taskset -c 0,1 env CARGO_BUILD_JOBS=2 cargo test -p mc-test-harness --test worldgen generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart -- --exact`
5. Run focused `mc-script` Lua/player-context tests and the named `mc-server` raw-TCP test, using the same resource envelope.
6. Run the focused Java-agent tests for P46, then the Python/runner and Rust manifest tests that cover its scenario IDs. Do not launch a long real-client run until these are green.
7. Review both interrupted slices independently. Fix only concrete findings.
8. Run the shortest real-client P46 before/restart/after gate. Success must come from observed client/protocol/world events; timeout may only fail.
9. At the package boundary, run scoped strict Clippy, fmt, no-sleep checks, and `cargo run -p xtask -- code-health`. State explicitly if full workspace gates remain skipped.
10. Only after integration evidence is recorded, append a new WAL entry and update `docs/playable/ACTIVE.md`.

## Constraints to preserve

- No sleeps, guessed elapsed-time success, quiet-period success, polling, or arbitrary tick waits used as proxies. Use push notifications and exact observed events.
- Timeout is failure-only.
- Prefer direct simple code and Pareto-critical gameplay paths.
- Keep test runs short and resource-limited while the owner uses the machine.
- Do not stage `.analysis/`, `data/vanilla/`, `.serena/`, `.opencode/`, run directories, or other local artifacts.
