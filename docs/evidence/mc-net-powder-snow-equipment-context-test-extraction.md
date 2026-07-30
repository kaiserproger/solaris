# `mc-net` powder-snow equipment-context test extraction

Date: 2026-07-31

Checkpoint base: `c9a1ae21438255e6264092e03e7503ee947873fa`

## Result

The singleton test
`powder_snow_collision_uses_player_equipment_and_movement_context` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/powder_snow_equipment_context.rs`.

The preserved test publishes powder snow and verifies all four collision
contexts: a player without leather boots sinks, leather boots support entry
from above, Shift permits descent, and boots do not make powder snow solid
after the player is already inside it.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding torch/campfire exact-shape test and
following `button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,736 physical lines and 72
test functions. The moved class contains 40 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,696 | 71 |
| `play/tests/powder_snow_equipment_context.rs` | 47 | 1 |

The exact original-versus-extracted 40-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 130 function names.
The combined test count remains 72.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 40-line class diff: empty.
- Original-versus-split function-name multiset: identical, 130 entries before
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
  the empty 40-line class diff, unchanged 130-name function multiset,
  preserved 72-test total, all four collision contexts, explicit imports,
  aggregate boundaries, evidence, links, and next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
