# `mc-net` game-mode command parser test extraction

Date: 2026-07-30

Checkpoint base: `5bcca96821460c37e7d9d36d043235bd130dbd8e`

## Result

The complete two-test game-mode command parser class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/gamemode_commands.rs`.

The positive matrix preserves all named modes (`survival`, `creative`,
`adventure`, and `spectator`) plus numeric mode `1`. The rejection matrix
preserves a wrong command root, an unknown game mode, and an extra argument.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding arrow-launch geometry test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,158 physical lines and 84
test functions. The moved class contains 30 physical lines and two tests.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,128 | 82 |
| `play/tests/gamemode_commands.rs` | 32 | 2 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 142 function names. The
combined test count remains 84.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 2 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 30-line class diff: empty.
- Original-versus-split function-name multiset: all 142 entries identical.
- Each moved test name occurs exactly once across aggregate and child modules.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding arrow-launch test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
