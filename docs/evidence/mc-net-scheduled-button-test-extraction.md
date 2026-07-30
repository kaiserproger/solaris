# `mc-net` scheduled-button test extraction

Date: 2026-07-30

Checkpoint base: `123579016f5f99b86c944ac32ca6f107f9a4557c`

## Result

The contiguous nine-test scheduled-button runtime and concurrency class moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/scheduled_buttons.rs`. Its exact boundary runs
from `scheduled_button_tick_releases_powered_button` through
`stale_resident_journal_commit_does_not_block_the_next_decision`; the preceding
real-door parity test and following leaf-distance scheduled-tick class remain
outside this module. The child module uses explicit imports and does not inherit
the aggregate file's `use super::*`.

The extracted tests retain the scheduled release, distinct- and repeated-region
planning, cross-region commit and abort/failure cleanup, global-writer
independence, resident update, and journal-ordering coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 17,487 physical lines and 289
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 16,495 | 280 |
| `play/tests/scheduled_buttons.rs` | 1,002 | 9 |

A sorted multiset comparison of all 366 function names in the original
aggregate file against both resulting modules is identical. The combined test
count remains 289.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 9 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-split function-name multiset: all 366 entries identical.
- The child module contains no `use super::*`.
- `git diff --check`: passed.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
