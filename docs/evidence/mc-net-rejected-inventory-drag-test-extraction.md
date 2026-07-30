# `mc-net` rejected inventory-drag test extraction

Date: 2026-07-31

Checkpoint base: `1ee104852cae7aa99cf3553db0f0322a99687b4f`

## Result

The singleton test
`rejected_inventory_drag_resyncs_without_mutation_or_owner_publication`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/rejected_inventory_drag.rs`.

The preserved test still builds the exact dirt registry and carried-stack
fixtures; runs the opening and rejected quick-craft clicks with the same
survival, pose, XP, and script context; and asserts unchanged inventory and
carried state, zero simulation depth, and one exact resync packet.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding outside-slot
classification test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,127 physical lines, 28 test
functions, and 81 function-name entries under the checkpoint's
whitespace-tolerant `fn` scan, including nested test-local helpers. The moved
class contains 57 physical lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,070 | 27 |
| `play/tests/rejected_inventory_drag.rs` | 66 | 1 |

The exact original-versus-extracted 57-line class diff is empty. Under the same
whitespace-tolerant scan, the original aggregate and split result retain the
same sorted multiset of 81 function names. The combined test count remains 28.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 57-line class diff: empty; both copies have
  SHA-256 `91a090566b99caf8482988a4ab01b1d92252833e9741ee02fc419b7cbbb13b6a`.
- Original-versus-split whitespace-tolerant function-name multiset, including
  nested test-local helpers: identical, 81 names.
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
