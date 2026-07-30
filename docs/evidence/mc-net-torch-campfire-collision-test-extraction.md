# `mc-net` torch/campfire collision test extraction

Date: 2026-07-31

Checkpoint base: `2e5d6db13f1b08dc9de43e05bbbd528303550a2d`

## Result

The singleton test
`player_collision_uses_exact_shapes_for_torch_and_campfire` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/torch_campfire_collision.rs`.

The preserved test verifies that the embedded table gives a torch an empty
collision shape and resolves the exact north-facing, lit, non-signal,
non-waterlogged campfire state. It checks both sides of the campfire's exact
`7/16`-block top: `64.4375` remains outside while `64.42` collides.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding exact-stone full-cube test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,696 physical lines and 71
test functions. The moved class contains 30 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,666 | 70 |
| `play/tests/torch_campfire_collision.rs` | 35 | 1 |

The exact original-versus-extracted 30-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 129 function names.
The combined test count remains 71.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 30-line class diff: empty.
- Original-versus-split function-name multiset: identical, 129 entries before
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
  the empty 30-line class diff, unchanged 129-name function multiset,
  preserved 71-test total, exact states and poses, explicit imports, aggregate
  boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
