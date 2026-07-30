# `mc-net` plant adapter test extraction

Date: 2026-07-30

Checkpoint base: `e2a4fa56aa426b4d4f0a6245e5c4d76a334e44d0`

## Result

The complete plant/crop/cactus/bamboo/sapling/bonemeal adapter-test class moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/plants.rs`. The child module has explicit imports
and does not inherit the aggregate file's `use super::*`.

This is a test-ownership change only. Production code and plant algorithms did
not change.

## Concentration

Before the extraction, `play/tests.rs` contained 21,707 physical lines and 379
test functions, counting test attributes with or without arguments. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 18,483 | 303 |
| `play/tests/plants.rs` | 3,246 | 76 |

The focused class contains 65 synchronous and 11 Tokio tests. It covers crop
drops and placement, farmland collision/trampling/tilling, cactus and vertical
plant survival, bamboo, random-tick growth, stems, sweet berries, cocoa,
bonemeal, and single/2x2 sapling growth. A sorted multiset comparison of every
function name in the original aggregate file against both resulting modules
has no difference; the total source test count remains 379.

Shared fixtures still used by unrelated aggregate tests remain in
`play/tests.rs`. The extraction therefore does not duplicate their authority or
pull unrelated short-grass/configured-loot, general collision, sign, fluid,
falling-block, or natural fire/leaves tests into the plant module.

Benchmark: not applicable. The checkpoint changes only test-module ownership.

## Validation

- Focused compile: `cargo test -p mc-net --lib play::tests::plants:: --no-run`
  passed.
- Focused list: exactly 76 tests.
- Focused execution: 76 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-split function-name multiset: identical.
- `git diff --check`: passed.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
