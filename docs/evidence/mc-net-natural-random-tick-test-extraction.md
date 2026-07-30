# `mc-net` natural random-tick test extraction

Date: 2026-07-30

Checkpoint base: `99b3d153178c85a97f9de7f46bbd7330453113fb`

## Result

The contiguous natural random-tick class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/natural_random_ticks.rs`. The child module has
explicit imports and does not inherit the aggregate file's `use super::*`.

The four tests cover leaf decay and fire state helpers, seeded fire spread,
zone-protected ambient fire edits, and the exact sapling/stick/apple leaf-decay
drop pools. The immediately following door/trapdoor toggle class remains in the
aggregate module.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 18,483 physical lines and 296
test attributes. Afterwards:

| File | Physical lines | Test attributes |
| --- | ---: | ---: |
| `play/tests.rs` | 18,260 | 292 |
| `play/tests/natural_random_ticks.rs` | 231 | 4 |

A sorted multiset comparison of all 381 function names in the original
aggregate file against both resulting modules is identical. The combined
source test-attribute count remains 296.

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
