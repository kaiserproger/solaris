# `mc-net` powder-snow long-fall test extraction

Date: 2026-07-31

Checkpoint base: `f72764cd6fbc266a1639a56bac07445d86c41772`

## Result

The singleton test
`powder_snow_uses_falling_collision_shape_after_long_fall` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/powder_snow_long_fall.rs`.

The preserved test publishes powder snow, starts both poses from a
`fall_start_y` of `68.0`, and verifies both sides of the exact `0.9F` falling
collision-shape boundary: `64.9` remains outside while `64.89` collides.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding equipment-and-movement-context test
and following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,756 physical lines and 73
test functions. The moved class contains 20 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,736 | 72 |
| `play/tests/powder_snow_long_fall.rs` | 25 | 1 |

The exact original-versus-extracted 20-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 131 function names.
The combined test count remains 73.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 20-line class diff: empty.
- Original-versus-split function-name multiset: identical, 131 entries before
  and after.
- Unique test/module ownership, explicit imports, and aggregate boundaries:
  pass. The module declaration and test each occur once, the child contains no
  wildcard parent import or public item, and both adjacent aggregate-owned
  boundaries remain present.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the verbatim body, exact boundary values, unchanged 131-name function
  multiset, preserved 73-test total, explicit child imports, aggregate
  boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
