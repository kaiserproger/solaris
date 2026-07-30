# `mc-net` use-item-on preflight test extraction

Date: 2026-07-30

Checkpoint base: `2879b5cd6b7a3dd0f8c6774d104febafef048726`

## Result

The complete use-item-on preflight class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/use_item_on_preflight.rs`. The moved class
contains its `test_use_item_on` packet helper and four tests covering a dead
Survival player, an unsupported Adventure mode, an out-of-reach Survival
target, rejection of an out-of-reach Creative target, and acceptance of
reachable Creative and Survival targets.

The child uses explicit parent imports and does not inherit the aggregate
file's `use super::*`. The immediately preceding
`loader_interaction_channel_is_claimed_before_extension_forwarding` test and
following ignored local-sidecar door parity gate remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,334 physical lines and 92
test functions. The moved body contains 101 physical lines, four tests, and
their single helper. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,234 | 88 |
| `play/tests/use_item_on_preflight.rs` | 106 | 4 |

The exact original-versus-extracted class diff is empty after normalizing only
the helper's required sibling-module visibility. The original aggregate and
the split result have the same sorted multiset of 151 function names. The
combined test count remains 92.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 4 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 101-line class diff: empty after normalizing
  only `test_use_item_on` from private to `pub(super)`.
- Original-versus-split function-name multiset: all 151 entries identical.
- The child module uses explicit imports, exposes its shared helper only to its
  parent module, and contains no `use super::*` or crate-wide public item.
- Boundary inspection confirms that the preceding Loader interaction test and
  following ignored local-sidecar door parity gate remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
