# `mc-net` bed/sleep test extraction

Date: 2026-07-30

Checkpoint base: `ece82c23f13661bffb2811132d65a2d75ec4be1d`

## Result

The contiguous twelve-test bed interaction and sleep-planning class moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/bed_sleep.rs`. Its exact boundary begins with the
`bed_occupancy_test_registry` helper and
`bed_respawn_pose_uses_block_above_bed`, and ends with
`sleep_skip_targets_the_next_morning`. The preceding block-placement/NBT class
and following `common_container_paper_cuts_resolve_to_existing_menus` test
remain in their existing modules. The child module uses explicit imports and
does not inherit the aggregate file's `use super::*`.

The extracted tests retain coverage for bed pose and reservation identity,
matching-half validation, ABA-safe occupancy commits, writer-independent
planning, suffocation obstruction, nearby hostile blocking, safe wake
selection, and next-morning time calculation.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 12,778 physical lines and 241
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 12,310 | 229 |
| `play/tests/bed_sleep.rs` | 479 | 12 |

The moved body contains 468 physical lines, twelve tests, and one private
registry helper. The final structural comparison covers all 310 function names
from the original aggregate module. An exact original-versus-extracted body
comparison has an empty diff, and the sorted function-name multiset is
identical. The combined test count remains 241.

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
- Exact original-versus-extracted 468-line body diff: empty.
- Original-versus-split function-name multiset: all 310 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding block-placement/NBT class
  and following common-container test remain in their intended modules.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
