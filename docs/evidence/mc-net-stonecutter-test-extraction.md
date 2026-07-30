# `mc-net` stonecutter test extraction

Date: 2026-07-30

Checkpoint base: `b6236f17315034c02e3a9861f67533ee0a48f1e4`

## Result

The contiguous twelve-test stonecutter class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/stonecutter.rs`. Its exact boundary runs from the
`stonecutter_test_recipe`, `stonecutter_test_items`, and
`register_stonecutter_owner` helpers through
`stonecutter_close_reopen_conserves_input_through_one_owner_turn`. The
preceding shared `interaction_state_for_items` helper and following
`stale_enchanting_click_rebuilds_inputs_from_owner_projection` test remain
aggregate-owned. The child module uses explicit imports and does not inherit
the aggregate file's `use super::*`.

The extracted tests retain stonecutter recipe filtering, invalid-selection,
quick-move capacity, owner-commit publication, stale snapshot/session
rejection, disconnect/rejoin recovery, owner-projection rebuild, and
close/reopen conservation coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 11,111 physical lines and 213
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 10,524 | 201 |
| `play/tests/stonecutter.rs` | 614 | 12 |

The moved body contains 589 physical lines, three local helpers, and twelve
tests. The final structural comparison covers all 281 function names from the
original aggregate module. After normalizing the two extracted helper
signatures from private to `pub(super)`, the original-versus-extracted body
diff is empty and the sorted function-name multiset is identical. The narrow
visibility change keeps the two existing aggregate fixtures that consume those
helpers working; no helper is visible outside the parent test module. The
combined test count remains 213.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 12 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-extracted 589-line body diff after the two necessary
  `pub(super)` visibility normalizations: empty.
- Original-versus-split function-name multiset: all 281 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared `interaction_state_for_items`
  helper and following enchanting test remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
