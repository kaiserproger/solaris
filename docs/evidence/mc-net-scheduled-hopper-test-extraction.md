# `mc-net` scheduled-hopper test extraction

Date: 2026-07-30

Checkpoint base: `4d3fa723ff259811b5a39c39f65d779aae02878d`

## Result

The contiguous 20-test scheduled-hopper class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/scheduled_hoppers.rs`. Its exact boundary runs
from
`scheduled_hopper_tick_pulls_one_item_into_hopper_before_ejecting_without_generating_neighbors`
through
`scheduled_hopper_transfer_preserves_hopper_slot_when_target_has_no_room`.
The shared `run_scheduled_block_ticks_for_range` helper moved from
`leaf_distance_ticks.rs` with the hopper class, which owns every remaining
caller, and returned from `pub(super)` to private visibility. The preceding
ignored real-door test and following scheduled-button class remain in the
aggregate module. The child module uses explicit imports and does not inherit
the aggregate file's `use super::*`.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 16,308 physical lines and 278
test functions, while `play/tests/leaf_distance_ticks.rs` contained 198
physical lines and two test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 13,679 | 258 |
| `play/tests/leaf_distance_ticks.rs` | 184 | 2 |
| `play/tests/scheduled_hoppers.rs` | 2,660 | 20 |

A sorted multiset comparison of all 357 function names across the affected
modules is identical before and after the extraction. The exact 2,627-line
hopper body comparison is empty. The exact 12-line helper body comparison is
also empty after removing its former `pub(super)` visibility. The combined
test count remains 280.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 20 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-split function-name multiset: all 357 entries identical.
- Exact 2,627-line hopper body comparison: no differences.
- Exact 12-line helper body comparison after removing former `pub(super)`:
  no differences.
- The child module contains no `use super::*`.
- Boundary inspection confirms the preceding ignored real-door test and
  following scheduled-button class remain aggregate-owned.
- Independent read-only review: passed with no findings.
