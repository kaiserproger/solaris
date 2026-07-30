# `mc-net` furnace test extraction

Date: 2026-07-30

Checkpoint base: `52b50c709ad57d60648478d9d34e84ae423c2ae6`

## Result

The contiguous fourteen-test furnace class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/furnace.rs`. Its exact boundary runs from
`furnace_window_swap_and_throw_mutate_menu_slots` through
`stale_furnace_click_after_peer_mutation_resyncs_without_mutating_storage`.
The preceding shared `decode_player_position_sync_packets` helper and
following `chest_window_swap_and_throw_mutate_storage_slots` test remain
aggregate-owned. The child module uses explicit imports and does not inherit
the aggregate file's `use super::*`.

The extracted tests retain furnace menu mutation, vanilla fuel and lava-bucket
handling, cook cooling and recipe experience, world-writer-independent
ticking, resident-state and lit-block publication, stale-wave replanning,
viewer updates, continued ticking after close, and stale-click resync
coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 10,093 physical lines and 194
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 8,931 | 180 |
| `play/tests/furnace.rs` | 1,187 | 14 |

The moved body contains 1,162 physical lines and fourteen tests. The final
structural comparison covers all 259 function names from the original
aggregate module. The exact original-versus-extracted body diff and the sorted
function-name multiset are checked below. The combined test count remains 194.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 14 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 1,162-line body diff: empty.
- Original-versus-split function-name multiset: all 259 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared
  `decode_player_position_sync_packets` helper and following chest test remain
  aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
