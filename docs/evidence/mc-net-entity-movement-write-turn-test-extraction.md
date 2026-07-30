# `mc-net` entity-movement write-turn test extraction

Date: 2026-07-31

Checkpoint base: `a58a95ebcf47fba3a275092586bd6a331c457c9b`

## Result

The singleton test
`entity_movement_write_turn_preserves_order_across_the_budget_boundary` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/entity_movement_write_turn.rs`.

The preserved test still builds the inclusive
`0..=ENTITY_MOVEMENTS_PER_WRITE_TURN` input with the exact per-index entity id,
position and absolute wire move, zero velocity and rotation, grounded state,
and disabled velocity/head-rotation sends. It then preserves the same
write-turn split, full current batch, first and last ids, exact remaining-value
expectation, and sole remaining id.

The child imports its movement types and private write-turn boundary explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding bounded-outbound-pressure test and following `state` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,104 physical lines and 44
test functions. The moved class contains 35 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,069 | 43 |
| `play/tests/entity_movement_write_turn.rs` | 40 | 1 |

The exact original-versus-extracted 35-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 102 function
names. The combined test count remains 44.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 35-line class diff: empty.
- Original-versus-split function-name multiset: identical, 102 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the bounded-outbound-pressure test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 35-line class preservation, unchanged 102-name function multiset
  and 44-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
