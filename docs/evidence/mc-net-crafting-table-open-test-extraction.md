# `mc-net` crafting-table-open test extraction

Date: 2026-07-31

Checkpoint base: `71a9c10721fddccae969fca4bc1226cc5a7195bc`

## Result

The writer-lock independence test
`crafting_table_open_does_not_wait_for_world_writer` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/crafting_table_open.rs`.

The preserved test publishes a loaded crafting table, holds the mutable world
lock, and polls the open operation directly. It requires an immediately
successful open for the exact player pose, block position, and sequence through
the published world view.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding movement block-read test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,967 physical lines and 77
test functions. The moved class contains 53 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,914 | 76 |
| `play/tests/crafting_table_open.rs` | 63 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 135 function names. The
combined test count remains 77.

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
- Exact original-versus-extracted 53-line class diff: empty.
- Original-versus-split function-name multiset: all 135 entries identical.
- The moved test name and module declaration each occur exactly once.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding movement block-read test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
