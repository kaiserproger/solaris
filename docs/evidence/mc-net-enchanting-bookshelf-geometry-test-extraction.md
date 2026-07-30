# `mc-net` enchanting bookshelf-geometry test extraction

Date: 2026-07-31

Checkpoint base: `5acc6d4a6e3cbba97bc350b432aba99be3bb6139`

## Result

The singleton test
`enchanting_bookshelf_geometry_requires_clear_midpoints_and_caps_at_fifteen`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_bookshelf_geometry.rs`.

The preserved test still uses the exact enchanting-table, two provider, and
two midpoint positions; the provider and clear-midpoint sets; the count of `2`
when both midpoints are clear; the count of `1` when only the first midpoint is
clear; and the cap of `15` when every candidate provider and midpoint passes.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding held-sharpness test
and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,670 physical lines, 38 test
functions, and 96 function-name entries. The moved class contains 44 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,626 | 37 |
| `play/tests/enchanting_bookshelf_geometry.rs` | 48 | 1 |

The exact original-versus-extracted 44-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 96 function
names. The combined test count remains 38.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 44-line class diff: empty; both copies have
  SHA-256 `0ec157cc3814dae4bf269c364639cb9dda2965135cb133d193ffaf98cda355d5`.
- Original-versus-split function-name multiset: identical, 96 names.
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
