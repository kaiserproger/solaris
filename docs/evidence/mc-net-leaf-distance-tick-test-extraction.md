# `mc-net` leaf-distance tick test extraction

Date: 2026-07-30

Checkpoint base: `663601f9ff12ac31189cb471acb7921ea5e79bf1`

## Result

The contiguous leaf-distance scheduled-tick class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/leaf_distance_ticks.rs`. The class contains
`removed_log_pushes_leaf_distance_updates_through_scheduled_ticks`, its
`run_scheduled_block_ticks_for_range` helper, and
`stable_leaf_tick_is_checkpoint_only_without_world_journal_decision`. The
preceding ignored real-door gate and following scheduled-hopper class remain in
the aggregate module. The child module uses explicit imports and does not
inherit the aggregate file's `use super::*`. The helper is `pub(super)` and
explicitly imported by the aggregate module because the following hopper tests
already share it. The next cursor relocates it with that hopper class, which
owns every remaining caller.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 16,495 physical lines and 280
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 16,308 | 278 |
| `play/tests/leaf_distance_ticks.rs` | 198 | 2 |

A sorted multiset comparison of all 357 function names in the original
aggregate file against both resulting modules is identical. The two test
bodies and helper body are unchanged; the only extracted-source difference is
the helper's required `pub(super)` visibility. The combined test count remains
280.

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
- Original-versus-split function-name multiset: all 357 entries identical.
- Extracted-source comparison: only the shared helper's visibility changed.
- The child module contains no `use super::*`.
- Boundary inspection confirms the preceding ignored real-door gate and
  following scheduled-hopper class remain aggregate-owned.
- Independent read-only review: passed with no findings.
