# `mc-net` enchanting and recipe-settlement test extraction

Date: 2026-07-30

Checkpoint base: `8040dc4ad675b2cb6383134c1165d69cc53b0b05`

## Result

The contiguous five-test enchanting projection and recipe-settlement class
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module
`crates/mc-net/src/play/tests/enchanting_recipe_settlement.rs`. Its exact
boundary runs from
`stale_enchanting_click_rebuilds_inputs_from_owner_projection` through
`placed_recipe_commits_inventory_and_publishes_aggregate_craft`. The preceding
shared `interaction_state_for_items` helper and following shared
`register_interaction_player` helper remain aggregate-owned. The child module
uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain stale enchanting projection recovery, fail-closed
disconnect settlement, bounded self-recreating max-craft behavior, large
aggregate craft accounting, and owner-committed recipe/script-event
publication coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 10,524 physical lines and 201
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 10,206 | 196 |
| `play/tests/enchanting_recipe_settlement.rs` | 333 | 5 |

The moved body contains 317 physical lines and five tests. The final structural
comparison covers all 266 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are checked below. The combined test count remains 201.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 5 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 317-line body diff: empty.
- Original-versus-split function-name multiset: all 266 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared
  `interaction_state_for_items` and `register_interaction_player` helpers
  remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
