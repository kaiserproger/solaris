# `mc-net` campfire-cooking test extraction

Date: 2026-07-30

Checkpoint base: `80e40f402ade464120288bdd0f7447f73de20e2f`

## Result

The contiguous nine-test furnace-recipe and campfire cooking, persistence,
hydration, and journal-durability class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/campfire_cooking.rs`. Its exact boundary runs
from `furnace_like_recipe_lookup_uses_matching_cooking_category` through
`campfire_tick_does_not_load_cold_chunks_and_is_durable_when_resident`. The
following `shield_use_starts_blocking_state_for_shield_stack` test remains
aggregate-owned. Shared aggregate helpers remain in the parent module and are
imported explicitly; the child module does not inherit the aggregate file's
`use super::*`.

The two `WorldChunkJournal` call sites use one explicit import after the move
instead of their original `super::world_journal` path, which no longer names
the production module from the extra test-module nesting. No other test-body
change was needed.

The extracted tests retain recipe-category selection, full-slot rejection,
unlit cooling, interaction conservation, completed-output staging, vanilla and
legacy NBT handling, resident-only startup hydration, and journal durability
without cold-chunk loading.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 7,593 physical lines and 158
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 6,927 | 149 |
| `play/tests/campfire_cooking.rs` | 693 | 9 |

The moved body contains 666 physical lines and nine tests. The final structural
comparison covers all 220 function names from the original aggregate module.
The original-versus-extracted body diff is empty after normalizing the two
required `WorldChunkJournal` path shortenings, and the sorted function-name
multiset is unchanged. The combined test count remains 158.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 9 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-extracted 666-line body diff after normalizing the two
  required journal path shortenings: empty.
- Original-versus-split function-name multiset: all 220 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that
  `shield_use_starts_blocking_state_for_shield_stack` remains aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
