# `mc-net` player-damage test extraction

Date: 2026-07-30

Checkpoint base: `8d1d56ce110f2d0e870424be3fbc3304fc5ff506`

## Result

The contiguous eleven-test player contact-damage, pushed-damage, armor,
knockback, and stale-publication tail moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/player_damage.rs`. Its exact boundary runs from
`player_collision_allows_lit_campfire_overlap_for_contact_damage` through
`stale_damage_publication_does_not_apply_health_side_effects`. The preceding
shared `campfire_test_interaction_state` helper and the following
`contact_damage.rs`, `gamerule_keep_inventory.rs`,
`inventory_and_survival.rs`, and `spawning_and_world.rs` includes remain
aggregate-owned. Shared aggregate helpers are imported explicitly; the child
module does not inherit the aggregate file's `use super::*`.

The extracted tests retain campfire contact collision and death behavior,
commit-before-client-publication semantics, shield and armor publication,
vanilla knockback boundaries, newer attacker-cost preservation, and stale
health-side-effect rejection.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 6,327 physical lines and 136
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 5,998 | 125 |
| `play/tests/player_damage.rs` | 359 | 11 |

The moved body contains 329 physical lines and eleven tests. The final
structural comparison covers all 198 function names from the original
aggregate module. The exact original-versus-extracted body diff and the sorted
function-name multiset are unchanged. The combined test count remains 136.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 11 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Exact original-versus-extracted 329-line body diff: empty.
- Original-versus-split function-name multiset: all 198 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the shared campfire helper and following
  include-based classes remain aggregate-owned.
- Independent read-only review: passed with no actionable findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
