# `mc-net` bottom-slab collision test extraction

Date: 2026-07-31

Checkpoint base: `2e0af6a1776ebadfb6d45dba3242870c61154853`

## Result

The singleton test `player_collision_uses_bottom_slab_box` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/bottom_slab_collision.rs`.

The preserved test publishes the exact `minecraft:stone_slab` state with
`type=bottom` and `waterlogged=false`. It verifies that a player may stand at
`(0.5, 64.5, 0.5)` on the slab's half-block top while the overlapping
`(0.5, 64.49, 0.5)` pose collides.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding exact-farmland-fallback test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,525 physical lines and 64
test functions. The moved class contains 19 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,506 | 63 |
| `play/tests/bottom_slab_collision.rs` | 24 | 1 |

The exact original-versus-extracted 19-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 122 function names.
The combined test count remains 64.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 19-line class diff: empty.
- Original-versus-split function-name multiset: identical, 122 entries before
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
  the empty 19-line class diff, unchanged 122-name function multiset,
  preserved 64-test total, exact slab state and poses, explicit imports,
  aggregate boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
