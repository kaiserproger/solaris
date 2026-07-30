# `mc-net` five-bookshelf fortune test extraction

Date: 2026-07-31

Checkpoint base: `bf57d4c622b3efc5839c3b795e8cf0c4b35b5b03`

## Result

The singleton test
`five_bookshelves_keep_efficiency_clue_and_add_fortune_to_pickaxes`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_fortune.rs`.

The preserved test still builds the required registries; resolves the pickaxe,
lapis, and fortune ids; creates the exact table input and XP state; asserts the
three pinned offer values; applies `enchanting_offer(5, 1)`; and asserts
ordered efficiency two plus fortune two, consumed lapis, final XP level `8`
with unchanged progress and total, and changed seed.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding supported-
efficiency-offer test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,377 physical lines, 32 test
functions, and 85 function-name entries. The moved class contains 52 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,325 | 31 |
| `play/tests/enchanting_fortune.rs` | 57 | 1 |

The exact original-versus-extracted 52-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 85 function
names. The combined test count remains 32.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 52-line class diff: empty; both copies have
  SHA-256 `5972f66e626133c08bb7fab090f969402f37348bbb66b750d7e2c416a24c2c57`.
- Original-versus-split function-name multiset: identical, 85 names.
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
