# `mc-net` falling-block test extraction

Date: 2026-07-30

Checkpoint base: `26835f1dc20bd8a2d8d5c311167dac93bf298a97`

## Result

The contiguous five-test falling-block start, landing, world-writer
independence, item-drop, and stale-plan class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/falling_blocks.rs`. Its exact boundary runs from
`falling_block_starts_when_support_edit_becomes_replaceable` through
`stale_falling_block_landing_plan_keeps_entity_and_replacement`. The following
ignored local-sidecar parity gate
`real_door_states_plan_hand_toggle_when_sidecar_is_present` remains
aggregate-owned. Shared aggregate helpers are imported explicitly; the child
module does not inherit the aggregate file's `use super::*`.

The child imports `FallingBlockStart`, `LandedFallingBlock`, and
`plan_falling_block_starts` directly from the production module, so their
now-unused aggregate import was removed after the strict Clippy gate identified
it.

The extracted tests retain support-loss start planning, planning completion
without the world writer, solid-landing item drop and despawn, landing planning
without the world writer, and stale landing-plan rejection that preserves both
the entity and replacement block.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 5,998 physical lines and 125
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 5,639 | 120 |
| `play/tests/falling_blocks.rs` | 379 | 5 |

The moved body contains 358 physical lines and five tests. The final structural
comparison covers all 187 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are unchanged. The combined test count remains 125.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 5 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed after
  removing the stale aggregate falling-block import reported by the first run.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 358-line body diff: empty.
- Original-versus-split function-name multiset: all 187 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the following ignored local-sidecar door
  parity gate remains aggregate-owned.
- Independent read-only review: passed the extraction with no actionable
  findings; the subsequent strict Clippy cleanup only removed the stale
  aggregate import described above.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
