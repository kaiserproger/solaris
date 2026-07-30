# `mc-net` enchanting-selection test extraction

Date: 2026-07-31

Checkpoint base: `15ac6b4b22d4b2a763ff41cf8de0799e82510e56`

## Result

The singleton test
`enchanting_selection_consumes_lapis_and_level_but_preserves_total_xp`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_selection.rs`.

The preserved test still builds the required registries; resolves and checks
the efficiency-enchantable stone pickaxe; creates the exact pickaxe, lapis,
and XP inputs; applies `enchanting_offer(0, 0)`; and asserts efficiency one,
one remaining lapis, spent level, unchanged progress and total XP, and changed
seed.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding stale inventory-
drag test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,295 physical lines, 30 test
functions, and 83 function-name entries. The moved class contains 41 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,254 | 29 |
| `play/tests/enchanting_selection.rs` | 46 | 1 |

The exact original-versus-extracted 41-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 83 function
names. The combined test count remains 30.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 41-line class diff: empty; both copies have
  SHA-256 `ff4b825aac7b2b5a868dc61205a08295c295b64a56cfabdae2d0891343506174`.
- Original-versus-split function-name multiset: identical, 83 names.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: passed.
- `cargo test -p mc-net`: passed, 1,852 tests; 5 ignored; 3 doc-tests.
- `git diff --check`: passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Independent read-only review: changes requested. The next cursor now names
  the exact hashed-stack fields, saved item ids and counts, and final slot and
  carried state.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
