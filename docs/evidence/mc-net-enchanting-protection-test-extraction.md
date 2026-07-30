# `mc-net` fifteen-bookshelf protection test extraction

Date: 2026-07-31

Checkpoint base: `34b37d8550423f99a6eb6d2f53adbdd6d6ab6d7c`

## Result

The singleton test
`fifteen_bookshelves_expose_and_apply_protection_to_armor`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_protection.rs`.

The preserved test still builds the required item and item-facts registries;
resolves the iron chestplate, lapis, protection id, and registry clue; creates
the exact table input and XP state; asserts all ten fifteen-bookshelf data
values; applies offer `enchanting_offer(15, 2)`; and asserts protection level
three, consumed lapis, and final XP level `27`.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding sharpness
enchanting test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,602 physical lines, 36 test
functions, and 94 function-name entries. The moved class contains 59 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,543 | 35 |
| `play/tests/enchanting_protection.rs` | 64 | 1 |

The exact original-versus-extracted 59-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 94 function
names. The combined test count remains 36.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 59-line class diff: empty; both copies have
  SHA-256 `74b6f489c14589e544f73a9cf3828619822a9e2771d91562efe4f4d50ae10201`.
- Original-versus-split function-name multiset: identical, 94 names.
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
