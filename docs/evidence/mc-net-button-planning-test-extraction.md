# `mc-net` button-planning test extraction

Date: 2026-07-30

Checkpoint base: `bcfcf6ab19919d838fa7799cc122321e15e55b0b`

## Result

The contiguous button interaction-planning class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/button_planning.rs`. The child module has explicit
imports and does not inherit the aggregate file's `use super::*`.

The three tests prove release-tick planning without a global scan, no
materialization of an unloaded adjacent chunk, and consumption of an already
powered button press without a duplicate release tick. The following scheduled
button runtime/concurrency class remains in the aggregate module.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 17,596 physical lines and 292
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 17,487 | 289 |
| `play/tests/button_planning.rs` | 115 | 3 |

A sorted multiset comparison of all 370 function names in the original
aggregate file against both resulting modules is identical. The combined test
count remains 292.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 3 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-split function-name multiset: identical.
- The child module contains no `use super::*`.
- `git diff --check`: passed.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
