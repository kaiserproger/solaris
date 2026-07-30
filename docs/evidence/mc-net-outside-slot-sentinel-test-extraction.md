# `mc-net` outside-slot sentinel test extraction

Date: 2026-07-31

Checkpoint base: `1960231b08fdde54252ea160060a9cd1d714c1ae`

## Result

The singleton test
`only_vanilla_outside_slot_sentinel_can_drop_the_cursor`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/outside_slot_sentinel.rs`.

The preserved test still builds the exact pickup-click fixture; classifies
slot `-999` as an outside pickup with button zero; and rejects slots `-1`,
`-2`, and `i16::MIN` as unsupported.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding dense entity-
cohort test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,070 physical lines, 27 test
functions, and 80 function-name entries under the checkpoint's
whitespace-tolerant `fn` scan, including nested test-local helpers. The moved
class contains 23 physical lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,047 | 26 |
| `play/tests/outside_slot_sentinel.rs` | 27 | 1 |

The exact original-versus-extracted 23-line class diff is empty. Under the same
whitespace-tolerant scan, the original aggregate and split result retain the
same sorted multiset of 80 function names. The combined test count remains 27.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 23-line class diff: empty; both copies have
  SHA-256 `ec69e88da975123f9d4668fa8ae8d19d4dee6a08d907f9aa25660c195a7e5d61`.
- Original-versus-split whitespace-tolerant function-name multiset, including
  nested test-local helpers: identical, 80 names.
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
