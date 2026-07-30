# `mc-net` exact farmland-fallback collision test extraction

Date: 2026-07-31

Checkpoint base: `1254f8abf242b3c95a4958eda2f22f0c0f6b065e`

## Result

The singleton test
`player_collision_uses_farmland_fallback_for_exact_low_id_semantics` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/farmland_fallback_collision.rs`.

The preserved test uses the unchanged aggregate-owned
`low_id_exact_farmland_test_state` fixture, which publishes exact farmland on
state id 1. It verifies that `(0.5, 64.9375, 0.5)` remains non-colliding at the
direct `15/16` top while the lower `(0.5, 64.90, 0.5)` pose collides.

The child imports its fixture and collision helpers explicitly and does not
inherit the aggregate file's `use super::*`. The immediately preceding
wrong-properties test and following `button_test_registry` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,506 physical lines and 63
test functions. The moved class contains 14 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,492 | 62 |
| `play/tests/farmland_fallback_collision.rs` | 19 | 1 |

The exact original-versus-extracted 14-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 121 function names.
The combined test count remains 63.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 14-line class diff: empty.
- Original-versus-split function-name multiset: identical, 121 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate fixture ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  the fixture retains its single aggregate definition and single child caller,
  and both adjacent boundaries remain present.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 14-line class diff, unchanged 121-name function multiset,
  preserved 63-test total, exact helper ownership, poses and messages, explicit
  imports, aggregate boundaries, evidence metrics and links, and the exact
  next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
