# `mc-net` stale-container update test extraction

Date: 2026-07-31

Checkpoint base: `67b0c8d412c4217cc9aa575298791ab82519dc26`

## Result

The singleton test
`stale_container_updates_do_not_discard_another_open_container` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/stale_container_updates.rs`.

The preserved test uses the same furnace position `(1, 64, 1)` and chest
position `(2, 64, 2)`. Chest container id `7` remains active after the stale
furnace lookup, then furnace container id `8` with `FurnaceKind::Furnace`
remains active after the stale chest lookup.

The child imports its container types and lookup helpers explicitly and does
not inherit the aggregate file's `use super::*`. The immediately preceding
pending-teleport guard test and following `state` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,324 physical lines and 54
test functions. The moved class contains 29 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,295 | 53 |
| `play/tests/stale_container_updates.rs` | 34 | 1 |

The exact original-versus-extracted 29-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 112 function
names. The combined test count remains 54.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 29-line class diff: empty.
- Original-versus-split function-name multiset: identical, 112 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the pending-teleport guard test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 29-line class diff, unchanged 112-name function multiset,
  preserved 54-test total, explicit private-module ownership, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
