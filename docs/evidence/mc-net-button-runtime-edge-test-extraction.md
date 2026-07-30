# `mc-net` button runtime-edge test extraction

Date: 2026-07-30

Checkpoint base: `a89ff421bc9d2c026a481d5693ad03875042ee58`

## Result

The contiguous five-test scheduled-button loaded-state, ABA, and
adjacent-power class moved from aggregate `crates/mc-net/src/play/tests.rs` to
the focused Rust module
`crates/mc-net/src/play/tests/button_runtime_edges.rs`. Its exact boundary runs
from `scheduled_button_tick_ignores_ticketed_chunk_until_loaded` through
`scheduled_button_release_keeps_piston_extended_when_head_is_protected`. The
preceding ignored real-door gate and following `button_test_registry` helper
class remain in the aggregate module. The child module uses explicit imports
and does not inherit the aggregate file's `use super::*`.

The extracted tests retain coverage for ticketed-but-unloaded planning without
a writer, due-tick retention after an ABA state change, release with an
alternate door control, button press and scheduled release against an adjacent
iron door, and release beside a protected piston head.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 13,679 physical lines and 258
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 13,332 | 253 |
| `play/tests/button_runtime_edges.rs` | 358 | 5 |

An exact comparison of the 347-line test body before and after the move has an
empty diff. A sorted multiset comparison of all 333 function names in the
original aggregate file against both resulting modules is identical. The
combined test count remains 258.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 5 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 347-line test-body diff: empty.
- Original-versus-split function-name multiset: all 333 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- The ignored real-door gate and `button_test_registry` helper class remain in
  the aggregate module.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
