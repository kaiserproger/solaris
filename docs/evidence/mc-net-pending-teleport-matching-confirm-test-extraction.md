# `mc-net` matching pending-teleport confirmation test extraction

Date: 2026-07-31

Checkpoint base: `2d8e1b81f721bb77b514375109a60df4cbed323e`

## Result

The singleton test `pending_teleport_confirm_clears_only_matching_id` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_matching_confirm.rs`.

The preserved test starts with pending teleport id `7` at sent tick `0`.
Confirmation id `8` still returns
`TeleportConfirmResult::Mismatched { expected: 7 }` and leaves pending state
present. Confirmation id `7` still returns `TeleportConfirmResult::Confirmed`
and clears the pending state.

The child imports its teleport state, confirmation result, and operation
explicitly and does not inherit the aggregate file's `use super::*`. The
immediately preceding teleport-id allocator test and following `state` helper
remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,140 physical lines and 47
test functions. The moved class contains 16 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,124 | 46 |
| `play/tests/pending_teleport_matching_confirm.rs` | 18 | 1 |

The exact original-versus-extracted 16-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 105 function
names. The combined test count remains 47.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 16-line class diff: empty.
- Original-versus-split function-name multiset: identical, 105 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the teleport-id allocator test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 16-line class preservation, unchanged 105-name function multiset
  and 47-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
