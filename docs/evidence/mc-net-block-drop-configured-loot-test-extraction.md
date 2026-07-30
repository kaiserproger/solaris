# `mc-net` configured block-loot test extraction

Date: 2026-07-31

Checkpoint base: `98709aaa532de4e735159fc4a022d8f084dafb50`

## Result

The singleton test
`block_drop_configured_loot_count_reaches_runtime_stack` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/block_drop_configured_loot.rs`.

The preserved test uses the same crop registry, carrot item protocol id `52`,
and default dirt state. Its configured dirt loot entry still produces three
carrots through `LootCount::Fixed(3)`, and the runtime result remains
`ItemStack::new(52, 3)`.

The child imports its aggregate-owned registry and drop helper explicitly and
does not inherit the aggregate file's `use super::*`. The immediately
preceding short-grass drop test and following `fluid_block` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,375 physical lines and 56
test functions. The moved class contains 26 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,349 | 55 |
| `play/tests/block_drop_configured_loot.rs` | 31 | 1 |

The exact original-versus-extracted 26-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 114 function
names. The combined test count remains 56.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 26-line class diff: empty.
- Original-versus-split function-name multiset: identical, 114 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the short-grass test, `crop_test_registry`, and `fluid_block` retain
  their single aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 26-line class diff, unchanged 114-name function multiset,
  preserved 56-test total, explicit private-module ownership, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
