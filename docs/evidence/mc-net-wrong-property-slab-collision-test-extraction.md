# `mc-net` wrong-property slab collision test extraction

Date: 2026-07-31

Checkpoint base: `8ad5ea8f47627ab7baba7887caee6ebd609cade1`

## Result

The singleton test
`player_collision_rejects_wrong_properties_under_canonical_slab_name_and_id`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/wrong_property_slab_collision.rs`.

The preserved test uses the unchanged aggregate-owned
`wrong_property_slab_overlap_test_state` fixture. That fixture retains the
canonical `minecraft:stone_slab` name and numeric state id while replacing the
ordered `type=bottom` property with `type=synthetic`. The test verifies that
the `(0.5, 64.5, 0.5)` pose collides rather than inheriting the vanilla slab
shape from name and id alone.

The child imports its fixture and collision helpers explicitly and does not
inherit the aggregate file's `use super::*`. The immediately preceding
fake-farmland-identity test and following `button_test_registry` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,492 physical lines and 62
test functions. The moved class contains 10 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,482 | 61 |
| `play/tests/wrong_property_slab_collision.rs` | 15 | 1 |

The exact original-versus-extracted 10-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 120 function names.
The combined test count remains 62.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 10-line class diff: empty.
- Original-versus-split function-name multiset: identical, 120 entries before
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
  the empty 10-line class diff, unchanged 120-name function multiset,
  preserved 62-test total, exact fixture ownership and property mutation,
  collision assertion, explicit imports, aggregate boundaries, evidence
  metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
