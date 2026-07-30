# `mc-net` oriented-stair collision test extraction

Date: 2026-07-31

Checkpoint base: `2380c93e98162ed1777cb6759dc85feef1afc08b`

## Result

The singleton test `player_collision_uses_oriented_stair_boxes` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/oriented_stair_collision.rs`.

The preserved test publishes the exact north-facing, bottom, straight,
non-waterlogged oak-stair state. It verifies that the upper step occupies the
north half at `(0.5, 64.5, 0.15)` while the south half above the lower step
remains empty at `(0.5, 64.5, 0.85)`.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding top-slab test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,596 physical lines and 67
test functions. The moved class contains 24 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,572 | 66 |
| `play/tests/oriented_stair_collision.rs` | 29 | 1 |

The exact original-versus-extracted 24-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 125 function names.
The combined test count remains 67.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 24-line class diff: empty.
- Original-versus-split function-name multiset: identical, 125 entries before
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
  the empty 24-line class diff, unchanged 125-name function multiset,
  preserved 67-test total, exact stair state and poses, explicit imports,
  aggregate boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
