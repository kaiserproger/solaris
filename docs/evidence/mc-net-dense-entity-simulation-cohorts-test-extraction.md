# `mc-net` dense entity-simulation cohort test extraction

Date: 2026-07-31

Checkpoint base: `75b1855deef0c41393558404ceaefb6b27a82ba9`

## Result

The singleton test
`dense_entity_simulation_rotates_lane_sized_cohorts`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/dense_entity_simulation_cohorts.rs`.

The preserved test still builds `5,120` sequential entity ids; runs ten
bounded turns with limit `512`; asserts the exact due-set size on every turn;
and proves every entity is visited exactly once.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding ordinary entity-
goal cadence test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,047 physical lines, 26 test
functions, and 79 function-name entries under the checkpoint's
whitespace-tolerant `fn` scan, including nested test-local helpers. The moved
class contains 18 physical lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,029 | 25 |
| `play/tests/dense_entity_simulation_cohorts.rs` | 22 | 1 |

The exact original-versus-extracted 18-line class diff is empty. Under the same
whitespace-tolerant scan, the original aggregate and split result retain the
same sorted multiset of 79 function names. The combined test count remains 26.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 18-line class diff: empty; both copies have
  SHA-256 `4fc00801306476b7fbfc1c198e1501cfbd51fc6d0f5ba5fc4b46f6ef9bfadc8d`.
- Original-versus-split whitespace-tolerant function-name multiset, including
  nested test-local helpers: identical, 79 names.
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
