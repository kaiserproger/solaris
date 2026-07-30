# `mc-net` play custom-payload test extraction

Date: 2026-07-30

Checkpoint base: `ed54c5c35f4102af3854aab4133a3ca1c84e459c`

## Result

The complete two-test play custom-payload classification class moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/play_custom_payload.rs`.

The first test preserves the fail-fast size fence at one byte above
`DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES` and asserts the exact oversized length. The
second constructs a `solaris:loader/interaction` payload and verifies that the
reserved Loader channel is claimed as `LoaderInteraction` before extension
forwarding.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding client-view-distance policy test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,194 physical lines and 87
test functions. The moved class contains 28 physical lines and two tests.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,166 | 85 |
| `play/tests/play_custom_payload.rs` | 33 | 2 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 145 function names. The
combined test count remains 87.

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
- Exact original-versus-extracted 28-line class diff: empty.
- Original-versus-split function-name multiset: all 145 entries identical.
- Each moved test name occurs exactly once across aggregate and child modules.
- The child module uses explicit imports and contains no `use super::*` or
  public item.
- Boundary inspection confirms that the preceding client-view-distance test
  and following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
