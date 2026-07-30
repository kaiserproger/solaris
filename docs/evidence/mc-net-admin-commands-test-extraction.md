# `mc-net` admin-command test extraction

Date: 2026-07-30

Checkpoint base: `a826436ffb9782a2b0a249edc36fdf4729aafb92`

## Result

The contiguous four-test admin-command parsing, permission-aware command-tree,
runtime-control status, and local-development permission class moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/admin_commands.rs`. Its exact boundary runs from
`admin_dispatcher_parses_slash_commands_and_permissions` through
`local_dev_profiles_are_op_capable_for_now`. The preceding
`persistent_container_claim_check_covers_furnace_and_both_chest_halves` test
and following ignored local-sidecar door parity gate remain aggregate-owned.

The child imports production command, login, control-status, and protocol types
explicitly and does not inherit the aggregate file's `use super::*`. The
extracted tests retain accepted and rejected admin-command parsing, gamerule
validation, permission-aware command trees and suggestions, disabled/draining
runtime-control status projection, and local-development operator
capabilities.

The now-unused aggregate `runtime_control_status_message` and
`command_tree_packet` imports were removed after the first focused compile
identified them; the child imports both test-only functions directly.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,987 physical lines and 106
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,789 | 102 |
| `play/tests/admin_commands.rs` | 206 | 4 |

The moved body contains 198 physical lines and four tests. The final structural
comparison covers all 165 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are unchanged. The combined test count remains 106.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 4 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed with `0 fail / KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed after
  removing the two stale aggregate test imports reported by the focused
  compile.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 198-line body diff: empty.
- Original-versus-split function-name multiset: all 165 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding persistent-container claim
  test and following ignored local-sidecar door parity gate remain
  aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
