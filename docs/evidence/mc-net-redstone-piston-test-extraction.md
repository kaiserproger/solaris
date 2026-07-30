# `mc-net` redstone piston test extraction

Date: 2026-07-30

Checkpoint base: `50e81500fc203488c718e8de7fc71bb8bd69f1dc`

## Result

The contiguous lever/piston redstone class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/redstone_pistons.rs`. The child module has
explicit imports and does not inherit the aggregate file's `use super::*`.

The four tests cover lever-powered iron doors, piston extension/retraction with
stale alternate power, extension beside an occupied destination, and
zone-protected atomic piston edits. The immediately following ignored
real-door sidecar parity gate remains in the aggregate module.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 17,832 physical lines and 296
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 17,596 | 292 |
| `play/tests/redstone_pistons.rs` | 243 | 4 |

A sorted multiset comparison of all 374 function names in the original
aggregate file against both resulting modules is identical. The combined test
count remains 296.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 4 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-split function-name multiset: identical.
- The child module contains no `use super::*`.
- `git diff --check`: passed.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
