# `mc-net` collision-correction entry test extraction

Date: 2026-07-31

Checkpoint base: `0e0fe31c924384b6cd0a184c075303513fcb6700`

## Result

The singleton test
`collision_correction_still_rejects_entry_from_free_space_into_solid` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/collision_correction_entry.rs`.

The preserved test places the same solid block at `(0, 64, 0)`, moves from the
free pose `(1.50, 64.0, 0.50)` into the colliding pose
`(0.50, 64.0, 0.50)`, and proves that collision correction emits exactly one
position-sync packet. The pending teleport keeps id `2` while the next id
advances from `2` to `3`.

The child imports its aggregate-owned fixture and collision helpers explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding existing-solid-overlap test and following
`vanilla_collision_test_state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,446 physical lines and 58
test functions. The moved class contains 34 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,412 | 57 |
| `play/tests/collision_correction_entry.rs` | 42 | 1 |

The exact original-versus-extracted 34-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 116 function
names. The combined test count remains 58.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 34-line class diff: empty.
- Original-versus-split function-name multiset: identical, 116 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  `insert_fluid_test_chunk`, the existing-solid-overlap test, and
  `vanilla_collision_test_state` each retain their single aggregate
  definition.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 34-line class diff, unchanged 116-name function multiset,
  preserved 58-test total, explicit private-module ownership, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
