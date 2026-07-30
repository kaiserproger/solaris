# `mc-net` entity-tick cadence test extraction

Date: 2026-07-30

Checkpoint base: `1693f59f59d7d392b8d92eaed8f1a98325e7f69e`

## Result

The entity cadence test
`entity_tick_cadence_matches_vanilla_cow_tracking` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/entity_tick_cadence.rs`.

The preserved matrix verifies the 50 ms entity-owner period, the matching
`0.05`-second physics tick, and the three-tick movement publication interval.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding toggle-planning writer-lock test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,110 physical lines and 81
test functions. The moved class contains six physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,104 | 80 |
| `play/tests/entity_tick_cadence.rs` | 10 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 139 function names. The
combined test count remains 81.

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
- Exact original-versus-extracted six-line class diff: empty.
- Original-versus-split function-name multiset: all 139 entries identical.
- The moved test name occurs exactly once across aggregate and child modules.
- The child module uses explicit imports and contains no `use super::*`,
  redundant single-component import, or public item.
- Boundary inspection confirms that the preceding toggle-planning test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
