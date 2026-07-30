# `mc-net` client-view-distance test extraction

Date: 2026-07-30

Checkpoint base: `69ec92a6933f579a623be5df9bf8cbb03be393b1`

## Result

The client-view-distance policy test
`client_view_distance_is_clamped_to_server_policy` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/client_view_distance.rs`.

The preserved matrix verifies the server cap (`12, 8 -> 8`), client cap
(`6, 10 -> 6`), minimum for zero (`0, 10 -> 2`), minimum for a negative client
value (`-8, 1 -> 2`), and absolute maximum
(`i8::MAX, i32::MAX -> 32`).

The child uses an explicit import and does not inherit the aggregate file's
`use super::*`. The immediately preceding game-mode parser test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,166 physical lines and 85
test functions. The moved class contains eight physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,158 | 84 |
| `play/tests/client_view_distance.rs` | 10 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 143 function names. The
combined test count remains 85.

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
- Exact original-versus-extracted eight-line class diff: empty.
- Original-versus-split function-name multiset: all 143 entries identical.
- The moved test name occurs exactly once across aggregate and child modules.
- The child module uses one explicit import and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding game-mode parser test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
