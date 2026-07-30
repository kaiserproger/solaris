# `mc-net` powder-snow collision-correction test extraction

Date: 2026-07-31

Checkpoint base: `c812ba310a2e69f933009f80a0e6f6157a2418c9`

## Result

The movement-context test
`collision_correction_applies_powder_snow_movement_context` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/powder_snow_collision_correction.rs`.

The preserved test verifies leather-boots correction from above, Shift descent
without correction, and the long-fall `0.9F` landing shape. The three paths
retain their exact positions, fall origin, and teleport sequences `10`, `11`,
and `12`.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding exact-state-identity test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,862 physical lines and 75
test functions. The moved class contains 71 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,791 | 74 |
| `play/tests/powder_snow_collision_correction.rs` | 78 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 133 function names. The
combined test count remains 75.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 71-line class diff: empty.
- Original-versus-split function-name multiset: all 133 entries identical.
- The moved test name and module declaration each occur exactly once.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding exact-state-identity test
  and following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
