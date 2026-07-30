# `mc-net` unexpected pending-teleport confirmation test extraction

Date: 2026-07-31

Checkpoint base: `d0112ccac82b25f3b181d324beac6aaf9bfd5f48`

## Result

The singleton test
`pending_teleport_reports_unexpected_confirm_without_pending_state` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_unexpected_confirm.rs`.

The preserved test starts without pending teleport state. Confirmation id `1`
still returns `TeleportConfirmResult::Unexpected`, and the pending state still
remains absent afterwards.

The child imports its confirmation result and operation explicitly and does
not inherit the aggregate file's `use super::*`. The immediately preceding
matching-confirm test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,150 physical lines and 48
test functions. The moved class contains 10 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,140 | 47 |
| `play/tests/pending_teleport_unexpected_confirm.rs` | 12 | 1 |

The exact original-versus-extracted 10-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 106 function
names. The combined test count remains 48.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 10-line class diff: empty.
- Original-versus-split function-name multiset: identical, 106 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the matching-confirm test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 10-line class preservation, unchanged 106-name function multiset
  and 48-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
