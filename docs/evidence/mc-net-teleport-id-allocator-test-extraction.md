# `mc-net` teleport-id allocator test extraction

Date: 2026-07-31

Checkpoint base: `e699cfbc64a17c2300fccc03aee52432dce55f24`

## Result

The singleton test `teleport_id_allocator_advances_and_wraps_to_positive_ids`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/teleport_id_allocator.rs`.

The preserved test starts with next id `2`, still receives ids `2` then `3`,
and retains resulting next id `4`. After resetting the next id to `i32::MAX`,
it still receives `i32::MAX` and then wraps to positive id `1`.

The child imports the teleport-id allocator explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding oversized
container-title test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,124 physical lines and 46
test functions. The moved class contains 12 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,112 | 45 |
| `play/tests/teleport_id_allocator.rs` | 14 | 1 |

The exact original-versus-extracted 12-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 104 function
names. The combined test count remains 46.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 12-line class diff: empty.
- Original-versus-split function-name multiset: identical, 104 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the oversized container-title test and `state` helper retain their
  single aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 12-line class preservation, unchanged 104-name function multiset
  and 46-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
