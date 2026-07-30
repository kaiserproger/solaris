# `mc-net` arrow-launch test extraction

Date: 2026-07-30

Checkpoint base: `ea5d98157f34b0c39fb00cc9b295dc722f09c790`

## Result

The arrow launch geometry and draw-power test
`arrow_launch_uses_player_look_direction_and_draw_power` moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/arrow_launch.rs`.

The preserved matrix constructs a player pose at `(1, 64, 2)` with yaw `90`
and pitch `-30`, then verifies the exact three-axis spawn position and the
half-draw three-axis arrow velocity.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding entity-tick cadence test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,128 physical lines and 82
test functions. The moved class contains 18 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,110 | 81 |
| `play/tests/arrow_launch.rs` | 20 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 140 function names. The
combined test count remains 82.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 18-line class diff: empty.
- Original-versus-split function-name multiset: all 140 entries identical.
- The moved test name occurs exactly once across aggregate and child modules.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding entity-tick cadence test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
