# `mc-net` stone full-cube collision test extraction

Date: 2026-07-31

Checkpoint base: `dee19d66ce762ff4b97aeb8a4e0dbee6a83b61b3`

## Result

The singleton test `player_collision_uses_exact_full_cube_shape_for_stone`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/stone_full_cube_collision.rs`.

The preserved test resolves the exact vanilla `minecraft:stone` state,
publishes it into the collision test world, and verifies that the centered
player pose at `(0.5, 64.0, 0.5)` collides with the full cube.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding deflated fence-top boundary test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,666 physical lines and 70
test functions. The moved class contains 11 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,655 | 69 |
| `play/tests/stone_full_cube_collision.rs` | 16 | 1 |

The exact original-versus-extracted 11-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 128 function names.
The combined test count remains 70.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 11-line class diff: empty.
- Original-versus-split function-name multiset: identical, 128 entries before
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
  the empty 11-line class diff, unchanged 128-name function multiset,
  preserved 70-test total, exact stone state and pose, explicit imports,
  aggregate boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
