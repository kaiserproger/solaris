# `mc-net` built-in short-grass drop test extraction

Date: 2026-07-31

Checkpoint base: `4abb6de1b10121ec25ce89abc092ccf033030421`

## Result

The singleton test `block_drop_builtin_short_grass_returns_wheat_seeds` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/block_drop_short_grass.rs`.

The preserved test builds the same air/short-grass registry, selects the
default short-grass state, and uses the wheat-seeds item protocol id `51`.
The built-in default loot path still produces `ItemStack::new(51, 1)`.

The child imports its aggregate-owned block fixture and drop helper explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding `crop_test_registry` helper and following `fluid_block` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,349 physical lines and 55
test functions. The moved class contains 25 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,324 | 54 |
| `play/tests/block_drop_short_grass.rs` | 29 | 1 |

The exact original-versus-extracted 25-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 113 function
names. The combined test count remains 55.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 25-line class diff: empty.
- Original-versus-split function-name multiset: identical, 113 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and `crop_test_registry` and `fluid_block` retain their single aggregate
  definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: changes. The reviewer confirmed the exact
  25-line extraction, metrics, ownership and evidence links, then requested a
  more explicit next-cursor mapping. `ACTIVE.md` now states that chest id `7`
  survives the stale furnace lookup and furnace id `8` with
  `FurnaceKind::Furnace` survives the stale chest lookup. The clarification
  passed the focused static/diff check; no reviewer cascade was run.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
