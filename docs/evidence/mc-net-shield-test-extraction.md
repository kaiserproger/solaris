# `mc-net` shield test extraction

Date: 2026-07-30

Checkpoint base: `dd38b2769175e57814ebd366cdcace39d8d636d6`

## Result

The contiguous thirteen-test shield use, activation, directional blocking,
durability, game-mode transition, publication, and CAS-conflict class moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/shield.rs`. Its exact boundary runs from
`shield_use_starts_blocking_state_for_shield_stack` through
`repeated_shield_cas_conflict_refreshes_owner_state_and_fails_closed`. The
following shared `campfire_test_interaction_state` helper remains
aggregate-owned. Shared aggregate helpers are imported explicitly; the child
module does not inherit the aggregate file's `use super::*`.

The three game-mode transition calls add one required `super` qualifier after
the move because `command_execution` is owned by `play`, one level above the
new child test module. The child imports the two living-entity flag constants
directly, so their now-unused `#[cfg(test)]` aggregate re-export was removed
from `play.rs`. No other test-body change was needed.

The extracted tests retain shield-use metadata, activation delay and
directional policy, active-stack durability and break behavior, game-mode
transition policy, projectile/PVP publication, owner-state refresh, and
fail-closed repeated-CAS behavior.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 6,927 physical lines and 149
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 6,327 | 136 |
| `play/tests/shield.rs` | 631 | 13 |

The moved body contains 600 physical lines and thirteen tests. The final
structural comparison covers all 211 function names from the original
aggregate module. The original-versus-extracted body diff is empty after
normalizing the three required `command_execution` path adjustments, and the
sorted function-name multiset is unchanged. The combined test count remains
149.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 13 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-extracted 600-line body diff after normalizing the three
  required `command_execution` path adjustments: empty.
- Original-versus-split function-name multiset: all 211 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that `campfire_test_interaction_state` remains
  aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
