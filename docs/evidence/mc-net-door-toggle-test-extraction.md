# `mc-net` door/trapdoor toggle test extraction

Date: 2026-07-30

Checkpoint base: `d2611e26b775a8048e6ff797944b4d56503e6875`

## Result

The contiguous door/trapdoor hand-toggle class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/door_toggles.rs`. The child module has explicit
imports and does not inherit the aggregate file's `use super::*`.

The three tests cover property-preserving boolean toggles, hand-toggle material
policy for doors/trapdoors/fence gates, and two-client door/trapdoor
convergence. The two-client gate retains exact block-delta packet decoding,
atomic two-half publication, stale-retry rejection, and the following-tick
no-publication fence. The immediately following lever/piston class remains in
the aggregate module.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 18,260 physical lines and 299
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 17,832 | 296 |
| `play/tests/door_toggles.rs` | 442 | 3 |

A sorted multiset comparison of all 377 function names in the original
aggregate file against both resulting modules is identical. The combined test
count remains 299.

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

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
