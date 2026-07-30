# `mc-net` container-safety test extraction

Date: 2026-07-30

Checkpoint base: `ddc40829834b8afb5b6f701376919376028ce5ed`

## Result

The contiguous two-test common-container and cauldron safe-interaction class
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/container_safety.rs`. Its exact boundary
runs from `common_container_paper_cuts_resolve_to_existing_menus` through
`cauldron_variants_are_safe_interaction_targets`. The preceding bed/sleep
class remains in its focused module, while the following
`interaction_state_for_items` helper remains aggregate-owned. The child module
uses explicit imports and does not inherit the aggregate file's
`use super::*`.

The extracted tests retain coverage for vanilla furnace-family menu mappings,
supported enchanting and stonecutter mappings, safe rejection of unsupported
survival stations, and all four cauldron variants.

This is a test-ownership change only. Production code and behavior did not
change. Three parent-module `#[cfg(test)]` menu-id imports became unused after
the move and were removed; runtime imports and code are unchanged.

## Concentration

Before the extraction, `play/tests.rs` contained 12,310 physical lines and 229
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 12,234 | 227 |
| `play/tests/container_safety.rs` | 85 | 2 |

The moved body contains 76 physical lines and two tests. The final structural
comparison covers all 297 function names from the original aggregate module.
After applying the required `super::containers` to
`super::super::containers` path change and formatting the normalized body, its
exact diff is empty. The sorted function-name multiset is identical, and the
combined test count remains 229.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 2 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact normalized and reformatted original-versus-extracted 76-line body
  diff: empty.
- Original-versus-split function-name multiset: all 297 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding bed/sleep class and
  following `interaction_state_for_items` helper remain in their intended
  modules.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
