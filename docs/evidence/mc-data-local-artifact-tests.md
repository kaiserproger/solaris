# `mc-data` local-artifact test classification

Scope: Phase 1 inventory of `mc-data` tests that require ignored local Mojang
reports or data-pack sidecars.

Twenty-five ordinary tests previously returned successfully when a local
prerequisite was absent. The same Cargo result could therefore mean either
“assertions passed” or “the local oracle was not exercised.”

The tests are now explicit opt-in gates. Ordinary Cargo runs report them as
ignored, and an explicit ignored-test invocation fails immediately when its
declared prerequisite is missing.

## Inventory

| Boundary | Tests | Count | Owner and exact close condition |
| --- | --- | ---: | --- |
| Biome spawns, variants, and tags | `biomes::tests::{loads_real_plains_spawns_when_present, real_sidecar_sheep_color_climates_match_resolved_variant_tags_when_present, loads_real_overworld_biome_tags_when_present}` | 3 | `mc-data::biomes`. Close only when each selected gate passes against locally extracted 26.1.2 biome and biome-tag directories. |
| Block registry and derived state facts | `blocks::tests::loads_real_blocks_report_if_present`, `block_facts::tests::{loads_real_random_tick_families_when_present, loads_real_fluid_facts_when_present}`, `block_explosion::tests::real_2612_sidecar_has_expected_cardinal_resistances`, `block_light::tests::real_table_matches_known_blocks`, `block_mining::tests::real_table_matches_vanilla_2612_blocks` | 6 | The named `mc-data` block modules. Close against the exact locally extracted 26.1.2 `blocks.json` plus the explosion, light, or mining report required by the selected gate. |
| Damage types | `damage_types::tests::loads_real_damage_types_when_present` | 1 | `mc-data::damage_types`. Close when the selected gate passes against the local 26.1.2 damage-type directory. |
| Fuel values from item tags | `fuel_values::tests::full_local_2612_tags_match_embedded_snapshot_when_available` | 1 | `mc-data::fuel_values` with the registry and tag loaders. Close when the local registries report and item tags resolve to the exact embedded 280-item snapshot. |
| Item registry and components | `items::tests::loads_real_item_registry_when_present`, `item_components::tests::{loads_real_apple_and_bread_components_when_present, loads_real_combat_components_when_present, loads_real_ordered_tool_rules_when_present}` | 4 | `mc-data::items` and `mc-data::item_components`. Close against the local 26.1.2 registries and item-component reports with all existing cardinal assertions passing. |
| Aggregate vanilla data loader | `tests::loads_real_vanilla_sidecar_when_present` | 1 | The `mc-data` root loader. Close when the populated local 26.1.2 `data/minecraft` tree loads and satisfies the registry assertions. |
| Block, crop, and sheep loot | `loot::tests::{loads_real_vanilla_subset_when_present, real_binomial_fixture_has_expected_parameters_when_present, real_crop_fixtures_load_state_conditioned_pools_when_present, completes_real_vanilla_sheep_table_when_present}` | 4 | `mc-data::loot`. Close against the local 26.1.2 loot tree; crop gates require all four declared crop files, and the sheep gate requires the real base sheep table before fallback completion is checked. |
| Entity-loot reference closure | `loot::entity_26_1_2::tests::local_26_1_2_entity_corpus_has_closed_references_when_sidecar_is_present` | 1 | `mc-data::loot::entity_26_1_2`. Close when one of the declared local 26.1.2 data roots contains the complete entity-loot corpus and the selected gate passes its exact root, table-type, and reference counts. |
| Recipes | `recipes::tests::loads_real_recipe_sidecar_when_present` | 1 | `mc-data::recipes`. Close when the local 26.1.2 recipe directory loads and satisfies the existing count and stick-recipe assertions. |
| World-generation facts and inventory | `worldgen_features::tests::loads_real_grass_feature_facts_when_present`, `worldgen_inventory::tests::inventories_real_worldgen_sidecar_when_present`, `worldgen_ores::tests::loads_real_diamond_ore_when_present` | 3 | The named `mc-data` worldgen modules. Close when each selected gate passes against the locally extracted 26.1.2 worldgen directory. |

Total: 25 explicit local-artifact gates.

The same modules retain always-executable checked-in coverage for synthetic
loaders and fixtures, embedded gameplay facts, schema and range rejection,
fallback behavior, and exact registry contracts. The opt-in tests extend those
fences to local Mojang artifacts; they do not replace them.

## Current disposition

Before this classification, the `mc-data` unit target reported 223 passing
tests even when these local assertions did not run. The focused post-change
suite reports `198 passed; 0 failed; 25 ignored`; its integration target also
reports `11 passed; 0 failed`.

No ignored local-oracle workload was executed in this checkpoint. Missing
declared local sidecar artifacts are now failures when an operator explicitly
selects one of these tests.

`benchmark: not applicable`: the checkpoint changes test classification only,
not a runtime path, and makes no performance claim.

## Reproduction

Run the always-executable suite:

```sh
cargo test -p mc-data
```

After running `tools/extract-vanilla-data.sh`, select only the relevant opt-in
gate, for example:

```sh
cargo test -p mc-data \
  block_light::tests::real_table_matches_known_blocks \
  -- --ignored --exact --nocapture
```
