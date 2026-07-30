# `mc-net` pending-teleport confirmation-behaviour test extraction

Date: 2026-07-31

Checkpoint base: `a83613d6d4a17949c4a6135f3f23aeaaa203ef79`

## Result

The singleton test
`pending_teleport_confirm_behaviour_after_unconfirmed_movement` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_confirm_behaviour.rs`.

The preserved test starts with pending teleport id `7` at sent tick `0`. The
movement guard still returns true for the exact
`ServerboundMovePlayerPos` packet name, confirmation id `8` still reports a
mismatch with expected id `7` while retaining pending id `7`, confirmation id
`7` still succeeds and clears the pending state, and the final movement guard
still returns false for the same packet name.

The child imports its teleport state and operations explicitly and does not
inherit the aggregate file's `use super::*`. The immediately preceding
movement-gate test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,188 physical lines and 50
test functions. The moved class contains 25 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,163 | 49 |
| `play/tests/pending_teleport_confirm_behaviour.rs` | 30 | 1 |

The exact original-versus-extracted 25-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 108 function
names. The combined test count remains 50.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 25-line class diff: empty.
- Original-versus-split function-name multiset: identical, 108 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the movement-gate test and `state` helper retain their single aggregate
  definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 25-line class preservation, unchanged 108-name function multiset
  and 50-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
