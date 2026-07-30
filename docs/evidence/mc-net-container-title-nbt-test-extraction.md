# `mc-net` oversized container-title NBT test extraction

Date: 2026-07-31

Checkpoint base: `d692fa6472f139f048b90b7c3b2b2f989b69f8d7`

## Result

The singleton test
`container_title_nbt_reports_oversized_text_instead_of_panicking` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/container_title_nbt.rs`.

The preserved test still builds text with length
`usize::from(u16::MAX) + 1`, uses the exact
`"oversized NBT title should fail"` expectation, and receives
`mc_protocol::CodecError::Nbt` instead of panicking.

The child imports the chest-menu title encoder explicitly and does not inherit
the aggregate file's `use super::*`. The immediately preceding entity-movement
write-turn test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,112 physical lines and 45
test functions. The moved class contains 8 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,104 | 44 |
| `play/tests/container_title_nbt.rs` | 10 | 1 |

The exact original-versus-extracted 8-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 103 function
names. The combined test count remains 45.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 8-line class diff: empty.
- Original-versus-split function-name multiset: identical, 103 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the entity-movement write-turn test and `state` helper retain their
  single aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 8-line class preservation, unchanged 103-name function multiset
  and 45-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
