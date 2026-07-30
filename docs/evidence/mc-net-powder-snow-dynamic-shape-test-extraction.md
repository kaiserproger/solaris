# `mc-net` powder-snow dynamic-shape test extraction

Date: 2026-07-31

Checkpoint base: `794ff21f3b81589ecf1e5803d85bb75e922154a9`

## Result

The exact-state identity test
`powder_snow_dynamic_shape_requires_exact_vanilla_state_identity` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/powder_snow_dynamic_shape.rs`.

The preserved test mutates the embedded powder-snow property fingerprint while
retaining its dense state id, equips leather boots, and verifies that the
collision path uses the conservative custom-block fallback rather than the
vanilla dynamic shape.

The child uses explicit imports and does not inherit the aggregate file's
`use super::*`. The immediately preceding long-fall shape test and following
`button_test_registry` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,791 physical lines and 74
test functions. The moved class contains 35 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,756 | 73 |
| `play/tests/powder_snow_dynamic_shape.rs` | 43 | 1 |

The exact original-versus-extracted class diff is empty. The original aggregate
and split result have the same sorted multiset of 132 function names. The
combined test count remains 74.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 35-line class diff: empty.
- Original-versus-split function-name multiset: identical, 132 entries before
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
  the empty 35-line body diff, unchanged 132-name function multiset, preserved
  74-test total, explicit child imports, aggregate boundaries, evidence, and
  next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
