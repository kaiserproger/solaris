# Phase 1 closeout — 2026-08-28

## Scope

This checkpoint closes the remaining Phase-1 test-trust work in `docs/PUBLIC_ALPHA_PLAN.md`: the final aggregate `mc-net::play` test ownership cleanup (item 3) and the phase L2 closeout (item 6). Items 1, 2, 4, and 5 already have their own linked inventories/evidence.

## Final focused-test extraction

The aggregate `crates/mc-net/src/play/tests.rs` still contained 26 executable tests after the earlier extraction sequence. They are now grouped under focused sibling modules while the root remains only shared fixtures and module wiring:

- `tests/script_inventory_owner.rs` — 1 script inventory owner/persistence test;
- `tests/chunk_stream_memory_wait.rs` — 1 exact memory-publication wake test;
- `tests/player_movement_survival.rs` — 5 player pose, movement validation, exhaustion and food tests;
- `tests/world_time.rs` — 1 monotonic/world-clock separation test;
- `tests/text_component_codec.rs` — 1 oversized text-component NBT failure test;
- `tests/login_persistence.rs` — 1 corrupt-playerdata fail-closed login test;
- `tests/outbound_delivery.rs` — 6 initial sync, outbound capacity and blocked-writer tests;
- `tests/keepalive.rs` — 4 keepalive matching/liveness tests;
- `tests/dense_entity_scheduling.rs` — 6 movement/publication/goal cohort tests.

The independent closeout review then found six remaining executable campfire recovery cases still embedded in `play.rs`. Those cases were moved without assertion changes to `play/campfire_output_recovery_tests/cases.rs`; the parent inline module now contains only their shared recovery fixtures/support. The focused class passes 6/6.

No behavioral assertions or fixtures were changed by these ownership moves. Shared helpers remain in `tests.rs` and the campfire support module so focused cases reuse one setup instead of cloning test infrastructure. Exact source scans now find no `#[test]` or `#[tokio::test]` in aggregate `play/tests.rs`, `play.rs`, or `play/session.rs`. The affected crate retains the same discovered suite size and passes `1975 passed / 5 ignored`.

## L2 failures found and repaired rather than hidden

The first workspace closeout runs exposed real stale test infrastructure; none was converted into a larger timeout or ignored test.

### Correlated Luau entity spawn fixtures

Three `mc-test-harness` fixtures still called the old five-argument `solaris.spawn_entity(actor, type, x, y, z)` surface after the production API became correlated `spawn_entity(request_id, actor, type, x, y, z)`.

- `commands::lua_villager_goal_reaches_the_regional_owner_and_returns_targeted_result` now spawns with request id `spawn` and performs `bind_nearest_villager` only from a successful `on_entity_spawn_result`.
- `player_entity_interacted_lua` now uses `far-target` / `near-target` request ids and publishes each ready fence only after the corresponding committed spawn result.
- `player_entity_killed_lua` now assigns a unique `kill-target-N` request id to each spawn.

The attempted 20-second workaround used while diagnosing the first failure was fully reverted. The focused villager-goal test passes with its original 5-second failure watchdog in about 0.30 s; `commands` passes 13/13, the entity-interaction wire test passes 1/1, and the entity-kill wire test passes 1/1.

### Entity parity command pacing

`entity_parity_26_1_2::EntityProtocolHarness::write_command` contained a test-only wall-clock token bucket mirroring the production `8` command burst / `2 per second` refill and used `tokio::time::sleep` to pace a catalog of otherwise independent scenarios. Because all scenarios shared one connection, earlier setup commands consumed the bucket and `collision-step` could exceed its own 8-second fail-only scenario deadline while waiting for the harness-created refill.

The harness no longer simulates production rate limiting with time. Solaris catalog scenarios now connect through `run_isolated_scenario_catalog`: one client connection per independent scenario, each receiving the real production per-connection admission burst. The test fixture allows eight concurrent/transient players so teardown timing cannot couple later scenarios. Shared world/server state remains the same. The strict optional vanilla comparison continues to use its existing vanilla connection while Solaris uses the isolated catalog.

The test-only `command_tokens`, `command_last_refill`, and `tokio::time::sleep` path are removed. The first closeout reviewer also found newly added wait debt outside that harness: wall-clock sleeps in `survival_pickup_overflow`, yield polling in `script/router`, readiness polling in three TCP presence tests, and sleep polling in the Loader/plugin Python runners. Those paths were replaced with bounded command-burst fixtures, simulation `wait_for_command`, already-ordered wire/owner events, paused-clock explicit future polls, process waits, and MCP `state_version`/`wait_state_change` events. `minecraft_observe` now returns the version used by that event handshake. A final qualified-call scan across first-party test/gate code under `crates`, `client-mod`, and `tools` finds no direct sleep/yield progress wait; two `std::thread::yield_now()` calls remain only in production regional-owner coordination and are outside the test/gate contract. The focused Solaris entity catalog passes in about 1.8 s; the whole `entity_parity_26_1_2` binary passes 40 runnable tests with its one previously classified local-oracle ignore.

## Graphical active-feature evidence

Phase-1 item 5 is closed by the exact bucket/block-resync route documented in [`phase1-deterministic-debug-loop-2026-08-27.md`](phase1-deterministic-debug-loop-2026-08-27.md). Its fixed `m94-02b-rejected-block-resync` continuation passed under isolated local Xvfb and the existing fail-closed real-client validator accepted:

`.analysis/real-client-runs/20260828T005009Z-m94-regression-pack-HL7nZB`

This is graphical client evidence; unit/TCP results are not substituted for it.

## Ignored, feature-gated and manual rows

The final workspace run still reports explicit ignored tests. They are not unexplained failures: Phase-1 item 1 already classified the ignored, feature-gated, local-artifact, retry/quarantine/environment-sensitive, and manual/graphical classes with owners and close conditions in the linked evidence inventory, including `phase1-flaky-test-inventory.md`, `mc-net-ignored-tests.md`, `mc-server-ignored-tests.md`, `mc-test-harness-ignored-tests.md`, `mc-worldgen-ignored-tests.md`, the local-artifact inventories, and `manual-client-test-gates.md`. This closeout adds no ignore attribute and does not reinterpret an ignored gate as passing.

## Final L2 on one source tree

After the reviewer findings and all behavioral fixes, the complete L2 sequence is green on the same source tree:

- `cargo run -p xtask -- code-health` — PASS, `0 fail / KEEP`;
- `cargo test --workspace --quiet` — PASS, every runnable test group completed with zero failures. The exact command was launched inside a detached local tmux session only to avoid the CodexPro foreground execution window; it exited `0`, recorded in `.ai-bridge/l2-workspace-final.exit`, with the complete 430-line output retained at `.analysis/codex-logs/l2-workspace-final.log`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS;
- `cargo fmt --all -- --check` — PASS.

No source edit occurred between those final L2 checks; only ignored run artifacts and evidence/status synchronization followed. The final `mc-net --lib` suite remains `1975 passed / 5 ignored`.

Because the wait-discipline repair touched the real-client tooling, it was also exercised rather than accepted from syntax alone: full `bridge-core` tests pass; `python3 tools/run-plugin-client-compat-gate.py --timeout-seconds 180` passes the vanilla-client server-only economy loop and the no-Loader client-required rejection in `.analysis/plugin-client-compat/20260828T085817`; and `python3 tools/run-loader-live-gate.py fabric --timeout-seconds 180` passes permission, both Loader owners, exact bundle-cache identities and in-Play continuity in `.analysis/loader-live-gate/runs/20260828T085908-fabric`.

Benchmark: not applicable. The checkpoint repairs test fixtures/synchronization and moves tests; it does not add or optimize a production hot path.

## Review status and disposition

The first independent read-only closeout review returned **CHANGES** with two High findings: six executable campfire recovery tests still lived in `play.rs`, and the dirty tree had newly introduced wall-clock/yield polling in test/gate paths. Both findings were fixed directly rather than waived or hidden behind larger timeouts.

At the owner's request, a post-fix **detached Pi/Luna** audit then reviewed the final implementation independently. Its verdict was `CHANGES` only because the canonical plan/evidence/status text still described the pre-fix state; all six scoped implementation boundaries were **PASS** and it reported no remaining Phase-1 code or test-trust blocker. This documentation synchronization resolves those remaining findings. Pi also noted an unrelated pre-existing blank-line-at-EOF `git diff --check` finding in `play/tests/inventory_and_survival.rs`; that file is outside this closeout change set and `git diff --check` is not an L2 gate defined by this phase.

Disposition: Phase-1 items 3 and 6 are closed. Together with the previously closed items 1, 2, 4 and 5, **Phase 1 is complete**.
