# `mc-net` fence deflation-boundary test extraction

Date: 2026-07-31

Checkpoint base: `3e39e0aedea522a1e861aaf8155cd5fa9a6b9b7a`

## Result

The singleton test
`player_collision_scans_fence_below_at_deflated_top_boundary` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/fence_deflation_boundary.rs`.

The preserved test publishes an isolated oak-fence state and uses the oracle
deflation `f64::from(1.0e-5_f32)` to check both sides of its `1.5`-block top.
A sub-boundary overlap is deflated away while the larger overlap proves that
the minimum-Y scan retains the fence below.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding tall/narrow fence-shape test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,655 physical lines and 69
test functions. The moved class contains 34 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,621 | 68 |
| `play/tests/fence_deflation_boundary.rs` | 39 | 1 |

The exact original-versus-extracted 34-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 127 function names.
The combined test count remains 69.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 34-line class diff: empty.
- Original-versus-split function-name multiset: identical, 127 entries before
  and after.
- Unique test/module ownership, explicit imports, and aggregate boundaries:
  pass. The module declaration and test each occur once, the child contains no
  wildcard parent import or public item, and both adjacent aggregate-owned
  boundaries remain present.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 34-line class diff, unchanged 127-name function multiset,
  preserved 69-test total, exact fence state and deflation poses, explicit
  imports, aggregate boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
