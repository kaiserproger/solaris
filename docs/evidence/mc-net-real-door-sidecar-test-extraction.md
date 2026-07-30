# `mc-net` real-door sidecar test extraction

Date: 2026-07-30

Checkpoint base: `a8476b63ea46061486af62ac41869cb8fbef04ef`

## Result

The ignored real-door parity test
`real_door_states_plan_hand_toggle_when_sidecar_is_present` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/real_door_sidecar.rs`. Its exact
`explicit local 26.1.2 blocks sidecar parity gate` ignore contract remains in
place.

When explicitly selected, the gate loads the local 26.1.2 blocks report,
constructs closed lower and upper oak-door states, and verifies that the
hand-toggle plan publishes the matching open state for both halves. The shared
registry, world, state-selection, and interaction-planning helpers remain
aggregate-owned.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding Loader interaction test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,234 physical lines and 88
test functions. The moved class contains 40 physical lines and one ignored
test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,194 | 87 |
| `play/tests/real_door_sidecar.rs` | 44 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 146 function names. The
combined test count remains 88.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Explicit selected ignored gate: 1 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 40-line class diff: empty.
- Original-versus-split function-name multiset: all 146 entries identical.
- The child module uses explicit imports and contains no `use super::*` or
  crate-wide public item.
- Boundary inspection confirms that the preceding Loader interaction test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
