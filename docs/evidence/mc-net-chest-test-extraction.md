# `mc-net` chest test extraction

Date: 2026-07-30

Checkpoint base: `3c0f20d56b6f3a38d0eb848959233ff76f5a790a`

## Result

The contiguous four-test chest class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/chest.rs`. Its exact boundary runs from
`chest_window_swap_and_throw_mutate_storage_slots` through
`shared_chest_same_version_click_commits_once_and_conserves_items`. The
preceding shared `decode_player_position_sync_packets` helper and following
`spawn_test_simulation_owner` helper remain aggregate-owned. The child module
uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain chest swap/throw mutation, stale-click resync,
world-content/state-id snapshot pairing, shared same-version commit
uniqueness, and item conservation coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 8,931 physical lines and 180
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 8,640 | 176 |
| `play/tests/chest.rs` | 303 | 4 |

The moved body contains 291 physical lines and four tests. The final structural
comparison covers all 245 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are checked below. The combined test count remains 180.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 4 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 291-line body diff: empty.
- Original-versus-split function-name multiset: all 245 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared
  `decode_player_position_sync_packets` and `spawn_test_simulation_owner`
  helpers remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
