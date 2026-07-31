# Break/drop/pickup lock and latency gate

Date: 2026-07-31

Checkpoint base: `e335aab` (`feat(plugins): complete deployment reporting`)

## Result

The release-candidate item-path gate is now reproducible without a graphical client or local Mojang sidecars. The ignored raw-TCP test
`two_hundred_torch_break_drop_pickups_stay_below_lock_and_tick_budgets` builds an embedded generated world, prepares 200 torch targets, and performs 200 complete survival actions through the production server:

1. move the player to the target;
2. break the torch through `ServerboundPlayerAction`;
3. observe the exact air update and block acknowledgement;
4. observe one item entity with the exact torch stack;
5. observe the owner-committed pickup animation and entity removal.

The gate resets only the global lock counters in the test-only `load-bench` build, then requires all 200 block edits, item pickups, take dispatches, and removal dispatches. It waits for one exact 1,200-tick telemetry window and enforces stricter limits than the plan requires:

- session-registry **maximum** wait and hold below 5 ms;
- player-persistence **maximum** wait and hold below 5 ms;
- no sampled tick at or above 50 ms.

Because every measured wait/hold is below 5 ms, the run also cannot emit the existing M39 wait/hold warnings at 10/25 ms for item spawn, pickup credit, or loaded-recipient paths.

## Optimization

The first complete run exposed one shared survival-break critical section at roughly 5.5 ms for both `session_registry` and `player_persistence`. The hot path held session/entity state and player persistence while applying the world edit and preparing item publication.

The implementation now:

- snapshots pickup player state under a short lock, performs inventory merge planning outside it, and commits a prebuilt inventory after exact revalidation;
- applies the survival block edit without session or player locks;
- rolls the block edit back through exact resulting mutation tokens if the session or held stack becomes stale before publication;
- commits only the changed held slot under the player lock;
- spawns item entities through the entity owner before taking the session lock;
- publishes routing, wire state, visibility, pickup readiness, and despawn scheduling in a separate short session section.

The existing stale-block, stale-session, held-stack mismatch, concurrent-winner, requester-loss, peer-ordering, and persistence regressions remain green. The held-stack mismatch test now also proves the new post-world-edit rollback path restores the original block without tool damage or drop publication.

## Measured debug run

Exact command:

```sh
cargo test -p mc-test-harness --test block_edit \
  two_hundred_torch_break_drop_pickups_stay_below_lock_and_tick_budgets \
  -- --ignored --exact --nocapture
```

First complete baseline before the lock split:

| Metric | Baseline maximum |
| --- | ---: |
| Session-registry hold | 5,266 us |
| Player-persistence hold | 5,263 us |
| Tick | 9,823 us |

Final strict run after optimization:

| Metric | Final maximum |
| --- | ---: |
| Session-registry wait | 40 us |
| Session-registry hold | 2,178 us |
| Player-persistence wait | 1 us |
| Player-persistence hold | 285 us |
| Tick | 9,643 us |

The final post-review-fix tick window contained 1,200 samples: p50 3,898 us, p95 4,857 us, p99 5,306 us, max 9,643 us. All 200 actions completed and all four authoritative counters increased by exactly 200.

These are local debug-build measurements, not owner release-host throughput evidence. The gate closes the item-path lock/M39 acceptance row; it does not replace broader release, graphical, worldgen, or multiplayer soak gates.

## Validation

- strict 200-action raw-TCP gate: passed;
- `cargo test -p mc-net`: 1,859 passed;
- `cargo test -p mc-test-harness`: passed across the full package; documented external gates remained ignored;
- `cargo test -p mc-test-harness --test block_edit`: 35 passed, 70 documented ignored gates;
- focused survival-break transaction tests: 11 passed;
- focused item-pickup transaction tests: 10 passed;
- focused lock-metric tests: 2 passed;
- `cargo clippy --workspace --all-targets -- -D warnings`: passed;
- `cargo fmt --all -- --check`: passed;
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`;
- `git diff --check`: passed;
- the same independent read-only reviewer rechecked its two P1 fixes and documentation finding: `No findings.`

A full `cargo test --workspace` attempt was externally terminated after 58 seconds while tests were still green. The complete affected-package suites above passed; the workspace-wide gate is not claimed complete from that attempt.
