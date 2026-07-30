# `mc-net` collision-correction escape test extraction

Date: 2026-07-31

Checkpoint base: `6f56e87a4e4dd04bb46e1b151f481358d474f9e6`

## Result

The singleton test
`collision_correction_does_not_teleport_back_into_existing_solid_overlap`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/collision_correction_escape.rs`.

The preserved test places the same solid block at `(0, 64, 0)`, moves from the
already-colliding pose `(0.50, 64.0, 0.50)` toward the escaping pose
`(0.55, 64.0, 0.50)`, and proves that collision correction does not teleport
the authoritative pose back into the solid. The writer remains empty, no
pending teleport is created, and the next teleport id remains `2`.

The child imports its aggregate-owned fixture and collision helpers explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding `insert_fluid_test_chunk` helper and following
`vanilla_collision_test_state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,412 physical lines and 57
test functions. The moved class contains 37 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,375 | 56 |
| `play/tests/collision_correction_escape.rs` | 44 | 1 |

The exact original-versus-extracted 37-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 115 function
names. The combined test count remains 57.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 37-line class diff: empty.
- Original-versus-split function-name multiset: identical, 115 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and `insert_fluid_test_chunk` and `vanilla_collision_test_state` retain their
  single aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 37-line class diff, unchanged 115-name function multiset,
  preserved 57-test total, explicit private-module ownership, aggregate helper
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
