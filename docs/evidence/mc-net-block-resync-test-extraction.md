# `mc-net` block-resync test extraction

Date: 2026-07-30

Checkpoint base: `b98ca1004076595483ab8ed091c2212cda563c0f`

## Result

The contiguous two-test authoritative block-resync class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/block_resync.rs`. Its exact boundary runs from
`rejected_visible_block_edit_resyncs_authoritative_cached_state` through
`rejected_use_item_on_resync_does_not_wait_for_world_writer`. The preceding
shared `register_interaction_player` helper and following `shield_item_state`
helper remain aggregate-owned. The child module uses explicit imports and does
not inherit the aggregate file's `use super::*`.

The extracted tests retain authoritative cached-state block publication,
world-writer independence, exact block-update ordering, held-item resync, and
block-change acknowledgement coverage.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 10,206 physical lines and 196
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 10,093 | 194 |
| `play/tests/block_resync.rs` | 122 | 2 |

The moved body contains 113 physical lines and two tests. The final structural
comparison covers all 261 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are checked below. The combined test count remains 196.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 2 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 113-line body diff: empty.
- Original-versus-split function-name multiset: all 261 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared `register_interaction_player`
  and `shield_item_state` helpers remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
