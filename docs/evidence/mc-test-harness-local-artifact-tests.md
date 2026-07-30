# `mc-test-harness` local-artifact test classification

Scope: Phase 1 inventory of nonignored `mc-test-harness` integration tests
whose result depended on locally extracted Mojang sidecars or an external
vanilla oracle.

Ninety-nine ordinary tests reached 91 prerequisite branches that printed a
message and returned successfully. Several shared fixture helpers made one
missing sidecar silently bypass multiple tests. The same Cargo result could
therefore mean either “the wire or persistence assertions passed” or “the
fixture never started.”

These tests are now explicit opt-in gates. Ordinary Cargo runs report them as
ignored. A deliberately selected gate panics at its existing exact prerequisite
check instead of returning a false-green result.

## Inventory summary

| Target | Newly explicit gates | Owner and exact close condition |
| --- | ---: | --- |
| `block_edit` | 68 | `mc-test-harness::block_edit` and the named gameplay owner. Close a selected gate only after the local 26.1.2 blocks and registries sidecars load and every existing wire, authority, inventory, relight, or persistence assertion passes. |
| `chunk_stream` | 2 | `mc-test-harness::chunk_stream`. Close against the local 26.1.2 blocks report with exact view-subscription, reconnect, ownership, and publication assertions passing. |
| `incremental_relight` | 1 | `mc-test-harness::incremental_relight`. Close against the local blocks and block-light reports with the incremental/full-recompute comparison passing. |
| `mob_presence` | 5 | `mc-test-harness::mob_presence` with `mc-entity` and session publication owners. Close against the local blocks and registries sidecars with every existing visibility, combat, drop, and shield assertion passing. |
| `persistence_inventory` | 2 | `mc-test-harness::persistence_inventory`. Close against the declared local data and light sidecars with both disk reopen compositions passing. |
| `physics_validation` | 16 | `mc-test-harness::physics_validation` with `mc-physics` and block-authority owners. Fifteen gates close against local blocks/registries sidecars. The external sugar-cane comparison closes only with `M43_VANILLA_ADDR` pointing at the prepared vanilla oracle and its observed result matching. |
| `player_presence` | 2 | `mc-test-harness::player_presence`. Close against the local blocks report with the two-client spawn, movement, disconnect, and reconnect visibility assertions passing. |
| `worldgen` | 3 | `mc-test-harness::worldgen`. The empty-world gate closes against the local blocks report; both generated-village gates additionally require the declared structure and worldgen sidecars. All existing generation, wire, and restart assertions must pass. |

Total: 99 newly explicit local-artifact gates. Together with the 27 previously
classified ignores, `mc-test-harness` now exposes 126 ignored tests.

## Exact source inventory

- `block_edit/block_breaking.rs` (5): `break_block_round_trips_update_ack_relight`, `break_block_broadcasts_update_to_second_subscriber`, `early_survival_stop_completes_after_server_progress_reaches_one`, `out_of_reach_survival_and_creative_breaks_are_ack_only`, `far_out_of_reach_survival_break_does_not_load_target_before_ack`
- `block_edit/campfire.rs` (6): `survival_campfire_cooks_held_input_into_item_entity`, `survival_unlit_campfire_does_not_finish_cooking`, `survival_campfire_in_flight_state_flushes_to_disk`, `survival_campfire_in_flight_state_resumes_after_reopen`, `survival_campfire_finishes_while_no_clients_are_connected`, `survival_campfire_finishes_after_restart_before_any_client_reconnects`
- `block_edit/cauldron.rs` (2): `survival_water_bucket_fills_and_drains_cauldron_with_persistence`, `cauldron_states_survive_disk_flush_and_reopen`
- `block_edit/chests_and_hoppers.rs` (5): `two_clients_stale_chest_click_after_peer_update_resyncs`, `server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp`, `chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots`, `chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack`, `unsupported_chest_click_modes_resync_without_trusting_client_slots`
- `block_edit/crafting_table.rs` (1): `crafting_table_container_crafts_shapeless_and_shaped_results`
- `block_edit/crop_bonemeal.rs` (1): `bonemeal_growth_debits_only_successful_survival_use`
- `block_edit/crop_harvest.rs` (1): `survival_break_mature_common_crops_drops_deterministic_items`
- `block_edit/enchanting.rs` (1): `survival_enchanting_table_applies_high_efficiency_sharpness_and_protection`
- `block_edit/fluid_scheduling.rs` (3): `water_bucket_spread_waits_for_scheduled_fluid_delay`, `lava_bucket_next_to_water_solidifies_through_scheduled_fluid_tick`, `water_bucket_scheduled_spread_survives_save_restart_without_duplicate_tick`
- `block_edit/furnaces.rs` (4): `two_clients_stale_furnace_click_after_peer_update_resyncs`, `malformed_furnace_clicks_resync_without_trusting_client_slots`, `survival_furnace_container_smelts_input_with_fuel`, `survival_specialized_furnaces_open_vanilla_menu_types`
- `block_edit/inventory_clicks.rs` (2): `survival_container_click_moves_stack_through_server_cursor`, `malformed_inventory_click_resyncs_and_next_valid_click_succeeds`
- `block_edit/inventory_crafting.rs` (2): `place_recipe_crafts_torch_from_authoritative_inventory`, `place_recipe_crafts_tag_based_planks_sticks_and_table`
- `block_edit/placement_rejection.rs` (7): `rejected_occupied_use_item_on_resyncs_clicked_and_target_before_ack`, `rejected_occupied_bucket_use_item_on_resyncs_blocks_and_held_slot_before_ack`, `rejected_world_border_use_item_on_resyncs_without_placing`, `far_world_border_use_item_on_does_not_load_blocks_or_resync_bucket`, `rejected_out_of_reach_use_item_on_resyncs_clicked_and_target_before_ack`, `rejected_out_of_reach_bucket_use_item_on_resyncs_blocks_without_held_slot_before_ack`, `rejected_wall_torch_on_fence_resyncs_before_ack_without_debit`
- `block_edit/plant_harvest.rs` (1): `survival_harvests_sweet_berry_bush_into_inventory`
- `block_edit/plant_lifecycle.rs` (1): `survival_plant_lifecycle_covers_stems_cocoa_and_harvest`
- `block_edit/sapling_growth.rs` (6): `survival_bonemeal_grows_oak_sapling_into_tree`, `survival_bonemeal_stage_one_oak_replaces_existing_canopy_leaf`, `survival_bonemeal_does_not_consume_on_single_dark_oak`, `survival_bonemeal_emits_client_visible_atomic_batch_for_two_by_two_spruce_and_jungle`, `survival_bonemeal_rejects_obstructed_two_by_two_spruce_and_jungle_without_client_visible_partial_batch`, `survival_bonemeal_rejects_unloaded_two_by_two_spruce_and_jungle_canopy_without_client_visible_partial_batch`
- `block_edit/sign_edit.rs` (2): `survival_places_sign_and_updates_plain_text`, `survival_sign_text_survives_flush_and_reopen`
- `block_edit/stations_and_placement.rs` (3): `station_noop_and_creative_placement_preserve_inventory`, `placing_torch_on_a_wall_publishes_exact_state_then_ack_then_one_debit`, `adjacent_stair_place_remove_recomputes_neighbor_on_wire_and_survives_restart`
- `block_edit/survival_inventory.rs` (5): `survival_break_drops_item_entity_and_picks_it_up`, `survival_can_place_naturally_picked_up_block`, `invalid_carried_item_slot_does_not_change_survival_placement_slot`, `survival_break_damages_held_tool`, `survival_hoe_use_tills_dirt_and_damages_tool`
- `block_edit/survival_lifecycle.rs` (7): `survival_double_chest_opens_combined_storage_and_mutates_second_half`, `survival_generic_damage_bypasses_armor_and_durability`, `survival_use_item_eats_apple_and_updates_food`, `survival_use_item_release_cancels_food_use`, `survival_bow_release_spawns_and_moves_arrow`, `dead_survival_player_cannot_mine_or_eat`, `dead_survival_player_can_respawn_and_act_again`
- `block_edit/toggle_blocks.rs` (1): `survival_hand_use_toggles_wood_and_copper_but_not_iron_doors_and_trapdoors`
- `block_edit/vertical_plant_growth.rs` (1): `survival_random_tick_grows_visible_vertical_plant_columns`
- `block_edit/wheat_harvest.rs` (1): `survival_break_mature_wheat_drops_wheat_and_seeds`
- `chunk_stream.rs` (2): `movement_across_chunk_boundary_replans_view_subscription`, `reconnect_during_chunk_prepare_receives_only_the_new_exact_view`
- `incremental_relight.rs` (1): `incremental_relight_wire_matches_full_recompute`
- `mob_presence.rs` (5): `vanilla_client_receives_server_owned_passive_mob_and_motion`, `two_clients_receive_same_server_owned_mob`, `survival_attack_passive_mob_uses_all_configured_drops`, `survival_zombie_damages_player_and_drops_rotten_flesh`, `survival_shield_blocks_frontal_zombie_damage`
- `persistence_inventory.rs` (2): `place_dirt_persists_through_flush_to_disk`, `item_despawn_deadline_survives_restart`
- `physics_validation.rs` (16): `deterministic_physics_fixture_materializes_named_shapes`, `physics_fixture_server_reaches_play_and_streams_spawn_chunk`, `shallow_water_entry_keeps_self_motion_client_predicted`, `deep_water_swim_and_exit_keep_self_motion_client_predicted`, `flat_ground_move_does_not_emit_position_correction`, `wall_collision_corrects_player_to_last_accepted_position`, `full_block_non_step_attempt_corrects_player`, `landing_fall_damage_uses_accumulated_descent`, `water_entry_suppresses_fall_damage`, `sugar_cane_support_break_emits_real_block_edit_observation`, `survival_sugar_cane_support_break_drops_cascaded_cane`, `falling_blocks_start_when_support_breaks`, `stacked_falling_blocks_all_start_when_support_breaks`, `falling_block_lands_as_block_and_despawns_entity`, `cactus_dirt_side_neighbor_placement_cascades_visible_column_removal`, `external_vanilla_sugar_cane_support_break_oracle`
- `player_presence.rs` (2): `two_clients_spawn_move_and_despawn_visible_players`, `disconnect_reconnect_replaces_player_visibility_cleanly`
- `worldgen.rs` (3): `empty_world_plus_generator_produces_terrain_on_demand`, `generated_village_villager_wire_and_restart_are_stable`, `generated_village_food_share_birth_wire_and_restart_are_stable`

The crate retains always-executable unit and integration coverage for replay
validation, protocol codecs, command routing, plugin/Lua adapters, real-client
manifest validation, and synthetic authority/state-machine boundaries. The
opt-in gates extend those fences through local 26.1.2 data and full TCP/disk
compositions; they do not replace the checked-in coverage.

## Current disposition

The focused ordinary package suite is green. Its affected targets report:
`block_edit` 35 passed/69 ignored, `chunk_stream` 0/3,
`incremental_relight` 0/1, `mob_presence` 1/5,
`persistence_inventory` 0/2, `physics_validation` 0/16,
`player_presence` 0/2, and `worldgen` 3/3. The complete ignored inventory
command lists exactly 126 tests.

No ignored local-sidecar or external-vanilla workload was executed in this
checkpoint. Missing declared prerequisites are now failures when an operator
selects one of these gates.

`benchmark: not applicable`: this checkpoint changes test classification and
prerequisite visibility only. It changes no measured runtime path and makes no
performance claim.

## Reproduction

Run the ordinary package suite:

```sh
cargo test -p mc-test-harness
```

List the complete current ignored inventory without executing it:

```sh
cargo test -p mc-test-harness -- --list --ignored
```

After preparing the exact local artifacts, select only the relevant gate, for
example:

```sh
cargo test -p mc-test-harness --test incremental_relight \
  incremental_relight_wire_matches_full_recompute \
  -- --exact --include-ignored --nocapture
```

Do not run all 126 ignored tests as an undifferentiated batch: that would mix
ordinary sidecar compositions with parity, soak, load, and profiling workloads.
