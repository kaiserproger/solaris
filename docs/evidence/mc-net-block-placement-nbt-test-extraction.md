# `mc-net` block-placement/NBT test extraction

Date: 2026-07-30

Checkpoint base: `0fa969db950a22422ff12cacaf2c0deb239dd690`

## Result

The contiguous twelve-test block-placement and block-entity NBT class moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/block_placement_nbt.rs`. Its exact boundary runs
from `door_half_state_builds_two_block_placement_states` through
`campfire_update_nbt_contains_visible_cooking_items_only` and includes the
oriented-placement and torch-planning helpers. The preceding
`button_and_door_test_registry` helper and following
`bed_occupancy_test_registry` helper remain in the aggregate module. The child
module uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain coverage for door half-state construction,
directional stair and slab placement, supported standing and wall torches,
cursor height, fail-closed noncanonical stairs, sign state and update NBT, and
visible campfire cooking-item NBT.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 13,332 physical lines and 253
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 12,778 | 241 |
| `play/tests/block_placement_nbt.rs` | 565 | 12 |

After normalizing the one required return-type path from the aggregate
module's `super::block_placement` to the child module's
`crate::play::block_placement`, an exact comparison of the 554-line moved body
has an empty diff. A sorted multiset comparison of all 328 function names in
the original aggregate file against both resulting modules is identical. The
combined test count remains 253.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 12 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact normalized original-versus-extracted 554-line body diff: empty.
- Original-versus-split function-name multiset: all 328 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that `button_and_door_test_registry` and
  `bed_occupancy_test_registry` remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
