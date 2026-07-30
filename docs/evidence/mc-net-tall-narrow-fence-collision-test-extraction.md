# `mc-net` tall/narrow fence collision test extraction

Date: 2026-07-31

Checkpoint base: `d4e7c0c0c0e594c1d19c116099b75a2419e48c6c`

## Result

The singleton test `player_collision_uses_tall_narrow_fence_box` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/tall_narrow_fence_collision.rs`.

The preserved test publishes an isolated oak-fence state. It verifies that
space beside the narrow post at `(0.05, 64.0, 0.5)` is empty while the centered
pose at `(0.5, 65.25, 0.5)` collides with the fence's `1.5`-block height.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding oriented-stair test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,621 physical lines and 68
test functions. The moved class contains 25 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,596 | 67 |
| `play/tests/tall_narrow_fence_collision.rs` | 30 | 1 |

The exact original-versus-extracted 25-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 126 function names.
The combined test count remains 68.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 25-line class diff: empty.
- Original-versus-split function-name multiset: identical, 126 entries before
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
  the empty 25-line class diff, unchanged 126-name function multiset,
  preserved 68-test total, exact fence state and poses, explicit imports,
  aggregate boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
