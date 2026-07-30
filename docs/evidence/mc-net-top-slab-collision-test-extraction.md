# `mc-net` top-slab collision test extraction

Date: 2026-07-31

Checkpoint base: `cd02a402aa015b40c6bbbe1c821015956c907666`

## Result

The singleton test `player_collision_uses_top_slab_box` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/top_slab_collision.rs`.

The preserved test publishes the exact `minecraft:stone_slab` state with
`type=top` and `waterlogged=false`. It verifies that the lower half remains
empty at `(0.5, 62.7, 0.5)` while the player's head may not enter the top-slab
box at `(0.5, 62.71, 0.5)`.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding oracle-AABB-deflation test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,572 physical lines and 66
test functions. The moved class contains 19 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,553 | 65 |
| `play/tests/top_slab_collision.rs` | 24 | 1 |

The exact original-versus-extracted 19-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 124 function names.
The combined test count remains 66.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 19-line class diff: empty.
- Original-versus-split function-name multiset: identical, 124 entries before
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
- Independent read-only review: changes resolved. The reviewer confirmed the
  exact 19-line class diff, unchanged 124-name function multiset, preserved
  66-test total, exact top-slab state and poses, explicit imports, aggregate
  boundaries, evidence metrics, and links. Its only finding was that the next
  cursor summarized rather than spelled out the oracle-deflation expressions
  and outcomes; `ACTIVE.md` now records both exact pose expressions and their
  collision results.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
