# `mc-net` attack/PVP test extraction

Date: 2026-07-30

Checkpoint base: `70ca040fffcba950802b96382ca2904abdfb6e73`

## Result

The contiguous fifteen-test attack-strength, PVP, weapon-cost, reach,
hurt-immunity, and lethal-reward class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/attack_pvp.rs`. Its exact boundary begins with
the `assert_attack_damage_close` and `attack_strength_test_state` helpers and
`empty_hand_attack_strength_scales_partial_and_full_damage`, includes the
`run_pvp_commit_cost_case` helper, and ends with
`concurrent_lethal_attacks_create_one_drop_and_one_xp_reward`. The following
`furnace_like_recipe_lookup_uses_matching_cooking_category` test remains
aggregate-owned. The child module uses explicit imports and does not inherit
the aggregate file's `use super::*`. The existing
`older_victim_publication_preserves_newer_attacker_costs` test also uses
`attack_strength_test_state`, so that helper has the narrow `pub(super)`
visibility needed by one explicit aggregate import.

The extracted tests retain attack-strength scaling across held weapons,
playable-mode damage policy, hurt-resistance preview and commit behavior,
creative/spectator rejection, authoritative exhaustion and durability costs,
reach and immunity handling, and concurrent lethal-drop/XP conservation.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 8,402 physical lines and 173
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 7,593 | 158 |
| `play/tests/attack_pvp.rs` | 840 | 15 |

The moved body contains 811 physical lines, three helpers, and fifteen tests.
The final structural comparison covers all 238 function names from the
original aggregate module. The original-versus-extracted body diff is empty
after normalizing the one required `pub(super)` visibility qualifier, and the
sorted function-name multiset is unchanged. The combined test count remains
173.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 15 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Original-versus-extracted 811-line body diff after normalizing the one
  required helper visibility qualifier: empty.
- Original-versus-split function-name multiset: all 238 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that
  `furnace_like_recipe_lookup_uses_matching_cooking_category` remains
  aggregate-owned.
- Independent read-only review found two documentation-only inconsistencies:
  the aggregate line count omitted the new module/import lines and the
  focused/package results were absent. Both are corrected here; the reviewer
  found no code, behavior, scope, or conservation issue.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
