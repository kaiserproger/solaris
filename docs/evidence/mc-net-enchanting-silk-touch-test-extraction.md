# `mc-net` fifteen-bookshelf silk-touch test extraction

Date: 2026-07-31

Checkpoint base: `e0f2312bc005dc49a69e5762ef2ba9987ecd4ca1`

## Result

The singleton test
`fifteen_bookshelves_keep_efficiency_clue_and_add_silk_touch_to_pickaxes`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_silk_touch.rs`.

The preserved test still builds the required registries; resolves the pickaxe,
lapis, and silk-touch id; creates the exact table input and XP state; asserts
the three pinned offer values; applies `enchanting_offer(15, 2)`; and asserts
ordered efficiency three plus silk touch one, consumed lapis, final XP level
`27` with unchanged progress and total, and changed seed.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding five-book fortune
test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,429 physical lines, 33 test
functions, and 91 function-name entries. The moved class contains 52 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,377 | 32 |
| `play/tests/enchanting_silk_touch.rs` | 57 | 1 |

The exact original-versus-extracted 52-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 91 function
names. The combined test count remains 33.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 52-line class diff: empty; both copies have
  SHA-256 `d7f6d644bbbb98df5d94a1fa47bef1314e27a4ac7b91168689beae3a77df11e1`.
- Original-versus-split function-name multiset: identical, 91 names.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: passed.
- `cargo test -p mc-net`: passed, 1,852 tests; 5 ignored; 3 doc-tests.
- `git diff --check`: passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Independent read-only review: changes requested. Both documentation findings
  were fixed: the next cursor now names the exact item ids, window id, and
  position, and the moved fixture is consistently called fifteen-bookshelf.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
