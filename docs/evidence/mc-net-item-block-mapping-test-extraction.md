# `mc-net` item-to-block mapping test extraction

Date: 2026-07-30

Checkpoint base: `28632e0734bb8770797dbf530794ec4509d6a23e`

## Result

The contiguous four-test registry-derived item-to-block and standing/wall sign
mapping class moved from aggregate `crates/mc-net/src/play/tests.rs` to the
focused Rust module
`crates/mc-net/src/play/tests/item_block_mapping.rs`. Its exact boundary runs
from `item_to_block_table_is_registry_derived` through
`sign_items_choose_floor_or_wall_sign_for_clicked_face`. The preceding
`local_dev_profiles_are_op_capable_for_now` test and following ignored
local-sidecar door parity gate remain aggregate-owned.

The child imports production types and shared aggregate test helpers
explicitly and does not inherit the aggregate file's `use super::*`. The
extracted tests retain registry-derived direct mapping and rejection,
stonecutter placement mapping, torch standing/use-on mapping, and floor-sign,
wall-sign, and rejected-down-face selection.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 5,127 physical lines and 110
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,987 | 106 |
| `play/tests/item_block_mapping.rs` | 149 | 4 |

The moved body contains 140 physical lines and four tests. The final structural
comparison covers all 169 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are unchanged. The combined test count remains 110.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 4 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed with `0 fail / KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 140-line body diff: empty.
- Original-versus-split function-name multiset: all 169 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding local-development
  permission test and following ignored local-sidecar door parity gate remain
  aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
