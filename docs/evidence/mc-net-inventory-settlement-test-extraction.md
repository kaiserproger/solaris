# `mc-net` inventory-settlement test extraction

Date: 2026-07-30

Checkpoint base: `a4adb206f55dafe04a647003da62120693b23472`

## Result

The contiguous fourteen-test inventory settlement, recovery, checkpoint, and
publication class moved from aggregate `crates/mc-net/src/play/tests.rs` to the
focused Rust module
`crates/mc-net/src/play/tests/inventory_settlement.rs`. Its exact boundary runs
from
`disconnected_cursor_is_preserved_when_simulation_owner_is_unavailable`
through `inventory_result_paths_publish_only_after_owner_commit`. The preceding
shared `interaction_state_for_items` helper and following
`stonecutter_test_recipe` helper class remain aggregate-owned. The child module
uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain coverage for disconnected cursor settlement,
crafting-grid recovery, checkpoint persistence, owner-turn conservation,
stale-click rebuilds, aggregate craft counts, FIFO publication fencing, and
script event publication after accepted owner commits.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 12,234 physical lines and 227
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 11,111 | 213 |
| `play/tests/inventory_settlement.rs` | 1,150 | 14 |

The moved body contains 1,123 physical lines and fourteen tests. The final
structural comparison covers all 295 function names from the original
aggregate module. The exact original-versus-extracted body diff is empty, and
the sorted function-name multiset is identical. The combined test count
remains 227.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 14 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 1,123-line body diff: empty.
- Original-versus-split function-name multiset: all 295 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared `interaction_state_for_items`
  helper and following stonecutter helper class remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
