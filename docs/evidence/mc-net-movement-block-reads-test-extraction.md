# `mc-net` movement block-read test extraction

Date: 2026-07-31

Checkpoint base: `bea1821ea15223d2aebd482d18b5894024cfa8f0`

## Result

The writer-lock independence test
`movement_block_reads_do_not_wait_for_world_writer` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/movement_block_reads.rs`.

The preserved test publishes exact solid and water block states, holds the
mutable world lock, and directly polls both movement read paths. Solid
collision must be immediately ready with `true`, while the water probe at its
exact pose must be immediately ready with `(true, false)`.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding powder-snow collision-correction
test and following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,914 physical lines and 76
test functions. The moved class contains 52 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,862 | 75 |
| `play/tests/movement_block_reads.rs` | 60 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 134 function names. The
combined test count remains 76.

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
- Exact original-versus-extracted 52-line class diff: empty.
- Original-versus-split function-name multiset: all 134 entries identical.
- The moved test name and module declaration each occur exactly once.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding powder-snow
  collision-correction test and following `button_test_registry` helper remain
  aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
