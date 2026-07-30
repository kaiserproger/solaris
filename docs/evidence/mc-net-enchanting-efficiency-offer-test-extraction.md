# `mc-net` efficiency-offer data test extraction

Date: 2026-07-31

Checkpoint base: `0842fc360a2d56a3f68eed1e608a5d2dabb22d30`

## Result

The singleton test
`enchanting_data_exposes_the_supported_efficiency_offer`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_efficiency_offer.rs`.

The preserved test still builds the required registries; resolves the stone
pickaxe; creates window `7` at `(0, 0, 0)` with the pickaxe input; uses seed
`123` over the default XP state; and asserts the complete zero-bookshelf
ten-property enchanting payload.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding enchanting-
selection test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,325 physical lines, 31 test
functions, and 84 function-name entries. The moved class contains 30 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,295 | 30 |
| `play/tests/enchanting_efficiency_offer.rs` | 32 | 1 |

The exact original-versus-extracted 30-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 84 function
names. The combined test count remains 31.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 30-line class diff: empty; both copies have
  SHA-256 `39f6e273afd16b080c146474d6c8a031fa335013f79073faccc29decf64f6cba`.
- Original-versus-split function-name multiset: identical, 84 names.
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
