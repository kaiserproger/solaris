# `mc-net` fake-farmland identity collision test extraction

Date: 2026-07-31

Checkpoint base: `f4091ee0467223d5cab4f71f4192e387d402d027`

## Result

The singleton test
`player_collision_rejects_fake_farmland_identity_on_overlapping_slab_id` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/fake_farmland_identity_collision.rs`.

The preserved test uses the unchanged aggregate-owned
`fake_farmland_slab_overlap_test_state` fixture. That fixture renames canonical
farmland to `solaris:canonical_farmland`, renames the dry bottom-slab block to
`minecraft:farmland`, and retains the overlapping slab state id and properties.
The test verifies full-cube collision at `(0.5, 64.5, 0.5)` rather than
inheriting either the vanilla slab table shape or farmland height.

The child imports its fixture and collision helpers explicitly and does not
inherit the aggregate file's `use super::*`. The immediately preceding
synthetic-Minecraft-slab test and following `button_test_registry` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,482 physical lines and 61
test functions. The moved class contains 10 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,472 | 60 |
| `play/tests/fake_farmland_identity_collision.rs` | 15 | 1 |

The exact original-versus-extracted 10-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 119 function names.
The combined test count remains 61.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 10-line class diff: empty.
- Original-versus-split function-name multiset: identical, 119 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  the fixture retains its single aggregate definition and single child caller,
  and both adjacent boundaries remain present.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 10-line class diff, unchanged 119-name function multiset,
  preserved 61-test total, exact fixture ownership and identity mutations,
  collision assertion, explicit imports, aggregate boundaries, evidence
  metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
