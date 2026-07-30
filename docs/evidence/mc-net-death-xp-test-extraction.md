# `mc-net` recoverable-death XP test extraction

Date: 2026-07-30

Checkpoint base: `f71180bcecc72a11a40694745e852f860cf93987`

## Result

The recoverable-death experience level-cap test moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/death_xp.rs`. The preceding
`use_item_on_preflight_rejects_out_of_reach_creative_and_allows_reachable_targets`
test and following ignored local-sidecar door parity gate remain
aggregate-owned.

The child imports `XpState` and `recoverable_death_xp` explicitly and does not
inherit the aggregate file's `use super::*`. The test retains both assertions:
a level-40 state with 1,000 total experience recovers 100, while level 3
recovers 21.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 4,347 physical lines and 93
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,334 | 92 |
| `play/tests/death_xp.rs` | 15 | 1 |

The moved body contains 13 physical lines and one test. The final structural
comparison covers all 152 function names from the original aggregate module.
The exact original-versus-extracted body diff and the sorted function-name
multiset are unchanged. The combined test count remains 93.

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
- Exact original-versus-extracted 13-line body diff: empty.
- Original-versus-split function-name multiset: all 152 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding use-item preflight test and
  following ignored local-sidecar door parity gate remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
