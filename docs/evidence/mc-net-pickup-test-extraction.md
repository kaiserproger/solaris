# `mc-net` pickup test extraction

Date: 2026-07-30

Checkpoint base: `436292ea81e7f589055e2958df9a9789b4f80631`

## Result

The contiguous three-test item, XP, and arrow-pickup class moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pickup.rs`. Its exact boundary runs from
`concurrent_pickup_tasks_conserve_item_and_xp_entities` through
`full_simulation_queue_leaves_item_pickup_state_unchanged`. The preceding
shared `register_survival_test_player` helper and following
`assert_attack_damage_close` helper remain aggregate-owned. The child module
uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain concurrent item/XP claimant conservation, grounded
arrow pickup inventory publication, and fail-closed full simulation-queue
behavior.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 8,640 physical lines and 176
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 8,402 | 173 |
| `play/tests/pickup.rs` | 250 | 3 |

The moved body contains 238 physical lines and three tests. The final
structural comparison covers all 241 function names from the original
aggregate module. The exact original-versus-extracted body diff and the sorted
function-name multiset are checked below. The combined test count remains 176.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 3 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 238-line body diff: empty.
- Original-versus-split function-name multiset: all 241 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared
  `register_survival_test_player` and `assert_attack_damage_close` helpers
  remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
