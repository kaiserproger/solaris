# `mc-net` toggle-planning test extraction

Date: 2026-07-30

Checkpoint base: `9196d6c5f1744feba492ce2fbacecc729e7f15ee`

## Result

The writer-lock independence test
`toggle_planning_does_not_wait_for_world_writer` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/toggle_planning.rs`.

The preserved test builds the published button state, captures its mutation
token, holds the mutable world lock, and verifies that toggle planning still
returns the powered-state edit, matching precondition, and tick-120 scheduled
release from the published world view.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding block-placement planning test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,104 physical lines and 80
test functions. The moved class contains 32 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,072 | 79 |
| `play/tests/toggle_planning.rs` | 39 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 138 function names. The
combined test count remains 80.

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
- Exact original-versus-extracted 32-line class diff: empty.
- Original-versus-split function-name multiset: all 138 entries identical.
- The moved test name and module declaration each occur exactly once.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding block-placement planning
  test and following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
