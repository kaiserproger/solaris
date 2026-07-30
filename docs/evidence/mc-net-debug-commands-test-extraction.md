# `mc-net` debug-command test extraction

Date: 2026-07-30

Checkpoint base: `16d44617b69308cbb0e63e3eccab919edd136e9f`

## Result

The contiguous three-test debug-command parsing, water-corridor fixture, and
zero-count give execution class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/debug_commands.rs`. Its exact boundary runs from
`debug_commands_parse_survival_mutations_and_give` through
`debug_give_zero_count_clears_hotbar_slot_before_item_lookup`. The preceding
`recoverable_death_xp_uses_level_cap` test and following ignored local-sidecar
door parity gate remain aggregate-owned.

The child imports shared aggregate types and helpers explicitly and does not
inherit the aggregate file's `use super::*`. The extracted tests retain valid
and invalid debug-command parsing, bounded outbound-pressure and water-corridor
arguments, a unique closed source-water corridor fixture, and the zero-count
give path that clears one hotbar slot and publishes the empty stack.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,491 physical lines and 96
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,347 | 93 |
| `play/tests/debug_commands.rs` | 156 | 3 |

The moved body contains 144 physical lines and three tests. The final
structural comparison covers all 155 function names from the original
aggregate module. The exact original-versus-extracted body diff and the sorted
function-name multiset are unchanged. The combined test count remains 96.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 3 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed with `0 fail / KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 144-line body diff: empty.
- Original-versus-split function-name multiset: all 155 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding recoverable-death XP test
  and following ignored local-sidecar door parity gate remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
