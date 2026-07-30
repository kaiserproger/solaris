# `mc-net` oracle-AABB-deflation collision test extraction

Date: 2026-07-31

Checkpoint base: `b19d36c753b019ea99e69ee4ce4ef6f8d33dafd1`

## Result

The singleton test `player_collision_uses_oracle_aabb_deflation_boundary`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/oracle_aabb_deflation_boundary.rs`.

The preserved test publishes the exact `minecraft:stone_slab` state with
`type=bottom` and `waterlogged=false`, then derives
`oracle_deflation` with `f64::from(1.0e-5_f32)`. It verifies that the
`64.5 - oracle_deflation / 2.0` overlap remains non-colliding while the
`64.5 - oracle_deflation * 2.0` overlap collides.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding bottom-slab test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,553 physical lines and 65
test functions. The moved class contains 28 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,525 | 64 |
| `play/tests/oracle_aabb_deflation_boundary.rs` | 33 | 1 |

The exact original-versus-extracted 28-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 123 function names.
The combined test count remains 65.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 28-line class diff: empty.
- Original-versus-split function-name multiset: identical, 123 entries before
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
  the empty 28-line class diff, unchanged 123-name function multiset,
  preserved 65-test total, exact slab state, oracle deflation, poses,
  assertions and messages, explicit imports, aggregate boundaries, evidence
  metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
