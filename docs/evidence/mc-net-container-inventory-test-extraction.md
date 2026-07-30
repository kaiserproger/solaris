# `mc-net` container-inventory test extraction

Date: 2026-07-30

Checkpoint base: `300875890dee18c838c874e0609ac35a8d3a2c26`

## Result

The contiguous six-test chest quick-move, item stack-limit, menu/crafting
revision, and persistent-container claim class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/container_inventory.rs`. Its exact boundary runs
from `chest_quick_move_places_player_stack_in_first_empty_storage_slot`
through
`persistent_container_claim_check_covers_furnace_and_both_chest_halves`. The
preceding `debug_give_zero_count_clears_hotbar_slot_before_item_lookup` test
and following ignored local-sidecar door parity gate remain aggregate-owned.

The child imports shared aggregate types and helpers explicitly and does not
inherit the aggregate file's `use super::*`. The extracted tests retain
player-to-chest and reverse chest-to-player quick movement, bucket and snowball
stack-limit behavior across quick-move, pickup, and quick-craft paths, exact
chest and crafting state-change counts, and persistent-claim checks for a
furnace and both halves of a double chest.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,789 physical lines and 102
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,491 | 96 |
| `play/tests/container_inventory.rs` | 309 | 6 |

The moved body contains 298 physical lines and six tests. The final structural
comparison covers all 161 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are unchanged. The combined test count remains 102.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 6 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed with `0 fail / KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 298-line body diff: empty.
- Original-versus-split function-name multiset: all 161 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding zero-count debug-give test
  and following ignored local-sidecar door parity gate remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
