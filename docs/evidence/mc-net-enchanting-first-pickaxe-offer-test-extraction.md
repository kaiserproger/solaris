# `mc-net` first pickaxe-offer test extraction

Date: 2026-07-31

Checkpoint base: `efdb1800634339ddb09e96764da60c6bc04c7bb2`

## Result

The singleton test
`fifteen_bookshelves_keep_efficiency_as_the_first_pickaxe_offer`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_first_pickaxe_offer.rs`.

The preserved test still builds the required item and item-facts registries;
resolves the stone pickaxe and lapis; creates the exact table input and XP
state; asserts all ten fifteen-book offer values; applies
`enchanting_offer(15, 0)`; and asserts exactly efficiency level one, remaining
lapis count two, and final XP level `29`, progress `0.25`, and total `1_395`.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding silk-touch
enchanting test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,484 physical lines, 34 test
functions, and 92 function-name entries. The moved class contains 55 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,429 | 33 |
| `play/tests/enchanting_first_pickaxe_offer.rs` | 60 | 1 |

The exact original-versus-extracted 55-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 92 function
names. The combined test count remains 34.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 55-line class diff: empty; both copies have
  SHA-256 `0322cde4e1443931969ff04e855ef5400243aba8e8f3745bc0512b5a4305a59c`.
- Original-versus-split function-name multiset: identical, 92 names.
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
