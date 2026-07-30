# `mc-net` fifteen-bookshelf sharpness test extraction

Date: 2026-07-31

Checkpoint base: `f42331cd24b6d7ba8ba4c37760cd3118a828afaf`

## Result

The singleton test
`fifteen_bookshelves_expose_and_apply_sharpness_to_swords`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_sharpness.rs`.

The preserved test still builds the required item and item-facts registries;
resolves the stone sword, lapis, sharpness id, and registry clue; creates the
exact table input and XP state; asserts all ten fifteen-bookshelf data values;
applies offer `enchanting_offer(15, 2)`; and asserts sharpness level three,
consumed lapis, and final XP level `27`.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding first pickaxe-offer
test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,543 physical lines, 35 test
functions, and 93 function-name entries. The moved class contains 59 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,484 | 34 |
| `play/tests/enchanting_sharpness.rs` | 64 | 1 |

The exact original-versus-extracted 59-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 93 function
names. The combined test count remains 35.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 59-line class diff: empty; both copies have
  SHA-256 `33be4a34cde709f41e241844be286b94ccf1649af556e7ec599efc7c8c91857f`.
- Original-versus-split function-name multiset: identical, 93 names.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: passed.
- `cargo test -p mc-net`: passed, 1,852 tests; 5 ignored; 3 doc-tests.
- `git diff --check`: passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
