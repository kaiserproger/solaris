//! Play state handler.
//!
//! Sends the vanilla-client Play transition burst, streams chunks,
//! tracks player/world interaction, and runs the keepalive loop until
//! the client disconnects or the peer-side keepalive timeout fires.
//!
//! ```text
//! S → C  Login (Play)
//! S → C  Synchronize Player Position
//! S → C  Set Default Spawn Position
//! S → C  Game Event (start_waiting_for_level_chunks)
//! S → C  Level Chunk With Light / Light Update / entity + inventory state
//! S → C  Keep Alive   (every 15 s; client must echo within 30 s)
//! ```

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes, BytesMut};
use mc_data::block_facts::FluidKind;
use mc_data::block_light::BlockLightTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_data::{Registry, VanillaData};
use mc_entity::{
    AttributeKind, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, GoalState,
    PathingBudget, PathingProbe, PathingProbeResult, RegionKey, Rotation, SpawnEntity, Vec3,
};
use mc_extension::{DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES, InboundEvent, PlayerId, ProtocolPhase};
#[cfg(test)]
use mc_nbt::ListTag;
use mc_nbt::Tag;
use mc_protocol::codec::{DEFAULT_MAX_STRING_LEN, Identifier, ReadMc};
use mc_protocol::frame::{Compression, encode_frame};
use mc_protocol::packets::login::GameProfileProperty;
use mc_protocol::packets::play::{
    AGEABLE_ENTITY_DATA_BABY_INDEX, AddEntity, BlockChangedAck, BlockEntityInfo, BlockUpdate,
    ChunkHeightmap, ClientboundBlockEntityData, ClientboundChangeDifficulty,
    ClientboundCommandSuggestions, ClientboundContainerClose, ClientboundContainerSetContent,
    ClientboundContainerSetData, ClientboundContainerSetSlot, ClientboundCooldown,
    ClientboundCustomPayload, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundMerchantOffers, ClientboundOpenScreen, ClientboundRecipeBookSettings,
    ClientboundRespawn, ClientboundSetEntityData, ClientboundSetExperience, ClientboundSetHealth,
    ClientboundSetHeldSlot, ClientboundSystemChat, ClientboundTakeItemEntity, ConfirmTeleportation,
    ContainerInput, Direction, ENTITY_DATA_POSE_INDEX, ENTITY_DATA_SHARED_FLAGS_INDEX,
    EntityAnimation, EntityAnimationAction, EntityDataValue, EntityEvent, EntityPose,
    EntityPositionSync, EntityVec3, ForgetLevelChunk, GameEvent, GameMode, HashedStack,
    ITEM_ENTITY_DATA_ITEM_INDEX, InteractionHand, ItemStack, LIVING_ENTITY_DATA_FLAGS_INDEX,
    LevelChunkWithLight, LevelEvent, LightData, LightUpdate, LoginPlay, MoveEntityPosRot,
    MovePlayerFlags, PlayDisconnect, PlayerActionKind, PlayerCommandAction, PlayerInfoActions,
    PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate, PlayerInput, PositionMoveRotation,
    RemoveEntities, RotateHead, SHEEP_ENTITY_DATA_WOOL_INDEX, SectionBlockChange,
    SectionBlocksUpdate, ServerboundAttack, ServerboundChangeGameMode, ServerboundChat,
    ServerboundChatAck, ServerboundChatCommand, ServerboundChunkBatchReceived,
    ServerboundClientCommand, ServerboundClientInformation, ServerboundClientTickEnd,
    ServerboundCommandSuggestion, ServerboundContainerButtonClick, ServerboundContainerClick,
    ServerboundContainerClose, ServerboundCustomPayload, ServerboundInteract, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerPosRot, ServerboundMovePlayerRot,
    ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe, ServerboundPlayerAction,
    ServerboundPlayerCommand, ServerboundPlayerInput, ServerboundPlayerLoaded,
    ServerboundRecipeBookChangeSettings, ServerboundRecipeBookSeenRecipe, ServerboundResourcePack,
    ServerboundSelectTrade, ServerboundSetCarriedItem, ServerboundSignUpdate, ServerboundSwing,
    ServerboundUseItem, ServerboundUseItemOn, SetCenterChunk, SetDefaultSpawnPosition,
    SetEntityMotion, SynchronizePlayerPosition, pack_section_pos, pack_section_relative_pos,
    unpack_block_pos,
};
use mc_protocol::packets::{CustomPayload, Packet};
use mc_script::{
    ScriptCraftingSource, ScriptEvent, ScriptInteractionHand, ScriptItemPickupSource,
    ScriptPlayerContext, ScriptPlayerId,
};
#[cfg(test)]
use mc_world::FurnaceSlot;
use mc_world::light::{ChunkLight, LightCache, LightWorkspace};
use mc_world::wire::{client_heightmaps, encode_chunk_data, encode_chunk_light};
use mc_world::{
    BlockRegistry, BlockStateId, ChestBlockEntity, Chunk, ChunkPos, FurnaceBlockEntity,
    SECTION_DIM, ScheduledBlockTick, ScheduledFluidTick,
};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use crate::chunk_pipeline::ChunkPipelineResources;
use crate::configuration::ConfigurationCustomPayload;
use crate::connection::{read_frame, write_packet};
use crate::error::ConnectionError;
use crate::loader::loader_interaction_channel;
use crate::login::LoggedInProfile;
use crate::script::PluginZoneAdapter;
use crate::server::{ExtensionEventSink, ScriptEventSink, ServerConfig, WorldHandle};
use crate::{
    ChunkPipelinePolicy, ChunkPipelineStopReason, ChunkPriority, ChunkRequest, ChunkScheduler,
    RuntimeControlHandle,
};

mod beds;
mod block_break;
#[cfg(test)]
mod block_break_tests;
mod block_edit_commit;
mod block_placement;
mod block_wire;
mod bucket_interactions;
mod campfire;
mod campfire_adapter;
mod chunk_stream;
mod client_load;
#[cfg(test)]
mod client_load_tests;
mod combat;
mod command_execution;
pub(crate) mod commands;
mod containers;
mod explosions;
mod falling_blocks;
mod fluids;
mod inhabited_time;
#[cfg(test)]
mod inhabited_time_tests;
mod inventory;
mod item_blocks;
mod lighting;
mod merchant_adapter;
mod movement;
#[cfg(test)]
mod movement_tests;
pub(crate) mod persistence;
mod plants;
mod player_breathing;
#[cfg(test)]
mod player_breathing_tests;
mod player_damage_adapter;
mod player_teleport;
#[cfg(test)]
mod player_teleport_tests;
mod random_ticks;
mod recipes;
mod scheduled_blocks;
mod script_gameplay_events;
#[cfg(test)]
mod script_gameplay_events_tests;

use client_load::ClientLoadGate;
use merchant_adapter::{handle_select_trade, open_merchant_container};
use player_breathing::{PlayerBreathingState, player_can_drown};
// Router and storage-owner wiring land separately; keep the bounded adapter
// contract available without creating a second ingress path here.
#[allow(dead_code)]
mod script_inventory_transaction;
pub(crate) use script_inventory_transaction::{
    ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
#[cfg(test)]
mod script_inventory_transaction_tests;
mod session;
mod simulation;
pub(crate) use simulation::SimulationWorldAccess;
mod spawn;
mod survival;
mod toggles;
mod use_item_on_adapter;
mod wire_entities;
#[cfg(test)]
mod wire_entities_tests;
pub(crate) mod world_journal;

#[cfg(test)]
use campfire::{
    CAMPFIRE_COOKING_SLOT_COUNT, CAMPFIRE_NBT_COOKING_TIMES, CAMPFIRE_NBT_COOKING_TOTAL_TIMES,
    LEGACY_CAMPFIRE_NBT_REMAINING, LEGACY_CAMPFIRE_NBT_TOTAL,
    campfire_block_entity_persistent_bytes, campfire_block_entity_persistent_nbt,
    campfire_block_entity_update_nbt, campfire_cooking_state_from_persistent_nbt,
    campfire_cooking_state_from_persistent_nbt_strict, campfire_output_uuid, compound_field,
    compound_int_array_field, pending_campfire_outputs_from_nbt,
};
pub(in crate::play) use campfire::{
    CampfireCookingState, PendingCampfireOutput, is_campfire_block,
};
#[cfg(test)]
use campfire_adapter::handle_campfire_use_on;
#[cfg(test)]
pub(crate) use campfire_adapter::hydrate_persisted_campfire_cooking;
pub(in crate::play) use campfire_adapter::{
    CAMPFIRE_BLOCK_ENTITY_TYPE_ID, CommittedCampfireCookingTick,
    dispatch_campfire_block_entity_update, run_campfire_cooking_ticks_owned,
};
pub(crate) use campfire_adapter::{
    CampfireCookingTickReport, hydrate_persisted_campfire_cooking_strict,
    recover_pending_campfire_outputs,
};
pub(crate) use chunk_stream::{
    passive_entity_passable_blocks, passive_herd_fallback_surface_blocks,
};
use combat::{
    ActiveShield, PlayerDamageKind, ShieldUseState, begin_player_attack_attempt,
    damage_active_shield_slots, damage_held_weapon_stack, player_horizontal_look_direction,
    shield_blocks_damage, shield_hand_slot, shield_use_flags, shield_use_from_stack,
    shield_use_matches, stack_is_shield, weapon_attacks_damage_held_item,
};
#[cfg(test)]
use combat::{
    PlayerDamageRequest, PlayerHurtResistance, PlayerHurtResolution, SHIELD_ACTIVATION_DELAY_TICKS,
    SHIELD_FALLBACK_MAX_DAMAGE, attack_damage_for_item, held_attack_damage,
    held_attack_damage_at_tick, held_attack_speed, melee_knockback, shield_block_knockback,
    shield_durability_damage,
};
pub(crate) use falling_blocks::LandedFallingBlock;
use falling_blocks::{
    FallingBlockLandingPlan, falling_block_landing_chunks, falling_block_start_chunks,
    is_falling_block_state, plan_falling_block_landings, plan_falling_block_starts,
};
pub(crate) use inhabited_time::InhabitedTimeAccumulator;
#[cfg(test)]
use mc_protocol::packets::play::{LIVING_ENTITY_FLAG_OFF_HAND, LIVING_ENTITY_FLAG_USING_ITEM};
pub(crate) use session::SessionRegistry;
#[cfg(test)]
pub(in crate::play) use session::{ENTITY_PICKUP_RADIUS, ITEM_PICKUP_DELAY_TICKS};

pub(crate) fn prewarm_entity_pathing_tables() -> std::num::NonZeroUsize {
    std::num::NonZeroUsize::new(session::prewarm_canonical_pathing_state_facts())
        .expect("canonical pathing table must contain collision-backed states")
}

#[cfg(test)]
pub(crate) use simulation::simulation_channel;
use simulation::{
    ActiveShieldTransition, AnimalFeedPlan, AnimalFeedTargets, AuthoritativePlayerStateSnapshot,
    BowReleasePlan, CommittedPlayerPose, FoodUsePlan, MerchantTradeDestination, MerchantTradePlan,
    PlayerSurvivalCommitOutcome, PlayerSurvivalPlan, SelectedItemDropPlan, SheepShearPlan,
};
pub use simulation::{EntityEffectHandle, EntityEffectRequestError};

/// One accepted attack as observed by the simulation authority.
///
/// Observations are emitted in `authority_sequence` order. The subscription is
/// bounded telemetry: slow receivers get `RecvError::Lagged` rather than a
/// silent gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAttackObservation {
    pub attacker_session_id: u64,
    pub target_entity_id: i32,
    pub cooldown_tick: u64,
    pub authority_tick: u64,
    pub authority_sequence: u64,
}
pub(crate) use simulation::{
    SIMULATION_COMMAND_BATCH_LIMIT, SimulationHandle, SimulationOwner, SimulationSaveSnapshot,
    SimulationTickReport, simulation_channel_with_explosion_seed,
};
pub(crate) use spawn::prepare_spawn_chunk;

#[cfg(test)]
use beds::{bed_respawn_pose, canonical_bed_position, next_morning_time};
use beds::{
    bed_sleep_is_blocked_by_monster, bed_sleep_is_obstructed, plan_bed_occupied_edits,
    plan_loaded_bed_interaction, safe_bed_wake_pose,
};
use block_break::{
    PendingBreak, block_break_loot_seed, break_replacement_state_in_storage,
    handle_block_destroy_action, plan_break_block_edits, plan_break_edit_preconditions,
    plan_survival_break_drops, tick_delayed_break,
};
#[cfg(test)]
use block_edit_commit::{
    apply_block_edit_batch_to_storage_conditionally,
    apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally,
    send_loaded_block_edit_resyncs,
};
use block_edit_commit::{
    apply_block_edit_to_storage, apply_player_block_edit_batch_conditionally,
    apply_visible_block_edit_batch_conditionally,
};
#[cfg(test)]
use block_placement::{
    append_cactus_side_neighbor_cascades, door_half_state, horizontal_facing_from_yaw,
    placed_sign_edit, sign_block_entity_update_nbt, sign_placement_state,
};
use block_wire::{
    BlockDelta, broadcast_block_deltas_to_sessions, broadcast_light_updates_to_sessions,
    send_block_deltas, send_light_updates,
};
#[cfg(test)]
use block_wire::{BlockDeltaPacket, plan_block_delta_packets};
#[cfg(test)]
use bucket_interactions::plan_bucket_replacement;
#[cfg(test)]
use chunk_stream::{
    ChunkBuildTiming, ChunkWriteTiming, passable_block_name, plan_passive_herd, spiral_chunks,
};
use chunk_stream::{
    ChunkStreamState, ChunkStreamStep, PreparedChunkFrame, desired_chunk_set, herd_uuid,
};
#[cfg(test)]
use command_execution::{apply_debug_command, runtime_control_status_message};
use command_execution::{
    apply_game_mode, clientbound_session_world_time, clientbound_world_time, command_error_message,
    execute_player_command, handle_client_command, prepare_game_mode_transition,
    send_command_feedback, send_world_time,
};
#[cfg(test)]
use commands::{
    AdminCommand, DebugCommand, SurvivalCommand, command_suggestions, command_tree_packet,
    parse_admin_command,
};
use commands::{
    CommandError, CommandPermissions, command_suggestions_with_plugin_roots,
    command_tree_packet_with_plugin_roots, player_abilities_for_mode,
};
#[cfg(test)]
use commands::{parse_debug_command, parse_gamemode_command};
use containers::{
    ActiveContainer, CRAFTING_MENU_TYPE_ID, ChestClickAction, ChestClickInput, ChestView,
    ChestWindow, CraftingTableWindow, ENCHANTING_MENU_SLOT_COUNT, ENCHANTING_MENU_TYPE_ID,
    EnchantingTableWindow, FURNACE_MENU_SLOT_COUNT, FurnaceClickAction, FurnaceClickInput,
    FurnaceKind, FurnaceWindow, MERCHANT_MENU_TYPE_ID, MerchantWindow, QuickCraftClick,
    QuickCraftOutcome, QuickCraftState, STONECUTTER_MENU_TYPE_ID, ScriptMenuClick,
    ScriptMenuClickDisposition, ScriptMenuOpenError, ScriptMenuWindow, StonecutterClickAction,
    StonecutterClickInput, StonecutterWindow, adjacent_chest_positions,
    can_place_in_enchanting_menu_slot as can_place_in_enchanting_menu_slot_with_data,
    chest_menu_state_change_count, chest_menu_title_nbt, chest_slot_stacks, chest_wire_items,
    client_close_matches, count_valid_enchanting_bookshelves, crafting_menu_title_nbt,
    crafting_table_input_from_projection, crafting_table_input_projection, crafting_wire_items,
    enchant_item_candidate, enchanting_data_values, enchanting_menu_stack,
    enchanting_menu_title_nbt, enchanting_offer, enchanting_player_slot,
    enchanting_table_input_from_projection, enchanting_table_input_projection,
    enchanting_wire_items, furnace_data_values, furnace_experience_seed, furnace_kind_for_block_id,
    furnace_kind_for_state, furnace_menu_title_for_state, furnace_menu_title_nbt,
    furnace_output_was_taken, furnace_slot_to_stack, is_barrel_state, is_chest_state,
    is_crafting_table_state, is_enchanting_table_state, is_furnace_state, is_lapis_stack,
    is_stonecutter_state, next_container_id, plan_chest_click, plan_click as plan_furnace_click,
    plan_stonecutter_click, refresh_crafting_result as refresh_crafting_result_with_data,
    select_stonecutter_recipe as select_stonecutter_recipe_with_data, set_enchanting_menu_stack,
    set_stonecutter_input as set_stonecutter_input_with_data, stonecutter_input_array,
    stonecutter_input_from_projection, stonecutter_input_projection, stonecutter_menu_title_nbt,
    stonecutter_wire_items, store_active_container, supported_enchantment_for_item,
    tick as tick_furnace_rules, unsupported_survival_station_for_state,
};
#[cfg(test)]
use containers::{
    BLAST_FURNACE_MENU_TYPE_ID, DOUBLE_CHEST_MENU_TYPE_ID, FURNACE_MENU_TYPE_ID,
    SINGLE_CHEST_STORAGE_SLOTS, SMOKER_MENU_TYPE_ID,
    apply_quick_move_click as chest_apply_quick_move_click,
    apply_swap_click as chest_apply_swap_click, apply_throw_click as chest_apply_throw_click,
    chest_player_slot, crafting_result_from_input, furnace_experience_award,
    inventory_crafting_input, item_is_efficiency_enchantable,
    refresh_inventory_crafting_result as refresh_inventory_crafting_result_with_data,
    repair_item_crafting_result, set_chest_menu_stack, stack_to_furnace_slot,
};
use containers::{
    merchant_input_from_projection, merchant_input_projection, merchant_menu_title_nbt,
    merchant_protocol_offers, merchant_wire_items, select_merchant_offer,
};
#[cfg(test)]
use fluids::{WATER_FLOW_DELAY_TICKS, fluid_tick_edits, supported_flow_state};
use fluids::{
    fluid_state_with_level, plan_fluid_ticks_near_applied, scheduled_fluid_planning_chunks,
};
#[cfg(test)]
use inventory::damage_equipped_armor;
#[cfg(test)]
use inventory::{
    ArmorStats, armor_reduced_damage, player_swap_slot, protection_reduced_damage, take_throw_stack,
};
use inventory::{
    PlayerInventory, apply_outside_pickup_click as apply_outside_pickup_click_with_carried,
    apply_regular_pickup_slot, apply_regular_swap_slot, apply_regular_throw_slot,
    can_place_in_player_slot, can_stack, hotbar_swap_slot, item_max_stack, pickup_click_max_stack,
    survival_damage_after_armor, survival_damage_after_protection,
};
use item_blocks::ItemToBlockTable;
#[cfg(test)]
use lighting::collect_incremental_light_updates_for_applied_edits;
use lighting::{
    IncrementalLightSources, capture_incremental_light_sources,
    collect_full_light_updates_for_current_world, compute_incremental_light_updates,
    incremental_light_sources_are_current, light_update_chunks, persist_baked_light_updates,
};
#[cfg(test)]
use movement::fall_damage_amount;
use movement::{
    AcceptedAbsoluteMovement, PendingTeleport, PlayerCollisionContext, TeleportConfirmResult,
    clamp_player_pose, confirm_pending_teleport, farmland_trample_pos,
    guard_pending_teleport_movement, movement_exhaustion, next_player_teleport_id,
    normalize_absolute_player_movement, player_pose_collides_with_solid_in_snapshot_with_context,
    player_water_overlap_in_snapshot, refresh_player_fall_state, validate_player_rotation,
};
#[cfg(test)]
use persistence::PersistedEntityRecord;
use persistence::{PersistedEntityCheckpoint, PlayerPersistedState, XpState, load_player_state};
#[cfg(test)]
use plants::{
    bonemeal_growth_edit, bonemeal_growth_edits, next_crop_growth_state, sweet_berry_harvest,
};
use player_damage_adapter::{
    PlayerDamageApplication, apply_contact_block_damage, apply_fall_damage, apply_player_damage,
    apply_player_damage_publication, player_melee_knockback,
};
use player_teleport::apply_script_player_teleport;
#[cfg(test)]
use random_ticks::{LeafDecayDropRolls, next_fire_state, next_leaf_decay_state, random_tick_edit};
use random_ticks::{
    leaf_decay_drop_rolls, natural_leaf_decay_drops, random_tick_candidate_seed,
    random_tick_edit_seeded, sample_random_tick_positions, section_may_random_tick,
};
#[cfg(test)]
use recipes::ingredient_accepts_item;
use recipes::{
    CraftedItem, craft_recipe, initial_recipe_book, initial_recipe_update, recipe_fits_grid,
};
use scheduled_blocks::{
    COMPARATOR_TICK_DELAY_TICKS, HOPPER_TICK_DELAY_TICKS, HopperTransferContext,
    HopperTransferUpdate, ScheduledBlockTickPlan, backfill_loaded_hopper_ticks,
    furnace_slot_stacks, plan_resident_hopper_transfer,
    plan_scheduled_block_tick_edits as plan_scheduled_block_tick_edits_with_blocks,
    resident_hopper_cooldown_plan, schedule_comparator_ticks_for_hopper_update,
    scheduled_block_planning_chunks, scheduled_block_tick_edits, scheduled_hopper_transfer,
};
#[cfg(test)]
use scheduled_blocks::{
    HOPPER_TRANSFER_DELAY_TICKS, HOPPER_TRANSFER_MAX_STACK, container_redstone_signal_at,
    insert_hopper_stack_into_campfire,
};
use script_gameplay_events::ScriptGameplayEventPublisher;
use session::{
    EntityAttackOutcome, OutboundCommand, OutboundLightUpdate, PlayerAttackResult,
    PlayerEntitySnapshot, ScriptMenuCloseRequest, ScriptMenuOpenRequest, ServerEntityMove,
    ServerEntitySnapshot, SessionAdmissionError, SessionId, SessionRegistration, SleepOutcome,
    VisibilityDispatch, apply_loader_item_grant, apply_script_player_inventory_transaction,
    dispatch_visibility_commands, publish_script_menu_click,
};
#[cfg(test)]
use session::{PlayerDamagePublication, PlayerInventorySlotDelta, within_block_reach};
#[cfg(test)]
use session::{entity_aabb, within_entity_reach};
#[cfg(test)]
use spawn::spawn_chunk_pos;
#[cfg(test)]
use spawn::spawn_y_from_chunk;
use spawn::{chunk_pos_from_coords, pack_block_pos, spawn_dimension, spawn_position};
use survival::{
    BlockMutationSnapshot, PendingUse, SurvivalHealthTick, SurvivalState, UseKind,
    arrow_entity_type_id, available_arrow_slot, block_break_is_denied, block_tag_contains,
    bow_draw_power, entity_item_stack, falling_block_entity_type_id, held_bow_max_damage,
    held_food_use, is_bow_item, is_hostile_entity, item_entity_type_id, item_use_ticks,
    mob_drop_stacks_from_seed, mob_xp_value, pending_use_is_complete, pending_use_matches,
    xp_orb_entity_type_id,
};
#[cfg(test)]
use survival::{
    block_drop_stacks_from, fallback_mining_time, fallback_tool_allows_block_drop,
    food_rule_for_item, is_durability_tool_path, max_tool_damage_for_path,
};
#[cfg(test)]
use toggles::plan_toggle_block_interaction;
#[cfg(test)]
use toggles::toggled_bool_state;
use toggles::{ToggleBlockPlan, plan_toggle_block_interaction_with_protection};
#[cfg(test)]
use use_item_on_adapter::{
    UseItemOnNoOpReason, UseItemOnOutcome, UseItemOnResyncOptions, classify_use_item_on_preflight,
    consume_bonemeal_after_growth, handle_block_item_placement, plan_hoe_tilling,
    plan_loaded_bonemeal_growth, plan_loaded_plant_harvest, plan_place_block_edits,
    reject_use_item_on_with_resync,
};
use use_item_on_adapter::{ack_use_item_noop, handle_sign_update, handle_use_item_on};
use wire_entities::{
    send_entity_data, send_entity_despawn, send_entity_health, send_entity_relative_move,
    send_entity_spawn, send_player_animation, send_player_despawn, send_player_move,
    send_player_spawn, send_take_item_entity,
};

#[cfg(test)]
fn refresh_crafting_result(state: &InteractionState, window: &mut CraftingTableWindow) {
    refresh_crafting_result_with_data(
        &state.items,
        &state.item_facts,
        &state.tags,
        &state.recipes,
        window,
    );
}

#[cfg(test)]
fn refresh_inventory_crafting_result(state: &mut InteractionState) {
    refresh_inventory_crafting_result_with_data(
        &state.items,
        &state.item_facts,
        &state.tags,
        &state.recipes,
        &mut state.inventory,
    );
}

#[cfg(test)]
fn apply_pickup_click(state: &mut InteractionState, slot: usize, button: i8) -> bool {
    state
        .inventory
        .apply_crafting_pickup_click(
            &state.items,
            &state.item_facts,
            &state.tags,
            &state.recipes,
            &mut state.carried_item,
            slot,
            button,
        )
        .0
}

#[cfg(test)]
fn apply_quick_move_click(state: &mut InteractionState, slot: usize) -> bool {
    state
        .inventory
        .apply_crafting_quick_move_click(
            &state.items,
            &state.item_facts,
            &state.tags,
            &state.recipes,
            slot,
        )
        .0
}

#[cfg(test)]
fn apply_crafting_pickup_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    slot: usize,
    button: i8,
) -> bool {
    window
        .apply_pickup_click(
            &state.items,
            &state.item_facts,
            &state.tags,
            &state.recipes,
            &mut state.inventory,
            &mut state.carried_item,
            slot,
            button,
        )
        .0
}

#[cfg(test)]
fn apply_outside_pickup_click(state: &mut InteractionState, button: i8) -> Option<ItemStack> {
    apply_outside_pickup_click_with_carried(&mut state.carried_item, button)
}

thread_local! {
    static CHUNK_LIGHT_WORKSPACE: RefCell<LightWorkspace> = RefCell::new(LightWorkspace::new());
}

/// How often we ping the client. Vanilla's value.
pub const KEEPALIVE_PERIOD: Duration = Duration::from_secs(15);
/// How long both a pending echo and all inbound traffic may remain idle.
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(not(test))]
const SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT: Duration = Duration::from_millis(10);
const SLOW_CLIENT_OUTBOUND_PRESSURE_NUMERATOR: usize = 3;
const SLOW_CLIENT_OUTBOUND_PRESSURE_DENOMINATOR: usize = 4;
const OUTBOUND_COMMANDS_PER_PLAYER_BURST: usize = 16;
const ENTITY_SPAWNS_PER_WRITE_TURN: usize = 16;
const ENTITY_MOVEMENTS_PER_WRITE_TURN: usize = 256;
const ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN: usize = 512;
const ENTITY_GOAL_UPDATES_PER_TICK: usize = 512;
const ENTITY_SIMULATION_UPDATES_PER_LANE_PER_TICK: usize = 256;
const TELEPORT_RESEND_DELAY_TICKS: u64 = 20;

const SPAWN_X: f64 = 0.5;
// The bundled test world uses vanilla's flat-preset surface: bedrock
// at Y=-64, dirt at Y=-63..-62, grass at Y=-61. Spawn one block
// above the grass so the client lands cleanly without freefall.
// (M3's old SPAWN_Y=64 worked only because the chunk burst was fast
// enough to land before the client picked up physics; M4's slower
// debug-mode burst exposed the latent bug.)
const DEFAULT_SPAWN_Y: f64 = -59.0;
const SPAWN_Z: f64 = 0.5;
const DEFAULT_SEA_LEVEL: i32 = 63;
const PLAYER_ENTITY_TYPE_ID: i32 = 155;
const SERVER_ENTITY_ID_START: i32 = 1_000_000;
pub(crate) const ENTITY_TICK_PERIOD: Duration = Duration::from_millis(50);
const DAY_LENGTH_TICKS: u64 = 24_000;
const NIGHT_START_TICK: u64 = 12_542;
const DAY_START_TICK: u64 = 0;
const SIGN_BLOCK_ENTITY_TYPE_ID: i32 = 7;

fn effective_block_entity_types(
    data: &VanillaData,
) -> mc_data::block_entity_types::BlockEntityTypeRegistry {
    if let Some(sidecar_root) = data.sidecar_root() {
        let registries_report = sidecar_root.join("reports").join("registries.json");
        if !registries_report.is_file() {
            warn!(
                path = %registries_report.display(),
                "block-entity type registry report missing; using embedded fallback",
            );
            return mc_data::block_entity_types::solaris_required_block_entity_types();
        }
        match mc_data::block_entity_types::load_block_entity_types_report(&registries_report) {
            Ok(report) => {
                return mc_data::block_entity_types::BlockEntityTypeRegistry::from_report(&report);
            }
            Err(err) => warn!(
                path = %registries_report.display(),
                error = %err,
                "block-entity type registry unavailable; using embedded fallback",
            ),
        }
    }
    mc_data::block_entity_types::solaris_required_block_entity_types()
}

pub(crate) fn configure_session_arrow_kill_rewards(
    sessions: &SessionRegistry,
    config: &ServerConfig,
) {
    sessions.configure_arrow_kill_rewards(
        item_entity_type_id(&config.entity_types),
        xp_orb_entity_type_id(&config.entity_types),
        arrow_entity_type_id(&config.entity_types),
        Arc::clone(&config.items),
        Arc::clone(&config.item_facts),
        Arc::clone(&config.loot),
    );
}

pub(crate) fn configure_session_player_combat(sessions: &SessionRegistry, config: &ServerConfig) {
    sessions.configure_player_combat(
        item_entity_type_id(&config.entity_types),
        xp_orb_entity_type_id(&config.entity_types),
        Arc::clone(&config.items),
        Arc::clone(&config.item_facts),
    );
}
// Keep vanilla's default cadence for ordinary entities. The natural-mob path
// opts its bounded population into every-tick publication separately.
const ENTITY_MOVE_SEND_INTERVAL_TICKS: u64 = 3;

fn ordinary_entity_is_due_for_movement_tracking(
    ordinal: usize,
    tick: u64,
    entity_count: usize,
) -> bool {
    if entity_count <= ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN {
        return true;
    }
    let turn = tick / ENTITY_MOVE_SEND_INTERVAL_TICKS;
    let start = (turn as usize * ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN) % entity_count;
    (ordinal + entity_count - start) % entity_count
        < ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN
}

fn bounded_entity_ids_due_for_tick(
    eligible_ids: &HashSet<EntityId>,
    tick: u64,
    limit: usize,
) -> HashSet<EntityId> {
    let limit = limit.max(1);
    if eligible_ids.len() <= limit {
        return eligible_ids.clone();
    }
    let mut ordered = eligible_ids.iter().copied().collect::<Vec<_>>();
    ordered.sort_unstable();
    let start = (tick as usize).wrapping_mul(limit) % ordered.len();
    (0..limit)
        .map(|offset| ordered[(start + offset) % ordered.len()])
        .collect()
}

fn entity_goal_ids_due_for_tick(
    eligible_ids: &HashSet<EntityId>,
    tick: u64,
    simulation_overloaded: bool,
) -> HashSet<EntityId> {
    if simulation_overloaded {
        bounded_entity_ids_due_for_tick(eligible_ids, tick, ENTITY_GOAL_UPDATES_PER_TICK)
    } else {
        eligible_ids.clone()
    }
}
const WORLD_TIME_SYNC_PERIOD: Duration = Duration::from_secs(1);
struct RegisteredSessionCleanup {
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    extension: Option<ExtensionEventSink>,
    extension_player_id: PlayerId,
    scripts: Option<ScriptEventSink>,
    script_zones: Option<PluginZoneAdapter>,
    active: bool,
}

impl RegisteredSessionCleanup {
    fn new(
        sessions: Arc<SessionRegistry>,
        session_id: SessionId,
        extension: Option<ExtensionEventSink>,
        scripts: Option<ScriptEventSink>,
        script_zones: Option<PluginZoneAdapter>,
    ) -> Self {
        Self {
            sessions,
            session_id,
            extension,
            extension_player_id: PlayerId::new(session_id),
            scripts,
            script_zones,
            active: true,
        }
    }

    fn unregister(mut self) {
        self.unregister_active();
    }

    fn unregister_active(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        dispatch_visibility_commands(
            self.sessions
                .unregister_preserving_player_state(self.session_id),
        );
        if let Some(extension) = self.extension.as_ref() {
            extension.enqueue_event(InboundEvent::PlayerLeft {
                player_id: self.extension_player_id,
                reason: "disconnected".to_owned(),
            });
        }
        if let Some(scripts) = self.scripts.as_ref() {
            scripts.enqueue_event(ScriptEvent::player_left(
                ScriptPlayerId::new(self.session_id),
                "disconnected",
            ));
        }
        if let Some(zones) = self.script_zones.as_ref()
            && let Err(error) = zones.forget_player(ScriptPlayerId::new(self.session_id))
        {
            debug!(
                ?error,
                session_id = self.session_id,
                "script zone cleanup rejected"
            );
        }
    }
}

impl Drop for RegisteredSessionCleanup {
    fn drop(&mut self) {
        self.unregister_active();
    }
}
const FURNACE_CONTAINER_ID_MIN: i32 = 1;
const FURNACE_CONTAINER_ID_MAX: i32 = 100;
const DEFAULT_FOOD_USE_DURATION: Duration = Duration::from_millis(1_600);
const HOSTILE_MELEE_RANGE: f64 = 1.8;
const HOSTILE_MELEE_VERTICAL_REACH: f64 = 2.25;
const HOSTILE_MELEE_PERIOD_TICKS: u64 = 20;
const CREEPER_FUSE_TICKS: u64 = 30;
const CREEPER_TRIGGER_RANGE: f64 = 3.0;
const CREEPER_CANCEL_RANGE: f64 = 7.0;
const CREEPER_EXPLOSION_POWER: f32 = 3.0;
const SKELETON_SHOT_PERIOD_TICKS: u64 = 40;
const SKELETON_SHOT_RANGE: f64 = 16.0;
const SKELETON_ARROW_SPEED: f64 = 1.6;
#[cfg(test)]
const HOSTILE_FOLLOW_SPEED: f64 = 1.25;
#[cfg(test)]
const PASSIVE_WANDER_SPEED: f64 = 0.8;
const MAX_PASSIVE_SPAWNS_PER_CHUNK: usize = 6;
const MAX_HOSTILE_SPAWNS_PER_CHUNK: usize = 3;
const MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER: f64 = 24.0;

fn world_time_is_night(world_time: u64) -> bool {
    world_time % DAY_LENGTH_TICKS >= NIGHT_START_TICK
}

fn world_time_advance_crosses_night_start(world_time: u64, ticks: u64) -> bool {
    if ticks == 0 {
        return false;
    }
    let day_tick = world_time % DAY_LENGTH_TICKS;
    let ticks_until_night = if day_tick < NIGHT_START_TICK {
        NIGHT_START_TICK - day_tick
    } else {
        DAY_LENGTH_TICKS - day_tick + NIGHT_START_TICK
    };
    ticks >= ticks_until_night
}
const ENTITY_HURT_INVULNERABLE_TICKS: u64 = 6;
pub const ITEM_DESPAWN_AGE_TICKS: u64 = 6_000;
const ITEM_DESPAWN_SWEEP_BUDGET: usize = 256;
const ARROW_ENTITY_HIT_DAMAGE: f32 = 4.0;
const ARROW_ENTITY_HIT_KNOCKBACK: f64 = 0.6;
const CHUNK_STREAM_STEPS_PER_TURN: usize = 1;
const DEFAULT_FLUID_TICK_BUDGET: usize = 256;

fn survival_damage_after_equipment(
    state: Option<&InteractionState>,
    amount: f32,
    kind: PlayerDamageKind,
) -> f32 {
    if !amount.is_finite() || !kind.is_supported() {
        return 0.0;
    }
    let damage = if kind.uses_armor() {
        survival_damage_after_armor(state, amount)
    } else {
        amount.max(0.0)
    };
    if kind.uses_protection() {
        survival_damage_after_protection(state, damage)
    } else {
        damage
    }
}

/// Default chunk radius around the player when no operator override is present.
pub const DEFAULT_VIEW_DISTANCE: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomTickPolicy {
    pub simulation_distance: i32,
    pub random_tick_speed: u32,
    pub chunk_budget: usize,
    pub fluid_tick_budget: usize,
    pub save_interval_ticks: u64,
    pub spawn_monsters: bool,
    pub seed: u64,
}

impl Default for RandomTickPolicy {
    fn default() -> Self {
        Self {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 3,
            chunk_budget: 64,
            fluid_tick_budget: DEFAULT_FLUID_TICK_BUDGET,
            save_interval_ticks: 20,
            spawn_monsters: true,
            seed: 0,
        }
    }
}

impl RandomTickPolicy {
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            simulation_distance: self
                .simulation_distance
                .clamp(crate::MIN_VIEW_DISTANCE, crate::MAX_VIEW_DISTANCE),
            random_tick_speed: self.random_tick_speed,
            chunk_budget: self.chunk_budget.max(1),
            fluid_tick_budget: self.fluid_tick_budget.max(1),
            save_interval_ticks: self.save_interval_ticks.max(1),
            spawn_monsters: self.spawn_monsters,
            seed: self.seed,
        }
    }

    #[must_use]
    pub fn is_enabled(self) -> bool {
        self.random_tick_speed > 0 && self.chunk_budget > 0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RandomTickReport {
    pub(crate) sampled: usize,
    pub(crate) eligible: usize,
    pub(crate) applied: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SheepGrazingReport {
    pub(crate) started: usize,
    pub(crate) ate: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScheduledFluidTickReport {
    pub(crate) drained: usize,
    pub(crate) applied: usize,
    pub(crate) budget: usize,
    pub(crate) budget_exhausted: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScheduledBlockTickReport {
    pub(crate) drained: usize,
    pub(crate) applied: usize,
    pub(crate) budget: usize,
    pub(crate) budget_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct HerdSpawn {
    chunk: (i32, i32),
    slot: u8,
    entity_type_id: i32,
    entity_type_name: String,
    position: Vec3,
    hostile: bool,
    sheep_color: Option<mc_entity::SheepColor>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SettlementInhabitantSpawn {
    claim: String,
    entity_type_id: i32,
    entity_type_name: String,
    position: Vec3,
    villager: mc_entity::VillagerData,
    villager_brain: mc_entity::villager_26_1_2::VillagerBrainState,
    villager_merchant: Option<mc_entity::villager_merchant_26_1_2::VillagerMerchantState>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsQuery {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: mc_physics::Aabb,
    pub on_ground: bool,
    pub fall_distance: f64,
    pub kind: EntityPhysicsKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityPhysicsKind {
    Default,
    Living,
    PowderSnowWalkableLiving,
    AquaticLiving,
    FallingBlock,
    ArrowProjectile {
        revision: Option<u64>,
        embedded_block: Option<mc_entity::projectile_26_1_2::BlockPosition>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsStep {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

/// World-snapshot collision endpoint for one arrow physics query.
///
/// This stays parallel to [`EntityPhysicsStep`] so non-projectile callers do
/// not need to manufacture projectile-only collision state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ArrowBlockHitFact {
    pub arrow_id: EntityId,
    pub block_state: mc_world::BlockStateId,
    pub block_position: mc_entity::projectile_26_1_2::BlockPosition,
    pub location: Vec3,
}

/// Projectile-only facts sampled from the same authoritative world snapshot
/// that produced an arrow's collision endpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ArrowPhysicsFact {
    pub arrow_id: EntityId,
    pub block_hit: Option<ArrowBlockHitFact>,
    pub embedded_in_block: bool,
    pub current_block_state: mc_world::BlockStateId,
    pub should_fall: bool,
    pub fall_velocity_scale: Vec3,
    pub in_water: bool,
    pub in_water_or_rain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockEdit {
    pos: mc_world::BlockPos,
    new_state: mc_world::BlockStateId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockEditPrecondition {
    pos: mc_world::BlockPos,
    expected_state: mc_world::BlockStateId,
    expected_token: mc_world::BlockMutationToken,
}

trait BlockPlanningRead {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<mc_world::BlockStateId>;
    fn block_mutation_token(&self, pos: mc_world::BlockPos)
    -> Option<mc_world::BlockMutationToken>;
}

impl BlockPlanningRead for mc_world::WorldStorage {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<mc_world::BlockStateId> {
        mc_world::WorldStorage::get_cached_block(self, pos)
    }

    fn block_mutation_token(
        &self,
        pos: mc_world::BlockPos,
    ) -> Option<mc_world::BlockMutationToken> {
        mc_world::WorldStorage::block_mutation_token(self, pos)
    }
}

impl BlockPlanningRead for mc_world::WorldReadSnapshot {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<mc_world::BlockStateId> {
        mc_world::WorldReadSnapshot::get_cached_block(self, pos)
    }

    fn block_mutation_token(
        &self,
        pos: mc_world::BlockPos,
    ) -> Option<mc_world::BlockMutationToken> {
        mc_world::WorldReadSnapshot::block_mutation_token(self, pos)
    }
}

impl BlockPlanningRead for mc_world::WorldReadView {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<mc_world::BlockStateId> {
        mc_world::WorldReadView::get_cached_block(self, pos)
    }

    fn block_mutation_token(
        &self,
        pos: mc_world::BlockPos,
    ) -> Option<mc_world::BlockMutationToken> {
        mc_world::WorldReadView::block_mutation_token(self, pos)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct HoeTillingPlan {
    edits: Vec<BlockEdit>,
    preconditions: Vec<BlockEditPrecondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AppliedBlockEdit {
    pos: mc_world::BlockPos,
    previous: mc_world::BlockStateId,
    new_state: mc_world::BlockStateId,
}

#[derive(Debug, Default)]
pub(super) struct BlockEditBatchOutcome {
    applied: Vec<AppliedBlockEdit>,
    resulting_tokens: HashMap<mc_world::BlockPos, mc_world::BlockMutationToken>,
    deltas: Vec<BlockDelta>,
    edit_chunks: HashSet<(i32, i32)>,
    light_edit_chunks: HashSet<(i32, i32)>,
    previous_light_chunks: HashMap<(i32, i32), ChunkLight>,
    cleared_campfires: Vec<mc_world::BlockPos>,
    precomputed_light_updates: Option<Vec<OutboundLightUpdate>>,
    pending_light_sources: Option<IncrementalLightSources>,
}

fn append_resident_block_outcome(
    target: &mut BlockEditBatchOutcome,
    mut additional: BlockEditBatchOutcome,
) {
    target.applied.append(&mut additional.applied);
    target.resulting_tokens.extend(additional.resulting_tokens);
    target.deltas.append(&mut additional.deltas);
    target.edit_chunks.extend(additional.edit_chunks);
    target
        .light_edit_chunks
        .extend(additional.light_edit_chunks);
    for (chunk, light) in additional.previous_light_chunks {
        target.previous_light_chunks.entry(chunk).or_insert(light);
    }
    target
        .cleared_campfires
        .append(&mut additional.cleared_campfires);
    if let Some(mut updates) = additional.precomputed_light_updates.take() {
        target
            .precomputed_light_updates
            .get_or_insert_default()
            .append(&mut updates);
    }
    debug_assert!(additional.pending_light_sources.is_none());
}

#[derive(Debug)]
enum SharedContainerCommit<T> {
    Committed {
        state_id: i32,
        inventory: PlayerInventory,
        carried_item: ItemStack,
        dispatches: Vec<VisibilityDispatch>,
    },
    Rejected {
        state_id: i32,
        authoritative: T,
        inventory: PlayerInventory,
        carried_item: ItemStack,
    },
}

#[derive(Debug, Clone)]
struct ContainerPlayerPlan {
    expected_inventory: PlayerInventory,
    expected_carried_item: ItemStack,
    updated_inventory: PlayerInventory,
    updated_carried_item: ItemStack,
    crafting_table_input: Option<CraftingTableInputPlan>,
    enchanting_table_input: Option<EnchantingTableInputPlan>,
    merchant_input: Option<MerchantInputPlan>,
    drops: Vec<ContainerDropPlan>,
    xp_orb: Option<ContainerXpPlan>,
}

#[derive(Debug, Clone)]
struct CraftingTableInputPlan {
    expected: Option<Box<[ItemStack; 9]>>,
    updated: Option<Box<[ItemStack; 9]>>,
}

#[derive(Debug, Clone)]
struct EnchantingTableInputPlan {
    expected: Option<Box<[ItemStack; 2]>>,
    updated: Option<Box<[ItemStack; 2]>>,
}

#[derive(Debug, Clone)]
struct MerchantInputPlan {
    expected: Option<Box<[ItemStack; 2]>>,
    updated: Option<Box<[ItemStack; 2]>>,
}

const MAX_CONTAINER_PLAYER_DROPS: usize = 14;

#[derive(Debug, Clone)]
struct ContainerDropPlan {
    entity_type_id: i32,
    position: Vec3,
    stack: EntityItemStack,
}

#[derive(Debug, Clone, Copy)]
struct ContainerXpPlan {
    entity_type_id: i32,
    position: Vec3,
    value: i32,
}

type ChestCommitOutcome = SharedContainerCommit<Vec<ChestBlockEntity>>;
type FurnaceCommitOutcome = SharedContainerCommit<FurnaceBlockEntity>;

#[derive(Debug)]
enum PlayerInventoryCommitOutcome {
    Committed {
        inventory: PlayerInventory,
        carried_item: ItemStack,
        crafting_table_input: Option<Box<[ItemStack; 9]>>,
        enchanting_table_input: Option<Box<[ItemStack; 2]>>,
        merchant_input: Option<Box<[ItemStack; 2]>>,
        dispatches: Vec<VisibilityDispatch>,
    },
    Rejected {
        inventory: PlayerInventory,
        carried_item: ItemStack,
        crafting_table_input: Option<Box<[ItemStack; 9]>>,
        enchanting_table_input: Option<Box<[ItemStack; 2]>>,
        merchant_input: Option<Box<[ItemStack; 2]>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RandomTickSample {
    pub chunk: (i32, i32),
    pub pos: mc_world::BlockPos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RandomTickCandidate {
    sample: RandomTickSample,
    state: mc_world::BlockStateId,
}

#[derive(Debug)]
struct RandomTickLeafDrop {
    source: mc_world::BlockPos,
    position: Vec3,
    stack: EntityItemStack,
}

#[derive(Debug, Default)]
struct RandomTickPlan {
    eligible: usize,
    edits: Vec<BlockEdit>,
    leaf_drops: Vec<RandomTickLeafDrop>,
    preconditions: Vec<SnapshotReadPrecondition>,
}

struct RandomTickRegionPlan {
    region: RegionKey,
    group: Vec<(usize, RandomTickCandidate)>,
    plan: RandomTickPlan,
}

struct RandomTickRegionJob {
    index: usize,
    #[cfg(test)]
    region: RegionKey,
    group: Vec<(usize, RandomTickCandidate)>,
    plan: RandomTickPlan,
    edits: Vec<mc_world::ResidentBlockEdit>,
    preconditions: Vec<mc_world::ResidentBlockPrecondition>,
    journal_chunks: Vec<ChunkPos>,
}

struct RandomTickRegionResult {
    index: usize,
    group: Vec<(usize, RandomTickCandidate)>,
    plan: RandomTickPlan,
    result: mc_world::ResidentBlockEditBatchResult,
    touched: Vec<ChunkPos>,
    panicked: bool,
}

struct ResidentBlockCommit<'a> {
    edits: &'a [mc_world::ResidentBlockEdit],
    preconditions: &'a [mc_world::ResidentBlockPrecondition],
    consumed_block_ticks: &'a [ScheduledBlockTick],
    consumed_fluid_ticks: &'a [ScheduledFluidTick],
    scheduled_fluid_ticks: &'a [ScheduledFluidTick],
    light_table: Option<&'a mc_data::block_light::BlockLightTable>,
    leaf_trigger_tick: Option<u64>,
}

#[derive(Debug, Default)]
struct ScheduledFluidTickPlan {
    edits: Vec<BlockEdit>,
    preconditions: Vec<SnapshotReadPrecondition>,
    scheduled_fluid_ticks: Vec<ScheduledFluidTick>,
}

struct ScheduledBlockRegionPlan {
    region: RegionKey,
    due: Vec<ScheduledBlockTick>,
    plan: ScheduledBlockTickPlan,
}

struct ScheduledBlockRegionJob {
    index: usize,
    #[cfg(test)]
    region: RegionKey,
    due: Vec<ScheduledBlockTick>,
    edits: Vec<mc_world::ResidentBlockEdit>,
    preconditions: Vec<mc_world::ResidentBlockPrecondition>,
    journal_chunks: Vec<ChunkPos>,
}

struct ScheduledBlockRegionResult {
    index: usize,
    due: Vec<ScheduledBlockTick>,
    result: mc_world::ResidentBlockEditBatchResult,
    touched: Vec<ChunkPos>,
    panicked: bool,
}

#[derive(Debug, Clone, Copy)]
struct SnapshotReadPrecondition {
    pos: mc_world::BlockPos,
    expected_state: Option<mc_world::BlockStateId>,
    expected_token: Option<mc_world::BlockMutationToken>,
}

struct SnapshotPlanningWorld<'a> {
    snapshot: &'a mc_world::WorldReadSnapshot,
    overrides: HashMap<mc_world::BlockPos, mc_world::BlockStateId>,
    read_positions: RefCell<HashSet<mc_world::BlockPos>>,
}

impl<'a> SnapshotPlanningWorld<'a> {
    fn new(snapshot: &'a mc_world::WorldReadSnapshot) -> Self {
        Self {
            snapshot,
            overrides: HashMap::new(),
            read_positions: RefCell::new(HashSet::new()),
        }
    }

    fn apply(&mut self, edit: BlockEdit) -> bool {
        let Some(current) = self.get_cached_block(edit.pos) else {
            return false;
        };
        if current == edit.new_state {
            return false;
        }
        self.overrides.insert(edit.pos, edit.new_state);
        true
    }

    fn preconditions(&self) -> Vec<SnapshotReadPrecondition> {
        let mut positions = self
            .read_positions
            .borrow()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
        positions
            .into_iter()
            .map(|pos| SnapshotReadPrecondition {
                pos,
                expected_state: self.snapshot.get_cached_block(pos),
                expected_token: self.snapshot.block_mutation_token(pos),
            })
            .collect()
    }
}

impl BlockPlanningRead for SnapshotPlanningWorld<'_> {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<mc_world::BlockStateId> {
        self.read_positions.borrow_mut().insert(pos);
        self.overrides
            .get(&pos)
            .copied()
            .or_else(|| self.snapshot.get_cached_block(pos))
    }

    fn block_mutation_token(
        &self,
        pos: mc_world::BlockPos,
    ) -> Option<mc_world::BlockMutationToken> {
        self.read_positions.borrow_mut().insert(pos);
        self.snapshot.block_mutation_token(pos)
    }
}

#[derive(Debug, Clone, Copy)]
struct PlayerPose {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    flags: MovePlayerFlags,
    input: PlayerInput,
    sprinting: bool,
    shifting: bool,
    in_water: bool,
    eye_in_water: bool,
    swimming: bool,
    fall_start_y: f64,
}

impl PlayerPose {
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(false, false),
            input: PlayerInput::default(),
            sprinting: false,
            shifting: false,
            in_water: false,
            eye_in_water: false,
            swimming: false,
            fall_start_y: y,
        }
    }

    fn chunk_pos(self) -> (i32, i32) {
        chunk_pos_from_coords(self.x, self.z)
    }

    fn entity_pose(self) -> EntityPose {
        if self.swimming {
            EntityPose::Swimming
        } else if self.shifting {
            EntityPose::Crouching
        } else {
            EntityPose::Standing
        }
    }

    fn body_height(self) -> f64 {
        if self.swimming {
            0.6
        } else if self.shifting {
            1.5
        } else {
            1.8
        }
    }

    fn eye_height(self) -> f64 {
        if self.swimming {
            0.4
        } else if self.shifting {
            1.27
        } else {
            1.62
        }
    }

    fn shared_flags(self) -> i8 {
        let mut flags = 0_u8;
        if self.shifting {
            flags |= 0x02;
        }
        if self.sprinting {
            flags |= 0x08;
        }
        if self.swimming {
            flags |= 0x10;
        }
        flags as i8
    }

    fn entity_data_values(self) -> Vec<EntityDataValue> {
        vec![
            EntityDataValue::Byte {
                index: ENTITY_DATA_SHARED_FLAGS_INDEX,
                value: self.shared_flags(),
            },
            EntityDataValue::Pose {
                index: ENTITY_DATA_POSE_INDEX,
                pose: self.entity_pose(),
            },
        ]
    }
}

fn script_player_context(
    profile: &LoggedInProfile,
    permissions: CommandPermissions,
    pose: PlayerPose,
) -> ScriptPlayerContext {
    script_player_context_from_values(&profile.uuid.to_string(), &profile.name, permissions, pose)
}

fn script_player_context_from_values(
    uuid: &str,
    username: &str,
    permissions: CommandPermissions,
    pose: PlayerPose,
) -> ScriptPlayerContext {
    ScriptPlayerContext::new(uuid, username, permissions.op, pose.x, pose.y, pose.z)
}

struct ScriptZoneObserver {
    zones: PluginZoneAdapter,
    player_id: ScriptPlayerId,
    uuid: String,
    username: String,
    permissions: CommandPermissions,
    dimension: String,
    revision: u64,
}

impl ScriptZoneObserver {
    async fn observe(&mut self, pose: PlayerPose) {
        let Some(revision) = self.revision.checked_add(1) else {
            warn!(
                player_id = self.player_id.value(),
                "script zone observation revision exhausted"
            );
            return;
        };
        self.revision = revision;
        let context =
            script_player_context_from_values(&self.uuid, &self.username, self.permissions, pose);
        if let Err(error) = self
            .zones
            .observe_player(self.player_id, revision, &self.dimension, context)
            .await
        {
            debug!(
                ?error,
                player_id = self.player_id.value(),
                revision,
                "script zone observation rejected"
            );
        }
    }
}

fn load_player_state_for_login(
    world_root: &std::path::Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedState,
) -> Result<PlayerPersistedState, ConnectionError> {
    let loaded = load_player_state(world_root, uuid, items, default.clone()).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("player state load failed: {error}"),
        )
    })?;
    Ok(loaded.unwrap_or(default))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    profile: &LoggedInProfile,
    profile_properties: &[GameProfileProperty],
    permissions: CommandPermissions,
    config: &ServerConfig,
    connection_world: crate::server::ConnectionWorld,
    sessions: Arc<SessionRegistry>,
    chunk_pipeline_resources: ChunkPipelineResources,
    dirty_flush: Option<crate::dirty_flush::DirtyFlushNotifier>,
    runtime_control: Option<RuntimeControlHandle>,
    simulation: SimulationHandle,
    configuration_custom_payloads: Vec<ConfigurationCustomPayload>,
    loader_session: Option<crate::LoaderSession>,
    extension: Option<ExtensionEventSink>,
    scripts: Option<ScriptEventSink>,
    script_zones: Option<PluginZoneAdapter>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let loader_eligible = loader_session.is_some();
    // `blocks` rides along on the config — currently unused by the
    // Play handler because the chunk encoder reads palette IDs straight
    // from the chunk; it'll matter once we synthesise placeholder
    // chunks or do block-update packets.
    let _ = &config.blocks;
    let data: &VanillaData = &config.data;
    let (dim_id, dim_name, dim_names) = spawn_dimension(data).ok_or_else(|| {
        ConnectionError::Codec(mc_protocol::CodecError::InvalidIdentifier(
            "no dimension_type entries available".into(),
        ))
    })?;

    info!(
        player = %profile.name,
        uuid = %profile.uuid,
        spawn_dimension = %dim_name,
        "entering Play state"
    );

    let (spawn_x, spawn_y, spawn_z) = spawn_position(config, connection_world.read.as_ref());
    let default_spawn_pose = PlayerPose::new(spawn_x, spawn_y, spawn_z);
    let world_root = connection_world.root;
    let world_read = connection_world.read;
    let world_mutation = connection_world.mutation;
    let chunk_source = connection_world.chunk_source;
    let default_player_state = PlayerPersistedState::new_default(default_spawn_pose);
    let mut player_state = if let Some(state) = sessions.recoverable_player_state(profile.uuid) {
        info!(player = %profile.name, state = %state, "recovered disconnected player state");
        state
    } else if let Some(root) = world_root.as_deref() {
        let state = load_player_state_for_login(
            root,
            profile.uuid,
            &config.items,
            default_player_state.clone(),
        )?;
        info!(player = %profile.name, state = %state, "initialized player state");
        state
    } else {
        default_player_state
    };
    player_state.pose = clamp_player_pose(player_state.pose);
    let respawn_pose = clamp_player_pose(player_state.spawn.pose());

    let (spawn_cx, spawn_cz) = player_state.pose.chunk_pos();
    let (outbound_tx, outbound_rx) = mpsc::channel(outbound_command_queue_capacity(config));
    let initial_desired = if config.world.is_some() {
        desired_chunk_set(spawn_cx, spawn_cz, config.view_distance)
    } else {
        HashSet::new()
    };
    let initial_pose = player_state.pose;
    let (session_id, visibility) = match sessions.try_register(SessionRegistration {
        profile,
        properties: profile_properties,
        center: (spawn_cx, spawn_cz),
        view_distance: config.view_distance,
        desired: initial_desired,
        tx: outbound_tx,
        pose: initial_pose,
        max_sessions: config.max_players as usize,
        script_operator: permissions.op,
        dimension: dim_name.as_str(),
        loader_session,
    }) {
        Ok(registered) => registered,
        Err(err) => {
            let reason = session_admission_message(&err);
            match &err {
                SessionAdmissionError::ServerFull { active, max } => warn!(
                    player = %profile.name,
                    uuid = %profile.uuid,
                    active_sessions = *active,
                    max_sessions = *max,
                    reason,
                    "play session rejected"
                ),
                SessionAdmissionError::DuplicateProfile { existing_session } => warn!(
                    player = %profile.name,
                    uuid = %profile.uuid,
                    existing_session = *existing_session,
                    active_sessions = sessions.active_session_count(),
                    reason,
                    "play session rejected"
                ),
            }
            write_packet(
                writer,
                &PlayDisconnect {
                    reason_nbt: text_component_nbt(reason)?,
                },
                compression,
            )
            .await?;
            return Ok(());
        }
    };
    let extension_player_id = PlayerId::new(session_id);
    let session_cleanup = RegisteredSessionCleanup::new(
        Arc::clone(&sessions),
        session_id,
        extension.clone(),
        scripts.clone(),
        script_zones.clone(),
    );
    if let Some(extension) = extension.as_ref() {
        extension.enqueue_event(InboundEvent::PlayerJoined {
            player_id: extension_player_id,
            username: profile.name.clone(),
        });
        for payload in &configuration_custom_payloads {
            extension.enqueue_custom_payload(
                extension_player_id,
                ProtocolPhase::Configuration,
                &payload.channel,
                payload.payload.as_ref(),
            );
        }
    }
    if let Some(scripts) = scripts.as_ref() {
        scripts.enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(session_id),
            script_player_context(profile, permissions, initial_pose),
        ));
    }

    // 1. Login (Play).
    let login = LoginPlay {
        entity_id: i32::try_from(session_id).unwrap_or(i32::MAX),
        is_hardcore: false,
        dimension_names: dim_names.to_vec(),
        max_players: config.max_players.min(i32::MAX as u32) as i32,
        view_distance: config.view_distance,
        simulation_distance: config.random_tick.normalized().simulation_distance,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id: dim_id,
        dimension_name: dim_name.clone(),
        hashed_seed: 0,
        game_mode: player_state.game_mode.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        enforces_secure_chat: false,
    };
    let respawn = ClientboundRespawn {
        dimension_type_id: login.dimension_type_id,
        dimension_name: login.dimension_name.clone(),
        hashed_seed: login.hashed_seed,
        game_mode: login.game_mode,
        previous_game_mode: login.previous_game_mode,
        is_debug: login.is_debug,
        is_flat: login.is_flat,
        death_location: None,
        portal_cooldown: login.portal_cooldown,
        sea_level: login.sea_level,
        data_to_keep: 0,
    };
    write_packet(writer, &login, compression).await?;
    // Vanilla 26.1.x sends ChangeDifficulty right after LoginPlay.
    // The ordinal 1 = EASY (Minecraft default for new worlds).
    write_packet(
        writer,
        &ClientboundChangeDifficulty {
            difficulty: 1,
            locked: false,
        },
        compression,
    )
    .await?;
    // Vanilla 26.1.x also sends PlayerAbilities after ChangeDifficulty.
    write_packet(
        writer,
        &player_abilities_for_mode(player_state.game_mode),
        compression,
    )
    .await?;
    write_packet(
        writer,
        &ClientboundSetHeldSlot {
            slot: i32::from(player_state.selected_hotbar_slot),
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &EntityEvent {
            entity_id: login.entity_id,
            event_id: if permissions.op { 28 } else { 24 },
        },
        compression,
    )
    .await?;
    let plugin_command_roots = scripts
        .as_ref()
        .map_or_else(Vec::new, ScriptEventSink::player_command_roots);
    write_packet(
        writer,
        &command_tree_packet_with_plugin_roots(
            permissions,
            &plugin_command_roots,
            &scripts
                .as_ref()
                .map_or_else(Vec::new, ScriptEventSink::operator_command_roots),
        ),
        compression,
    )
    .await?;
    dispatch_visibility_commands(visibility);

    // 2. Synchronize Player Position. teleport_id=1; Play movement is gated
    //    until the client confirms this teleport.
    write_packet(
        writer,
        &SynchronizePlayerPosition {
            teleport_id: 1,
            x: initial_pose.x,
            y: initial_pose.y,
            z: initial_pose.z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: initial_pose.yaw,
            pitch: initial_pose.pitch,
            relative_flags: 0,
        },
        compression,
    )
    .await?;

    // 3. Level info. Vanilla sends border, full clock sync, default spawn,
    //    optional weather, then LEVEL_CHUNKS_LOAD_START. Solaris uses a static
    //    vanilla-default border until world border state exists.
    write_packet(
        writer,
        &ClientboundInitializeBorder {
            center_x: 0.0,
            center_z: 0.0,
            old_size: 59_999_968.0,
            new_size: 59_999_968.0,
            lerp_time: 0,
            absolute_max_size: 29_999_984,
            warning_blocks: 5,
            warning_time: 15,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &clientbound_session_world_time(&sessions),
        compression,
    )
    .await?;
    write_packet(
        writer,
        &SetDefaultSpawnPosition {
            dimension: dim_name.clone(),
            position: pack_block_pos(
                spawn_x.floor() as i32,
                spawn_y.floor() as i32,
                spawn_z.floor() as i32,
            ),
            yaw: 0.0,
            pitch: 0.0,
        },
        compression,
    )
    .await?;

    // 4. Game Event: start waiting for chunks. Tells the client to
    //    drop the loading screen even though no chunks are coming.
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        },
        compression,
    )
    .await?;

    // 5. Set Center Chunk + view-distance window. Spawn is at
    //    (SPAWN_X, SPAWN_Z); the chunk anchor is the chunk that
    //    contains it, and we stream ±view_distance around it.
    write_packet(
        writer,
        &SetCenterChunk {
            chunk_x: spawn_cx,
            chunk_z: spawn_cz,
        },
        compression,
    )
    .await?;

    write_packet(
        writer,
        &initial_recipe_update(&config.recipes, &config.items, &config.item_facts),
        compression,
    )
    .await?;
    write_packet(
        writer,
        &ClientboundRecipeBookSettings::default(),
        compression,
    )
    .await?;
    write_packet(
        writer,
        &initial_recipe_book(&config.recipes, &config.items),
        compression,
    )
    .await?;

    let passive_herd_surface = mc_data::Identifier::parse("minecraft:grass_block")
        .ok()
        .and_then(|id| config.blocks.block(&id).map(|block| block.default));
    let passive_herd_water = mc_data::Identifier::parse("minecraft:water")
        .ok()
        .and_then(|id| config.blocks.block(&id).map(|block| block.default));
    let passive_herd_water_states = Arc::new(fluid_state_ids(
        &config.blocks,
        &config.block_facts,
        FluidKind::Water,
        passive_herd_water,
    ));
    let passive_herd_passable = Arc::new(passive_entity_passable_blocks(&config.blocks));
    let passive_herd_fallback_surfaces =
        Arc::new(passive_herd_fallback_surface_blocks(&config.blocks));
    let block_entity_types = Arc::new(effective_block_entity_types(data));
    let mut light_cache = LightCache::new();
    let mut chunk_stream = config.world.as_ref().and_then(|world| {
        let biomes = data.registry("worldgen/biome")?;
        Some(
            ChunkStreamState::new(
                Arc::clone(world),
                Arc::new(biomes.clone()),
                Arc::clone(&config.blocks),
                config.block_light.as_ref().map(Arc::clone),
                Arc::clone(&config.items),
                Arc::clone(&config.tags),
                Arc::clone(&config.recipes),
                Arc::clone(&block_entity_types),
                passive_herd_surface,
                Arc::clone(&passive_herd_fallback_surfaces),
                Arc::clone(&passive_herd_water_states),
                Arc::clone(&passive_herd_passable),
                Arc::clone(&config.biome_spawns),
                Arc::clone(&config.entity_types),
                compression,
                Arc::clone(&sessions),
                session_id,
                spawn_cx,
                spawn_cz,
                initial_pose.yaw,
                config.view_distance,
                chunk_pipeline_resources.clone(),
                config.chunk_pipeline,
            )
            .with_spawn_monsters(config.random_tick.spawn_monsters)
            .with_world_read(world_read.clone())
            .with_world_mutation(world_mutation.clone())
            .with_chunk_source(chunk_source)
            .with_simulation(simulation.clone())
            .with_dirty_flush(dirty_flush)
            .with_runtime_control(runtime_control.clone()),
        )
    });
    if config.world.is_some() && chunk_stream.is_none() {
        warn!("worldgen/biome registry missing; skipping chunk emission");
    }
    let player_simulation = simulation.for_session(session_id);
    let player_save_state = Arc::new(Mutex::new(player_state.clone()));
    sessions.register_player_persistence(session_id, Arc::clone(&player_save_state));
    let mut player_inventory_settled = true;
    let result = async {
        if let Some(stream) = chunk_stream.as_mut() {
            let Some(step) = slow_client_chunk_stream_step_timeout(
                &sessions,
                session_id,
                stream.step(writer, &mut light_cache),
                SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT,
            )
            .await?
            else {
                return Ok(());
            };
            if step == ChunkStreamStep::Complete {
                stream.log_summary_once();
            }
        }

        // 6. Restore the server-authoritative player inventory. Test and
        //    dev-only inventory mutation goes through explicit debug commands;
        //    normal survival no longer gets a starter kit.
        let initial_inventory = player_state.inventory.clone();
        let recipes = (*config.recipes).clone();
        let mut interaction = config.world.as_ref().map(|world| InteractionState {
            world: Arc::clone(world),
            world_read: world_read
                .clone()
                .expect("world-backed interaction has a read view"),
            blocks: Arc::clone(&config.blocks),
            block_light: config.block_light.as_ref().map(Arc::clone),
            block_facts: Arc::clone(&config.block_facts),
            water: passive_herd_water,
            sessions: Arc::clone(&sessions),
            simulation: player_simulation.clone(),
            session_id,
            workspace: LightWorkspace::new(),
            light_cache: std::mem::take(&mut light_cache),
            compression,
            selected_hotbar_slot: player_state.selected_hotbar_slot,
            inventory: initial_inventory,
            carried_item: player_state.carried_item.clone(),
            player_persistence: Arc::clone(&player_save_state),
            inventory_state_id: 1,
            inventory_quickcraft: QuickCraftState::default(),
            items: Arc::clone(&config.items),
            item_facts: Arc::clone(&config.item_facts),
            entity_types: Arc::clone(&config.entity_types),
            item_to_block: ItemToBlockTable::build(&config.items, &config.blocks),
            tags: Arc::clone(&config.tags),
            recipes,
            loot: Arc::clone(&config.loot),
            script_zones: script_zones.clone(),
            next_container_id: FURNACE_CONTAINER_ID_MIN,
            active_container: None,
            pending_break: None,
            delayed_break: None,
            pending_use: None,
            pending_sign_edit: None,
            shield_use: None,
            last_entity_attack_tick: None,
        });
        if let Some(state) = interaction.as_mut() {
            settle_recovered_player_inventory(state, &player_state).await?;
        }
        let initial_items = interaction.as_ref().map_or_else(
            || player_state.inventory.as_wire_list(),
            |state| state.inventory.as_wire_list(),
        );
        let initial_carried_item = interaction.as_ref().map_or_else(
            || player_state.carried_item.clone(),
            |state| state.carried_item.clone(),
        );
        write_packet(
            writer,
            &ClientboundContainerSetContent {
                container_id: 0,
                state_id: 1,
                items: initial_items,
                carried_item: initial_carried_item,
            },
            compression,
        )
        .await?;

        // 7. Play loop. Runs until the connection drops or the client
        //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`. The
        //    interaction state passes the M5.d/M5.e/M6.f break/place
        //    handlers everything they need to mutate the world and emit
        //    relight + container packets back to the client.
        let play_result = play_loop(
            reader,
            writer,
            buf,
            compression,
            interaction.as_mut(),
            chunk_stream,
            runtime_control.clone(),
            chunk_pipeline_resources.clone(),
            Arc::clone(&sessions),
            player_simulation,
            config,
            session_id,
            loader_eligible,
            initial_pose,
            respawn_pose,
            respawn,
            permissions,
            player_state.survival,
            player_state.xp,
            player_state.game_mode,
            outbound_rx,
            config.view_distance,
            profile.uuid.to_string(),
            profile.name.clone(),
            extension,
            extension_player_id,
            scripts,
            script_zones,
        )
        .await;
        if let Some(state) = interaction.as_mut() {
            if let Some(bed) = sessions.request_sleep_wake(session_id) {
                release_disconnected_sleep_bed(state, writer, bed).await;
            }
            player_inventory_settled =
                settle_disconnected_inventory(state, &player_save_state).await;
        }
        play_result
    }
    .await;

    if !player_inventory_settled {
        warn!(
            player = %profile.name,
            "disconnect inventory did not settle; owner state retained for checkpoint recovery"
        );
    }

    session_cleanup.unregister();
    result
}

/// Per-connection state the M5.d / M5.e / M6 interaction handlers
/// carry.
struct InteractionState {
    world: WorldHandle,
    world_read: mc_world::WorldReadView,
    blocks: Arc<mc_world::BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    water: Option<mc_world::BlockStateId>,
    sessions: Arc<SessionRegistry>,
    simulation: SimulationHandle,
    session_id: SessionId,
    #[allow(dead_code)] // Retained for existing direct test fixtures.
    workspace: LightWorkspace,
    /// Per-session chunk light cache, populated during the spawn burst.
    light_cache: LightCache,
    compression: Compression,
    /// M6.d: which item the player is currently holding. Bumped by
    /// `ServerboundSetCarriedItem` (0..=8) and consulted by
    /// `handle_use_item_on` to resolve the placed block.
    selected_hotbar_slot: u8,
    /// M6.e: a 46-slot window-0 inventory. Indices follow vanilla's
    /// numbering: 0..4 crafting (output + 2×2 input), 5..8 armor,
    /// 9..35 main rows, 36..44 hotbar, 45 offhand.
    inventory: PlayerInventory,
    /// Server-authoritative cursor stack for vanilla container clicks.
    carried_item: ItemStack,
    /// Durable mirror committed only by this exact session owner.
    player_persistence: Arc<Mutex<PlayerPersistedState>>,
    /// M6.e: per-vanilla, the server bumps this counter on every
    /// inventory mutation it ships to the client; the client uses
    /// it to detect desyncs. Starts at 1 (after the seed
    /// ContainerSetContent on login).
    inventory_state_id: i32,
    inventory_quickcraft: QuickCraftState,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
    entity_types: Arc<EntityTypeRegistry>,
    /// Registry-derived item→default-block resolver. Built once from
    /// vanilla item/block registries at construction time.
    item_to_block: ItemToBlockTable,
    tags: Arc<TagsData>,
    recipes: Vec<mc_data::recipes::Recipe>,
    loot: Arc<mc_data::loot::LootTables>,
    script_zones: Option<PluginZoneAdapter>,
    next_container_id: i32,
    active_container: Option<ActiveContainer>,
    pending_break: Option<PendingBreak>,
    delayed_break: Option<PendingBreak>,
    pending_use: Option<PendingUse>,
    pending_sign_edit: Option<PendingSignEdit>,
    shield_use: Option<ShieldUseState>,
    last_entity_attack_tick: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSignEdit {
    position: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    token: mc_world::BlockMutationToken,
    is_front_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientPreferences {
    language: String,
    requested_view_distance: i8,
    clamped_view_distance: i32,
    chat_visibility: mc_protocol::packets::ChatVisibility,
    chat_colors: bool,
    model_customisation: u8,
    main_hand: mc_protocol::packets::MainHand,
    text_filtering_enabled: bool,
    allows_listing: bool,
    particle_status: mc_protocol::packets::ParticleStatus,
    brand: Option<String>,
}

impl ClientPreferences {
    fn from_packet(
        information: mc_protocol::packets::ClientInformation,
        server_view_distance: i32,
        brand: Option<String>,
    ) -> Self {
        Self {
            clamped_view_distance: clamp_client_view_distance(
                information.view_distance,
                server_view_distance,
            ),
            language: information.language,
            requested_view_distance: information.view_distance,
            chat_visibility: information.chat_visibility,
            chat_colors: information.chat_colors,
            model_customisation: information.model_customisation,
            main_hand: information.main_hand,
            text_filtering_enabled: information.text_filtering_enabled,
            allows_listing: information.allows_listing,
            particle_status: information.particle_status,
            brand,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlayCustomPayloadAction {
    Brand(String),
    LoaderInteraction(Bytes),
    Unknown { channel: String, payload: Bytes },
    Oversized { len: usize },
}

fn classify_play_custom_payload(
    mut body: Bytes,
) -> Result<PlayCustomPayloadAction, ConnectionError> {
    if body.len() > DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES {
        return Ok(PlayCustomPayloadAction::Oversized { len: body.len() });
    }

    let channel = body.read_identifier()?;
    if channel == *CustomPayload::brand_channel() {
        let brand = body.read_string(DEFAULT_MAX_STRING_LEN)?;
        return Ok(PlayCustomPayloadAction::Brand(brand));
    }

    let payload = body.copy_to_bytes(body.remaining());
    if channel == *loader_interaction_channel() {
        return Ok(PlayCustomPayloadAction::LoaderInteraction(payload));
    }
    Ok(PlayCustomPayloadAction::Unknown {
        channel: channel.as_str().to_string(),
        payload,
    })
}

fn clamp_client_view_distance(requested: i8, server_view_distance: i32) -> i32 {
    let server_view_distance =
        server_view_distance.clamp(crate::MIN_VIEW_DISTANCE, crate::MAX_VIEW_DISTANCE);
    i32::from(requested).clamp(crate::MIN_VIEW_DISTANCE, server_view_distance)
}

async fn write_block_ack<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(writer, &BlockChangedAck { sequence }, compression).await
}

async fn write_block_resync_then_ack<W>(
    state: &InteractionState,
    writer: &mut W,
    position: i64,
    sequence: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_block_resync(state, writer, position).await?;
    write_block_ack(writer, state.compression, sequence).await
}

async fn write_block_resync<W>(
    state: &InteractionState,
    writer: &mut W,
    position: i64,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (x, y, z) = unpack_block_pos(position);
    let block_position = mc_world::BlockPos { x, y, z };
    #[cfg(test)]
    let state_id = {
        let mut storage = state.world.lock().await;
        match storage.get_block(block_position) {
            Ok(Some(state_id)) => Some(state_id),
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, x, y, z, "block resync read failed");
                None
            }
        }
    };
    #[cfg(not(test))]
    let state_id = match state.simulation.read_block_snapshot(block_position).await {
        Ok(snapshot) => snapshot.map(|snapshot| snapshot.state),
        Err(error) => {
            debug!(?error, x, y, z, "simulation block resync snapshot rejected");
            None
        }
    };
    let update = state_id.map(|state_id| BlockUpdate {
        position,
        state_id: outbound_block_state_id(state, state_id),
    });
    if let Some(update) = update {
        write_packet(writer, &update, state.compression).await?;
    }
    Ok(())
}

fn outbound_block_state_id(state: &InteractionState, block_state: mc_world::BlockStateId) -> i32 {
    state
        .sessions
        .loader_block_projection(state.session_id, &state.blocks)
        .map_or(block_state, |projection| projection.project(block_state))
        .0 as i32
}

async fn write_crafting_content<W>(
    state: &mut InteractionState,
    writer: &mut W,
    window: &CraftingTableWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: crafting_wire_items(window, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn store_inventory_crafting_inputs(
    state: &mut InteractionState,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError> {
    settle_player_inventory_returns(
        state,
        InventoryReturnPlan {
            enchanting_table_input: None,
            merchant_input: None,
            crafting_table_input: None,
            return_crafting_table_input: false,
            return_inventory_crafting_inputs: true,
            return_cursor: false,
            player_pose,
        },
    )
    .await
}

async fn write_stonecutter_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &StonecutterWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: stonecutter_wire_items(window, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn write_merchant_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &MerchantWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: merchant_wire_items(window, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn write_merchant_offers<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &MerchantWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundMerchantOffers {
            container_id: window.container_id,
            offers: merchant_protocol_offers(window),
            villager_level: i32::from(window.merchant.level()),
            villager_xp: window.merchant.xp,
            show_progress: true,
            can_restock: true,
        },
        state.compression,
    )
    .await
}

async fn write_merchant_window<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &MerchantWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_merchant_content(state, writer, window).await?;
    write_merchant_offers(state, writer, window).await
}

async fn open_stonecutter_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    sequence: i32,
    position: mc_world::BlockPos,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let clicked = state.world_read.get_cached_block(position);
    if !clicked.is_some_and(|block_state| is_stonecutter_state(state, block_state)) {
        return Ok(false);
    }
    store_active_container(state, player_pose).await?;
    let window = StonecutterWindow::at_position(next_container_id(state), position);
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: STONECUTTER_MENU_TYPE_ID,
            title_nbt: stonecutter_menu_title_nbt()?,
        },
        state.compression,
    )
    .await?;
    write_stonecutter_content(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::Stonecutter(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn open_crafting_table_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = mc_world::BlockPos { x, y, z };
    let clicked = state.world_read.get_cached_block(position);
    if !clicked.is_some_and(|block_state| is_crafting_table_state(state, block_state)) {
        return Ok(false);
    }

    store_active_container(state, player_pose).await?;
    let mut window = CraftingTableWindow::new(next_container_id(state));
    refresh_crafting_result_with_data(
        &state.items,
        &state.item_facts,
        &state.tags,
        &state.recipes,
        &mut window,
    );
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: CRAFTING_MENU_TYPE_ID,
            title_nbt: crafting_menu_title_nbt()?,
        },
        state.compression,
    )
    .await?;
    write_crafting_content(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

fn crafting_menu_state_change_count(
    before_window: &CraftingTableWindow,
    after_window: &CraftingTableWindow,
    before_inventory: &PlayerInventory,
    after_inventory: &PlayerInventory,
    before_carried: &ItemStack,
    after_carried: &ItemStack,
) -> i32 {
    let result_changes = usize::from(before_window.result != after_window.result);
    let input_changes = before_window
        .input
        .iter()
        .zip(&after_window.input)
        .filter(|(before, after)| before != after)
        .count();
    let inventory_changes = (9..=44)
        .filter(|slot| before_inventory.slots[*slot] != after_inventory.slots[*slot])
        .count();
    let carried_changes = usize::from(before_carried != after_carried);
    i32::try_from(result_changes + input_changes + inventory_changes + carried_changes)
        .unwrap_or(i32::MAX)
        .max(1)
}

async fn handle_crafting_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: Box<CraftingTableWindow>,
    script_events: Option<&ScriptGameplayEventPublisher>,
    game_mode: GameMode,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<Box<CraftingTableWindow>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if packet.state_id != window.state_id {
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    let action = classify_container_click(&packet);
    if !matches!(action, ContainerClickAction::QuickCraft(_)) {
        window.quickcraft.reset();
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_window = window.clone();
    let mut dropped = None;
    let mut discarded_remainders = Vec::new();
    let mut quickcraft_outcome = None;
    let crafted_result = match &action {
        ContainerClickAction::Pickup { slot: 0, .. }
        | ContainerClickAction::QuickMove { slot: 0 } => Some(before_window.result.clone()),
        _ => None,
    };
    let quick_moved_result = matches!(&action, ContainerClickAction::QuickMove { slot: 0 });
    let changed = match action {
        ContainerClickAction::Pickup { slot, button } => {
            let (changed, discarded) = window.apply_pickup_click(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                &mut state.inventory,
                &mut state.carried_item,
                slot,
                button,
            );
            discarded_remainders = discarded;
            changed
        }
        ContainerClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click_with_carried(&mut state.carried_item, button);
            dropped.is_some()
        }
        ContainerClickAction::QuickMove { slot } => {
            let (changed, discarded) = window.apply_quick_move_click(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                &mut state.inventory,
                slot,
            );
            discarded_remainders = discarded;
            changed
        }
        ContainerClickAction::Swap { slot, button } => window.apply_swap_click(
            &state.items,
            &state.item_facts,
            &state.tags,
            &state.recipes,
            &mut state.inventory,
            slot,
            button,
        ),
        ContainerClickAction::Throw { slot, button } => {
            if item_entity_type_id(&state.entity_types).is_some() {
                dropped = window.apply_throw_click(
                    &state.items,
                    &state.item_facts,
                    &state.tags,
                    &state.recipes,
                    &mut state.inventory,
                    slot,
                    button,
                );
                dropped.is_some()
            } else {
                false
            }
        }
        ContainerClickAction::QuickCraft(click) => {
            let outcome = window.apply_quickcraft_click(
                &state.items,
                &state.item_facts,
                &mut state.inventory,
                &mut state.carried_item,
                click,
                &state.tags,
                &state.recipes,
            );
            quickcraft_outcome = Some(outcome);
            outcome == QuickCraftOutcome::Changed
        }
        ContainerClickAction::Unsupported => false,
    };
    for remaining in discarded_remainders {
        debug!(
            item_id = remaining.item_id,
            count = remaining.count,
            "dropping crafting remainder because inventory is full"
        );
    }
    if quickcraft_outcome == Some(QuickCraftOutcome::Pending) {
        if client_carried_item_matches(&packet.carried_item, &state.carried_item) {
            return Ok(window);
        }
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        window = before_window;
        window.quickcraft.reset();
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        window = before_window;
        window.quickcraft.reset();
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    if !changed {
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    let state_id_increment = crafting_menu_state_change_count(
        &before_window,
        &window,
        &before_inventory,
        &state.inventory,
        &before_carried_item,
        &state.carried_item,
    );
    let crafted = crafted_result.as_ref().and_then(|result| {
        if quick_moved_result {
            crafted_item_from_inventory_delta(result, &before_inventory, &state.inventory)
        } else {
            CraftedItem::from_single_result(result)
        }
    });
    if commit_crafting_table_candidate(
        state,
        &mut window,
        &before_window.input,
        before_inventory,
        before_carried_item,
        dropped,
        player_pose,
    )
    .await?
    {
        if let (Some(script_events), Some(crafted)) = (script_events, crafted) {
            script_events
                .publish_item_crafted(
                    &state.items,
                    crafted.item_id,
                    crafted.count,
                    crafted.craft_count,
                    ScriptCraftingSource::CraftingTable,
                    player_pose,
                    game_mode,
                )
                .await;
        }
        window.state_id = window.state_id.wrapping_add(state_id_increment);
    }
    write_crafting_content(state, writer, &window).await?;
    Ok(window)
}

async fn handle_stonecutter_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: StonecutterWindow,
    _player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<StonecutterWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if packet.state_id != window.state_id {
        write_stonecutter_content(state, writer, &window).await?;
        return Ok(window);
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_window = window.clone();
    let action = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => {
            StonecutterClickAction::Pickup { slot, button }
        }
        ContainerClickAction::QuickMove { slot } => StonecutterClickAction::QuickMove { slot },
        ContainerClickAction::OutsidePickup { .. }
        | ContainerClickAction::Swap { .. }
        | ContainerClickAction::Throw { .. }
        | ContainerClickAction::QuickCraft(_)
        | ContainerClickAction::Unsupported => StonecutterClickAction::Unsupported,
    };
    let plan = plan_stonecutter_click(StonecutterClickInput {
        recipes: &state.recipes,
        items: &state.items,
        item_facts: &state.item_facts,
        tags: &state.tags,
        window: window.clone(),
        inventory: state.inventory.clone(),
        carried_item: state.carried_item.clone(),
        action,
    });
    let planned_carried_item = plan
        .as_ref()
        .map_or(&state.carried_item, |plan| &plan.carried_item);
    if !client_carried_item_matches(&packet.carried_item, planned_carried_item) {
        write_stonecutter_content(state, writer, &window).await?;
        return Ok(window);
    }
    let Some(plan) = plan else {
        write_stonecutter_content(state, writer, &window).await?;
        return Ok(window);
    };
    window = plan.window;
    state.inventory = plan.inventory;
    state.carried_item = plan.carried_item;
    if commit_stonecutter_candidate(
        state,
        &mut window,
        &before_window.input,
        before_inventory,
        before_carried_item,
    )
    .await?
    {
        window.state_id = window.state_id.wrapping_add(1);
    }
    write_stonecutter_content(state, writer, &window).await?;
    Ok(window)
}

#[cfg(test)]
fn select_stonecutter_recipe(
    state: &InteractionState,
    window: &mut StonecutterWindow,
    selection: usize,
) -> bool {
    select_stonecutter_recipe_with_data(
        &state.recipes,
        &state.items,
        &state.item_facts,
        &state.tags,
        window,
        selection,
    )
}

#[cfg(test)]
fn apply_stonecutter_quick_move_click(
    state: &mut InteractionState,
    window: &mut StonecutterWindow,
    menu_slot: usize,
) -> bool {
    let Some(plan) = plan_stonecutter_click(StonecutterClickInput {
        recipes: &state.recipes,
        items: &state.items,
        item_facts: &state.item_facts,
        tags: &state.tags,
        window: window.clone(),
        inventory: state.inventory.clone(),
        carried_item: state.carried_item.clone(),
        action: StonecutterClickAction::QuickMove { slot: menu_slot },
    }) else {
        return false;
    };
    *window = plan.window;
    state.inventory = plan.inventory;
    state.carried_item = plan.carried_item;
    true
}

fn cached_enchantment_power_provider(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> bool {
    let Some(block) = state
        .world_read
        .get_cached_block(position)
        .and_then(|block_state| state.blocks.by_id(block_state))
    else {
        return false;
    };
    block.block.id.path() == "bookshelf"
        || block_tag_contains(
            &state.tags,
            "minecraft:enchantment_power_provider",
            block.block.raw_id,
        )
}

fn cached_enchantment_power_transmitter(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> bool {
    let Some(block) = state
        .world_read
        .get_cached_block(position)
        .and_then(|block_state| state.blocks.by_id(block_state))
    else {
        return false;
    };
    matches!(block.block.id.path(), "air" | "cave_air" | "void_air")
        || block_tag_contains(
            &state.tags,
            "minecraft:enchantment_power_transmitter",
            block.block.raw_id,
        )
}

fn enchanting_bookshelf_count(state: &InteractionState, table: mc_world::BlockPos) -> u8 {
    count_valid_enchanting_bookshelves(
        table,
        |position| cached_enchantment_power_provider(state, position),
        |position| cached_enchantment_power_transmitter(state, position),
    )
}

fn can_place_in_enchanting_menu_slot(
    state: &InteractionState,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    can_place_in_enchanting_menu_slot_with_data(
        &state.items,
        &state.item_facts,
        menu_slot,
        stack,
        |slot, stack| can_place_in_player_slot(&state.item_facts, &state.items, slot, stack),
    )
}

async fn write_enchanting_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &EnchantingTableWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: enchanting_wire_items(window, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn write_enchanting_data<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &EnchantingTableWindow,
    xp: &XpState,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let bookshelf_count = enchanting_bookshelf_count(state, window.position);
    for (id, value) in
        enchanting_data_values(&state.items, &state.item_facts, window, xp, bookshelf_count)
    {
        write_packet(
            writer,
            &ClientboundContainerSetData {
                container_id: window.container_id,
                id,
                value,
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

fn apply_enchanting_swap_click(
    state: &mut InteractionState,
    window: &mut EnchantingTableWindow,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= ENCHANTING_MENU_SLOT_COUNT {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if enchanting_player_slot(menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = enchanting_menu_stack(window, &state.inventory, menu_slot) else {
        return false;
    };
    let swap = state.inventory.slots[player_slot].clone();
    let can_place_swap = can_place_in_enchanting_menu_slot(state, menu_slot, &swap);
    let can_place_clicked =
        can_place_in_player_slot(&state.item_facts, &state.items, player_slot, &clicked);
    let Some((new_clicked, new_swap)) =
        apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
    else {
        return false;
    };
    if !set_enchanting_menu_stack(window, &mut state.inventory, menu_slot, new_clicked) {
        return false;
    }
    state.inventory.slots[player_slot] = new_swap;
    true
}

fn apply_enchanting_throw_click(
    state: &mut InteractionState,
    window: &mut EnchantingTableWindow,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if menu_slot >= ENCHANTING_MENU_SLOT_COUNT {
        return None;
    }
    let (stack, dropped) = apply_regular_throw_slot(
        enchanting_menu_stack(window, &state.inventory, menu_slot)?,
        button,
    )?;
    set_enchanting_menu_stack(window, &mut state.inventory, menu_slot, stack).then_some(dropped)
}

fn apply_enchanting_pickup_click(
    state: &mut InteractionState,
    window: &mut EnchantingTableWindow,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= ENCHANTING_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = enchanting_menu_stack(window, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = pickup_click_max_stack(
        &state.item_facts,
        &state.items,
        &state.carried_item,
        &slot_stack,
    );
    let can_place_cursor = can_place_in_enchanting_menu_slot(state, menu_slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        &mut state.carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    set_enchanting_menu_stack(window, &mut state.inventory, menu_slot, new_slot)
}

fn merge_stack_into_enchanting_slot(
    state: &InteractionState,
    window: &mut EnchantingTableWindow,
    menu_slot: usize,
    stack: ItemStack,
) -> ItemStack {
    if stack.is_empty() || !can_place_in_enchanting_menu_slot(state, menu_slot, &stack) {
        return stack;
    }
    let target = &mut window.inputs[menu_slot];
    let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
    if target.is_empty() {
        let moved = stack.count.min(max_stack);
        let mut moved_stack = stack.clone();
        moved_stack.count = moved;
        *target = moved_stack;
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else if can_stack(target, &stack) && target.count < max_stack {
        let moved = (max_stack - target.count).min(stack.count);
        target.count += moved;
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else {
        stack
    }
}

fn apply_enchanting_quick_move_click(
    state: &mut InteractionState,
    window: &mut EnchantingTableWindow,
    menu_slot: usize,
) -> bool {
    if menu_slot >= ENCHANTING_MENU_SLOT_COUNT {
        return false;
    }
    match menu_slot {
        0..=1 => {
            let original = window.inputs[menu_slot].clone();
            if original.is_empty() {
                return false;
            }
            let max_stack = item_max_stack(&state.item_facts, &state.items, &original);
            let (remaining, _) = state.inventory.merge_stack(original.clone(), max_stack);
            window.inputs[menu_slot] = remaining.clone();
            remaining != original
        }
        _ => {
            let Some(player_slot) = enchanting_player_slot(menu_slot) else {
                return false;
            };
            let original = state.inventory.slots[player_slot].clone();
            if original.is_empty() {
                return false;
            }
            let target = if is_lapis_stack(&state.items, &original) {
                Some(1)
            } else if state
                .items
                .name_of(original.item_id)
                .and_then(|id| supported_enchantment_for_item(&state.item_facts, id))
                .is_some()
            {
                Some(0)
            } else {
                None
            };
            let Some(target) = target else {
                return false;
            };
            state.inventory.slots[player_slot] = ItemStack::EMPTY;
            let remaining =
                merge_stack_into_enchanting_slot(state, window, target, original.clone());
            state.inventory.slots[player_slot] = remaining;
            state.inventory.slots[player_slot] != original
        }
    }
}

async fn open_enchanting_table_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    xp: &XpState,
    player_pose: PlayerPose,
    sequence: i32,
    position: mc_world::BlockPos,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let clicked = state.world_read.get_cached_block(position);
    if !clicked.is_some_and(|block_state| is_enchanting_table_state(state, block_state)) {
        return Ok(false);
    }

    store_active_container(state, player_pose).await?;
    let window = EnchantingTableWindow::at_position(next_container_id(state), position);
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: ENCHANTING_MENU_TYPE_ID,
            title_nbt: enchanting_menu_title_nbt()?,
        },
        state.compression,
    )
    .await?;
    write_enchanting_content(state, writer, &window).await?;
    write_enchanting_data(state, writer, &window, xp).await?;
    state.active_container = Some(ActiveContainer::EnchantingTable(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn handle_enchanting_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: EnchantingTableWindow,
    xp: &XpState,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<EnchantingTableWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if packet.state_id != window.state_id {
        write_enchanting_content(state, writer, &window).await?;
        write_enchanting_data(state, writer, &window, xp).await?;
        return Ok(window);
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_window = window.clone();
    let mut dropped = None;
    let changed = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => {
            apply_enchanting_pickup_click(state, &mut window, slot, button)
        }
        ContainerClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click_with_carried(&mut state.carried_item, button);
            dropped.is_some()
        }
        ContainerClickAction::QuickMove { slot } => {
            apply_enchanting_quick_move_click(state, &mut window, slot)
        }
        ContainerClickAction::Swap { slot, button } => {
            apply_enchanting_swap_click(state, &mut window, slot, button)
        }
        ContainerClickAction::Throw { slot, button } => {
            if item_entity_type_id(&state.entity_types).is_some() {
                dropped = apply_enchanting_throw_click(state, &mut window, slot, button);
                dropped.is_some()
            } else {
                false
            }
        }
        ContainerClickAction::QuickCraft(_) | ContainerClickAction::Unsupported => false,
    };
    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        window = before_window;
    } else if changed
        && commit_enchanting_table_candidate(
            state,
            &mut window,
            &before_window.inputs,
            before_inventory,
            before_carried_item,
            dropped,
            player_pose,
        )
        .await?
    {
        window.state_id = window.state_id.wrapping_add(1);
    }
    write_enchanting_content(state, writer, &window).await?;
    write_enchanting_data(state, writer, &window, xp).await?;
    Ok(window)
}

fn furnace_wire_items(furnace: &FurnaceBlockEntity, inventory: &PlayerInventory) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(FURNACE_MENU_SLOT_COUNT);
    items.extend(furnace.slots.iter().map(furnace_slot_to_stack));
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

async fn load_chest_view(
    state: &InteractionState,
    window: &ChestWindow,
) -> Result<ChestView, ConnectionError> {
    #[cfg(test)]
    {
        let mut storage = state.world.lock().await;
        load_chest_view_from_storage(&mut storage, window)
    }
    #[cfg(not(test))]
    {
        state
            .simulation
            .read_chest_snapshot(window.positions.clone())
            .await
            .map(|snapshot| snapshot.view)
            .map_err(|error| {
                debug!(?error, ?window.positions, "simulation chest snapshot rejected");
                ConnectionError::RuntimeUnavailable {
                    operation: "reading chest state through simulation owner",
                }
            })
    }
}

#[cfg(test)]
fn load_chest_view_from_storage(
    storage: &mut mc_world::WorldStorage,
    window: &ChestWindow,
) -> Result<ChestView, ConnectionError> {
    let mut chests = Vec::with_capacity(window.positions.len());
    for &position in &window.positions {
        let chest = storage
            .chest_block_entity(position)
            .map_err(|err| {
                warn!(error = %err, ?position, "chest state read failed");
                err
            })?
            .unwrap_or_default();
        chests.push(chest);
    }
    Ok(ChestView { chests })
}

async fn load_chest_commit_snapshot(
    state: &InteractionState,
    window: &ChestWindow,
) -> Result<(ChestView, i32), ConnectionError> {
    #[cfg(test)]
    {
        let mut storage = state.world.lock().await;
        let view = load_chest_view_from_storage(&mut storage, window)?;
        let state_id = state.sessions.chest_state_id(window.position());
        Ok((view, state_id))
    }
    #[cfg(not(test))]
    {
        state
            .simulation
            .read_chest_snapshot(window.positions.clone())
            .await
            .map(|snapshot| (snapshot.view, snapshot.state_id))
            .map_err(|error| {
                debug!(?error, ?window.positions, "simulation chest snapshot rejected");
                ConnectionError::RuntimeUnavailable {
                    operation: "reading chest state through simulation owner",
                }
            })
    }
}

async fn write_furnace_data<W>(
    writer: &mut W,
    compression: Compression,
    window: &FurnaceWindow,
    furnace: &FurnaceBlockEntity,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for (id, value) in furnace_data_values(furnace) {
        write_packet(
            writer,
            &ClientboundContainerSetData {
                container_id: window.container_id,
                id,
                value,
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_furnace_data_changes<W>(
    writer: &mut W,
    compression: Compression,
    window: &FurnaceWindow,
    changed: &[(i16, i16)],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for &(id, value) in changed {
        write_packet(
            writer,
            &ClientboundContainerSetData {
                container_id: window.container_id,
                id,
                value,
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_container_slots<W>(
    writer: &mut W,
    compression: Compression,
    container_id: i32,
    state_id: i32,
    slots: impl IntoIterator<Item = ItemStack>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for (slot, item_stack) in slots.into_iter().enumerate() {
        write_packet(
            writer,
            &ClientboundContainerSetSlot {
                container_id,
                state_id,
                slot: slot as i16,
                item_stack,
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_furnace_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &FurnaceWindow,
    furnace: &FurnaceBlockEntity,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: furnace_wire_items(furnace, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn write_script_menu_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &ScriptMenuWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: window.wire_items(&state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn open_script_menu<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    request: ScriptMenuOpenRequest,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if request.player_id.value() != state.session_id {
        return Ok(());
    }
    let window = match ScriptMenuWindow::open(
        state.next_container_id,
        request.owner,
        request.player_id,
        request.menu,
        &state.items,
    ) {
        Ok(window) => window,
        Err(ScriptMenuOpenError::UnknownItem(item)) => {
            debug!(%item, "script menu open rejected unknown item");
            return Ok(());
        }
        Err(ScriptMenuOpenError::InvalidRows) => {
            debug!("script menu open rejected invalid row count");
            return Ok(());
        }
    };

    store_active_container(state, player_pose).await?;
    let allocated_id = next_container_id(state);
    debug_assert_eq!(allocated_id, window.container_id);
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: window.menu_type(),
            title_nbt: text_component_nbt(window.title())?,
        },
        state.compression,
    )
    .await?;
    write_script_menu_content(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::Script(window));
    Ok(())
}

async fn close_script_menu<W>(
    state: &mut InteractionState,
    writer: &mut W,
    request: ScriptMenuCloseRequest,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(active) = state.active_container.take() else {
        return Ok(());
    };
    match active {
        ActiveContainer::Script(window)
            if window.matches_close(&request.plugin_id, request.player_id, &request.menu_id) =>
        {
            write_packet(
                writer,
                &ClientboundContainerClose {
                    container_id: window.container_id,
                },
                state.compression,
            )
            .await?;
        }
        ActiveContainer::Script(window) => {
            write_script_menu_content(state, writer, &window).await?;
            state.active_container = Some(ActiveContainer::Script(window));
        }
        other => state.active_container = Some(other),
    }
    Ok(())
}

async fn write_chest_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &ChestWindow,
    view: &ChestView,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: chest_wire_items(view, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn open_chest_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    player_pose: PlayerPose,
    sequence: i32,
    position: mc_world::BlockPos,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (positions, title) = {
        let clicked = state.world_read.get_cached_block(position);
        let Some(clicked) = clicked else {
            return Ok(false);
        };
        let title = if is_chest_state(state, clicked) {
            "Chest"
        } else if is_barrel_state(state, clicked) {
            "Barrel"
        } else {
            return Ok(false);
        };
        let mut positions = vec![position];
        if title == "Chest" {
            for neighbour in adjacent_chest_positions(position) {
                let neighbour_state = state.world_read.get_cached_block(neighbour);
                if neighbour_state.is_some_and(|block_state| is_chest_state(state, block_state)) {
                    positions.push(neighbour);
                    break;
                }
            }
        }
        positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
        (positions, title)
    };
    if script_events.is_some_and(|events| {
        positions
            .iter()
            .any(|position| !events.block_mutation_allowed(*position))
    }) {
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(true);
    }

    store_active_container(state, player_pose).await?;
    let container_id = next_container_id(state);
    let mut window = ChestWindow::new(positions, container_id);
    window.state_id = state
        .sessions
        .register_chest_viewer(state.session_id, window.position());
    let (view, state_id) = load_chest_commit_snapshot(state, &window).await?;
    window.state_id = state_id;
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id,
            menu_type: window.menu_type(),
            title_nbt: chest_menu_title_nbt(title)?,
        },
        state.compression,
    )
    .await?;
    write_chest_content(state, writer, &window, &view).await?;
    state.active_container = Some(ActiveContainer::Chest(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn open_furnace_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = mc_world::BlockPos { x, y, z };
    let (title, kind) = {
        let clicked = state.world_read.get_cached_block(position);
        let Some(clicked) = clicked else {
            return Ok(false);
        };
        if !is_furnace_state(state, clicked) {
            return Ok(false);
        }
        let Some(title) = furnace_menu_title_for_state(state, clicked) else {
            return Ok(false);
        };
        let Some(kind) = furnace_kind_for_state(state, clicked) else {
            return Ok(false);
        };
        (title, kind)
    };

    store_active_container(state, player_pose).await?;
    let container_id = next_container_id(state);
    let mut window = FurnaceWindow::new(position, container_id, kind);
    window.state_id = state
        .sessions
        .register_furnace_viewer(state.session_id, window.position);
    let (furnace, state_id) = load_furnace_commit_snapshot(state, position).await?;
    window.state_id = state_id;
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id,
            menu_type: window.menu_type(),
            title_nbt: furnace_menu_title_nbt(title)?,
        },
        state.compression,
    )
    .await?;
    write_furnace_content(state, writer, &window, &furnace).await?;
    write_furnace_data(writer, state.compression, &window, &furnace).await?;
    state.active_container = Some(ActiveContainer::Furnace(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn reject_unsupported_survival_station_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = mc_world::BlockPos { x, y, z };
    let station = {
        let clicked = state
            .world_read
            .get_cached_block(position)
            .and_then(|block_state| unsupported_survival_station_for_state(state, block_state));
        let Some(station) = clicked else {
            return Ok(false);
        };
        station
    };

    debug!(
        station,
        x, y, z, "unsupported survival station use rejected safely"
    );
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn handle_place_recipe<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    player_pose: PlayerPose,
    game_mode: GameMode,
    survival_state: SurvivalState,
    packet: ServerboundPlaceRecipe,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    state.pending_use = None;
    clear_shield_use(state);
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            recipe = packet.recipe_display_id,
            "place recipe ignored for dead player"
        );
        return Ok(());
    }
    let Some(recipe) = packet
        .recipe_display_id
        .try_into()
        .ok()
        .and_then(|idx: usize| state.recipes.get(idx).cloned())
    else {
        debug!(
            recipe = packet.recipe_display_id,
            "place recipe ignored: unknown recipe display id"
        );
        return Ok(());
    };

    let (active_crafting, grid_width, grid_height) = if packet.container_id == 0 {
        (None, 2, 2)
    } else {
        match state.active_container.take() {
            Some(ActiveContainer::CraftingTable(window))
                if window.container_id == packet.container_id =>
            {
                (Some(window), 3, 3)
            }
            Some(active) => {
                debug!(
                    container_id = packet.container_id,
                    active_id = active.container_id(),
                    "place recipe ignored for inactive container"
                );
                state.active_container = Some(active);
                return Ok(());
            }
            None => {
                debug!(
                    container_id = packet.container_id,
                    "place recipe ignored: no active container"
                );
                return Ok(());
            }
        }
    };
    if !recipe_fits_grid(&recipe, grid_width, grid_height) {
        debug!(
            recipe = %recipe.id,
            container_id = packet.container_id,
            grid_width,
            grid_height,
            "place recipe ignored: recipe does not fit active crafting grid"
        );
        if let Some(window) = active_crafting {
            state.active_container = Some(ActiveContainer::CraftingTable(window));
        }
        return Ok(());
    }

    let expected_inventory = state.inventory.clone();
    let expected_carried_item = state.carried_item.clone();
    if let Some(outcome) = craft_recipe(state, &recipe, packet.use_max_items) {
        if commit_player_inventory_candidate(
            state,
            expected_inventory,
            expected_carried_item,
            None,
            player_pose,
        )
        .await?
        {
            if let Some(script_events) = script_events {
                let source = if packet.container_id == 0 {
                    ScriptCraftingSource::Inventory
                } else {
                    ScriptCraftingSource::CraftingTable
                };
                script_events
                    .publish_item_crafted(
                        &state.items,
                        outcome.crafted.item_id,
                        outcome.crafted.count,
                        outcome.crafted.craft_count,
                        source,
                        player_pose,
                        game_mode,
                    )
                    .await;
            }
            write_inventory_slot_updates(state, writer, outcome.changed_slots).await?;
        } else {
            write_inventory_content_resync(state, writer).await?;
        }
    } else {
        debug!(recipe = %recipe.id, "place recipe ignored: missing ingredients or output space");
    }
    if let Some(window) = active_crafting {
        write_crafting_content(state, writer, &window).await?;
        state.active_container = Some(ActiveContainer::CraftingTable(window));
    }
    Ok(())
}

async fn write_inventory_slot_updates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    changed: Vec<(usize, ItemStack)>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for (slot, item_stack) in changed {
        state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
        write_packet(
            writer,
            &ClientboundContainerSetSlot {
                container_id: 0,
                state_id: state.inventory_state_id,
                slot: slot as i16,
                item_stack,
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_inventory_content<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    write_inventory_content_with_state_id(state, writer, state.inventory_state_id).await
}

fn commit_session_owner_script_player_inventory(
    state: &mut InteractionState,
    transaction: &mc_script::ScriptPlayerInventoryTransaction,
) -> Result<(), mc_script::ScriptPlayerInventoryFailure> {
    let items = Arc::clone(&state.items);
    let item_facts = Arc::clone(&state.item_facts);
    let player_persistence = Arc::clone(&state.player_persistence);
    let mut persisted = player_persistence
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!(
                "player persistence mutex was poisoned during session-owner script inventory commit; recovering state"
            );
            poisoned.into_inner()
        });
    apply_script_player_inventory_transaction(
        transaction,
        &mut state.inventory,
        &mut persisted,
        &items,
        &item_facts,
    )
}

fn commit_session_owner_loader_item_grant(
    state: &mut InteractionState,
    stack: &ItemStack,
) -> Result<(), mc_script::ScriptPlayerInventoryFailure> {
    let items = Arc::clone(&state.items);
    let item_facts = Arc::clone(&state.item_facts);
    let player_persistence = Arc::clone(&state.player_persistence);
    let mut persisted = player_persistence.lock().unwrap_or_else(|poisoned| {
        warn!("player persistence mutex was poisoned during Loader item grant; recovering state");
        poisoned.into_inner()
    });
    apply_loader_item_grant(
        stack,
        &mut state.inventory,
        &mut persisted,
        &items,
        &item_facts,
    )
}

async fn write_inventory_content_resync<W>(
    state: &InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_inventory_content_with_state_id(state, writer, state.inventory_state_id).await
}

async fn write_inventory_content_with_state_id<W>(
    state: &InteractionState,
    writer: &mut W,
    state_id: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: 0,
            state_id,
            items: state.inventory.as_wire_list(),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerClickAction {
    Pickup { slot: usize, button: i8 },
    OutsidePickup { button: i8 },
    QuickMove { slot: usize },
    Swap { slot: usize, button: i8 },
    Throw { slot: usize, button: i8 },
    QuickCraft(QuickCraftClick),
    Unsupported,
}

#[cfg(test)]
fn apply_chest_quick_move_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
) -> bool {
    chest_apply_quick_move_click(
        &state.items,
        &state.item_facts,
        view,
        &mut state.inventory,
        menu_slot,
    )
}

#[cfg(test)]
fn apply_chest_swap_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
    button: i8,
) -> bool {
    chest_apply_swap_click(view, &mut state.inventory, menu_slot, button)
}

#[cfg(test)]
fn apply_chest_throw_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    chest_apply_throw_click(view, &mut state.inventory, menu_slot, button)
}

fn classify_container_click(packet: &ServerboundContainerClick) -> ContainerClickAction {
    if packet.container_input == ContainerInput::QuickCraft {
        return ContainerClickAction::QuickCraft(QuickCraftClick {
            header: (i32::from(packet.button_num) & 3) as i8,
            kind: ((i32::from(packet.button_num) >> 2) & 3) as i8,
            slot: usize::try_from(packet.slot_num).ok(),
        });
    }
    if packet.slot_num == -999 {
        return match packet.container_input {
            ContainerInput::Pickup => ContainerClickAction::OutsidePickup {
                button: packet.button_num,
            },
            _ => ContainerClickAction::Unsupported,
        };
    }
    if packet.slot_num < 0 {
        return ContainerClickAction::Unsupported;
    }
    let Ok(slot) = usize::try_from(packet.slot_num) else {
        return ContainerClickAction::Unsupported;
    };
    match packet.container_input {
        ContainerInput::Pickup => ContainerClickAction::Pickup {
            slot,
            button: packet.button_num,
        },
        ContainerInput::QuickMove if packet.button_num == 0 => {
            ContainerClickAction::QuickMove { slot }
        }
        ContainerInput::Swap => ContainerClickAction::Swap {
            slot,
            button: packet.button_num,
        },
        ContainerInput::Throw => ContainerClickAction::Throw {
            slot,
            button: packet.button_num,
        },
        _ => ContainerClickAction::Unsupported,
    }
}

fn client_carried_item_matches(client: &HashedStack, server: &ItemStack) -> bool {
    match client {
        // Older protocol harnesses often omit the predicted post-click carried
        // item. Treat an empty client prediction as non-authoritative and let
        // the following resync carry Solaris' cursor state back to the client.
        HashedStack::Empty => true,
        HashedStack::Actual {
            item_id,
            count,
            components: _,
        } if !server.is_empty() => *item_id == server.item_id && *count == server.count,
        HashedStack::Actual { .. } => false,
    }
}

const PLAYER_SELECTED_DROP_FORWARD_OFFSET: f64 = 2.1;

async fn commit_crafting_table_candidate(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    expected_input: &[ItemStack; 9],
    expected_inventory: PlayerInventory,
    expected_carried_item: ItemStack,
    dropped: Option<ItemStack>,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError> {
    let drops = if let Some(stack) = dropped {
        let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            window.input = expected_input.clone();
            refresh_crafting_result_with_data(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                window,
            );
            return Ok(false);
        };
        vec![ContainerDropPlan {
            entity_type_id,
            position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
            stack: entity_item_stack(stack),
        }]
    } else {
        Vec::new()
    };
    let plan = ContainerPlayerPlan {
        expected_inventory: expected_inventory.clone(),
        expected_carried_item: expected_carried_item.clone(),
        updated_inventory: state.inventory.clone(),
        updated_carried_item: state.carried_item.clone(),
        crafting_table_input: Some(CraftingTableInputPlan {
            expected: crafting_table_input_projection(expected_input),
            updated: crafting_table_input_projection(&window.input),
        }),
        enchanting_table_input: None,
        merchant_input: None,
        drops,
        xp_orb: None,
    };
    let outcome = match state.simulation.commit_player_inventory(plan).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            window.input = expected_input.clone();
            refresh_crafting_result_with_data(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                window,
            );
            debug!(?error, "simulation crafting table request rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing crafting table input",
            });
        }
    };
    match outcome {
        PlayerInventoryCommitOutcome::Committed {
            inventory,
            carried_item,
            crafting_table_input,
            enchanting_table_input: _,
            merchant_input: _,
            dispatches: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window.input = crafting_table_input_from_projection(crafting_table_input);
            refresh_crafting_result_with_data(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                window,
            );
            Ok(true)
        }
        PlayerInventoryCommitOutcome::Rejected {
            inventory,
            carried_item,
            crafting_table_input,
            enchanting_table_input: _,
            merchant_input: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window.input = crafting_table_input_from_projection(crafting_table_input);
            refresh_crafting_result_with_data(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                window,
            );
            Ok(false)
        }
    }
}

async fn commit_stonecutter_candidate(
    state: &mut InteractionState,
    window: &mut StonecutterWindow,
    expected_input: &ItemStack,
    expected_inventory: PlayerInventory,
    expected_carried_item: ItemStack,
) -> Result<bool, ConnectionError> {
    let plan = ContainerPlayerPlan {
        expected_inventory: expected_inventory.clone(),
        expected_carried_item: expected_carried_item.clone(),
        updated_inventory: state.inventory.clone(),
        updated_carried_item: state.carried_item.clone(),
        // The simulation owner already fences one transient crafting projection.
        // A stonecutter uses only projection slot 0 while this window is active.
        crafting_table_input: Some(CraftingTableInputPlan {
            expected: stonecutter_input_projection(expected_input),
            updated: stonecutter_input_projection(&window.input),
        }),
        enchanting_table_input: None,
        merchant_input: None,
        drops: Vec::new(),
        xp_orb: None,
    };
    let outcome = match state.simulation.commit_player_inventory(plan).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            set_stonecutter_input_with_data(
                &state.recipes,
                &state.items,
                &state.item_facts,
                &state.tags,
                window,
                expected_input.clone(),
            );
            debug!(?error, "simulation stonecutter request rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing stonecutter input",
            });
        }
    };
    match outcome {
        PlayerInventoryCommitOutcome::Committed {
            inventory,
            carried_item,
            crafting_table_input,
            enchanting_table_input: _,
            merchant_input: _,
            dispatches: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            let input = stonecutter_input_from_projection(crafting_table_input);
            set_stonecutter_input_with_data(
                &state.recipes,
                &state.items,
                &state.item_facts,
                &state.tags,
                window,
                input,
            );
            Ok(true)
        }
        PlayerInventoryCommitOutcome::Rejected {
            inventory,
            carried_item,
            crafting_table_input,
            enchanting_table_input: _,
            merchant_input: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            let input = stonecutter_input_from_projection(crafting_table_input);
            set_stonecutter_input_with_data(
                &state.recipes,
                &state.items,
                &state.item_facts,
                &state.tags,
                window,
                input,
            );
            Ok(false)
        }
    }
}

async fn commit_enchanting_table_candidate(
    state: &mut InteractionState,
    window: &mut EnchantingTableWindow,
    expected_input: &[ItemStack; 2],
    expected_inventory: PlayerInventory,
    expected_carried_item: ItemStack,
    dropped: Option<ItemStack>,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError> {
    let drops = if let Some(stack) = dropped {
        let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            window.inputs = expected_input.clone();
            return Ok(false);
        };
        vec![ContainerDropPlan {
            entity_type_id,
            position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
            stack: entity_item_stack(stack),
        }]
    } else {
        Vec::new()
    };
    let plan = ContainerPlayerPlan {
        expected_inventory: expected_inventory.clone(),
        expected_carried_item: expected_carried_item.clone(),
        updated_inventory: state.inventory.clone(),
        updated_carried_item: state.carried_item.clone(),
        crafting_table_input: None,
        enchanting_table_input: Some(EnchantingTableInputPlan {
            expected: enchanting_table_input_projection(expected_input),
            updated: enchanting_table_input_projection(&window.inputs),
        }),
        merchant_input: None,
        drops,
        xp_orb: None,
    };
    let outcome = match state.simulation.commit_player_inventory(plan).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            window.inputs = expected_input.clone();
            debug!(?error, "simulation enchanting table request rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing enchanting table input",
            });
        }
    };
    match outcome {
        PlayerInventoryCommitOutcome::Committed {
            inventory,
            carried_item,
            crafting_table_input: _,
            enchanting_table_input,
            merchant_input: _,
            dispatches: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window.inputs = enchanting_table_input_from_projection(enchanting_table_input);
            Ok(true)
        }
        PlayerInventoryCommitOutcome::Rejected {
            inventory,
            carried_item,
            crafting_table_input: _,
            enchanting_table_input,
            merchant_input: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            window.inputs = enchanting_table_input_from_projection(enchanting_table_input);
            Ok(false)
        }
    }
}

async fn commit_player_inventory_candidate(
    state: &mut InteractionState,
    expected_inventory: PlayerInventory,
    expected_carried_item: ItemStack,
    dropped: Option<ItemStack>,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError> {
    let drops = if let Some(stack) = dropped {
        let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            return Ok(false);
        };
        vec![ContainerDropPlan {
            entity_type_id,
            position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
            stack: entity_item_stack(stack),
        }]
    } else {
        Vec::new()
    };
    let plan = ContainerPlayerPlan {
        expected_inventory: expected_inventory.clone(),
        expected_carried_item: expected_carried_item.clone(),
        updated_inventory: state.inventory.clone(),
        updated_carried_item: state.carried_item.clone(),
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops,
        xp_orb: None,
    };
    let outcome = match state.simulation.commit_player_inventory(plan).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.inventory = expected_inventory;
            state.carried_item = expected_carried_item;
            debug!(?error, "simulation player inventory request rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing player inventory",
            });
        }
    };
    match outcome {
        PlayerInventoryCommitOutcome::Committed {
            inventory,
            carried_item,
            crafting_table_input: _,
            enchanting_table_input: _,
            merchant_input: _,
            dispatches: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            Ok(true)
        }
        PlayerInventoryCommitOutcome::Rejected {
            inventory,
            carried_item,
            crafting_table_input: _,
            enchanting_table_input: _,
            merchant_input: _,
        } => {
            state.inventory = inventory;
            state.carried_item = carried_item;
            Ok(false)
        }
    }
}

struct InventoryReturnPlan<'a> {
    enchanting_table_input: Option<&'a [ItemStack; 2]>,
    merchant_input: Option<&'a [ItemStack; 2]>,
    crafting_table_input: Option<&'a [ItemStack; 9]>,
    return_crafting_table_input: bool,
    return_inventory_crafting_inputs: bool,
    return_cursor: bool,
    player_pose: PlayerPose,
}

impl<'a> InventoryReturnPlan<'a> {
    #[cfg(test)]
    fn cursor(player_pose: PlayerPose) -> Self {
        Self {
            enchanting_table_input: None,
            merchant_input: None,
            crafting_table_input: None,
            return_crafting_table_input: false,
            return_inventory_crafting_inputs: false,
            return_cursor: true,
            player_pose,
        }
    }

    fn disconnect(
        enchanting_table_input: Option<&'a [ItemStack; 2]>,
        merchant_input: Option<&'a [ItemStack; 2]>,
        crafting_table_input: Option<&'a [ItemStack; 9]>,
        return_crafting_table_input: bool,
        player_pose: PlayerPose,
    ) -> Self {
        Self {
            enchanting_table_input,
            merchant_input,
            crafting_table_input,
            return_crafting_table_input,
            return_inventory_crafting_inputs: true,
            return_cursor: true,
            player_pose,
        }
    }

    fn container(
        enchanting_table_input: Option<&'a [ItemStack; 2]>,
        merchant_input: Option<&'a [ItemStack; 2]>,
        crafting_table_input: Option<&'a [ItemStack; 9]>,
        return_crafting_table_input: bool,
        player_pose: PlayerPose,
    ) -> Self {
        Self {
            enchanting_table_input,
            merchant_input,
            crafting_table_input,
            return_crafting_table_input,
            return_inventory_crafting_inputs: false,
            return_cursor: false,
            player_pose,
        }
    }
}

async fn settle_player_inventory_returns(
    state: &mut InteractionState,
    plan: InventoryReturnPlan<'_>,
) -> Result<(), ConnectionError> {
    let InventoryReturnPlan {
        enchanting_table_input,
        merchant_input,
        crafting_table_input,
        return_crafting_table_input,
        return_inventory_crafting_inputs,
        return_cursor,
        player_pose,
    } = plan;
    let return_enchanting_table_input = enchanting_table_input.is_some();
    let return_merchant_input = merchant_input.is_some();
    let has_inventory_crafting_inputs = return_inventory_crafting_inputs
        && state.inventory.slots[1..=4]
            .iter()
            .any(|stack| !stack.is_empty());
    let has_cursor = return_cursor && !state.carried_item.is_empty();
    if !return_crafting_table_input
        && !return_enchanting_table_input
        && !return_merchant_input
        && !has_inventory_crafting_inputs
        && !has_cursor
    {
        if return_inventory_crafting_inputs {
            state.inventory.slots[0] = ItemStack::EMPTY;
        }
        return Ok(());
    }

    let mut authoritative_crafting_table_input =
        crafting_table_input.and_then(crafting_table_input_projection);
    let mut authoritative_enchanting_table_input =
        enchanting_table_input.and_then(enchanting_table_input_projection);
    let mut authoritative_merchant_input = merchant_input.and_then(merchant_input_projection);
    loop {
        let expected_inventory = state.inventory.clone();
        let expected_carried_item = state.carried_item.clone();
        let mut updated_inventory = expected_inventory.clone();
        let mut updated_carried_item = expected_carried_item.clone();
        let mut returned = Vec::new();

        if return_enchanting_table_input
            && let Some(input) = authoritative_enchanting_table_input.as_deref()
        {
            returned.extend(input.iter().cloned());
        }

        if return_merchant_input && let Some(input) = authoritative_merchant_input.as_deref() {
            returned.extend(input.iter().cloned());
        }

        if return_crafting_table_input
            && let Some(input) = authoritative_crafting_table_input.as_deref()
        {
            returned.extend(input.iter().cloned());
        }

        if return_inventory_crafting_inputs {
            updated_inventory.slots[0] = ItemStack::EMPTY;
            for slot in 1..=4 {
                returned.push(std::mem::take(&mut updated_inventory.slots[slot]));
            }
        }
        if return_cursor {
            returned.push(std::mem::take(&mut updated_carried_item));
        }

        let mut overflow = Vec::new();
        for stack in returned {
            if stack.is_empty() {
                continue;
            }
            let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
            let (remaining, _) = updated_inventory.merge_stack(stack, max_stack);
            if !remaining.is_empty() {
                overflow.push(remaining);
            }
        }

        let entity_type_id = if overflow.is_empty() {
            None
        } else {
            Some(item_entity_type_id(&state.entity_types).ok_or(
                ConnectionError::RuntimeUnavailable {
                    operation: "settling crafting overflow without item entity type",
                },
            )?)
        };
        let drops = overflow
            .into_iter()
            .map(|stack| ContainerDropPlan {
                entity_type_id: entity_type_id.expect("overflow has an item entity type"),
                position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
                stack: entity_item_stack(stack),
            })
            .collect::<Vec<_>>();
        debug_assert!(drops.len() <= MAX_CONTAINER_PLAYER_DROPS);

        let plan = ContainerPlayerPlan {
            expected_inventory,
            expected_carried_item,
            updated_inventory,
            updated_carried_item,
            crafting_table_input: return_crafting_table_input.then(|| CraftingTableInputPlan {
                expected: authoritative_crafting_table_input.clone(),
                updated: None,
            }),
            enchanting_table_input: return_enchanting_table_input.then(|| {
                EnchantingTableInputPlan {
                    expected: authoritative_enchanting_table_input.clone(),
                    updated: None,
                }
            }),
            merchant_input: return_merchant_input.then(|| MerchantInputPlan {
                expected: authoritative_merchant_input.clone(),
                updated: None,
            }),
            drops,
            xp_orb: None,
        };
        match state.simulation.commit_player_inventory(plan).await {
            Ok(PlayerInventoryCommitOutcome::Committed {
                inventory,
                carried_item,
                crafting_table_input: _,
                enchanting_table_input: _,
                merchant_input: _,
                dispatches: _,
            }) => {
                state.inventory = inventory;
                state.carried_item = carried_item;
                return Ok(());
            }
            Ok(PlayerInventoryCommitOutcome::Rejected {
                inventory,
                carried_item,
                crafting_table_input,
                enchanting_table_input,
                merchant_input,
            }) => {
                state.inventory = inventory;
                state.carried_item = carried_item;
                if return_crafting_table_input {
                    authoritative_crafting_table_input = crafting_table_input;
                }
                if return_enchanting_table_input {
                    authoritative_enchanting_table_input = enchanting_table_input;
                }
                if return_merchant_input {
                    authoritative_merchant_input = merchant_input;
                }
            }
            Err(error) => {
                debug!(?error, "simulation inventory settlement rejected");
                return Err(ConnectionError::RuntimeUnavailable {
                    operation: "settling player inventory",
                });
            }
        }
    }
}

async fn settle_recovered_player_inventory(
    state: &mut InteractionState,
    recovered: &PlayerPersistedState,
) -> Result<(), ConnectionError> {
    settle_player_inventory_returns(
        state,
        InventoryReturnPlan {
            enchanting_table_input: recovered.enchanting_table_input.as_deref(),
            merchant_input: recovered.merchant_input.as_deref(),
            crafting_table_input: recovered.crafting_table_input.as_deref(),
            return_crafting_table_input: recovered.crafting_table_input.is_some(),
            return_inventory_crafting_inputs: true,
            return_cursor: true,
            player_pose: recovered.pose,
        },
    )
    .await
}

fn hand_inventory_slot(state: &InteractionState, hand: InteractionHand) -> usize {
    match hand {
        InteractionHand::MainHand => {
            PlayerInventory::HOTBAR_BASE + state.selected_hotbar_slot as usize
        }
        InteractionHand::OffHand => 45,
    }
}

struct FurnaceTickPlan {
    position: mc_world::BlockPos,
    block_state: BlockStateId,
    after_block_state: BlockStateId,
    kind: FurnaceKind,
    before: FurnaceBlockEntity,
    after: FurnaceBlockEntity,
    slots_changed: bool,
    data_changed: Vec<(i16, i16)>,
}

type FurnaceViewerUpdate = (
    mc_world::BlockPos,
    FurnaceBlockEntity,
    bool,
    Vec<(i16, i16)>,
    Option<(BlockStateId, BlockStateId)>,
);

fn furnace_tick_block_state(
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
    furnace: &FurnaceBlockEntity,
) -> BlockStateId {
    blocks
        .by_id(block_state)
        .and_then(|state| {
            sibling_state_with_bool_property(blocks, state, "lit", furnace.burn_remaining > 0)
        })
        .unwrap_or(block_state)
}

fn replan_resident_furnace_tick(
    config: &ServerConfig,
    mutation: &mc_world::WorldMutationView,
    position: mc_world::BlockPos,
) -> Option<FurnaceTickPlan> {
    let (block_state, before) = mutation.furnace_tick_snapshot(position)?;
    let kind = config
        .blocks
        .by_id(block_state)
        .and_then(|state| furnace_kind_for_block_id(state.block.id.as_str()))?;
    let tick = tick_furnace_rules(&config.recipes, &config.items, &config.tags, &before, kind);
    let after_block_state = furnace_tick_block_state(&config.blocks, block_state, &tick.furnace);
    (tick.slots_changed || !tick.data_changed.is_empty() || after_block_state != block_state)
        .then_some(FurnaceTickPlan {
            position,
            block_state,
            after_block_state,
            kind,
            before,
            after: tick.furnace,
            slots_changed: tick.slots_changed,
            data_changed: tick.data_changed,
        })
}

fn commit_resident_furnace_tick_wave(
    config: &ServerConfig,
    _sessions: &SessionRegistry,
    mutation: &mc_world::WorldMutationView,
    mut pending: Vec<FurnaceTickPlan>,
) -> Vec<FurnaceViewerUpdate> {
    let mut updates = Vec::new();
    while !pending.is_empty() {
        let mut retry = Vec::new();
        for plan in pending {
            match mutation.commit_furnace_tick_conditionally(
                plan.position,
                plan.block_state,
                plan.after_block_state,
                &plan.before,
                &plan.after,
            ) {
                mc_world::ResidentFurnaceTickCommitResult::Applied => {
                    updates.push((
                        plan.position,
                        plan.after,
                        plan.slots_changed,
                        plan.data_changed,
                        (plan.block_state != plan.after_block_state)
                            .then_some((plan.block_state, plan.after_block_state)),
                    ));
                    #[cfg(test)]
                    _sessions.pause_after_server_furnace_commit_for_test();
                }
                mc_world::ResidentFurnaceTickCommitResult::Missing => {}
                mc_world::ResidentFurnaceTickCommitResult::Stale => {
                    if let Some(plan) =
                        replan_resident_furnace_tick(config, mutation, plan.position)
                    {
                        retry.push(plan);
                    }
                }
            }
        }
        pending = retry;
    }
    updates
}

async fn run_furnace_ticks_owned(
    _authority: &simulation::SimulationAuthority,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
) -> usize {
    let Some(world) = config.world.as_ref() else {
        return 0;
    };
    let loaded_chunks = sessions.loaded_chunks_sorted();
    if loaded_chunks.is_empty() {
        return 0;
    }
    let owned_world_read = if world_read.is_none() {
        Some(world.lock().await.read_view())
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_read.as_ref())
        .expect("world read view is available");
    let loaded_chunk_positions = loaded_chunks
        .into_iter()
        .map(|(x, z)| ChunkPos { x, z })
        .collect::<Vec<_>>();
    let mut furnace_snapshots = world_read.furnace_snapshots(&loaded_chunk_positions);
    furnace_snapshots.sort_unstable_by_key(|(position, _)| (position.x, position.y, position.z));
    furnace_snapshots.dedup_by_key(|(position, _)| *position);

    let mut plans = Vec::new();
    for (position, before) in furnace_snapshots {
        let Some(block_state) = world_read.get_cached_block(position) else {
            continue;
        };
        let Some(kind) = config
            .blocks
            .by_id(block_state)
            .and_then(|state| furnace_kind_for_block_id(state.block.id.as_str()))
        else {
            continue;
        };
        let tick = tick_furnace_rules(&config.recipes, &config.items, &config.tags, &before, kind);
        let after_block_state =
            furnace_tick_block_state(&config.blocks, block_state, &tick.furnace);
        if tick.slots_changed || !tick.data_changed.is_empty() || after_block_state != block_state {
            plans.push(FurnaceTickPlan {
                position,
                block_state,
                after_block_state,
                kind,
                before,
                after: tick.furnace,
                slots_changed: tick.slots_changed,
                data_changed: tick.data_changed,
            });
        }
    }
    if plans.is_empty() {
        return 0;
    }

    let updates = if let Some(mutation) = world_mutation {
        commit_resident_furnace_tick_wave(config, sessions, mutation, plans)
    } else {
        let mut updates = Vec::new();
        for plan in plans {
            let mut storage = world.lock().await;
            let Some(block_state) = storage.get_cached_block(plan.position) else {
                continue;
            };
            let Some(current_kind) = config
                .blocks
                .by_id(block_state)
                .and_then(|state| furnace_kind_for_block_id(state.block.id.as_str()))
            else {
                continue;
            };
            let current = match storage.furnace_block_entity(plan.position) {
                Ok(Some(furnace)) => furnace,
                Ok(None) => FurnaceBlockEntity::default(),
                Err(error) => {
                    warn!(%error, position = ?plan.position, "furnace tick read failed");
                    continue;
                }
            };
            let (furnace, slots_changed, data_changed, after_block_state) =
                if current_kind == plan.kind && current == plan.before {
                    (
                        plan.after,
                        plan.slots_changed,
                        plan.data_changed,
                        plan.after_block_state,
                    )
                } else {
                    let tick = tick_furnace_rules(
                        &config.recipes,
                        &config.items,
                        &config.tags,
                        &current,
                        current_kind,
                    );
                    let after_block_state =
                        furnace_tick_block_state(&config.blocks, block_state, &tick.furnace);
                    if !tick.slots_changed
                        && tick.data_changed.is_empty()
                        && after_block_state == block_state
                    {
                        continue;
                    }
                    (
                        tick.furnace,
                        tick.slots_changed,
                        tick.data_changed,
                        after_block_state,
                    )
                };
            let block_change =
                (block_state != after_block_state).then_some((block_state, after_block_state));
            let mutation = storage.mutation_view();
            let update = match mutation.commit_furnace_tick_conditionally(
                plan.position,
                block_state,
                after_block_state,
                &current,
                &furnace,
            ) {
                mc_world::ResidentFurnaceTickCommitResult::Applied => Some((
                    plan.position,
                    furnace,
                    slots_changed,
                    data_changed,
                    block_change,
                )),
                mc_world::ResidentFurnaceTickCommitResult::Missing
                | mc_world::ResidentFurnaceTickCommitResult::Stale => None,
            };
            if let Some(update) = update {
                updates.push(update);
                #[cfg(test)]
                sessions.pause_after_server_furnace_commit_for_test();
            }
        }
        updates
    };

    let updated = updates.len();
    let mut dispatches = Vec::new();
    let mut lit_outcome = BlockEditBatchOutcome::default();
    let light_table = config.block_light.as_deref();
    for (position, furnace, slots_changed, data_changed, block_change) in updates {
        if let Some((previous, new_state)) = block_change {
            lit_outcome.applied.push(AppliedBlockEdit {
                pos: position,
                previous,
                new_state,
            });
            lit_outcome.deltas.push(BlockDelta {
                x: position.x,
                y: position.y,
                z: position.z,
                state_id: new_state,
            });
            let chunk = (position.x.div_euclid(16), position.z.div_euclid(16));
            lit_outcome.edit_chunks.insert(chunk);
            if light_table.is_some_and(|table| block_edit_changes_light(table, previous, new_state))
            {
                lit_outcome.light_edit_chunks.insert(chunk);
            }
        }
        if slots_changed {
            dispatches.extend(
                sessions
                    .server_furnace_slot_dispatches(position, furnace_slot_stacks(&furnace))
                    .1,
            );
        }
        if !data_changed.is_empty() {
            dispatches.extend(sessions.server_furnace_data_dispatches(position, data_changed));
        }
    }
    if !lit_outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&lit_outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(
            sessions,
            &lit_outcome.edit_chunks,
            &lit_outcome.deltas,
            None,
        );
        if let Some(table) = light_table
            && !lit_outcome.light_edit_chunks.is_empty()
        {
            let light_updates =
                collect_server_origin_light_updates(world, sessions, table, &lit_outcome).await;
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
        }
    }
    dispatch_visibility_commands(dispatches);
    updated
}

#[cfg(test)]
fn take_death_inventory_drops(
    inventory: &mut PlayerInventory,
    carried_item: &mut ItemStack,
) -> Vec<ItemStack> {
    let mut drops = Vec::new();
    for slot in 1..inventory.slots.len() {
        let stack = std::mem::take(&mut inventory.slots[slot]);
        if !stack.is_empty() {
            drops.push(stack);
        }
    }
    let carried = std::mem::take(carried_item);
    if !carried.is_empty() {
        drops.push(carried);
    }
    drops
}

#[allow(clippy::too_many_arguments)]
async fn commit_player_survival_update<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    expected_inventory: PlayerInventory,
    updated_survival: SurvivalState,
    updated_xp: XpState,
    enchanting_table_input: Option<EnchantingTableInputPlan>,
    write_health: bool,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    Ok(matches!(
        commit_player_survival_update_with_shield(
            state,
            writer,
            survival_state,
            xp_state,
            expected_inventory,
            updated_survival,
            updated_xp,
            None,
            enchanting_table_input,
            write_health,
            player_pose,
        )
        .await?,
        PlayerSurvivalUpdateOutcome::Committed
    ))
}

enum PlayerSurvivalUpdateOutcome {
    Committed,
    Rejected,
}

#[allow(clippy::too_many_arguments)]
async fn commit_player_survival_update_with_shield<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    expected_inventory: PlayerInventory,
    updated_survival: SurvivalState,
    updated_xp: XpState,
    active_shield: Option<ActiveShieldTransition>,
    enchanting_table_input: Option<EnchantingTableInputPlan>,
    write_health: bool,
    player_pose: PlayerPose,
) -> Result<PlayerSurvivalUpdateOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let expected_survival = *survival_state;
    let expected_xp = xp_state.clone();
    let shield_transition_requested = active_shield.is_some();
    let committed = match state
        .simulation
        .commit_player_survival(PlayerSurvivalPlan {
            expected_survival,
            updated_survival,
            expected_inventory: expected_inventory.clone(),
            updated_inventory: state.inventory.clone(),
            expected_carried_item: state.carried_item.clone(),
            expected_xp: expected_xp.clone(),
            updated_xp,
            active_shield,
            enchanting_table_input,
            item_entity_type_id: item_entity_type_id(&state.entity_types),
            xp_orb_entity_type_id: xp_orb_entity_type_id(&state.entity_types),
            keep_inventory: state.sessions.keep_inventory(),
            position: Vec3::new(player_pose.x, player_pose.y, player_pose.z),
        })
        .await
    {
        Ok(Some(PlayerSurvivalCommitOutcome::Committed(committed))) => committed,
        Ok(Some(PlayerSurvivalCommitOutcome::Rejected(authoritative))) => {
            if shield_transition_requested {
                restore_authoritative_shield_state(state, authoritative);
            } else {
                state.inventory = expected_inventory;
                refresh_shield_use_state(state);
            }
            debug!("player survival transition rejected because owner state changed");
            return Ok(PlayerSurvivalUpdateOutcome::Rejected);
        }
        Ok(None) => {
            state.inventory = expected_inventory;
            refresh_shield_use_state(state);
            debug!("player survival transition rejected because owner state changed");
            return Ok(PlayerSurvivalUpdateOutcome::Rejected);
        }
        Err(error) => {
            state.inventory = expected_inventory;
            refresh_shield_use_state(state);
            debug!(?error, "simulation player survival request rejected");
            return Ok(PlayerSurvivalUpdateOutcome::Rejected);
        }
    };

    if let Some(bed) = state.sessions.sleeping_bed(state.session_id) {
        match release_staged_sleep_bed(state, writer, bed).await? {
            Some(SleepBedRelease::Completed) | None => {}
            Some(SleepBedRelease::Rejected { .. }) => {
                return Err(ConnectionError::RuntimeUnavailable {
                    operation: "releasing bed before damage publication",
                });
            }
        }
    }

    let changed_slots = expected_inventory
        .slots
        .iter()
        .zip(&committed.inventory.slots)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(slot, (_, after))| (slot, after.clone()))
        .collect::<Vec<_>>();
    let survival_changed = *survival_state != committed.survival;
    let xp_changed = *xp_state != committed.xp;
    *survival_state = committed.survival;
    *xp_state = committed.xp;
    state.inventory = committed.inventory;
    state.carried_item = committed.carried_item;

    if committed.died {
        state.pending_break = None;
        state.pending_use = None;
        clear_shield_use(state);
    }
    if survival_changed && write_health {
        write_packet(writer, &survival_state.as_packet(), state.compression).await?;
    }
    if committed.died {
        write_inventory_content(state, writer).await?;
    } else if !changed_slots.is_empty() {
        write_inventory_slot_updates(state, writer, changed_slots).await?;
    }
    if xp_changed {
        write_packet(writer, &xp_state.as_packet(), state.compression).await?;
    }
    Ok(PlayerSurvivalUpdateOutcome::Committed)
}

fn recoverable_death_xp(xp_state: &XpState) -> i32 {
    xp_state.level.saturating_mul(7).clamp(0, 100)
}

async fn commit_chest_click(
    state: &InteractionState,
    window: &ChestWindow,
    expected: &ChestView,
    updated: &ChestView,
    player: ContainerPlayerPlan,
) -> Result<ChestCommitOutcome, ConnectionError> {
    #[cfg(test)]
    {
        let state_id_increment = chest_menu_state_change_count(
            expected,
            updated,
            &player.expected_inventory,
            &player.updated_inventory,
            &player.expected_carried_item,
            &player.updated_carried_item,
        );
        let mut storage = state.world.lock().await;
        let mut authoritative = Vec::with_capacity(window.positions.len());
        for &position in &window.positions {
            authoritative.push(
                storage
                    .chest_block_entity(position)
                    .map_err(|err| {
                        warn!(error = %err, ?position, "chest state read failed");
                        err
                    })?
                    .unwrap_or_default(),
            );
        }
        if authoritative != expected.chests {
            return Ok(SharedContainerCommit::Rejected {
                state_id: state.sessions.chest_state_id(window.position()),
                authoritative,
                inventory: player.expected_inventory,
                carried_item: player.expected_carried_item,
            });
        }
        let (state_id, dispatches) = match state.sessions.try_chest_slot_dispatches(
            window.position(),
            window.state_id,
            state_id_increment,
            state.session_id,
            chest_slot_stacks(updated),
        ) {
            Ok(committed) => committed,
            Err(state_id) => {
                return Ok(SharedContainerCommit::Rejected {
                    state_id,
                    authoritative,
                    inventory: player.expected_inventory,
                    carried_item: player.expected_carried_item,
                });
            }
        };
        for (&position, chest) in window.positions.iter().zip(&updated.chests) {
            storage
                .set_chest_block_entity(position, chest.clone())
                .map_err(|err| {
                    warn!(error = %err, ?position, "chest state write failed");
                    err
                })?;
        }
        let mut dispatches = dispatches;
        for drop in &player.drops {
            dispatches.extend(state.sessions.spawn_item_drop(
                drop.entity_type_id,
                drop.position,
                drop.stack.clone(),
            ));
        }
        Ok(SharedContainerCommit::Committed {
            state_id,
            inventory: player.updated_inventory,
            carried_item: player.updated_carried_item,
            dispatches,
        })
    }

    #[cfg(not(test))]
    {
        match state
            .simulation
            .commit_chest(
                window.position(),
                window.positions.clone(),
                window.state_id,
                expected.chests.clone(),
                updated.chests.clone(),
                player.clone(),
            )
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                debug!(?error, ?window.positions, "simulation chest commit rejected");
                let (authoritative, state_id) = load_chest_commit_snapshot(state, window).await?;
                let (inventory, carried_item) = state
                    .sessions
                    .player_container_state(state.session_id)
                    .unwrap_or((player.expected_inventory, player.expected_carried_item));
                Ok(SharedContainerCommit::Rejected {
                    state_id,
                    authoritative: authoritative.chests,
                    inventory,
                    carried_item,
                })
            }
        }
    }
}

async fn handle_chest_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: ChestWindow,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<ChestWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (mut view, authoritative_state_id) = load_chest_commit_snapshot(state, &window).await?;
    let mut dispatches = Vec::new();
    if window.state_id != authoritative_state_id {
        window.state_id = authoritative_state_id;
    }
    if packet.state_id != window.state_id {
        window.quickcraft.reset();
        write_chest_content(state, writer, &window, &view).await?;
        return Ok(window);
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_view = view.clone();
    let item_entity_type = item_entity_type_id(&state.entity_types);
    let action = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => ChestClickAction::Pickup { slot, button },
        ContainerClickAction::OutsidePickup { button } if item_entity_type.is_some() => {
            ChestClickAction::OutsidePickup { button }
        }
        ContainerClickAction::QuickMove { slot } => ChestClickAction::QuickMove { slot },
        ContainerClickAction::Swap { slot, button } => ChestClickAction::Swap { slot, button },
        ContainerClickAction::Throw { slot, button } if item_entity_type.is_some() => {
            ChestClickAction::Throw { slot, button }
        }
        ContainerClickAction::QuickCraft(click) => ChestClickAction::QuickCraft(click),
        ContainerClickAction::OutsidePickup { .. }
        | ContainerClickAction::Throw { .. }
        | ContainerClickAction::Unsupported => ChestClickAction::Unsupported,
    };
    let plan = plan_chest_click(ChestClickInput {
        items: &state.items,
        item_facts: &state.item_facts,
        window: window.clone(),
        view: view.clone(),
        inventory: state.inventory.clone(),
        carried_item: state.carried_item.clone(),
        action,
    });
    if !client_carried_item_matches(&packet.carried_item, &plan.carried_item) {
        window.quickcraft.reset();
        write_chest_content(state, writer, &window, &view).await?;
        return Ok(window);
    }
    window = plan.window;
    view = plan.view;
    state.inventory = plan.inventory;
    state.carried_item = plan.carried_item;
    let dropped = plan.dropped;
    let changed = plan.changed;
    if changed {
        let drop = dropped.map(|stack| ContainerDropPlan {
            entity_type_id: item_entity_type.expect("drop requires the item entity type"),
            position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
            stack: entity_item_stack(stack),
        });
        let player = ContainerPlayerPlan {
            expected_inventory: before_inventory,
            expected_carried_item: before_carried_item,
            updated_inventory: state.inventory.clone(),
            updated_carried_item: state.carried_item.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            merchant_input: None,
            drops: drop.into_iter().collect(),
            xp_orb: None,
        };
        match commit_chest_click(state, &window, &before_view, &view, player).await? {
            SharedContainerCommit::Committed {
                state_id,
                inventory,
                carried_item,
                dispatches: committed_dispatches,
            } => {
                window.state_id = state_id;
                state.inventory = inventory;
                state.carried_item = carried_item;
                dispatches = committed_dispatches;
            }
            SharedContainerCommit::Rejected {
                state_id,
                authoritative,
                inventory,
                carried_item,
            } => {
                state.inventory = inventory;
                state.carried_item = carried_item;
                view = ChestView {
                    chests: authoritative,
                };
                window.quickcraft.reset();
                window.state_id = state_id;
                write_chest_content(state, writer, &window, &view).await?;
                return Ok(window);
            }
        }
    }
    if changed {
        dispatch_visibility_commands(dispatches);
    }
    write_chest_content(state, writer, &window, &view).await?;
    Ok(window)
}

async fn load_furnace_commit_snapshot(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> Result<(FurnaceBlockEntity, i32), ConnectionError> {
    #[cfg(test)]
    {
        let mut storage = state.world.lock().await;
        let furnace = storage
            .furnace_block_entity(position)
            .map_err(|err| {
                warn!(error = %err, ?position, "furnace state read failed");
                err
            })?
            .unwrap_or_default();
        let state_id = state.sessions.furnace_state_id(position);
        Ok((furnace, state_id))
    }
    #[cfg(not(test))]
    {
        state
            .simulation
            .read_furnace_snapshot(position)
            .await
            .map(|snapshot| (snapshot.furnace, snapshot.state_id))
            .map_err(|error| {
                debug!(?error, ?position, "simulation furnace snapshot rejected");
                ConnectionError::RuntimeUnavailable {
                    operation: "reading furnace state through simulation owner",
                }
            })
    }
}

async fn commit_furnace_click(
    state: &InteractionState,
    window: &FurnaceWindow,
    expected: &FurnaceBlockEntity,
    updated: &FurnaceBlockEntity,
    player: ContainerPlayerPlan,
) -> Result<FurnaceCommitOutcome, ConnectionError> {
    #[cfg(test)]
    {
        let mut storage = state.world.lock().await;
        let authoritative = storage
            .furnace_block_entity(window.position)
            .map_err(|err| {
                warn!(error = %err, ?window.position, "furnace state read failed");
                err
            })?
            .unwrap_or_default();
        if &authoritative != expected {
            return Ok(SharedContainerCommit::Rejected {
                state_id: state.sessions.furnace_state_id(window.position),
                authoritative,
                inventory: player.expected_inventory,
                carried_item: player.expected_carried_item,
            });
        }
        let (state_id, dispatches) = match state.sessions.try_furnace_slot_dispatches(
            window.position,
            window.state_id,
            state.session_id,
            furnace_slot_stacks(updated),
        ) {
            Ok(committed) => committed,
            Err(state_id) => {
                return Ok(SharedContainerCommit::Rejected {
                    state_id,
                    authoritative,
                    inventory: player.expected_inventory,
                    carried_item: player.expected_carried_item,
                });
            }
        };
        storage
            .set_furnace_block_entity(window.position, updated.clone())
            .map_err(|err| {
                warn!(error = %err, ?window.position, "furnace state write failed");
                err
            })?;
        let mut dispatches = dispatches;
        for drop in &player.drops {
            dispatches.extend(state.sessions.spawn_item_drop(
                drop.entity_type_id,
                drop.position,
                drop.stack.clone(),
            ));
        }
        if let Some(xp_orb) = player.xp_orb {
            dispatches.extend(state.sessions.spawn_xp_orb(
                xp_orb.entity_type_id,
                xp_orb.position,
                xp_orb.value,
            ));
        }
        Ok(SharedContainerCommit::Committed {
            state_id,
            inventory: player.updated_inventory,
            carried_item: player.updated_carried_item,
            dispatches,
        })
    }

    #[cfg(not(test))]
    {
        match state
            .simulation
            .commit_furnace(
                window.position,
                window.state_id,
                expected.clone(),
                updated.clone(),
                player.clone(),
            )
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                debug!(?error, ?window.position, "simulation furnace commit rejected");
                let (authoritative, state_id) =
                    load_furnace_commit_snapshot(state, window.position).await?;
                let (inventory, carried_item) = state
                    .sessions
                    .player_container_state(state.session_id)
                    .unwrap_or((player.expected_inventory, player.expected_carried_item));
                Ok(SharedContainerCommit::Rejected {
                    state_id,
                    authoritative,
                    inventory,
                    carried_item,
                })
            }
        }
    }
}

async fn handle_furnace_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: FurnaceWindow,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<FurnaceWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (mut furnace, authoritative_state_id) =
        load_furnace_commit_snapshot(state, window.position).await?;
    let mut dispatches = Vec::new();
    if window.state_id != authoritative_state_id {
        window.state_id = authoritative_state_id;
    }
    if packet.state_id != window.state_id {
        write_furnace_content(state, writer, &window, &furnace).await?;
        return Ok(window);
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_furnace = furnace.clone();
    let item_entity_type = item_entity_type_id(&state.entity_types);
    let action = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => {
            FurnaceClickAction::Pickup { slot, button }
        }
        ContainerClickAction::OutsidePickup { button } if item_entity_type.is_some() => {
            FurnaceClickAction::OutsidePickup { button }
        }
        ContainerClickAction::QuickMove { slot } => FurnaceClickAction::QuickMove { slot },
        ContainerClickAction::Swap { slot, button } => FurnaceClickAction::Swap { slot, button },
        ContainerClickAction::Throw { slot, button } if item_entity_type.is_some() => {
            FurnaceClickAction::Throw { slot, button }
        }
        ContainerClickAction::OutsidePickup { .. }
        | ContainerClickAction::Throw { .. }
        | ContainerClickAction::QuickCraft(_)
        | ContainerClickAction::Unsupported => FurnaceClickAction::Unsupported,
    };
    let plan = plan_furnace_click(FurnaceClickInput {
        recipes: &state.recipes,
        items: &state.items,
        item_facts: &state.item_facts,
        tags: &state.tags,
        kind: window.kind,
        furnace: furnace.clone(),
        inventory: state.inventory.clone(),
        carried_item: state.carried_item.clone(),
        action,
        experience_seed: furnace_experience_seed(window.position, state.sessions.simulation_tick()),
    });
    let planned_carried_item = plan
        .as_ref()
        .map_or(&state.carried_item, |plan| &plan.carried_item);
    if !client_carried_item_matches(&packet.carried_item, planned_carried_item) {
        write_furnace_content(state, writer, &window, &furnace).await?;
        return Ok(window);
    }
    let changed = plan.is_some();
    if let Some(plan) = plan {
        let drop = plan.dropped.map(|stack| ContainerDropPlan {
            entity_type_id: item_entity_type.expect("drop requires the item entity type"),
            position: Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
            stack: entity_item_stack(stack),
        });
        let xp_orb = if plan.experience > 0 {
            let Some(entity_type_id) = xp_orb_entity_type_id(&state.entity_types) else {
                write_furnace_content(state, writer, &window, &furnace).await?;
                return Ok(window);
            };
            Some(ContainerXpPlan {
                entity_type_id,
                position: Vec3::new(player_pose.x, player_pose.y, player_pose.z),
                value: plan.experience,
            })
        } else {
            None
        };
        furnace = plan.furnace;
        let player = ContainerPlayerPlan {
            expected_inventory: before_inventory,
            expected_carried_item: before_carried_item,
            updated_inventory: plan.inventory,
            updated_carried_item: plan.carried_item,
            crafting_table_input: None,
            enchanting_table_input: None,
            merchant_input: None,
            drops: drop.into_iter().collect(),
            xp_orb,
        };
        match commit_furnace_click(state, &window, &before_furnace, &furnace, player).await? {
            SharedContainerCommit::Committed {
                state_id,
                inventory,
                carried_item,
                dispatches: committed_dispatches,
            } => {
                window.state_id = state_id;
                state.inventory = inventory;
                state.carried_item = carried_item;
                dispatches = committed_dispatches;
            }
            SharedContainerCommit::Rejected {
                state_id,
                authoritative,
                inventory,
                carried_item,
            } => {
                state.inventory = inventory;
                state.carried_item = carried_item;
                furnace = authoritative;
                window.state_id = state_id;
                write_furnace_content(state, writer, &window, &furnace).await?;
                return Ok(window);
            }
        }
    }
    if changed {
        dispatch_visibility_commands(dispatches);
    }
    write_furnace_content(state, writer, &window, &furnace).await?;
    Ok(window)
}

#[cfg(test)]
fn tick_furnace(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
) -> (bool, Vec<(i16, i16)>) {
    let tick = tick_furnace_rules(recipes, items, tags, furnace, kind);
    *furnace = tick.furnace;
    (tick.slots_changed, tick.data_changed)
}

#[cfg(test)]
fn apply_furnace_swap_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    slot: usize,
    button: i8,
) -> bool {
    apply_furnace_click_for_test(
        state,
        furnace,
        kind,
        FurnaceClickAction::Swap { slot, button },
    )
    .is_some()
}

#[cfg(test)]
fn apply_furnace_throw_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    slot: usize,
    button: i8,
) -> Option<ItemStack> {
    apply_furnace_click_for_test(
        state,
        furnace,
        FurnaceKind::Furnace,
        FurnaceClickAction::Throw { slot, button },
    )
    .flatten()
}

#[cfg(test)]
fn apply_furnace_click_for_test(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    action: FurnaceClickAction,
) -> Option<Option<ItemStack>> {
    let plan = plan_furnace_click(FurnaceClickInput {
        recipes: &state.recipes,
        items: &state.items,
        item_facts: &state.item_facts,
        tags: &state.tags,
        kind,
        furnace: furnace.clone(),
        inventory: state.inventory.clone(),
        carried_item: state.carried_item.clone(),
        action,
        experience_seed: 0,
    })?;
    *furnace = plan.furnace;
    state.inventory = plan.inventory;
    state.carried_item = plan.carried_item;
    Some(plan.dropped)
}

struct ContainerClickContext<'a> {
    game_mode: GameMode,
    survival_state: SurvivalState,
    xp_state: &'a XpState,
    player_pose: PlayerPose,
    script_events: Option<&'a ScriptGameplayEventPublisher>,
    scripts: Option<&'a ScriptEventSink>,
    script_player_id: ScriptPlayerId,
    script_context: ScriptPlayerContext,
}

fn persistent_container_claim_allowed(
    active: &ActiveContainer,
    mut block_allowed: impl FnMut(mc_world::BlockPos) -> bool,
) -> bool {
    match active {
        ActiveContainer::Furnace(window) => block_allowed(window.position),
        ActiveContainer::Chest(window) => window
            .positions
            .iter()
            .all(|position| block_allowed(*position)),
        _ => true,
    }
}

async fn handle_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    context: ContainerClickContext<'_>,
    packet: ServerboundContainerClick,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let ContainerClickContext {
        game_mode,
        survival_state,
        xp_state,
        player_pose,
        script_events,
        scripts,
        script_player_id,
        script_context,
    } = context;
    state.pending_break = None;
    state.pending_use = None;
    clear_shield_use(state);
    if game_mode == GameMode::Spectator
        || matches!(game_mode, GameMode::Survival | GameMode::Adventure) && survival_state.is_dead()
    {
        if packet.container_id == 0 {
            write_inventory_content_resync(state, writer).await?;
        } else if let Some(active) = state.active_container.take() {
            match active {
                ActiveContainer::Furnace(window) => {
                    let (furnace, _) = load_furnace_commit_snapshot(state, window.position).await?;
                    write_furnace_content(state, writer, &window, &furnace).await?;
                    state.active_container = Some(ActiveContainer::Furnace(window));
                }
                ActiveContainer::CraftingTable(window) => {
                    write_crafting_content(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::CraftingTable(window));
                }
                ActiveContainer::EnchantingTable(window) => {
                    write_enchanting_content(state, writer, &window).await?;
                    write_enchanting_data(state, writer, &window, xp_state).await?;
                    state.active_container = Some(ActiveContainer::EnchantingTable(window));
                }
                ActiveContainer::Stonecutter(window) => {
                    write_stonecutter_content(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::Stonecutter(window));
                }
                ActiveContainer::Chest(window) => {
                    let view = load_chest_view(state, &window).await?;
                    write_chest_content(state, writer, &window, &view).await?;
                    state.active_container = Some(ActiveContainer::Chest(window));
                }
                ActiveContainer::Merchant(window) => {
                    write_merchant_window(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::Merchant(window));
                }
                ActiveContainer::Script(window) => {
                    write_script_menu_content(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::Script(window));
                }
            }
        }
        return Ok(());
    }

    if packet.container_id != 0 {
        let Some(active) = state.active_container.take() else {
            write_inventory_content_resync(state, writer).await?;
            return Ok(());
        };
        let claim_rejected = script_events.is_some_and(|events| {
            !persistent_container_claim_allowed(&active, |position| {
                events.block_mutation_allowed(position)
            })
        });
        if claim_rejected {
            match &active {
                ActiveContainer::Furnace(window) => {
                    let (furnace, _) = load_furnace_commit_snapshot(state, window.position).await?;
                    write_furnace_content(state, writer, window, &furnace).await?;
                }
                ActiveContainer::Chest(window) => {
                    let view = load_chest_view(state, window).await?;
                    write_chest_content(state, writer, window, &view).await?;
                }
                _ => unreachable!("only persistent world containers can be claim-rejected"),
            }
            state.active_container = Some(active);
            return Ok(());
        }
        match active {
            ActiveContainer::CraftingTable(crafting)
                if crafting.container_id == packet.container_id =>
            {
                let crafting = handle_crafting_container_click(
                    state,
                    writer,
                    crafting,
                    script_events,
                    game_mode,
                    player_pose,
                    packet,
                )
                .await?;
                state.active_container = Some(ActiveContainer::CraftingTable(crafting));
            }
            ActiveContainer::EnchantingTable(enchanting)
                if enchanting.container_id == packet.container_id =>
            {
                let enchanting = handle_enchanting_container_click(
                    state,
                    writer,
                    enchanting,
                    xp_state,
                    player_pose,
                    packet,
                )
                .await?;
                state.active_container = Some(ActiveContainer::EnchantingTable(enchanting));
            }
            ActiveContainer::Stonecutter(stonecutter)
                if stonecutter.container_id == packet.container_id =>
            {
                let stonecutter = handle_stonecutter_container_click(
                    state,
                    writer,
                    stonecutter,
                    player_pose,
                    packet,
                )
                .await?;
                state.active_container = Some(ActiveContainer::Stonecutter(stonecutter));
            }
            ActiveContainer::Furnace(furnace) if furnace.container_id == packet.container_id => {
                let furnace =
                    handle_furnace_container_click(state, writer, furnace, player_pose, packet)
                        .await?;
                state.active_container = Some(ActiveContainer::Furnace(furnace));
            }
            ActiveContainer::Chest(chest) if chest.container_id == packet.container_id => {
                let chest =
                    handle_chest_container_click(state, writer, chest, player_pose, packet).await?;
                state.active_container = Some(ActiveContainer::Chest(chest));
            }
            ActiveContainer::Merchant(merchant) if merchant.container_id == packet.container_id => {
                let merchant = merchant_adapter::handle_merchant_container_click(
                    state, writer, merchant, packet,
                )
                .await?;
                state.active_container = Some(ActiveContainer::Merchant(merchant));
            }
            ActiveContainer::Script(window) => {
                let click = ScriptMenuClick::from_packet(
                    window.container_id,
                    window.state_id,
                    packet.container_id,
                    packet.state_id,
                    packet.slot_num,
                    packet.container_input,
                    packet.button_num,
                );
                match window.click(click, script_player_id, script_context) {
                    Ok(event) => {
                        if !publish_script_menu_click(scripts, event).await {
                            debug!(
                                container_id = window.container_id,
                                "script menu click rejected because targeted delivery is unavailable"
                            );
                        }
                    }
                    Err(ScriptMenuClickDisposition::Resync) => {}
                    Err(ScriptMenuClickDisposition::Clicked { .. }) => {
                        unreachable!("clicked dispositions are converted to script events")
                    }
                }
                write_script_menu_content(state, writer, &window).await?;
                state.active_container = Some(ActiveContainer::Script(window));
            }
            other => {
                debug!(
                    container_id = packet.container_id,
                    active_id = other.container_id(),
                    "container click for inactive container ignored"
                );
                state.active_container = Some(other);
            }
        }
        return Ok(());
    }

    if let Some(ActiveContainer::Script(window)) = state.active_container.take() {
        write_script_menu_content(state, writer, &window).await?;
        state.active_container = Some(ActiveContainer::Script(window));
        return Ok(());
    }

    if packet.state_id != state.inventory_state_id {
        debug!(
            client_state = packet.state_id,
            server_state = state.inventory_state_id,
            "applying queued container click against current server state"
        );
    }

    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let mut dropped = None;
    let mut discarded_remainders = Vec::new();
    let action = classify_container_click(&packet);
    if !matches!(action, ContainerClickAction::QuickCraft(_)) {
        state.inventory_quickcraft.reset();
    }
    let crafted_result = match &action {
        ContainerClickAction::Pickup { slot: 0, .. }
        | ContainerClickAction::QuickMove { slot: 0 } => Some(before_inventory.slots[0].clone()),
        _ => None,
    };
    let quick_moved_result = matches!(&action, ContainerClickAction::QuickMove { slot: 0 });
    let changed = match action {
        ContainerClickAction::Pickup { slot, button } => {
            let (changed, discarded) = state.inventory.apply_crafting_pickup_click(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                &mut state.carried_item,
                slot,
                button,
            );
            discarded_remainders = discarded;
            changed
        }
        ContainerClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click_with_carried(&mut state.carried_item, button);
            dropped.is_some()
        }
        ContainerClickAction::QuickMove { slot } => {
            let (changed, discarded) = state.inventory.apply_crafting_quick_move_click(
                &state.items,
                &state.item_facts,
                &state.tags,
                &state.recipes,
                slot,
            );
            discarded_remainders = discarded;
            changed
        }
        ContainerClickAction::Swap { slot, button } => state.inventory.apply_crafting_swap_click(
            &state.items,
            &state.item_facts,
            &state.tags,
            &state.recipes,
            slot,
            button,
        ),
        ContainerClickAction::Throw { slot, button } => {
            if item_entity_type_id(&state.entity_types).is_some() {
                dropped = state.inventory.apply_crafting_throw_click(
                    &state.items,
                    &state.item_facts,
                    &state.tags,
                    &state.recipes,
                    slot,
                    button,
                );
                dropped.is_some()
            } else {
                false
            }
        }
        ContainerClickAction::QuickCraft(click) => {
            match state.inventory.apply_crafting_quickcraft_click(
                &state.items,
                &state.item_facts,
                &mut state.carried_item,
                &mut state.inventory_quickcraft,
                click,
                &state.tags,
                &state.recipes,
            ) {
                QuickCraftOutcome::Pending => {
                    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
                        state.inventory_quickcraft.reset();
                        write_inventory_content_resync(state, writer).await?;
                    }
                    return Ok(());
                }
                QuickCraftOutcome::Changed => true,
                QuickCraftOutcome::Rejected => false,
            }
        }
        ContainerClickAction::Unsupported => false,
    };
    for remaining in discarded_remainders {
        debug!(
            item_id = remaining.item_id,
            count = remaining.count,
            "dropping inventory crafting remainder because inventory is full"
        );
    }
    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
        debug!("container click resynced mismatched carried item");
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        write_inventory_content_resync(state, writer).await?;
        return Ok(());
    }

    if !changed {
        debug!(
            slot = packet.slot_num,
            button = packet.button_num,
            input = ?packet.container_input,
            "container click unsupported or no-op; resyncing"
        );
        write_inventory_content_resync(state, writer).await?;
        return Ok(());
    }
    let crafted = crafted_result.as_ref().and_then(|result| {
        if quick_moved_result {
            crafted_item_from_inventory_delta(result, &before_inventory, &state.inventory)
        } else {
            CraftedItem::from_single_result(result)
        }
    });
    if commit_player_inventory_candidate(
        state,
        before_inventory,
        before_carried_item,
        dropped,
        player_pose,
    )
    .await?
    {
        if let (Some(script_events), Some(crafted)) = (script_events, crafted) {
            script_events
                .publish_item_crafted(
                    &state.items,
                    crafted.item_id,
                    crafted.count,
                    crafted.craft_count,
                    ScriptCraftingSource::Inventory,
                    player_pose,
                    game_mode,
                )
                .await;
        }
        write_inventory_content(state, writer).await
    } else {
        write_inventory_content_resync(state, writer).await
    }
}

fn crafted_item_from_inventory_delta(
    result: &ItemStack,
    before: &PlayerInventory,
    after: &PlayerInventory,
) -> Option<CraftedItem> {
    let matching_count = |inventory: &PlayerInventory| {
        inventory.slots[9..=44]
            .iter()
            .filter(|stack| can_stack(stack, result))
            .map(|stack| u64::try_from(stack.count).unwrap_or_default())
            .sum::<u64>()
    };
    let added = matching_count(after).checked_sub(matching_count(before))?;
    let result_count = u64::try_from(result.count).ok()?;
    if result_count == 0 || added == 0 || !added.is_multiple_of(result_count) {
        return None;
    }
    Some(CraftedItem {
        item_id: result.item_id,
        count: added,
        craft_count: u32::try_from(added / result_count).ok()?,
    })
}

async fn handle_container_button_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    packet: ServerboundContainerButtonClick,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    state.pending_use = None;
    clear_shield_use(state);

    let Some(active) = state.active_container.take() else {
        return Ok(());
    };
    let mut window = match active {
        ActiveContainer::Stonecutter(mut window) => {
            if window.container_id == packet.container_id {
                let changed = usize::try_from(packet.button_id)
                    .ok()
                    .is_some_and(|selection| {
                        select_stonecutter_recipe_with_data(
                            &state.recipes,
                            &state.items,
                            &state.item_facts,
                            &state.tags,
                            &mut window,
                            selection,
                        )
                    });
                if changed {
                    window.state_id = window.state_id.wrapping_add(1);
                }
                let selected = window
                    .selected_recipe
                    .and_then(|selection| i16::try_from(selection).ok())
                    .unwrap_or(-1);
                write_packet(
                    writer,
                    &ClientboundContainerSetData {
                        container_id: window.container_id,
                        id: 0,
                        value: selected,
                    },
                    state.compression,
                )
                .await?;
                write_stonecutter_content(state, writer, &window).await?;
            }
            state.active_container = Some(ActiveContainer::Stonecutter(window));
            return Ok(());
        }
        ActiveContainer::EnchantingTable(window) => window,
        other => {
            state.active_container = Some(other);
            return Ok(());
        }
    };
    if window.container_id != packet.container_id {
        state.active_container = Some(ActiveContainer::EnchantingTable(window));
        return Ok(());
    }

    if game_mode == GameMode::Survival && !survival_state.is_dead() {
        let bookshelf_count = enchanting_bookshelf_count(state, window.position);
        let Some(offer) = enchanting_offer(bookshelf_count, packet.button_id) else {
            write_enchanting_content(state, writer, &window).await?;
            write_enchanting_data(state, writer, &window, xp_state).await?;
            state.active_container = Some(ActiveContainer::EnchantingTable(window));
            return Ok(());
        };
        let mut updated_inputs = window.inputs.clone();
        let mut updated_xp = xp_state.clone();
        if enchant_item_candidate(
            &state.items,
            &state.item_facts,
            &mut updated_inputs,
            &mut updated_xp,
            offer,
        ) {
            let expected_inventory = state.inventory.clone();
            if commit_player_survival_update(
                state,
                writer,
                survival_state,
                xp_state,
                expected_inventory,
                *survival_state,
                updated_xp,
                Some(EnchantingTableInputPlan {
                    expected: enchanting_table_input_projection(&window.inputs),
                    updated: enchanting_table_input_projection(&updated_inputs),
                }),
                false,
                player_pose,
            )
            .await?
            {
                window.inputs = updated_inputs;
                window.state_id = window.state_id.wrapping_add(1);
            }
        }
    }

    write_enchanting_content(state, writer, &window).await?;
    write_enchanting_data(state, writer, &window, xp_state).await?;
    state.active_container = Some(ActiveContainer::EnchantingTable(window));
    Ok(())
}

async fn pickup_candidate_entities<W>(
    state: &mut InteractionState,
    writer: &mut W,
    xp_state: &mut XpState,
    candidates: Vec<ServerEntitySnapshot>,
    script_events: Option<&ScriptGameplayEventPublisher>,
    player_pose: PlayerPose,
    game_mode: GameMode,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    pickup_item_candidates(
        state,
        writer,
        &candidates,
        script_events,
        player_pose,
        game_mode,
    )
    .await?;
    pickup_arrow_candidates(
        state,
        writer,
        &candidates,
        script_events,
        player_pose,
        game_mode,
    )
    .await?;
    pickup_experience_candidates(state, writer, xp_state, &candidates).await
}

async fn pickup_item_candidates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    candidates: &[ServerEntitySnapshot],
    script_events: Option<&ScriptGameplayEventPublisher>,
    player_pose: PlayerPose,
    game_mode: GameMode,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for entity in candidates {
        let Some(ref stack) = entity.item_stack else {
            continue;
        };
        let probe = ItemStack {
            item_id: stack.item_id,
            count: stack.count,
            damage: stack.damage,
            enchantments: stack.enchantments.clone(),
            custom_name: stack.custom_name.as_deref().cloned(),
            item_model: stack.item_model.as_deref().cloned().map(Arc::new),
        };
        let max_stack = item_max_stack(&state.item_facts, &state.items, &probe);
        let credited = match state
            .simulation
            .pickup_item_into_inventory(
                entity.id,
                stack.item_id,
                stack.damage,
                stack.enchantments.clone(),
                max_stack,
            )
            .await
        {
            Ok(Some(credited)) => credited,
            Ok(None) => continue,
            Err(error) => {
                debug!(
                    ?error,
                    entity_id = entity.id.0,
                    "simulation item pickup request rejected"
                );
                continue;
            }
        };
        debug!(
            entity_id = entity.id.0,
            item_id = credited.credited.item_id,
            count = credited.credited.count,
            "simulation credited item pickup"
        );
        if let Some(script_events) = script_events {
            match u64::try_from(credited.credited.count) {
                Ok(count) if count > 0 => {
                    script_events
                        .publish_item_picked_up(
                            &state.items,
                            credited.credited.item_id,
                            count,
                            ScriptItemPickupSource::ItemEntity,
                            player_pose,
                            game_mode,
                        )
                        .await;
                }
                _ => warn!(
                    entity_id = entity.id.0,
                    count = credited.credited.count,
                    "committed item pickup has invalid credited count"
                ),
            }
        }
        state.inventory = credited.inventory;
        write_inventory_slot_updates(state, writer, credited.changed_slots).await?;
    }
    Ok(())
}

async fn pickup_arrow_candidates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    candidates: &[ServerEntitySnapshot],
    script_events: Option<&ScriptGameplayEventPublisher>,
    player_pose: PlayerPose,
    game_mode: GameMode,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let arrow = Identifier::parse("minecraft:arrow").expect("static identifier");
    let Some(item_id) = state.items.id_of(&arrow) else {
        return Ok(());
    };
    for entity in candidates
        .iter()
        .filter(|entity| entity.type_name == "minecraft:arrow")
    {
        let arrow = ItemStack::new(item_id, 1);
        let max_stack = item_max_stack(&state.item_facts, &state.items, &arrow);
        let credited = match state
            .simulation
            .pickup_arrow_into_inventory(entity.id, item_id, max_stack)
            .await
        {
            Ok(Some(credited)) => credited,
            Ok(None) => continue,
            Err(error) => {
                debug!(
                    ?error,
                    entity_id = entity.id.0,
                    "simulation arrow pickup request rejected"
                );
                continue;
            }
        };
        debug!(
            entity_id = entity.id.0,
            item_id, "simulation credited arrow pickup"
        );
        if let Some(script_events) = script_events {
            script_events
                .publish_item_picked_up(
                    &state.items,
                    item_id,
                    1,
                    ScriptItemPickupSource::Arrow,
                    player_pose,
                    game_mode,
                )
                .await;
        }
        state.inventory = credited.inventory;
        write_inventory_slot_updates(state, writer, credited.changed_slots).await?;
    }
    Ok(())
}

async fn pickup_experience_candidates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    xp_state: &mut XpState,
    candidates: &[ServerEntitySnapshot],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut changed = false;
    for entity in candidates
        .iter()
        .filter(|entity| entity.experience_value.is_some())
    {
        let credited = match state
            .simulation
            .pickup_experience_into_player(entity.id)
            .await
        {
            Ok(Some(credited)) => credited,
            Ok(None) => continue,
            Err(error) => {
                debug!(
                    ?error,
                    entity_id = entity.id.0,
                    "simulation experience pickup request rejected"
                );
                continue;
            }
        };
        debug!(
            entity_id = entity.id.0,
            value = credited.value,
            total = credited.xp.total,
            "simulation credited experience pickup"
        );
        *xp_state = credited.xp;
        changed = true;
    }
    if changed {
        write_packet(writer, &xp_state.as_packet(), state.compression).await?;
    }
    Ok(())
}

#[cfg(test)]
async fn pickup_nearby_items<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state
        .sessions
        .nearby_item_entities(position, ENTITY_PICKUP_RADIUS);
    pickup_item_candidates(
        state,
        writer,
        &candidates,
        None,
        player_pose,
        GameMode::Survival,
    )
    .await
}

#[cfg(test)]
async fn pickup_nearby_arrows<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state
        .sessions
        .nearby_grounded_arrows(position, ENTITY_PICKUP_RADIUS);
    pickup_arrow_candidates(
        state,
        writer,
        &candidates,
        None,
        player_pose,
        GameMode::Survival,
    )
    .await
}

#[cfg(test)]
async fn pickup_nearby_xp<W>(
    state: &mut InteractionState,
    writer: &mut W,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state
        .sessions
        .nearby_experience_entities(position, ENTITY_PICKUP_RADIUS);
    pickup_experience_candidates(state, writer, xp_state, &candidates).await
}

async fn handle_interact<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    packet: ServerboundInteract,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    let accepted = if packet.location.x.is_finite()
        && packet.location.y.is_finite()
        && packet.location.z.is_finite()
    {
        state
            .sessions
            .accept_script_entity_interaction(state.session_id, EntityId(packet.entity_id))
    } else {
        None
    };

    let Some(accepted) = accepted else {
        return Ok(());
    };
    if script_events.is_some_and(|events| {
        !events.block_mutation_allowed(mc_world::BlockPos {
            x: accepted.entity_position.x.floor() as i32,
            y: accepted.entity_position.y.floor() as i32,
            z: accepted.entity_position.z.floor() as i32,
        })
    }) {
        return Ok(());
    }

    let merchant_opened = accepted.entity_type == "minecraft:villager"
        && !packet.using_secondary_action
        && open_merchant_container(state, writer, accepted.entity_id, accepted.player_pose).await?;
    if !merchant_opened {
        handle_vanilla_interact(state, writer, packet).await?;
    }
    if let Some(script_events) = script_events {
        let hand = match packet.hand {
            InteractionHand::MainHand => ScriptInteractionHand::MainHand,
            InteractionHand::OffHand => ScriptInteractionHand::OffHand,
        };
        let _ = script_events
            .publish_entity_interacted(
                accepted.entity_id,
                &accepted.entity_type,
                hand,
                packet.using_secondary_action,
                accepted.player_pose,
                accepted.game_mode,
            )
            .await;
    }
    Ok(())
}

async fn handle_vanilla_interact<W>(
    state: &mut InteractionState,
    writer: &mut W,
    packet: ServerboundInteract,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let held_slot = hand_inventory_slot(state, packet.hand);
    let expected_held = state.inventory.slots[held_slot].clone();
    let held_is_shears = state
        .items
        .name_of(expected_held.item_id)
        .is_some_and(|item| item.as_str() == "minecraft:shears");
    if held_is_shears {
        let Some(plan) = sheep_shear_plan(state, packet.entity_id, held_slot, expected_held) else {
            debug!(
                entity_id = packet.entity_id,
                "sheep shear interaction missing required registry data"
            );
            return Ok(());
        };
        let committed = match state.simulation.commit_sheep_shear(plan).await {
            Ok(Some(committed)) => committed,
            Ok(None) => {
                debug!(entity_id = packet.entity_id, "sheep shear rejected");
                return Ok(());
            }
            Err(error) => {
                debug!(
                    ?error,
                    entity_id = packet.entity_id,
                    "sheep shear request rejected"
                );
                return Ok(());
            }
        };
        state.inventory = committed.inventory;
        write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
        return Ok(());
    }

    let targets = animal_feed_targets(&state.tags, expected_held.item_id);
    if expected_held.is_empty() || targets.is_empty() {
        debug!(
            entity_id = packet.entity_id,
            "unsupported entity interaction ignored"
        );
        return Ok(());
    }
    let committed = match state
        .simulation
        .commit_animal_feed(AnimalFeedPlan {
            entity_id: EntityId(packet.entity_id),
            held_slot,
            food_item_id: expected_held.item_id,
            expected_held,
            targets,
        })
        .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            debug!(entity_id = packet.entity_id, "animal feed rejected");
            return Ok(());
        }
        Err(error) => {
            debug!(
                ?error,
                entity_id = packet.entity_id,
                "animal feed request rejected"
            );
            return Ok(());
        }
    };
    state.inventory = committed.inventory;
    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    Ok(())
}

fn sheep_shear_plan(
    state: &InteractionState,
    entity_id: i32,
    held_slot: usize,
    expected_held: ItemStack,
) -> Option<SheepShearPlan> {
    let shears = Identifier::parse("minecraft:shears").ok()?;
    let shears_item_id = state.items.id_of(&shears)?;
    if expected_held.item_id != shears_item_id {
        return None;
    }
    let shears_max_damage = state
        .item_facts
        .get(&shears)
        .and_then(|facts| facts.max_damage)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(238);
    let item_entity_type_id = item_entity_type_id(&state.entity_types)?;
    let mut wool_item_ids = [0; 16];
    for color in mc_entity::SheepColor::ALL {
        let item = Identifier::parse(color.wool_item_name()).ok()?;
        wool_item_ids[usize::from(color.id())] = state.items.id_of(&item)?;
    }
    Some(SheepShearPlan {
        entity_id: EntityId(entity_id),
        held_slot,
        expected_held,
        shears_item_id,
        shears_max_damage,
        item_entity_type_id,
        wool_item_ids,
    })
}

fn animal_feed_targets(tags: &TagsData, item_id: u32) -> AnimalFeedTargets {
    let Ok(item_id) = i32::try_from(item_id) else {
        return AnimalFeedTargets::default();
    };
    let item_registry = Identifier::parse("minecraft:item").expect("static item registry id");
    let Some(item_tags) = tags.registries.get(&item_registry) else {
        return AnimalFeedTargets::default();
    };
    let contains = |tag_name: &str| {
        Identifier::parse(tag_name)
            .ok()
            .and_then(|tag| item_tags.get(&tag))
            .is_some_and(|entries| entries.contains(&item_id))
    };
    AnimalFeedTargets {
        cow: contains("minecraft:cow_food"),
        sheep: contains("minecraft:sheep_food"),
        chicken: contains("minecraft:chicken_food"),
    }
}

async fn damage_held_weapon_after_attack<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let slot = PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot);
    if let Some(stack) = damage_held_weapon_stack(
        &state.items,
        &state.item_facts,
        &mut state.inventory.slots[slot],
    ) {
        write_inventory_slot_updates(state, writer, vec![(slot, stack)]).await?;
    }
    Ok(())
}

async fn apply_successful_player_attack_costs<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode == GameMode::Survival {
        let mut updated_survival = *survival_state;
        let packet_changed =
            updated_survival.add_exhaustion(SurvivalState::ENTITY_ATTACK_EXHAUSTION);
        if updated_survival != *survival_state {
            let expected_inventory = state.inventory.clone();
            commit_player_survival_update(
                state,
                writer,
                survival_state,
                xp_state,
                expected_inventory,
                updated_survival,
                xp_state.clone(),
                None,
                packet_changed,
                player_pose,
            )
            .await?;
        }
    }
    if weapon_attacks_damage_held_item(game_mode) {
        damage_held_weapon_after_attack(state, writer).await?;
    }
    Ok(())
}

async fn handle_attack<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    packet: ServerboundAttack,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(held) = state.inventory.held(state.selected_hotbar_slot).cloned() else {
        debug!(
            slot = state.selected_hotbar_slot,
            "entity attack ignored for invalid selected hotbar slot"
        );
        return Ok(());
    };
    state.pending_break = None;
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for dead player"
        );
        return Ok(());
    }
    if matches!(game_mode, GameMode::Spectator) {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored in spectator"
        );
        return Ok(());
    }

    let entity_id = EntityId(packet.entity_id);
    let current_tick = state.sessions.simulation_tick();
    let Some(damage) = begin_player_attack_attempt(
        &state.item_facts,
        &state.items,
        &held,
        game_mode,
        state.last_entity_attack_tick,
        current_tick,
    ) else {
        return Ok(());
    };
    let Some(attacker_costs) =
        player_attack_cost_plan(state, game_mode, *survival_state, xp_state, player_pose)
    else {
        return Ok(());
    };
    let result = match state
        .simulation
        .player_attack_server_entity_with_costs(entity_id, damage, attacker_costs, current_tick)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            debug!(
                ?error,
                entity_id = packet.entity_id,
                "simulation entity attack request rejected"
            );
            return Ok(());
        }
    };
    let attack = match result {
        PlayerAttackResult::ValidationRejected => {
            debug!(
                entity_id = packet.entity_id,
                "entity attack ignored for invalid target"
            );
            return Ok(());
        }
        PlayerAttackResult::AcceptedNoDamage => {
            state.last_entity_attack_tick = Some(current_tick);
            debug!(
                entity_id = packet.entity_id,
                "reachable entity attack accepted without damage"
            );
            return Ok(());
        }
        PlayerAttackResult::Damaged(outcome) => {
            state.last_entity_attack_tick = Some(current_tick);
            *outcome
        }
    };
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    match attack {
        EntityAttackOutcome::PlayerDamaged {
            target_session,
            dispatches,
            damage_applied,
            attacker_costs,
        } => {
            dispatch_visibility_commands(dispatches);
            if !damage_applied {
                debug!(
                    entity_id = packet.entity_id,
                    target_session, "player attack committed without target damage"
                );
                return Ok(());
            }
            let Some(attacker_costs) = attacker_costs else {
                debug!(
                    target_session,
                    "player attack committed without attacker cost result"
                );
                return Ok(());
            };
            apply_committed_player_attack_costs(state, writer, survival_state, attacker_costs)
                .await?;
            debug!(
                entity_id = packet.entity_id,
                target_session, "player attack routed to target session"
            );
        }
        EntityAttackOutcome::Damaged {
            damage,
            dispatches,
            attacker_costs,
        } => {
            if let Some(attacker_costs) = attacker_costs {
                apply_committed_player_attack_costs(state, writer, survival_state, attacker_costs)
                    .await?;
            } else {
                apply_successful_player_attack_costs(
                    state,
                    writer,
                    game_mode,
                    survival_state,
                    xp_state,
                    player_pose,
                )
                .await?;
            }
            dispatch_visibility_commands(dispatches);
            write_packet(
                writer,
                &EntityEvent {
                    entity_id: packet.entity_id,
                    event_id: 2,
                },
                state.compression,
            )
            .await?;
            debug!(
                entity_id = packet.entity_id,
                health = damage.snapshot.health,
                "entity attack damaged target"
            );
            return Ok(());
        }
        EntityAttackOutcome::Killed {
            damage,
            entity,
            dispatches,
            attacker_costs,
        } => {
            if let Some(attacker_costs) = attacker_costs {
                apply_committed_player_attack_costs(state, writer, survival_state, attacker_costs)
                    .await?;
            } else {
                apply_successful_player_attack_costs(
                    state,
                    writer,
                    game_mode,
                    survival_state,
                    xp_state,
                    player_pose,
                )
                .await?;
            }
            dispatch_visibility_commands(dispatches);
            debug!(
                entity_id = packet.entity_id,
                entity_type = %entity.type_name,
                health = damage.snapshot.health,
                "entity attack killed target"
            );
        }
    }
    Ok(())
}

async fn apply_committed_player_attack_costs<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    committed: session::CommittedPlayerAttackCosts,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let old_survival = *survival_state;
    survival_state.food = committed.survival.food;
    survival_state.saturation = committed.survival.saturation;
    survival_state.exhaustion = committed.survival.exhaustion;
    let changed_slots = state
        .inventory
        .slots
        .iter()
        .zip(&committed.inventory.slots)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(slot, (_, after))| (slot, after.clone()))
        .collect::<Vec<_>>();
    state.inventory = committed.inventory;
    refresh_shield_use_state(state);
    if old_survival != *survival_state {
        write_packet(writer, &survival_state.as_packet(), state.compression).await?;
    }
    if !changed_slots.is_empty() {
        write_inventory_slot_updates(state, writer, changed_slots).await?;
    }
    Ok(())
}

fn player_attack_cost_plan(
    state: &InteractionState,
    game_mode: GameMode,
    survival: SurvivalState,
    xp: &XpState,
    player_pose: PlayerPose,
) -> Option<PlayerSurvivalPlan> {
    let mut updated_survival = survival;
    let mut updated_inventory = state.inventory.clone();
    if game_mode == GameMode::Survival {
        let held = updated_inventory.held_mut(state.selected_hotbar_slot)?;
        updated_survival.add_exhaustion(SurvivalState::ENTITY_ATTACK_EXHAUSTION);
        damage_held_weapon_stack(&state.items, &state.item_facts, held);
    }
    Some(PlayerSurvivalPlan {
        expected_survival: survival,
        updated_survival,
        expected_inventory: state.inventory.clone(),
        updated_inventory,
        expected_carried_item: state.carried_item.clone(),
        expected_xp: xp.clone(),
        updated_xp: xp.clone(),
        active_shield: None,
        enchanting_table_input: None,
        item_entity_type_id: None,
        xp_orb_entity_type_id: None,
        keep_inventory: false,
        position: Vec3::new(player_pose.x, player_pose.y, player_pose.z),
    })
}

async fn start_falling_blocks_after_edits<W>(
    state: &mut InteractionState,
    writer: &mut W,
    applied: &[AppliedBlockEdit],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(entity_type_id) = falling_block_entity_type_id(&state.entity_types) else {
        return Ok(());
    };
    let air = air_state_id(&state.blocks);
    let chunks = falling_block_start_chunks(applied);
    let snapshot = state.world_read.snapshot_chunks(&chunks);
    let plan =
        plan_falling_block_starts(&state.blocks, &state.block_facts, &snapshot, applied, air);
    #[cfg(test)]
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| count.set(count.get() + 1));
    if plan.starts.is_empty() {
        return Ok(());
    }

    let removal_edits = plan
        .starts
        .iter()
        .map(|start| BlockEdit {
            pos: start.pos,
            new_state: air,
        })
        .collect::<Vec<_>>();
    let Some(outcome) = apply_visible_block_edit_batch_conditionally(
        state,
        writer,
        &removal_edits,
        &plan.preconditions,
        &[],
    )
    .await?
    else {
        return Ok(());
    };
    if outcome.applied.is_empty() {
        return Ok(());
    }
    schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;

    for edit in &outcome.applied {
        if !is_falling_block_state(&state.blocks, edit.previous) {
            continue;
        }
        dispatch_visibility_commands(state.sessions.spawn_falling_block(
            entity_type_id,
            Vec3::new(
                f64::from(edit.pos.x) + 0.5,
                f64::from(edit.pos.y),
                f64::from(edit.pos.z) + 0.5,
            ),
            edit.previous,
        ));
    }
    Ok(())
}

/// M5.d/M22.b: handle serverbound block-destroy actions. Creative keeps the
/// historical instant edit path; survival now requires a server-timed start/stop
/// pair before the shared mutation back-half can run.
#[allow(clippy::too_many_arguments)]
async fn handle_player_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    action: ServerboundPlayerAction,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let is_destroy = matches!(
        action.action,
        PlayerActionKind::StartDestroyBlock
            | PlayerActionKind::AbortDestroyBlock
            | PlayerActionKind::StopDestroyBlock
    );
    if matches!(action.action, PlayerActionKind::ReleaseUseItem) {
        clear_shield_use(state);
        let current_tick = state.sessions.simulation_tick();
        if game_mode == GameMode::Survival
            && let Some(pending) = state.pending_use.take()
            && matches!(pending.kind, UseKind::Bow)
            && pending_use_matches(state, &pending)
            && bow_draw_power(pending.started_tick, current_tick) > 0.0
            && let Some(entity_type_id) = arrow_entity_type_id(&state.entity_types)
            && let Some(expected_slot) = available_arrow_slot(state)
        {
            let power = bow_draw_power(pending.started_tick, current_tick);
            let position = arrow_spawn_position(player_pose);
            let velocity = arrow_velocity(player_pose, power);
            let rotation = Rotation {
                yaw: player_pose.yaw,
                pitch: player_pose.pitch,
                head_yaw: player_pose.yaw,
            };
            let expected_bow = state.inventory.slots[pending.held_slot].clone();
            let expected_arrow = state.inventory.slots[expected_slot].clone();
            let Some(bow_max_damage) = held_bow_max_damage(state, pending.held_slot) else {
                return write_block_ack(writer, state.compression, action.sequence).await;
            };
            match state
                .simulation
                .commit_bow_release(BowReleasePlan {
                    bow_slot: pending.held_slot,
                    expected_bow,
                    arrow_slot: expected_slot,
                    expected_arrow,
                    bow_max_damage,
                    entity_type_id,
                    position,
                    velocity,
                    rotation,
                })
                .await
            {
                Ok(Some(committed)) => {
                    state.inventory = committed.inventory;
                    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
                }
                Ok(None) => {
                    debug!("bow release rejected because player inventory changed");
                }
                Err(error) => {
                    debug!(?error, "simulation bow release request rejected");
                }
            }
        } else {
            state.pending_use = None;
        }
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if matches!(
        action.action,
        PlayerActionKind::DropItem | PlayerActionKind::DropAllItems
    ) {
        state.pending_break = None;
        state.pending_use = None;
        clear_shield_use(state);
        if game_mode == GameMode::Survival && survival_state.is_dead() {
            return write_block_ack(writer, state.compression, action.sequence).await;
        }

        let slot = PlayerInventory::HOTBAR_BASE + state.selected_hotbar_slot as usize;
        let expected_held = state.inventory.slots[slot].clone();
        if !expected_held.is_empty()
            && let Some(entity_type_id) = item_entity_type_id(&state.entity_types)
        {
            let drop_count = if matches!(action.action, PlayerActionKind::DropItem) {
                1
            } else {
                expected_held.count
            };
            let forward = player_horizontal_look_direction(player_pose.yaw);
            match state
                .simulation
                .commit_selected_item_drop(SelectedItemDropPlan {
                    held_hotbar_slot: state.selected_hotbar_slot,
                    expected_held,
                    drop_count,
                    entity_type_id,
                    position: Vec3::new(
                        player_pose.x + forward.x * PLAYER_SELECTED_DROP_FORWARD_OFFSET,
                        player_pose.y + 1.0,
                        player_pose.z + forward.z * PLAYER_SELECTED_DROP_FORWARD_OFFSET,
                    ),
                })
                .await
            {
                Ok(Some(committed)) => {
                    state.inventory = committed.inventory;
                    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
                }
                Ok(None) => {
                    debug!("selected item drop rejected because player inventory changed");
                }
                Err(error) => {
                    debug!(?error, "simulation selected item drop request rejected");
                }
            }
        }
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if matches!(action.action, PlayerActionKind::SwapItemWithOffhand) {
        state.pending_break = None;
        if game_mode == GameMode::Spectator {
            return write_block_ack(writer, state.compression, action.sequence).await;
        }
        state.pending_use = None;
        clear_shield_use(state);

        let main_hand_slot = PlayerInventory::HOTBAR_BASE + state.selected_hotbar_slot as usize;
        let expected_inventory = state.inventory.clone();
        state
            .inventory
            .slots
            .swap(main_hand_slot, PlayerInventory::OFFHAND_SLOT);
        if commit_player_inventory_candidate(
            state,
            expected_inventory,
            state.carried_item.clone(),
            None,
            player_pose,
        )
        .await?
        {
            write_inventory_slot_updates(
                state,
                writer,
                vec![
                    (
                        main_hand_slot,
                        state.inventory.slots[main_hand_slot].clone(),
                    ),
                    (
                        PlayerInventory::OFFHAND_SLOT,
                        state.inventory.slots[PlayerInventory::OFFHAND_SLOT].clone(),
                    ),
                ],
            )
            .await?;
        } else {
            write_inventory_content_resync(state, writer).await?;
        }
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if !is_destroy {
        // STAB and currently unsupported non-destroy actions are acked so the
        // client doesn't hang on a prediction.
        debug!(
            action = ?action.action,
            sequence = action.sequence,
            "non-destroy player action ignored"
        );
        write_block_ack(writer, state.compression, action.sequence).await?;
        return Ok(());
    }

    handle_block_destroy_action(
        state,
        writer,
        script_events,
        game_mode,
        survival_state,
        xp_state,
        player_pose,
        action,
    )
    .await
}

fn arrow_spawn_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y + 1.62, pose.z)
}

fn player_look_direction(pose: PlayerPose) -> Vec3 {
    let yaw = f64::from(pose.yaw).to_radians();
    let pitch = f64::from(pose.pitch).to_radians();
    let pitch_cos = pitch.cos();
    Vec3::new(-yaw.sin() * pitch_cos, -pitch.sin(), yaw.cos() * pitch_cos)
}

fn shield_use_entity_data_value(shield_use: Option<&ShieldUseState>) -> EntityDataValue {
    EntityDataValue::Byte {
        index: LIVING_ENTITY_DATA_FLAGS_INDEX,
        value: shield_use_flags(shield_use),
    }
}

fn dispatch_shield_use_metadata(state: &InteractionState) {
    dispatch_visibility_commands(state.sessions.broadcast_player_entity_data(
        state.session_id,
        vec![shield_use_entity_data_value(state.shield_use.as_ref())],
    ));
}

fn clear_shield_use(state: &mut InteractionState) {
    if state.shield_use.take().is_some() {
        state.sessions.set_active_shield(state.session_id, None);
        dispatch_shield_use_metadata(state);
    }
}

fn start_shield_use(
    state: &mut InteractionState,
    hand: mc_protocol::packets::play::InteractionHand,
) -> bool {
    if state.shield_use.is_some() {
        return true;
    }
    let slot = shield_hand_slot(
        hand,
        PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot),
        PlayerInventory::OFFHAND_SLOT,
    );
    let stack = state.inventory.slots[slot].clone();
    let is_shield = stack_is_shield(&state.items, &stack);
    if !is_shield {
        return false;
    }
    if state
        .sessions
        .shield_disable_remaining_ticks(state.session_id, state.sessions.simulation_tick())
        .is_some()
    {
        return true;
    }
    let Some(shield_use) =
        shield_use_from_stack(hand, slot, stack, state.sessions.world_time(), true)
    else {
        return false;
    };
    state.pending_break = None;
    state.pending_use = None;
    state.sessions.set_active_shield(
        state.session_id,
        Some(ActiveShield {
            started_tick: shield_use.started_tick,
            slot: shield_use.slot,
            expected_stack: shield_use.stack.clone(),
        }),
    );
    state.shield_use = Some(shield_use);
    dispatch_shield_use_metadata(state);
    true
}

fn refresh_shield_use_state(state: &mut InteractionState) {
    let Some(shield_use) = &state.shield_use else {
        return;
    };
    let current_hand_slot = shield_hand_slot(
        shield_use.hand,
        PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot),
        PlayerInventory::OFFHAND_SLOT,
    );
    if !shield_use_matches(
        shield_use,
        current_hand_slot,
        &state.inventory.slots,
        &state.items,
    ) {
        clear_shield_use(state);
    }
}

fn restore_authoritative_shield_state(
    state: &mut InteractionState,
    authoritative: AuthoritativePlayerStateSnapshot,
) {
    let old_flags = shield_use_flags(state.shield_use.as_ref());
    state.inventory = authoritative.inventory;
    state.carried_item = authoritative.carried_item;
    state.shield_use = authoritative.active_shield.and_then(|shield| {
        let main_hand_slot = PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot);
        let hand = if shield.slot == main_hand_slot {
            mc_protocol::packets::play::InteractionHand::MainHand
        } else if shield.slot == PlayerInventory::OFFHAND_SLOT {
            mc_protocol::packets::play::InteractionHand::OffHand
        } else {
            return None;
        };
        let stack = state.inventory.slots.get(shield.slot)?;
        if stack != &shield.expected_stack || !stack_is_shield(&state.items, stack) {
            return None;
        }
        Some(ShieldUseState {
            hand,
            started_tick: shield.started_tick,
            slot: shield.slot,
            stack: shield.expected_stack,
        })
    });
    if old_flags != shield_use_flags(state.shield_use.as_ref()) {
        dispatch_shield_use_metadata(state);
    }
}

struct PlannedActiveShieldDamage {
    #[cfg(test)]
    slot: usize,
    #[cfg(test)]
    stack: ItemStack,
    transition: ActiveShieldTransition,
    shield_use_after: Option<ShieldUseState>,
}

fn plan_active_shield_damage(
    state: &mut InteractionState,
    blocked_damage: f32,
) -> Option<PlannedActiveShieldDamage> {
    let mut shield_use = state.shield_use.clone()?;
    let expected_active_shield = ActiveShield {
        started_tick: shield_use.started_tick,
        slot: shield_use.slot,
        expected_stack: shield_use.stack.clone(),
    };
    let current_hand_slot = shield_hand_slot(
        shield_use.hand,
        PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot),
        PlayerInventory::OFFHAND_SLOT,
    );
    if !shield_use_matches(
        &shield_use,
        current_hand_slot,
        &state.inventory.slots,
        &state.items,
    ) {
        clear_shield_use(state);
        return None;
    }
    let changed = damage_active_shield_slots(
        &state.items,
        &state.item_facts,
        &mut state.inventory.slots,
        &mut shield_use,
        blocked_damage,
    )?;
    let shield_use_after = (!changed.2).then_some(shield_use);
    let updated_active_shield = shield_use_after.as_ref().map(|shield| ActiveShield {
        started_tick: shield.started_tick,
        slot: shield.slot,
        expected_stack: shield.stack.clone(),
    });
    Some(PlannedActiveShieldDamage {
        #[cfg(test)]
        slot: changed.0,
        #[cfg(test)]
        stack: changed.1,
        transition: ActiveShieldTransition {
            expected: Some(expected_active_shield),
            updated: updated_active_shield,
        },
        shield_use_after,
    })
}

fn finish_committed_shield_damage(
    state: &mut InteractionState,
    planned: PlannedActiveShieldDamage,
) {
    let old_flags = shield_use_flags(state.shield_use.as_ref());
    if let Some(shield_use) = planned.shield_use_after {
        state.shield_use = Some(shield_use);
    } else {
        state.shield_use = None;
    }
    if old_flags != shield_use_flags(state.shield_use.as_ref()) {
        dispatch_shield_use_metadata(state);
    }
}

#[cfg(test)]
fn damage_active_shield(
    state: &mut InteractionState,
    blocked_damage: f32,
) -> Option<(usize, ItemStack)> {
    let planned = plan_active_shield_damage(state, blocked_damage)?;
    let changed = (planned.slot, planned.stack.clone());
    finish_committed_shield_damage(state, planned);
    Some(changed)
}

fn shield_blocks_current_damage(
    state: &mut InteractionState,
    player_pose: PlayerPose,
    source_origin: Option<Vec3>,
) -> bool {
    refresh_shield_use_state(state);
    shield_blocks_damage(
        Vec3::new(player_pose.x, player_pose.y, player_pose.z),
        player_pose.yaw,
        source_origin,
        state.sessions.world_time(),
        state.shield_use.as_ref(),
    )
}

fn arrow_velocity(pose: PlayerPose, power: f64) -> Vec3 {
    let direction = player_look_direction(pose);
    let speed = 3.0 * power.clamp(0.0, 1.0);
    Vec3::new(
        direction.x * speed,
        direction.y * speed,
        direction.z * speed,
    )
}

struct SheepFoodStates {
    air: BlockStateId,
    dirt: BlockStateId,
    grass: HashSet<BlockStateId>,
    edible_plants: HashSet<BlockStateId>,
}

fn sheep_food_states(blocks: &BlockRegistry) -> Option<SheepFoodStates> {
    let block = |name: &str| {
        let id = Identifier::parse(name).ok()?;
        blocks.block(&id)
    };
    let air = block("minecraft:air")?.default;
    let dirt = block("minecraft:dirt")?.default;
    let grass = block("minecraft:grass_block")?
        .states
        .iter()
        .copied()
        .collect();
    let edible_plants = [
        "minecraft:short_grass",
        "minecraft:short_dry_grass",
        "minecraft:tall_dry_grass",
        "minecraft:fern",
    ]
    .into_iter()
    .filter_map(block)
    .flat_map(|block| block.states.iter().copied())
    .collect();
    Some(SheepFoodStates {
        air,
        dirt,
        grass,
        edible_plants,
    })
}

fn sheep_food_edit(
    world: &impl BlockPlanningRead,
    candidate: session::SheepGrazingCandidate,
    states: &SheepFoodStates,
) -> Option<BlockEdit> {
    let position = candidate.block_position;
    let current = world.get_cached_block(position)?;
    if states.edible_plants.contains(&current) {
        return Some(BlockEdit {
            pos: position,
            new_state: states.air,
        });
    }
    let below = mc_world::BlockPos {
        x: position.x,
        y: position.y.saturating_sub(1),
        z: position.z,
    };
    states
        .grass
        .contains(&world.get_cached_block(below)?)
        .then_some(BlockEdit {
            pos: below,
            new_state: states.dirt,
        })
}

async fn run_sheep_grazing_owned(
    authority: &simulation::SimulationAuthority,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
    tick: u64,
) -> SheepGrazingReport {
    let Some(world) = config.world.as_ref() else {
        return SheepGrazingReport::default();
    };
    let Some(states) = sheep_food_states(&config.blocks) else {
        return SheepGrazingReport::default();
    };
    let plan = sessions.plan_sheep_grazing(authority, tick);
    if plan.starts.is_empty() && plan.actions.is_empty() {
        return SheepGrazingReport::default();
    }

    let mut chunks = plan
        .starts
        .iter()
        .chain(&plan.actions)
        .map(|candidate| ChunkPos {
            x: candidate.block_position.x.div_euclid(16),
            z: candidate.block_position.z.div_euclid(16),
        })
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| (chunk.x, chunk.z));
    chunks.dedup();
    let owned_world_read = if world_read.is_none() {
        Some(world.lock().await.read_view())
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_read.as_ref())
        .expect("world-backed sheep grazing has a read view");
    let snapshot = world_read.snapshot_chunks(&chunks);

    let starts = plan
        .starts
        .into_iter()
        .filter(|candidate| sheep_food_edit(&snapshot, *candidate, &states).is_some())
        .collect::<Vec<_>>();
    let (started, start_dispatches) = sessions.start_sheep_grazing(authority, &starts);
    dispatch_visibility_commands(start_dispatches);

    let actions = plan
        .actions
        .into_iter()
        .filter(|candidate| sheep_food_edit(&snapshot, *candidate, &states).is_some())
        .collect::<Vec<_>>();
    if actions.is_empty() {
        return SheepGrazingReport { started, ate: 0 };
    }

    let table = config.block_light.as_deref();
    let mut planned_entities = HashMap::new();
    let mut resident_edits = Vec::new();
    let mut resident_preconditions = Vec::new();
    for candidate in &actions {
        let Some(edit) = sheep_food_edit(&snapshot, *candidate, &states) else {
            continue;
        };
        if planned_entities.contains_key(&edit.pos) {
            continue;
        }
        let (Some(expected_state), Some(expected_token)) = (
            snapshot.get_cached_block(edit.pos),
            snapshot.block_mutation_token(edit.pos),
        ) else {
            continue;
        };
        resident_edits.push(mc_world::ResidentBlockEdit {
            pos: edit.pos,
            new_state: edit.new_state,
            preserve_light: table.is_some_and(|table| {
                !block_edit_changes_light(table, expected_state, edit.new_state)
            }),
        });
        resident_preconditions.push(mc_world::ResidentBlockPrecondition {
            pos: edit.pos,
            expected_state,
            expected_token,
        });
        planned_entities.insert(edit.pos, candidate.entity_id);
    }

    let resident = if let Some(mutation) = world_mutation {
        match commit_resident_block_edits(
            sessions,
            world_read,
            mutation,
            tick,
            ResidentBlockCommit {
                edits: &resident_edits,
                preconditions: &resident_preconditions,
                consumed_block_ticks: &[],
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table: table,
                leaf_trigger_tick: None,
            },
        )
        .await
        {
            Ok(result) => result,
            Err(()) => return SheepGrazingReport { started, ate: 0 },
        }
    } else {
        None
    };
    let outcome = match resident {
        Some(outcome) => outcome,
        None => {
            let mut outcome = BlockEditBatchOutcome::default();
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "sheep grazing block edit",
                Instant::now(),
                world.lock().await,
            );
            planned_entities.clear();
            for candidate in actions {
                let Some(edit) = sheep_food_edit(&*storage, candidate, &states) else {
                    continue;
                };
                let applied_before = outcome.applied.len();
                apply_block_edit_to_storage(&mut storage, table, &edit, &mut outcome);
                if outcome.applied.len() > applied_before {
                    planned_entities.insert(edit.pos, candidate.entity_id);
                }
            }
            outcome
        }
    };
    let eaten_entities = outcome
        .applied
        .iter()
        .filter_map(|edit| planned_entities.get(&edit.pos).copied())
        .collect::<Vec<_>>();

    if !outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates =
                collect_server_origin_light_updates(world, sessions, table, &outcome).await;
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
        }
    }

    let (ate, data_dispatches) = sessions.finish_sheep_grazing(authority, &eaten_entities);
    dispatch_visibility_commands(data_dispatches);
    SheepGrazingReport { started, ate }
}

#[allow(clippy::too_many_arguments)]
async fn commit_random_tick_region_fanout(
    sessions: &SessionRegistry,
    world_read: &mc_world::WorldReadView,
    mutation: &mc_world::WorldMutationView,
    light_table: Option<Arc<BlockLightTable>>,
    world_tick: u64,
    resources: &ChunkPipelineResources,
    plans: Vec<RandomTickRegionPlan>,
    #[cfg(test)] probe: Option<simulation::RegionalBlockEditProbe>,
) -> Result<
    (
        BlockEditBatchOutcome,
        usize,
        Vec<RandomTickLeafDrop>,
        Vec<Vec<(usize, RandomTickCandidate)>>,
    ),
    (),
> {
    let lane_count = resources.cpu_limit().max(1).min(plans.len());
    let mut lanes = BTreeMap::<usize, Vec<RandomTickRegionJob>>::new();
    for (index, planned) in plans.into_iter().enumerate() {
        let journal_chunks = resident_block_journal_chunks(world_read, &planned.plan.edits);
        let (edits, preconditions) =
            random_tick_resident_inputs(&planned.plan, light_table.as_deref())
                .expect("fanout plans were preflighted as resident random ticks");
        let lane = ((planned.region.x as u32).wrapping_mul(31) ^ planned.region.z as u32) as usize
            % lane_count;
        lanes.entry(lane).or_default().push(RandomTickRegionJob {
            index,
            #[cfg(test)]
            region: planned.region,
            group: planned.group,
            plan: planned.plan,
            edits,
            preconditions,
            journal_chunks,
        });
    }

    let mut wave = ResidentWorldJournalWave::checkpoint_only();
    let decision_id = wave.decision_id;
    let mut workers = tokio::task::JoinSet::new();
    for jobs in lanes.into_values() {
        let permit = resources.acquire_cpu().await.map_err(|_| ())?;
        let mutation = mutation.clone();
        let light_table = light_table.clone();
        #[cfg(test)]
        let probe = probe.clone();
        workers.spawn_blocking(move || {
            let _permit = permit;
            let mut results = Vec::with_capacity(jobs.len());
            for job in jobs {
                let stamped = if let Some(decision_id) = decision_id {
                    match mutation.stamp_chunks_for_world_journal(decision_id, &job.journal_chunks)
                    {
                        mc_world::JournalStampResult::Stamped(_) => true,
                        mc_world::JournalStampResult::NewerDecision(_) => {
                            results.push(RandomTickRegionResult {
                                index: job.index,
                                group: job.group,
                                plan: job.plan,
                                result: mc_world::ResidentBlockEditBatchResult::Stale,
                                touched: Vec::new(),
                                panicked: false,
                            });
                            continue;
                        }
                        mc_world::JournalStampResult::Missing => {
                            results.push(RandomTickRegionResult {
                                index: job.index,
                                group: job.group,
                                plan: job.plan,
                                result: mc_world::ResidentBlockEditBatchResult::Missing,
                                touched: Vec::new(),
                                panicked: false,
                            });
                            continue;
                        }
                    }
                } else {
                    false
                };
                let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if let Some(probe) = probe.as_ref() {
                        probe.enter(job.region);
                    }
                    if let Some(decision_id) = decision_id {
                        mutation.apply_block_edits_conditionally_journaled(
                            decision_id,
                            &job.edits,
                            &job.preconditions,
                            &[],
                            light_table.as_deref(),
                            Some(world_tick.saturating_add(1)),
                        )
                    } else {
                        (
                            mutation.apply_block_edits_conditionally(
                                &job.edits,
                                &job.preconditions,
                                &[],
                                light_table.as_deref(),
                                Some(world_tick.saturating_add(1)),
                            ),
                            Vec::new(),
                        )
                    }
                }));
                let (result, mut touched, panicked) = match applied {
                    Ok((result, touched)) => (result, touched, false),
                    Err(_) => (
                        mc_world::ResidentBlockEditBatchResult::Stale,
                        Vec::new(),
                        true,
                    ),
                };
                if stamped {
                    touched.extend(job.journal_chunks.iter().copied());
                    touched.sort_unstable_by_key(|position| (position.x, position.z));
                    touched.dedup();
                }
                results.push(RandomTickRegionResult {
                    index: job.index,
                    group: job.group,
                    plan: job.plan,
                    result,
                    touched,
                    panicked,
                });
            }
            results
        });
    }

    let mut results = Vec::new();
    let mut worker_failed = false;
    while let Some(joined) = workers.join_next().await {
        match joined {
            Ok(mut lane_results) => results.append(&mut lane_results),
            Err(error) => {
                warn!(?error, "random-tick regional worker failed");
                worker_failed = true;
            }
        }
    }
    results.sort_unstable_by_key(|result| result.index);

    let mut outcome = BlockEditBatchOutcome::default();
    let mut eligible = 0;
    let mut leaf_drops = Vec::new();
    let mut fallback = Vec::new();
    for result in results {
        worker_failed |= result.panicked;
        wave.touched.extend(result.touched.iter().copied());
        match result.result {
            mc_world::ResidentBlockEditBatchResult::Applied(applied) => {
                eligible += result.plan.eligible;
                let applied_positions = applied.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
                leaf_drops.extend(
                    result
                        .plan
                        .leaf_drops
                        .into_iter()
                        .filter(|drop| applied_positions.contains(&drop.source)),
                );
                let additional = simulation::resident_block_edit_result_outcome(
                    mc_world::ResidentBlockEditBatchResult::Applied(applied),
                )
                .expect("applied random-tick regional job has an outcome");
                append_resident_block_outcome(&mut outcome, additional);
            }
            mc_world::ResidentBlockEditBatchResult::Stale
            | mc_world::ResidentBlockEditBatchResult::Missing
            | mc_world::ResidentBlockEditBatchResult::CrossRegion => fallback.push(result.group),
        }
    }
    wave.finish(sessions, world_read, mutation, world_tick)
        .await?;
    if worker_failed {
        sessions.report_world_chunk_journal_failure();
        return Err(());
    }
    Ok((outcome, eligible, leaf_drops, fallback))
}

#[allow(clippy::too_many_arguments)]
async fn run_random_ticks_owned(
    authority: &simulation::SimulationAuthority,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    access: SimulationWorldAccess<'_>,
    #[cfg(test)] probe: Option<simulation::RegionalBlockEditProbe>,
    protection: Option<&crate::script::ZoneProtectionSnapshot>,
    world_tick: u64,
    chunk_budget: usize,
) -> RandomTickReport {
    let world_read = access.read;
    let world_mutation = access.mutation;
    let cpu_resources = access.cpu;
    let mut policy = config.random_tick.normalized();
    policy.chunk_budget = chunk_budget.max(1);
    let Some(world) = config.world.as_ref() else {
        return RandomTickReport::default();
    };
    let loaded_chunks = sessions.loaded_chunks_sorted();
    if loaded_chunks.is_empty() {
        return RandomTickReport::default();
    }
    let samples = sample_random_tick_positions(policy, world_tick, &loaded_chunks);
    if samples.is_empty() {
        return RandomTickReport::default();
    }

    let owned_world_read = if world_read.is_none() {
        Some(world.lock().await.read_view())
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_read.as_ref())
        .expect("world-backed random ticks have a read view");
    let (sampled, candidates) = random_tick_candidates(world_read, &config.block_facts, &samples);
    if candidates.is_empty() {
        return RandomTickReport {
            sampled,
            ..RandomTickReport::default()
        };
    }

    let table = config.block_light.as_deref();
    let mut groups = VecDeque::from(random_tick_candidate_groups(&candidates));
    let mut eligible = 0;
    let mut outcome = BlockEditBatchOutcome::default();
    let mut applied_by_pass = HashSet::<mc_world::BlockPos>::new();
    let mut accepted_leaf_drops = Vec::new();
    let mut wave = None;
    let fanout_plans = if cpu_resources.is_some_and(|resources| resources.cpu_limit() > 1)
        && world_mutation.is_some()
        && groups.len() > 1
    {
        let plans = groups
            .iter()
            .cloned()
            .map(|group| {
                let planning_candidates = group
                    .iter()
                    .map(|(_, candidate)| *candidate)
                    .collect::<Vec<_>>();
                let snapshot =
                    world_read.snapshot_chunks(&random_tick_planning_chunks(&planning_candidates));
                let plan = plan_indexed_random_tick_edits(
                    config, policy, world_tick, &snapshot, &group, protection,
                );
                let first = group
                    .first()
                    .expect("random-tick candidate groups are non-empty")
                    .1;
                RandomTickRegionPlan {
                    region: RegionKey::from_chunk(first.sample.chunk.0, first.sample.chunk.1),
                    group,
                    plan,
                }
            })
            .collect::<Vec<_>>();
        let unique_regions = plans
            .iter()
            .map(|planned| planned.region)
            .collect::<HashSet<_>>()
            .len()
            == plans.len();
        (unique_regions
            && plans.iter().all(|planned| {
                !planned.plan.edits.is_empty()
                    && random_tick_plan_fits_region(planned.region, &planned.plan)
                    && random_tick_resident_inputs(&planned.plan, table).is_some()
            }))
        .then_some(plans)
    } else {
        None
    };
    if let Some(plans) = fanout_plans {
        groups.clear();
        match commit_random_tick_region_fanout(
            sessions,
            world_read,
            world_mutation.expect("random-tick fanout requires a mutation view"),
            config.block_light.clone(),
            world_tick,
            cpu_resources.expect("random-tick fanout requires CPU resources"),
            plans,
            #[cfg(test)]
            probe.clone(),
        )
        .await
        {
            Ok((additional, fanout_eligible, leaf_drops, fallback)) => {
                eligible += fanout_eligible;
                applied_by_pass.extend(additional.applied.iter().map(|edit| edit.pos));
                append_resident_block_outcome(&mut outcome, additional);
                accepted_leaf_drops.extend(leaf_drops);
                groups.extend(fallback);
            }
            Err(()) => {
                return RandomTickReport {
                    sampled,
                    eligible,
                    applied: 0,
                };
            }
        }
    }
    'groups: while let Some(mut group) = groups.pop_front() {
        let planning_candidates = group
            .iter()
            .map(|(_, candidate)| *candidate)
            .collect::<Vec<_>>();
        let planning_chunks = random_tick_planning_chunks(&planning_candidates);
        let snapshot = world_read.snapshot_chunks(&planning_chunks);
        for (_, candidate) in &mut group {
            if applied_by_pass.contains(&candidate.sample.pos)
                && let Some(state) = snapshot.get_cached_block(candidate.sample.pos)
            {
                candidate.state = state;
            }
        }
        let plan = plan_indexed_random_tick_edits(
            config, policy, world_tick, &snapshot, &group, protection,
        );
        // The plan owns every edit and precondition needed below. Releasing the
        // snapshot here lets the resident mutation update its chunk in place.
        drop(snapshot);
        eligible += plan.eligible;
        if plan.edits.is_empty() {
            continue;
        }
        let resident_result = if random_tick_plan_fits_resident_region(&plan)
            && let Some(mutation) = world_mutation
            && let Some((edits, preconditions)) = random_tick_resident_inputs(&plan, table)
        {
            if wave.is_none() {
                wave = Some(ResidentWorldJournalWave::checkpoint_only());
            }
            Some(
                wave.as_mut()
                    .expect("resident journal wave was initialized")
                    .commit_block_edits(
                        mutation,
                        ResidentBlockCommit {
                            edits: &edits,
                            preconditions: &preconditions,
                            consumed_block_ticks: &[],
                            consumed_fluid_ticks: &[],
                            scheduled_fluid_ticks: &[],
                            light_table: table,
                            leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                        },
                    ),
            )
        } else {
            None
        };
        let mut plan_applied_positions = HashSet::new();
        match resident_result {
            Some(mc_world::ResidentBlockEditBatchResult::Applied(applied)) => {
                plan_applied_positions.extend(applied.iter().map(|edit| edit.pos));
                let additional = simulation::resident_block_edit_result_outcome(
                    mc_world::ResidentBlockEditBatchResult::Applied(applied),
                )
                .expect("applied resident random ticks have an outcome");
                append_resident_block_outcome(&mut outcome, additional);
            }
            Some(mc_world::ResidentBlockEditBatchResult::Stale) => {
                if group.len() > 1 {
                    for candidate in group.into_iter().rev() {
                        groups.push_front(vec![candidate]);
                    }
                }
                continue;
            }
            None
            | Some(mc_world::ResidentBlockEditBatchResult::Missing)
            | Some(mc_world::ResidentBlockEditBatchResult::CrossRegion) => {
                if let Some(current_wave) = wave.take()
                    && let Some(mutation) = world_mutation
                    && current_wave
                        .finish(sessions, world_read, mutation, world_tick)
                        .await
                        .is_err()
                {
                    return RandomTickReport {
                        sampled,
                        eligible,
                        applied: 0,
                    };
                }
                let mut coordinator_wave = ResidentWorldJournalWave::checkpoint_only();
                loop {
                    if coordinator_wave
                        .wait_for_append_turn(sessions)
                        .await
                        .is_err()
                    {
                        return RandomTickReport {
                            sampled,
                            eligible,
                            applied: 0,
                        };
                    }
                    let mut storage = crate::lock_metrics::timed_guard(
                        crate::lock_metrics::LockMetricKind::WorldStorage,
                        "random tick region-boundary commit",
                        Instant::now(),
                        world.lock().await,
                    );
                    let coordinator_mutation = world_mutation
                        .cloned()
                        .unwrap_or_else(|| storage.mutation_view());
                    if !snapshot_read_preconditions_are_current(&storage, &plan.preconditions) {
                        if coordinator_wave
                            .finish_coordinator(
                                sessions,
                                &coordinator_mutation,
                                world_tick,
                                &[],
                                Vec::new(),
                            )
                            .await
                            .is_err()
                        {
                            return RandomTickReport {
                                sampled,
                                eligible,
                                applied: 0,
                            };
                        }
                        if group.len() > 1 {
                            for candidate in group.into_iter().rev() {
                                groups.push_front(vec![candidate]);
                            }
                        }
                        continue 'groups;
                    }

                    let journal_positions =
                        coordinator_random_tick_journal_positions(&storage, &plan.edits);
                    if let Some(decision_id) = coordinator_wave.decision_id {
                        match storage
                            .stamp_cached_chunks_for_world_journal(decision_id, &journal_positions)
                        {
                            mc_world::JournalStampResult::Stamped(_) => {}
                            mc_world::JournalStampResult::NewerDecision(_) => {
                                if coordinator_wave
                                    .finish_coordinator(
                                        sessions,
                                        &coordinator_mutation,
                                        world_tick,
                                        &[],
                                        Vec::new(),
                                    )
                                    .await
                                    .is_err()
                                {
                                    return RandomTickReport {
                                        sampled,
                                        eligible,
                                        applied: 0,
                                    };
                                }
                                drop(storage);
                                coordinator_wave = ResidentWorldJournalWave::checkpoint_only();
                                continue;
                            }
                            mc_world::JournalStampResult::Missing => {
                                warn!("coordinator world journal snapshot was incomplete");
                                sessions.report_world_chunk_journal_failure();
                                return RandomTickReport {
                                    sampled,
                                    eligible,
                                    applied: 0,
                                };
                            }
                        }
                    }

                    let mut boundary_outcome = BlockEditBatchOutcome::default();
                    for edit in &plan.edits {
                        apply_block_edit_to_storage(
                            &mut storage,
                            table,
                            edit,
                            &mut boundary_outcome,
                        );
                    }
                    let leaf_tick_chunks = schedule_leaf_ticks_near_applied(
                        &mut storage,
                        world_tick,
                        &boundary_outcome.applied,
                    );
                    debug_assert!(
                        leaf_tick_chunks
                            .iter()
                            .all(|position| journal_positions.contains(position))
                    );
                    let journal_snapshots = journal_positions
                        .iter()
                        .filter_map(|position| storage.cached_chunk_snapshot(*position))
                        .collect::<Vec<_>>();
                    if journal_snapshots.len() != journal_positions.len() {
                        warn!("coordinator world journal snapshot was incomplete after mutation");
                        sessions.report_world_chunk_journal_failure();
                        return RandomTickReport {
                            sampled,
                            eligible,
                            applied: 0,
                        };
                    }
                    plan_applied_positions
                        .extend(boundary_outcome.applied.iter().map(|edit| edit.pos));
                    if coordinator_wave
                        .finish_coordinator(
                            sessions,
                            &coordinator_mutation,
                            world_tick,
                            &journal_positions,
                            journal_snapshots,
                        )
                        .await
                        .is_err()
                    {
                        return RandomTickReport {
                            sampled,
                            eligible,
                            applied: 0,
                        };
                    }
                    drop(storage);
                    append_resident_block_outcome(&mut outcome, boundary_outcome);
                    break;
                }
            }
        }
        applied_by_pass.extend(&plan_applied_positions);
        accepted_leaf_drops.extend(
            plan.leaf_drops
                .into_iter()
                .filter(|drop| plan_applied_positions.contains(&drop.source)),
        );
    }
    #[cfg(test)]
    RANDOM_TICK_PLANNING_COMPLETION_COUNT.with(|count| count.set(count.get() + 1));
    if let Some(current_wave) = wave
        && let Some(mutation) = world_mutation
        && current_wave
            .finish(sessions, world_read, mutation, world_tick)
            .await
            .is_err()
    {
        return RandomTickReport {
            sampled,
            eligible,
            applied: 0,
        };
    }

    if !outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates =
                collect_server_origin_light_updates(world, sessions, table, &outcome).await;
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
        }
    }

    if let Some(entity_type_id) = item_entity_type_id(&config.entity_types) {
        for drop in accepted_leaf_drops {
            dispatch_visibility_commands(sessions.spawn_item_drop_checkpoint_only_owned(
                authority,
                entity_type_id,
                drop.position,
                drop.stack,
            ));
        }
    }

    if eligible > 0 {
        debug!(
            world_tick,
            sampled,
            eligible,
            applied = outcome.applied.len(),
            "random tick sampled eligible blocks"
        );
    }
    RandomTickReport {
        sampled,
        eligible,
        applied: outcome.applied.len(),
    }
}

async fn commit_resident_block_edits(
    sessions: &SessionRegistry,
    world_read: &mc_world::WorldReadView,
    mutation: &mc_world::WorldMutationView,
    world_tick: u64,
    commit: ResidentBlockCommit<'_>,
) -> Result<Option<BlockEditBatchOutcome>, ()> {
    let Some(journal) = sessions.world_chunk_journal() else {
        let result = if !commit.consumed_block_ticks.is_empty() {
            mutation.apply_scheduled_block_tick_plan_conditionally(
                &mc_world::ResidentScheduledBlockTickPlan {
                    consumed_ticks: commit.consumed_block_ticks,
                    edits: commit.edits,
                    preconditions: commit.preconditions,
                    light_table: commit.light_table,
                    leaf_trigger_tick: commit.leaf_trigger_tick,
                },
            )
        } else if commit.consumed_fluid_ticks.is_empty() && commit.scheduled_fluid_ticks.is_empty()
        {
            mutation.apply_block_edits_conditionally(
                commit.edits,
                commit.preconditions,
                &[],
                commit.light_table,
                commit.leaf_trigger_tick,
            )
        } else {
            mutation.apply_fluid_tick_plan_conditionally(&mc_world::ResidentFluidTickPlan {
                consumed_ticks: commit.consumed_fluid_ticks,
                edits: commit.edits,
                preconditions: commit.preconditions,
                scheduled_ticks: commit.scheduled_fluid_ticks,
                light_table: commit.light_table,
                leaf_trigger_tick: commit.leaf_trigger_tick,
            })
        };
        return Ok(match result {
            mc_world::ResidentBlockEditBatchResult::Applied(applied) => {
                simulation::resident_block_edit_result_outcome(
                    mc_world::ResidentBlockEditBatchResult::Applied(applied),
                )
            }
            mc_world::ResidentBlockEditBatchResult::Stale => Some(BlockEditBatchOutcome::default()),
            mc_world::ResidentBlockEditBatchResult::Missing
            | mc_world::ResidentBlockEditBatchResult::CrossRegion => None,
        });
    };

    let reservation = tokio::task::spawn_blocking({
        let journal = journal.clone();
        move || journal.reserve_decision_ids(1)
    })
    .await;
    let decision_id = match reservation {
        Ok(Ok(ids)) => ids
            .into_iter()
            .next()
            .expect("one requested journal decision id is returned"),
        Ok(Err(error)) => {
            warn!(%error, "resident block journal decision reservation failed");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
        Err(error) => {
            warn!(?error, "resident block journal reservation worker failed");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
    };
    let (result, touched) = if !commit.consumed_block_ticks.is_empty() {
        mutation.apply_scheduled_block_tick_plan_conditionally_journaled(
            decision_id,
            &mc_world::ResidentScheduledBlockTickPlan {
                consumed_ticks: commit.consumed_block_ticks,
                edits: commit.edits,
                preconditions: commit.preconditions,
                light_table: commit.light_table,
                leaf_trigger_tick: commit.leaf_trigger_tick,
            },
        )
    } else if commit.consumed_fluid_ticks.is_empty() && commit.scheduled_fluid_ticks.is_empty() {
        mutation.apply_block_edits_conditionally_journaled(
            decision_id,
            commit.edits,
            commit.preconditions,
            &[],
            commit.light_table,
            commit.leaf_trigger_tick,
        )
    } else {
        mutation.apply_fluid_tick_plan_conditionally_journaled(
            decision_id,
            &mc_world::ResidentFluidTickPlan {
                consumed_ticks: commit.consumed_fluid_ticks,
                edits: commit.edits,
                preconditions: commit.preconditions,
                scheduled_ticks: commit.scheduled_fluid_ticks,
                light_table: commit.light_table,
                leaf_trigger_tick: commit.leaf_trigger_tick,
            },
        )
    };
    let applied = match result {
        mc_world::ResidentBlockEditBatchResult::Applied(applied) => applied,
        mc_world::ResidentBlockEditBatchResult::Stale => {
            record_empty_resident_journal_decision(
                sessions,
                journal.clone(),
                world_tick,
                decision_id,
            )
            .await?;
            return Ok(Some(BlockEditBatchOutcome::default()));
        }
        mc_world::ResidentBlockEditBatchResult::Missing
        | mc_world::ResidentBlockEditBatchResult::CrossRegion => {
            record_empty_resident_journal_decision(
                sessions,
                journal.clone(),
                world_tick,
                decision_id,
            )
            .await?;
            return Ok(None);
        }
    };
    let snapshots = world_read.snapshot_chunks(&touched);
    let snapshots = touched
        .iter()
        .filter_map(|position| snapshots.chunk(*position))
        .collect::<Vec<_>>();
    if snapshots.len() != touched.len() {
        warn!("resident block journal snapshot was incomplete");
        sessions.report_world_chunk_journal_failure();
        return Err(());
    }
    let append = tokio::task::spawn_blocking({
        let journal = journal.clone();
        move || journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, snapshots)])
    })
    .await;
    match append {
        Ok(Ok(())) => {
            mutation.clear_journal_pending_conditionally(decision_id, &touched);
        }
        Ok(Err(error)) => {
            warn!(
                outcome_unknown = error.outcome_unknown(),
                %error,
                "resident block journal append failed"
            );
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
        Err(error) => {
            warn!(?error, "resident block journal append worker failed");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
    }
    Ok(simulation::resident_block_edit_result_outcome(
        mc_world::ResidentBlockEditBatchResult::Applied(applied),
    ))
}

async fn record_empty_resident_journal_decision(
    sessions: &SessionRegistry,
    journal: world_journal::WorldChunkJournal,
    world_tick: u64,
    decision_id: u64,
) -> Result<(), ()> {
    let append = tokio::task::spawn_blocking(move || {
        journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, Vec::new())])
    })
    .await;
    match append {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            warn!(
                outcome_unknown = error.outcome_unknown(),
                %error,
                "empty resident block journal decision append failed"
            );
            sessions.report_world_chunk_journal_failure();
            Err(())
        }
        Err(error) => {
            warn!(?error, "empty resident block journal append worker failed");
            sessions.report_world_chunk_journal_failure();
            Err(())
        }
    }
}

async fn commit_cross_region_scheduled_block_tick(
    sessions: &SessionRegistry,
    mutation: &mc_world::WorldMutationView,
    world_tick: u64,
    commit: ResidentBlockCommit<'_>,
) -> Result<Option<BlockEditBatchOutcome>, ()> {
    let Some(journal) = sessions.world_chunk_journal() else {
        warn!("cross-region scheduled block transaction requires a world journal");
        sessions.report_world_chunk_journal_failure();
        return Err(());
    };
    let mutation = mutation.clone();
    let edits = commit.edits.to_vec();
    let preconditions = commit.preconditions.to_vec();
    let consumed_ticks = commit.consumed_block_ticks.to_vec();
    let light_table = commit.light_table.cloned();
    let leaf_trigger_tick = commit.leaf_trigger_tick;
    let runtime = tokio::runtime::Handle::current();
    let failure = sessions.world_chunk_journal_failure_reporter();
    let worker = tokio::task::spawn_blocking(move || {
        let fail_stop = || {
            failure.send_replace(true);
            Err(())
        };
        let decision_id = match journal.reserve_decision_ids(1) {
            Ok(ids) => ids[0],
            Err(error) => {
                warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block journal decision reservation failed");
                return fail_stop();
            }
        };
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(decision_id),
            &mc_world::ResidentScheduledBlockTickPlan {
                consumed_ticks: &consumed_ticks,
                edits: &edits,
                preconditions: &preconditions,
                light_table: light_table.as_ref(),
                leaf_trigger_tick,
            },
        );
        if let Err(error) = runtime.block_on(journal.wait_for_append_turn(decision_id)) {
            warn!(%error, "cross-region scheduled block journal append turn failed");
            return fail_stop();
        }
        let close_empty = || {
            journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, Vec::new())])
        };
        let transaction = match prepared {
            mc_world::resident::ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(
                transaction,
            ) => transaction,
            mc_world::resident::ResidentCrossRegionScheduledBlockTickPrepareResult::Stale => {
                return match close_empty() {
                    Ok(()) => Ok(Some(BlockEditBatchOutcome::default())),
                    Err(error) => {
                        warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block stale reservation closure failed");
                        fail_stop()
                    }
                };
            }
            mc_world::resident::ResidentCrossRegionScheduledBlockTickPrepareResult::Missing => {
                return match close_empty() {
                    Ok(()) => Ok(None),
                    Err(error) => {
                        warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block missing reservation closure failed");
                        fail_stop()
                    }
                };
            }
        };
        match transaction.commit_durably(|snapshots| {
            journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, snapshots)])
        }) {
            mc_world::resident::ResidentCrossRegionScheduledBlockTickCommitResult::Applied(
                applied,
            ) => Ok(simulation::resident_block_edit_result_outcome(
                mc_world::ResidentBlockEditBatchResult::Applied(applied),
            )),
            mc_world::resident::ResidentCrossRegionScheduledBlockTickCommitResult::Stale => {
                match close_empty() {
                    Ok(()) => Ok(Some(BlockEditBatchOutcome::default())),
                    Err(error) => {
                        warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block stale commit closure failed");
                        fail_stop()
                    }
                }
            }
            mc_world::resident::ResidentCrossRegionScheduledBlockTickCommitResult::Missing => {
                match close_empty() {
                    Ok(()) => Ok(None),
                    Err(error) => {
                        warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block missing commit closure failed");
                        fail_stop()
                    }
                }
            }
            mc_world::resident::ResidentCrossRegionScheduledBlockTickCommitResult::DurabilityFailed(error) => {
                let outcome_unknown = error.outcome_unknown();
                warn!(outcome_unknown, %error, "cross-region scheduled block journal append failed");
                if outcome_unknown {
                    return fail_stop();
                }
                match close_empty() {
                    Ok(()) => Ok(Some(BlockEditBatchOutcome::default())),
                    Err(error) => {
                        warn!(outcome_unknown = error.outcome_unknown(), %error, "cross-region scheduled block failed reservation closure");
                        fail_stop()
                    }
                }
            }
        }
    })
    .await;
    match worker {
        Ok(result) => result,
        Err(error) => {
            warn!(
                ?error,
                "cross-region scheduled block transaction worker failed"
            );
            sessions.report_world_chunk_journal_failure();
            Err(())
        }
    }
}

struct ResidentWorldJournalWave {
    journal: Option<world_journal::WorldChunkJournal>,
    decision_id: Option<u64>,
    touched: HashSet<ChunkPos>,
}

impl ResidentWorldJournalWave {
    fn checkpoint_only() -> Self {
        Self {
            journal: None,
            decision_id: None,
            touched: HashSet::new(),
        }
    }

    fn decision_id_or(&self, fallback: u64) -> u64 {
        self.decision_id.unwrap_or(fallback)
    }

    async fn begin(sessions: &SessionRegistry) -> Result<Self, ()> {
        let Some(journal) = sessions.world_chunk_journal() else {
            return Ok(Self::checkpoint_only());
        };
        let reservation = tokio::task::spawn_blocking({
            let journal = journal.clone();
            move || journal.reserve_decision_ids(1)
        })
        .await;
        let decision_id = match reservation {
            Ok(Ok(ids)) => ids
                .into_iter()
                .next()
                .expect("one requested journal decision id is returned"),
            Ok(Err(error)) => {
                warn!(%error, "resident world journal decision reservation failed");
                sessions.report_world_chunk_journal_failure();
                return Err(());
            }
            Err(error) => {
                warn!(?error, "resident world journal reservation worker failed");
                sessions.report_world_chunk_journal_failure();
                return Err(());
            }
        };
        Ok(Self {
            journal: Some(journal),
            decision_id: Some(decision_id),
            touched: HashSet::new(),
        })
    }

    fn commit_block_edits(
        &mut self,
        mutation: &mc_world::WorldMutationView,
        commit: ResidentBlockCommit<'_>,
    ) -> mc_world::ResidentBlockEditBatchResult {
        let (result, touched) = if let Some(decision_id) = self.decision_id {
            if !commit.consumed_block_ticks.is_empty() {
                mutation.apply_scheduled_block_tick_plan_conditionally_journaled(
                    decision_id,
                    &mc_world::ResidentScheduledBlockTickPlan {
                        consumed_ticks: commit.consumed_block_ticks,
                        edits: commit.edits,
                        preconditions: commit.preconditions,
                        light_table: commit.light_table,
                        leaf_trigger_tick: commit.leaf_trigger_tick,
                    },
                )
            } else if commit.consumed_fluid_ticks.is_empty()
                && commit.scheduled_fluid_ticks.is_empty()
            {
                mutation.apply_block_edits_conditionally_journaled(
                    decision_id,
                    commit.edits,
                    commit.preconditions,
                    &[],
                    commit.light_table,
                    commit.leaf_trigger_tick,
                )
            } else {
                mutation.apply_fluid_tick_plan_conditionally_journaled(
                    decision_id,
                    &mc_world::ResidentFluidTickPlan {
                        consumed_ticks: commit.consumed_fluid_ticks,
                        edits: commit.edits,
                        preconditions: commit.preconditions,
                        scheduled_ticks: commit.scheduled_fluid_ticks,
                        light_table: commit.light_table,
                        leaf_trigger_tick: commit.leaf_trigger_tick,
                    },
                )
            }
        } else {
            let result = if !commit.consumed_block_ticks.is_empty() {
                mutation.apply_scheduled_block_tick_plan_conditionally(
                    &mc_world::ResidentScheduledBlockTickPlan {
                        consumed_ticks: commit.consumed_block_ticks,
                        edits: commit.edits,
                        preconditions: commit.preconditions,
                        light_table: commit.light_table,
                        leaf_trigger_tick: commit.leaf_trigger_tick,
                    },
                )
            } else if commit.consumed_fluid_ticks.is_empty()
                && commit.scheduled_fluid_ticks.is_empty()
            {
                mutation.apply_block_edits_conditionally(
                    commit.edits,
                    commit.preconditions,
                    &[],
                    commit.light_table,
                    commit.leaf_trigger_tick,
                )
            } else {
                mutation.apply_fluid_tick_plan_conditionally(&mc_world::ResidentFluidTickPlan {
                    consumed_ticks: commit.consumed_fluid_ticks,
                    edits: commit.edits,
                    preconditions: commit.preconditions,
                    scheduled_ticks: commit.scheduled_fluid_ticks,
                    light_table: commit.light_table,
                    leaf_trigger_tick: commit.leaf_trigger_tick,
                })
            };
            (result, Vec::new())
        };
        if matches!(result, mc_world::ResidentBlockEditBatchResult::Applied(_)) {
            self.touched.extend(touched);
        }
        result
    }

    fn commit_hopper(
        &mut self,
        mutation: &mc_world::WorldMutationView,
        consumed_tick: &ScheduledBlockTick,
        plan: &mc_world::ResidentHopperTransferPlan,
    ) -> mc_world::ResidentHopperTransferCommitResult {
        let Some(decision_id) = self.decision_id else {
            return mutation.commit_scheduled_hopper_transfer_conditionally(
                std::slice::from_ref(consumed_tick),
                plan,
            );
        };
        let (result, touched) = mutation.commit_scheduled_hopper_transfer_conditionally_journaled(
            decision_id,
            std::slice::from_ref(consumed_tick),
            plan,
        );
        if result == mc_world::ResidentHopperTransferCommitResult::Applied {
            self.touched.extend(touched);
        }
        result
    }

    fn commit_opaque_block_entity(
        &mut self,
        mutation: &mc_world::WorldMutationView,
        position: mc_world::BlockPos,
        expected_state: BlockStateId,
        expected_token: mc_world::BlockMutationToken,
        bytes: Vec<u8>,
    ) -> mc_world::ResidentOpaqueBlockEntityCommitResult {
        let Some(decision_id) = self.decision_id else {
            return mutation.commit_opaque_block_entity_conditionally(
                position,
                expected_state,
                expected_token,
                bytes,
            );
        };
        let (result, touched) = mutation.commit_opaque_block_entity_conditionally_journaled(
            decision_id,
            position,
            expected_state,
            expected_token,
            bytes,
        );
        if result == mc_world::ResidentOpaqueBlockEntityCommitResult::Applied {
            self.touched.extend(touched);
        }
        result
    }

    async fn wait_for_append_turn(&self, sessions: &SessionRegistry) -> Result<(), ()> {
        let (Some(journal), Some(decision_id)) = (&self.journal, self.decision_id) else {
            return Ok(());
        };
        if let Err(error) = journal.wait_for_append_turn(decision_id).await {
            warn!(%error, "world journal append turn failed");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
        Ok(())
    }

    async fn finish(
        self,
        sessions: &SessionRegistry,
        world_read: &mc_world::WorldReadView,
        mutation: &mc_world::WorldMutationView,
        world_tick: u64,
    ) -> Result<(), ()> {
        let (Some(journal), Some(decision_id)) = (self.journal, self.decision_id) else {
            return Ok(());
        };
        let mut touched = self.touched.into_iter().collect::<Vec<_>>();
        touched.sort_unstable_by_key(|position| (position.x, position.z));
        let snapshots = world_read.snapshot_chunks(&touched);
        let snapshots = touched
            .iter()
            .filter_map(|position| snapshots.chunk(*position))
            .collect::<Vec<_>>();
        if snapshots.len() != touched.len() {
            warn!("resident world journal snapshot was incomplete");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
        let append = tokio::task::spawn_blocking({
            let journal = journal.clone();
            move || {
                journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, snapshots)])
            }
        })
        .await;
        match append {
            Ok(Ok(())) => {
                mutation.clear_journal_pending_conditionally(decision_id, &touched);
                Ok(())
            }
            Ok(Err(error)) => {
                warn!(
                    outcome_unknown = error.outcome_unknown(),
                    %error,
                    "resident world journal append failed"
                );
                sessions.report_world_chunk_journal_failure();
                Err(())
            }
            Err(error) => {
                warn!(?error, "resident world journal append worker failed");
                sessions.report_world_chunk_journal_failure();
                Err(())
            }
        }
    }

    async fn finish_coordinator(
        self,
        sessions: &SessionRegistry,
        mutation: &mc_world::WorldMutationView,
        world_tick: u64,
        positions: &[ChunkPos],
        snapshots: Vec<mc_world::ChunkSnapshot>,
    ) -> Result<(), ()> {
        let (Some(journal), Some(decision_id)) = (self.journal, self.decision_id) else {
            return Ok(());
        };
        let append = tokio::task::spawn_blocking(move || {
            journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, snapshots)])
        })
        .await;
        match append {
            Ok(Ok(())) => {
                mutation.clear_journal_pending_conditionally(decision_id, positions);
                Ok(())
            }
            Ok(Err(error)) => {
                warn!(
                    outcome_unknown = error.outcome_unknown(),
                    %error,
                    "coordinator world journal append failed"
                );
                sessions.report_world_chunk_journal_failure();
                Err(())
            }
            Err(error) => {
                warn!(?error, "coordinator world journal append worker failed");
                sessions.report_world_chunk_journal_failure();
                Err(())
            }
        }
    }
}

fn random_tick_resident_inputs(
    plan: &RandomTickPlan,
    table: Option<&mc_data::block_light::BlockLightTable>,
) -> Option<(
    Vec<mc_world::ResidentBlockEdit>,
    Vec<mc_world::ResidentBlockPrecondition>,
)> {
    resident_block_edit_inputs(&plan.edits, &plan.preconditions, table)
}

fn random_tick_plan_fits_resident_region(plan: &RandomTickPlan) -> bool {
    let mut positions = plan.edits.iter().map(|edit| edit.pos).chain(
        plan.preconditions
            .iter()
            .map(|precondition| precondition.pos),
    );
    let Some(first) = positions.next() else {
        return true;
    };
    let owner = block_world_region(first);
    if positions.any(|position| block_world_region(position) != owner) {
        return false;
    }
    plan.edits.iter().all(|edit| {
        [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .all(|(dx, dz)| {
                let Some(x) = edit.pos.x.checked_add(dx) else {
                    return false;
                };
                let Some(z) = edit.pos.z.checked_add(dz) else {
                    return false;
                };
                block_world_region(mc_world::BlockPos {
                    x,
                    y: edit.pos.y,
                    z,
                }) == owner
            })
    })
}

fn random_tick_plan_fits_region(region: RegionKey, plan: &RandomTickPlan) -> bool {
    random_tick_plan_fits_resident_region(plan)
        && plan
            .edits
            .iter()
            .map(|edit| edit.pos)
            .chain(
                plan.preconditions
                    .iter()
                    .map(|precondition| precondition.pos),
            )
            .all(|position| block_world_region(position) == (region.x, region.z))
}

fn resident_block_journal_chunks(
    world_read: &mc_world::WorldReadView,
    edits: &[BlockEdit],
) -> Vec<ChunkPos> {
    let mut positions = edits
        .iter()
        .flat_map(|edit| std::iter::once(edit.pos).chain(fluid_neighbour_positions(edit.pos)))
        .map(|position| ChunkPos {
            x: position.x.div_euclid(SECTION_DIM as i32),
            z: position.z.div_euclid(SECTION_DIM as i32),
        })
        .collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions.dedup();
    let snapshots = world_read.snapshot_chunks(&positions);
    positions.retain(|position| snapshots.chunk(*position).is_some());
    positions
}

fn block_world_region(position: mc_world::BlockPos) -> (i32, i32) {
    let chunk_x = position.x.div_euclid(SECTION_DIM as i32);
    let chunk_z = position.z.div_euclid(SECTION_DIM as i32);
    (
        chunk_x.div_euclid(mc_entity::REGION_SIZE_CHUNKS),
        chunk_z.div_euclid(mc_entity::REGION_SIZE_CHUNKS),
    )
}

fn resident_block_edit_inputs(
    edits: &[BlockEdit],
    preconditions: &[SnapshotReadPrecondition],
    table: Option<&mc_data::block_light::BlockLightTable>,
) -> Option<(
    Vec<mc_world::ResidentBlockEdit>,
    Vec<mc_world::ResidentBlockPrecondition>,
)> {
    let preconditions = preconditions
        .iter()
        .map(|precondition| {
            Some(mc_world::ResidentBlockPrecondition {
                pos: precondition.pos,
                expected_state: precondition.expected_state?,
                expected_token: precondition.expected_token?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let edits = edits
        .iter()
        .map(|edit| mc_world::ResidentBlockEdit {
            pos: edit.pos,
            new_state: edit.new_state,
            preserve_light: table.is_some_and(|table| {
                preconditions
                    .iter()
                    .find(|precondition| precondition.pos == edit.pos)
                    .is_some_and(|precondition| {
                        !block_edit_changes_light(
                            table,
                            precondition.expected_state,
                            edit.new_state,
                        )
                    })
            }),
        })
        .collect();
    Some((edits, preconditions))
}

fn random_tick_planning_chunks(candidates: &[RandomTickCandidate]) -> Vec<ChunkPos> {
    let mut positions = HashSet::new();
    for candidate in candidates {
        let centre = ChunkPos {
            x: candidate.sample.chunk.0,
            z: candidate.sample.chunk.1,
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (Some(x), Some(z)) = (centre.x.checked_add(dx), centre.z.checked_add(dz))
                else {
                    continue;
                };
                positions.insert(ChunkPos { x, z });
            }
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions
}

#[cfg(test)]
fn plan_random_tick_edits(
    config: &ServerConfig,
    policy: RandomTickPolicy,
    world_tick: u64,
    snapshot: &mc_world::WorldReadSnapshot,
    candidates: &[RandomTickCandidate],
) -> RandomTickPlan {
    let indexed = candidates.iter().copied().enumerate().collect::<Vec<_>>();
    plan_indexed_random_tick_edits(config, policy, world_tick, snapshot, &indexed, None)
}

#[cfg(test)]
fn plan_random_tick_region_edits(
    config: &ServerConfig,
    policy: RandomTickPolicy,
    world_tick: u64,
    snapshot: &mc_world::WorldReadSnapshot,
    candidates: &[RandomTickCandidate],
) -> Vec<RandomTickPlan> {
    random_tick_candidate_groups(candidates)
        .into_iter()
        .map(|candidates| {
            plan_indexed_random_tick_edits(config, policy, world_tick, snapshot, &candidates, None)
        })
        .collect()
}

fn random_tick_candidate_groups(
    candidates: &[RandomTickCandidate],
) -> Vec<Vec<(usize, RandomTickCandidate)>> {
    let mut groups = Vec::<(((i32, i32), bool), Vec<(usize, RandomTickCandidate)>)>::new();
    for (candidate_index, candidate) in candidates.iter().copied().enumerate() {
        let region = (
            candidate
                .sample
                .chunk
                .0
                .div_euclid(mc_entity::REGION_SIZE_CHUNKS),
            candidate
                .sample
                .chunk
                .1
                .div_euclid(mc_entity::REGION_SIZE_CHUNKS),
        );
        let region_axis_blocks = SECTION_DIM as i32 * mc_entity::REGION_SIZE_CHUNKS;
        let x_in_region = candidate.sample.pos.x.rem_euclid(region_axis_blocks);
        let z_in_region = candidate.sample.pos.z.rem_euclid(region_axis_blocks);
        let boundary_margin = 4;
        let has_region_boundary_neighbour = x_in_region < boundary_margin
            || x_in_region >= region_axis_blocks - boundary_margin
            || z_in_region < boundary_margin
            || z_in_region >= region_axis_blocks - boundary_margin;
        let key = (region, has_region_boundary_neighbour);
        if let Some((last_key, group)) = groups.last_mut()
            && *last_key == key
        {
            group.push((candidate_index, candidate));
        } else {
            groups.push((key, vec![(candidate_index, candidate)]));
        }
    }
    groups
        .into_iter()
        .map(|(_, candidates)| candidates)
        .collect()
}

fn plan_indexed_random_tick_edits(
    config: &ServerConfig,
    policy: RandomTickPolicy,
    world_tick: u64,
    snapshot: &mc_world::WorldReadSnapshot,
    candidates: &[(usize, RandomTickCandidate)],
    protection: Option<&crate::script::ZoneProtectionSnapshot>,
) -> RandomTickPlan {
    let mut world = SnapshotPlanningWorld::new(snapshot);
    let mut edited_positions = HashSet::new();
    let mut plan = RandomTickPlan::default();
    for &(candidate_index, candidate) in candidates {
        let Some(state) = world.get_cached_block(candidate.sample.pos) else {
            continue;
        };
        if state != candidate.state && !edited_positions.contains(&candidate.sample.pos) {
            continue;
        }
        let Some(family) = config.block_facts.random_tick_family(state.0) else {
            continue;
        };
        plan.eligible += 1;
        let Some(edits) = random_tick_edit_seeded(
            &config.blocks,
            &config.block_facts,
            &world,
            candidate.sample.pos,
            state,
            family,
            random_tick_candidate_seed(
                policy.seed,
                world_tick,
                candidate.sample.pos,
                candidate_index,
            ),
        ) else {
            continue;
        };
        if edits
            .iter()
            .any(|edit| world.get_cached_block(edit.pos).is_none())
        {
            continue;
        }
        let applied_before = plan.edits.len();
        for edit in edits {
            if !ambient_random_tick_edit_allowed(family, candidate.sample.pos, edit.pos, protection)
            {
                continue;
            }
            if world.apply(edit) {
                edited_positions.insert(edit.pos);
                plan.edits.push(edit);
            }
        }
        if family == mc_data::block_facts::RandomTickFamily::Leaves
            && plan.edits.len() > applied_before
        {
            let rolls = leaf_decay_drop_rolls(policy.seed, world_tick, candidate.sample.pos);
            plan.leaf_drops.extend(
                natural_leaf_decay_drops(&config.blocks, &config.items, state, rolls)
                    .into_iter()
                    .map(|stack| RandomTickLeafDrop {
                        source: candidate.sample.pos,
                        position: Vec3::new(
                            f64::from(candidate.sample.pos.x) + 0.5,
                            f64::from(candidate.sample.pos.y) + 0.5,
                            f64::from(candidate.sample.pos.z) + 0.5,
                        ),
                        stack: entity_item_stack(stack),
                    }),
            );
        }
    }
    plan.preconditions = world.preconditions();
    plan
}

fn ambient_random_tick_edit_allowed(
    family: mc_data::block_facts::RandomTickFamily,
    source: mc_world::BlockPos,
    target: mc_world::BlockPos,
    protection: Option<&crate::script::ZoneProtectionSnapshot>,
) -> bool {
    family != mc_data::block_facts::RandomTickFamily::Fire
        || target == source
        || protection.is_none_or(|protection| {
            protection.ambient_block_mutation_allowed("minecraft:overworld", target)
        })
}

fn snapshot_read_preconditions_are_current(
    storage: &mc_world::WorldStorage,
    preconditions: &[SnapshotReadPrecondition],
) -> bool {
    preconditions.iter().all(|precondition| {
        storage.get_cached_block(precondition.pos) == precondition.expected_state
            && storage.block_mutation_token(precondition.pos) == precondition.expected_token
    })
}

fn random_tick_candidates(
    world_read: &mc_world::WorldReadView,
    facts: &mc_data::block_facts::BlockFactsTable,
    samples: &[RandomTickSample],
) -> (usize, Vec<RandomTickCandidate>) {
    let mut chunk_positions = samples
        .iter()
        .map(|sample| ChunkPos {
            x: sample.chunk.0,
            z: sample.chunk.1,
        })
        .collect::<Vec<_>>();
    chunk_positions.sort_unstable_by_key(|pos| (pos.x, pos.z));
    chunk_positions.dedup();
    let snapshot = world_read.snapshot_chunks(&chunk_positions);
    let active_sections = chunk_positions
        .into_iter()
        .map(|position| {
            let sections = snapshot
                .chunk(position)
                .map(|chunk| {
                    std::array::from_fn(|section| {
                        section_may_random_tick(&chunk.sections[section], facts)
                    })
                })
                .unwrap_or([false; mc_world::SECTION_COUNT]);
            ((position.x, position.z), sections)
        })
        .collect::<HashMap<_, _>>();

    let mut sampled = 0usize;
    let mut candidates = Vec::new();
    for &sample in samples {
        let Some(section) = sample
            .pos
            .y
            .checked_sub(mc_world::MIN_Y)
            .map(|y| y as usize / mc_world::SECTION_DIM)
            .filter(|section| *section < mc_world::SECTION_COUNT)
        else {
            continue;
        };
        if !active_sections
            .get(&sample.chunk)
            .is_some_and(|sections| sections[section])
        {
            continue;
        }
        let Some(state) = snapshot.get_cached_block(sample.pos) else {
            continue;
        };
        sampled += 1;
        if facts.random_tick_family(state.0).is_some() {
            candidates.push(RandomTickCandidate { sample, state });
        }
    }
    (sampled, candidates)
}

async fn run_scheduled_fluid_ticks_owned(
    _authority: &simulation::SimulationAuthority,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
    world_tick: u64,
    budget: usize,
) -> ScheduledFluidTickReport {
    let budget = budget.max(1);
    let Some(world) = config.world.as_ref() else {
        return ScheduledFluidTickReport {
            budget,
            ..ScheduledFluidTickReport::default()
        };
    };
    let loaded_chunks = sessions.loaded_chunks_sorted();
    if loaded_chunks.is_empty() {
        return ScheduledFluidTickReport {
            budget,
            ..ScheduledFluidTickReport::default()
        };
    }

    let owned_world_read = if world_read.is_none() {
        Some(world.lock().await.read_view())
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_read.as_ref())
        .expect("world-backed scheduled fluid ticks have a read view");
    let loaded_positions = loaded_chunks
        .iter()
        .map(|&(x, z)| ChunkPos { x, z })
        .collect::<Vec<_>>();
    let loaded_snapshot = world_read.snapshot_chunks(&loaded_positions);
    let due = due_scheduled_fluid_ticks(&loaded_snapshot, &loaded_positions, world_tick, budget);
    #[cfg(test)]
    SCHEDULED_FLUID_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(count.get() + 1));

    let drained = due.len();
    let planning_chunks = scheduled_fluid_planning_chunks(&due);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);
    let plan = fluids::plan_scheduled_fluid_tick_edits(
        &config.blocks,
        &config.block_facts,
        world_tick,
        &snapshot,
        &due,
    );
    let table = config.block_light.as_deref();
    let resident = if let Some(mutation) = world_mutation {
        if let Some((edits, preconditions)) =
            resident_block_edit_inputs(&plan.edits, &plan.preconditions, table)
        {
            match mutation.apply_fluid_tick_plan_conditionally(&mc_world::ResidentFluidTickPlan {
                consumed_ticks: &due,
                edits: &edits,
                preconditions: &preconditions,
                scheduled_ticks: &plan.scheduled_fluid_ticks,
                light_table: table,
                leaf_trigger_tick: Some(world_tick.saturating_add(1)),
            }) {
                mc_world::ResidentBlockEditBatchResult::Applied(applied) => {
                    simulation::resident_block_edit_result_outcome(
                        mc_world::ResidentBlockEditBatchResult::Applied(applied),
                    )
                }
                mc_world::ResidentBlockEditBatchResult::Stale => {
                    Some(BlockEditBatchOutcome::default())
                }
                mc_world::ResidentBlockEditBatchResult::Missing
                | mc_world::ResidentBlockEditBatchResult::CrossRegion => None,
            }
        } else {
            None
        }
    } else {
        None
    };
    let outcome = match resident {
        Some(outcome) => outcome,
        None => {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "scheduled fluid tick commit",
                Instant::now(),
                world.lock().await,
            );
            if !claim_scheduled_fluid_ticks(&mut storage, &due, world_tick) {
                BlockEditBatchOutcome::default()
            } else {
                commit_scheduled_fluid_tick_plan(
                    &mut storage,
                    table,
                    &config.block_facts,
                    world_tick,
                    &plan,
                    &due,
                )
                .unwrap_or_default()
            }
        }
    };
    let budget_exhausted = drained >= budget;
    if budget_exhausted {
        warn!(
            world_tick,
            drained, budget, "scheduled fluid tick budget exhausted"
        );
    }

    if !outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates =
                collect_server_origin_light_updates(world, sessions, table, &outcome).await;
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
        }
        debug!(
            world_tick,
            drained,
            applied = outcome.applied.len(),
            "scheduled fluid ticks applied edits"
        );
    }
    ScheduledFluidTickReport {
        drained,
        applied: outcome.applied.len(),
        budget,
        budget_exhausted,
    }
}

fn due_scheduled_fluid_ticks(
    snapshot: &mc_world::WorldReadSnapshot,
    loaded_chunks: &[ChunkPos],
    world_tick: u64,
    budget: usize,
) -> Vec<ScheduledFluidTick> {
    let mut due = Vec::new();
    for &position in loaded_chunks {
        let Some(chunk) = snapshot.chunk(position) else {
            continue;
        };
        let remaining = budget.saturating_sub(due.len());
        if remaining == 0 {
            break;
        }
        due.extend(
            chunk
                .scheduled_fluid_ticks()
                .iter()
                .take_while(|tick| tick.trigger_tick <= world_tick)
                .take(remaining)
                .cloned(),
        );
    }
    due
}

#[cfg(test)]
fn plan_scheduled_fluid_tick_edits(
    config: &ServerConfig,
    world_tick: u64,
    snapshot: &mc_world::WorldReadSnapshot,
    ticks: &[ScheduledFluidTick],
) -> ScheduledFluidTickPlan {
    fluids::plan_scheduled_fluid_tick_edits(
        &config.blocks,
        &config.block_facts,
        world_tick,
        snapshot,
        ticks,
    )
}

fn claim_scheduled_fluid_ticks(
    storage: &mut mc_world::WorldStorage,
    ticks: &[ScheduledFluidTick],
    world_tick: u64,
) -> bool {
    let mut grouped = HashMap::<ChunkPos, Vec<ScheduledFluidTick>>::new();
    for tick in ticks {
        grouped
            .entry(ChunkPos {
                x: tick.pos.x.div_euclid(SECTION_DIM as i32),
                z: tick.pos.z.div_euclid(SECTION_DIM as i32),
            })
            .or_default()
            .push(tick.clone());
    }
    let mut grouped = grouped.into_iter().collect::<Vec<_>>();
    grouped.sort_unstable_by_key(|(position, _)| (position.x, position.z));
    if grouped.iter().any(|(position, expected)| {
        storage
            .cached_chunk_snapshot(*position)
            .is_none_or(|chunk| !chunk.scheduled_fluid_ticks().starts_with(expected))
    }) {
        return false;
    }
    for (position, expected) in grouped {
        let claimed = storage.drain_due_cached_fluid_ticks(position, world_tick, expected.len());
        assert_eq!(claimed, expected, "preflighted fluid-tick prefix changed");
    }
    true
}

fn requeue_stale_scheduled_fluid_ticks(
    storage: &mut mc_world::WorldStorage,
    ticks: &[ScheduledFluidTick],
) {
    for tick in ticks {
        if let Err(error) = storage.schedule_fluid_tick(tick.clone()) {
            warn!(%error, pos = ?tick.pos, "stale scheduled fluid tick requeue failed");
        }
    }
}

fn commit_scheduled_fluid_tick_plan(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    facts: &mc_data::block_facts::BlockFactsTable,
    world_tick: u64,
    plan: &ScheduledFluidTickPlan,
    due: &[ScheduledFluidTick],
) -> Option<BlockEditBatchOutcome> {
    if !snapshot_read_preconditions_are_current(storage, &plan.preconditions) {
        requeue_stale_scheduled_fluid_ticks(storage, due);
        return None;
    }

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &plan.edits {
        apply_block_edit_to_storage(storage, table, edit, &mut outcome);
    }
    schedule_fluid_ticks_near_applied(storage, facts, world_tick, &outcome.applied);
    schedule_leaf_ticks_near_applied(storage, world_tick, &outcome.applied);
    Some(outcome)
}

fn due_scheduled_block_ticks(
    snapshot: &mc_world::WorldReadSnapshot,
    loaded_chunks: &[ChunkPos],
    world_tick: u64,
    budget: usize,
) -> Vec<ScheduledBlockTick> {
    let mut due = Vec::new();
    for &position in loaded_chunks {
        let Some(chunk) = snapshot.chunk(position) else {
            continue;
        };
        let remaining = budget.saturating_sub(due.len());
        if remaining == 0 {
            break;
        }
        due.extend(
            chunk
                .scheduled_block_ticks()
                .iter()
                .take_while(|tick| tick.trigger_tick <= world_tick)
                .take(remaining)
                .cloned(),
        );
    }
    due
}

fn plan_scheduled_block_tick_edits(
    config: &ServerConfig,
    snapshot: &mc_world::WorldReadSnapshot,
    ticks: &[ScheduledBlockTick],
    protection: Option<&crate::script::ZoneProtectionSnapshot>,
) -> Option<ScheduledBlockTickPlan> {
    plan_scheduled_block_tick_edits_with_blocks(&config.blocks, snapshot, ticks, protection)
}

async fn plan_scheduled_block_regions_off_owner(
    blocks: Arc<BlockRegistry>,
    snapshot: mc_world::WorldReadSnapshot,
    region_ticks: Vec<(RegionKey, Vec<ScheduledBlockTick>)>,
    protection: Option<Arc<crate::script::ZoneProtectionSnapshot>>,
    cpu_resources: Option<&ChunkPipelineResources>,
) -> Result<Vec<ScheduledBlockRegionPlan>, ()> {
    let permit = match cpu_resources {
        Some(resources) => match resources.acquire_cpu().await {
            Ok(permit) => Some(permit),
            Err(error) => {
                warn!(%error, "scheduled block planning CPU admission closed");
                return Err(());
            }
        },
        None => None,
    };
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        region_ticks
            .into_iter()
            .map(|(region, due)| ScheduledBlockRegionPlan {
                region,
                plan: plan_scheduled_block_tick_edits_with_blocks(
                    &blocks,
                    &snapshot,
                    &due,
                    protection.as_deref(),
                )
                .expect("simple scheduled block region has a plan"),
                due,
            })
            .collect()
    })
    .await
    .map_err(|error| {
        warn!(%error, "scheduled block planning worker failed");
    })
}

async fn plan_scheduled_block_region_off_owner(
    blocks: Arc<BlockRegistry>,
    snapshot: mc_world::WorldReadSnapshot,
    region: RegionKey,
    due: Vec<ScheduledBlockTick>,
    protection: Option<Arc<crate::script::ZoneProtectionSnapshot>>,
    cpu_resources: Option<&ChunkPipelineResources>,
) -> Result<ScheduledBlockRegionPlan, ()> {
    let mut plans = plan_scheduled_block_regions_off_owner(
        blocks,
        snapshot,
        vec![(region, due)],
        protection,
        cpu_resources,
    )
    .await?;
    Ok(plans
        .pop()
        .expect("one scheduled block region produces one plan"))
}

fn requeue_stale_scheduled_block_ticks(
    storage: &mut mc_world::WorldStorage,
    ticks: &[ScheduledBlockTick],
) {
    for tick in ticks {
        if let Err(error) = storage.schedule_block_tick(tick.clone()) {
            warn!(%error, pos = ?tick.pos, "stale scheduled block tick requeue failed");
        }
    }
}

fn claim_scheduled_block_ticks(
    storage: &mut mc_world::WorldStorage,
    ticks: &[ScheduledBlockTick],
    world_tick: u64,
) -> bool {
    let grouped = scheduled_block_ticks_by_chunk(ticks);
    if !scheduled_block_tick_prefixes_are_current(storage, &grouped) {
        return false;
    }
    for (position, expected) in grouped {
        let claimed = storage.drain_due_cached_block_ticks(position, world_tick, expected.len());
        assert_eq!(claimed, expected, "preflighted block-tick prefix changed");
    }
    true
}

fn scheduled_block_ticks_by_chunk(
    ticks: &[ScheduledBlockTick],
) -> Vec<(ChunkPos, Vec<ScheduledBlockTick>)> {
    let mut grouped = HashMap::<ChunkPos, Vec<ScheduledBlockTick>>::new();
    for tick in ticks {
        grouped
            .entry(ChunkPos {
                x: tick.pos.x.div_euclid(SECTION_DIM as i32),
                z: tick.pos.z.div_euclid(SECTION_DIM as i32),
            })
            .or_default()
            .push(tick.clone());
    }
    let mut grouped = grouped.into_iter().collect::<Vec<_>>();
    grouped.sort_unstable_by_key(|(position, _)| (position.x, position.z));
    grouped
}

fn scheduled_block_tick_prefixes_are_current(
    storage: &mc_world::WorldStorage,
    grouped: &[(ChunkPos, Vec<ScheduledBlockTick>)],
) -> bool {
    !grouped.iter().any(|(position, expected)| {
        storage
            .cached_chunk_snapshot(*position)
            .is_none_or(|chunk| !chunk.scheduled_block_ticks().starts_with(expected))
    })
}

fn commit_scheduled_block_tick_plan(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    world_tick: u64,
    plan: &ScheduledBlockTickPlan,
    due: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    if !snapshot_read_preconditions_are_current(storage, &plan.preconditions) {
        requeue_stale_scheduled_block_ticks(storage, due);
        return None;
    }

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &plan.edits {
        apply_block_edit_to_storage(storage, table, edit, &mut outcome);
    }
    schedule_leaf_ticks_near_applied(storage, world_tick, &outcome.applied);
    Some(outcome)
}

fn coordinator_scheduled_block_journal_positions(
    storage: &mc_world::WorldStorage,
    plan: &ScheduledBlockTickPlan,
    due: &[ScheduledBlockTick],
) -> Vec<ChunkPos> {
    let mut positions = coordinator_random_tick_journal_positions(storage, &plan.edits);
    positions.extend(due.iter().map(|tick| ChunkPos {
        x: tick.pos.x.div_euclid(SECTION_DIM as i32),
        z: tick.pos.z.div_euclid(SECTION_DIM as i32),
    }));
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions.dedup();
    positions
}

#[allow(clippy::too_many_arguments)]
async fn commit_scheduled_block_tick_coordinator(
    sessions: &SessionRegistry,
    world: &WorldHandle,
    mutation: &mc_world::WorldMutationView,
    table: Option<&BlockLightTable>,
    world_tick: u64,
    plan: &ScheduledBlockTickPlan,
    due: &[ScheduledBlockTick],
) -> Result<Option<BlockEditBatchOutcome>, ()> {
    let grouped_due = scheduled_block_ticks_by_chunk(due);
    let mut wave = ResidentWorldJournalWave::begin(sessions).await?;
    loop {
        wave.wait_for_append_turn(sessions).await?;
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "scheduled block tick coordinator journal commit",
            Instant::now(),
            world.lock().await,
        );
        let current = scheduled_block_tick_prefixes_are_current(&storage, &grouped_due)
            && snapshot_read_preconditions_are_current(&storage, &plan.preconditions);
        if !current {
            wave.finish_coordinator(sessions, mutation, world_tick, &[], Vec::new())
                .await?;
            return Ok(None);
        }

        let positions = coordinator_scheduled_block_journal_positions(&storage, plan, due);
        if let Some(decision_id) = wave.decision_id {
            match storage.stamp_cached_chunks_for_world_journal(decision_id, &positions) {
                mc_world::JournalStampResult::Stamped(_) => {}
                mc_world::JournalStampResult::NewerDecision(_) => {
                    wave.finish_coordinator(sessions, mutation, world_tick, &[], Vec::new())
                        .await?;
                    drop(storage);
                    wave = ResidentWorldJournalWave::begin(sessions).await?;
                    continue;
                }
                mc_world::JournalStampResult::Missing => {
                    warn!("scheduled coordinator journal snapshot was incomplete");
                    sessions.report_world_chunk_journal_failure();
                    return Err(());
                }
            }
        }

        assert!(
            claim_scheduled_block_ticks(&mut storage, due, world_tick),
            "preflighted scheduled block ticks remain current under coordinator lock"
        );
        let outcome = commit_scheduled_block_tick_plan(&mut storage, table, world_tick, plan, due)
            .expect("preflighted scheduled block plan remains current under coordinator lock");
        let snapshots = positions
            .iter()
            .filter_map(|position| storage.cached_chunk_snapshot(*position))
            .collect::<Vec<_>>();
        if snapshots.len() != positions.len() {
            warn!("scheduled coordinator journal snapshot was incomplete after mutation");
            sessions.report_world_chunk_journal_failure();
            return Err(());
        }
        wave.finish_coordinator(sessions, mutation, world_tick, &positions, snapshots)
            .await?;
        return Ok(Some(outcome));
    }
}

fn scheduled_block_region_plan_fits(region: RegionKey, plan: &ScheduledBlockTickPlan) -> bool {
    let owns = |position: mc_world::BlockPos| block_world_region(position) == (region.x, region.z);
    if !plan.edits.iter().all(|edit| owns(edit.pos))
        || !plan
            .preconditions
            .iter()
            .all(|precondition| owns(precondition.pos))
    {
        return false;
    }
    plan.edits
        .iter()
        .all(|edit| fluid_neighbour_positions(edit.pos).into_iter().all(owns))
}

#[allow(clippy::too_many_arguments)]
async fn commit_scheduled_block_region_fanout(
    sessions: &SessionRegistry,
    world_read: &mc_world::WorldReadView,
    mutation: &mc_world::WorldMutationView,
    light_table: Option<Arc<BlockLightTable>>,
    world_tick: u64,
    resources: &ChunkPipelineResources,
    plans: Vec<ScheduledBlockRegionPlan>,
    #[cfg(test)] probe: Option<simulation::RegionalBlockEditProbe>,
) -> Result<(BlockEditBatchOutcome, Vec<ScheduledBlockTick>), ()> {
    let lane_count = resources.cpu_limit().max(1).min(plans.len());
    let mut lanes = BTreeMap::<usize, Vec<ScheduledBlockRegionJob>>::new();
    for (index, planned) in plans.into_iter().enumerate() {
        let mut journal_chunks = resident_block_journal_chunks(world_read, &planned.plan.edits);
        journal_chunks.extend(planned.due.iter().map(|tick| ChunkPos {
            x: tick.pos.x.div_euclid(SECTION_DIM as i32),
            z: tick.pos.z.div_euclid(SECTION_DIM as i32),
        }));
        journal_chunks.sort_unstable_by_key(|position| (position.x, position.z));
        journal_chunks.dedup();
        let (edits, preconditions) = resident_block_edit_inputs(
            &planned.plan.edits,
            &planned.plan.preconditions,
            light_table.as_deref(),
        )
        .expect("fanout plans were preflighted as resident block edits");
        let lane = ((planned.region.x as u32).wrapping_mul(31) ^ planned.region.z as u32) as usize
            % lane_count;
        lanes
            .entry(lane)
            .or_default()
            .push(ScheduledBlockRegionJob {
                index,
                #[cfg(test)]
                region: planned.region,
                due: planned.due,
                edits,
                preconditions,
                journal_chunks,
            });
    }

    let mut wave = ResidentWorldJournalWave::begin(sessions).await?;
    let decision_id = wave.decision_id;
    let mut workers = tokio::task::JoinSet::new();
    for jobs in lanes.into_values() {
        let permit = resources.acquire_cpu().await.map_err(|_| ())?;
        let mutation = mutation.clone();
        let light_table = light_table.clone();
        #[cfg(test)]
        let probe = probe.clone();
        workers.spawn_blocking(move || {
            let _permit = permit;
            let mut results = Vec::with_capacity(jobs.len());
            for job in jobs {
                let stamped = if let Some(decision_id) = decision_id {
                    match mutation.stamp_chunks_for_world_journal(decision_id, &job.journal_chunks)
                    {
                        mc_world::JournalStampResult::Stamped(_) => true,
                        mc_world::JournalStampResult::NewerDecision(_) => {
                            results.push(ScheduledBlockRegionResult {
                                index: job.index,
                                due: job.due,
                                result: mc_world::ResidentBlockEditBatchResult::Stale,
                                touched: Vec::new(),
                                panicked: false,
                            });
                            continue;
                        }
                        mc_world::JournalStampResult::Missing => {
                            results.push(ScheduledBlockRegionResult {
                                index: job.index,
                                due: job.due,
                                result: mc_world::ResidentBlockEditBatchResult::Missing,
                                touched: Vec::new(),
                                panicked: false,
                            });
                            continue;
                        }
                    }
                } else {
                    false
                };
                let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if let Some(probe) = probe.as_ref() {
                        probe.enter(job.region);
                    }
                    let plan = mc_world::ResidentScheduledBlockTickPlan {
                        consumed_ticks: &job.due,
                        edits: &job.edits,
                        preconditions: &job.preconditions,
                        light_table: light_table.as_deref(),
                        leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                    };
                    if let Some(decision_id) = decision_id {
                        mutation.apply_scheduled_block_tick_plan_conditionally_journaled(
                            decision_id,
                            &plan,
                        )
                    } else {
                        (
                            mutation.apply_scheduled_block_tick_plan_conditionally(&plan),
                            Vec::new(),
                        )
                    }
                }));
                let (result, mut touched, panicked) = match applied {
                    Ok((result, touched)) => (result, touched, false),
                    Err(_) => (
                        mc_world::ResidentBlockEditBatchResult::Stale,
                        Vec::new(),
                        true,
                    ),
                };
                if stamped {
                    touched.extend(job.journal_chunks.iter().copied());
                    touched.sort_unstable_by_key(|position| (position.x, position.z));
                    touched.dedup();
                }
                results.push(ScheduledBlockRegionResult {
                    index: job.index,
                    due: job.due,
                    result,
                    touched,
                    panicked,
                });
            }
            results
        });
    }

    let mut results = Vec::new();
    let mut worker_failed = false;
    while let Some(joined) = workers.join_next().await {
        match joined {
            Ok(mut lane_results) => results.append(&mut lane_results),
            Err(error) => {
                warn!(?error, "scheduled regional worker failed");
                worker_failed = true;
            }
        }
    }
    results.sort_unstable_by_key(|result| result.index);

    let mut outcome = BlockEditBatchOutcome::default();
    let mut fallback_due = Vec::new();
    for result in results {
        worker_failed |= result.panicked;
        wave.touched.extend(result.touched.iter().copied());
        match result.result {
            mc_world::ResidentBlockEditBatchResult::Applied(applied) => {
                let additional = simulation::resident_block_edit_result_outcome(
                    mc_world::ResidentBlockEditBatchResult::Applied(applied),
                )
                .expect("applied scheduled regional job has an outcome");
                append_resident_block_outcome(&mut outcome, additional);
            }
            mc_world::ResidentBlockEditBatchResult::Stale => {}
            mc_world::ResidentBlockEditBatchResult::Missing
            | mc_world::ResidentBlockEditBatchResult::CrossRegion => {
                fallback_due.extend(result.due);
            }
        }
    }
    wave.finish(sessions, world_read, mutation, world_tick)
        .await?;
    if worker_failed {
        sessions.report_world_chunk_journal_failure();
        return Err(());
    }
    Ok((outcome, fallback_due))
}

pub(crate) async fn run_scheduled_block_ticks_background(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    access: SimulationWorldAccess<'_>,
    protection: Option<Arc<crate::script::ZoneProtectionSnapshot>>,
    world_tick: u64,
    budget: usize,
) -> ScheduledBlockTickReport {
    run_scheduled_block_ticks_owned(
        config,
        sessions,
        access,
        #[cfg(test)]
        None,
        protection,
        world_tick,
        budget,
    )
    .await
}

async fn run_scheduled_block_ticks_owned(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    access: SimulationWorldAccess<'_>,
    #[cfg(test)] probe: Option<simulation::RegionalBlockEditProbe>,
    protection: Option<Arc<crate::script::ZoneProtectionSnapshot>>,
    world_tick: u64,
    budget: usize,
) -> ScheduledBlockTickReport {
    let world_read = access.read;
    let world_mutation = access.mutation;
    let cpu_resources = access.cpu;
    let budget = budget.max(1);
    let Some(_admission) = sessions.try_begin_scheduled_block_ticks() else {
        return ScheduledBlockTickReport {
            budget,
            ..ScheduledBlockTickReport::default()
        };
    };
    let Some(world) = config.world.as_ref() else {
        return ScheduledBlockTickReport {
            budget,
            ..ScheduledBlockTickReport::default()
        };
    };
    let loaded_chunks = sessions.loaded_chunks_sorted();
    if loaded_chunks.is_empty() {
        return ScheduledBlockTickReport {
            budget,
            ..ScheduledBlockTickReport::default()
        };
    }

    let loaded_positions = loaded_chunks
        .iter()
        .map(|&(x, z)| ChunkPos { x, z })
        .collect::<Vec<_>>();
    if let Some(mutation) = world_mutation {
        mutation.backfill_hopper_ticks(
            &loaded_positions,
            world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
        );
    } else {
        let mut storage = world.lock().await;
        backfill_loaded_hopper_ticks(
            &mut storage,
            &config.blocks,
            &loaded_chunks,
            world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
        );
    }
    let owned_world_views = if world_read.is_none() || world_mutation.is_none() {
        let storage = world.lock().await;
        Some((storage.read_view(), storage.mutation_view()))
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_views.as_ref().map(|(read, _)| read))
        .expect("world-backed scheduled block ticks have a read view");
    let world_mutation = world_mutation
        .or(owned_world_views.as_ref().map(|(_, mutation)| mutation))
        .expect("world-backed scheduled block ticks have a mutation view");
    let loaded_snapshot = world_read.snapshot_chunks(&loaded_positions);
    let mut due =
        due_scheduled_block_ticks(&loaded_snapshot, &loaded_positions, world_tick, budget);

    let table = config.block_light.as_deref();
    let mut applied_mutations = 0usize;
    let mut hopper_updates = Vec::new();
    let mut hopper_container_dispatches = Vec::new();
    let mut outcome = BlockEditBatchOutcome::default();
    let drained = due.len();
    let planning_chunks = scheduled_block_planning_chunks(&due);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);
    let contains_comparator_tick = due.iter().any(|tick| {
        snapshot
            .get_cached_block(tick.pos)
            .and_then(|state| config.blocks.by_id(state))
            .is_some_and(|state| state.block.id.path() == "comparator")
    });
    let mut due_claimed_by_coordinator = false;
    if !due.iter().any(|tick| {
        snapshot
            .get_cached_block(tick.pos)
            .and_then(|state| config.blocks.by_id(state))
            .is_some_and(|state| state.block.id.path() == "hopper")
    }) && !contains_comparator_tick
    {
        #[cfg(test)]
        if !due.is_empty() {
            SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(count.get() + 1));
        }
        let mut region_ticks = Vec::<(RegionKey, Vec<ScheduledBlockTick>)>::new();
        for tick in due.drain(..) {
            let chunk_x = tick.pos.x.div_euclid(SECTION_DIM as i32);
            let chunk_z = tick.pos.z.div_euclid(SECTION_DIM as i32);
            let region = RegionKey::from_chunk(chunk_x, chunk_z);
            if let Some((last_region, ticks)) = region_ticks.last_mut()
                && *last_region == region
            {
                ticks.push(tick);
            } else {
                region_ticks.push((region, vec![tick]));
            }
        }
        let mut region_plans = match plan_scheduled_block_regions_off_owner(
            Arc::clone(&config.blocks),
            snapshot.clone(),
            region_ticks,
            protection.clone(),
            cpu_resources,
        )
        .await
        {
            Ok(plans) => Some(plans),
            Err(()) => {
                return ScheduledBlockTickReport {
                    budget,
                    ..ScheduledBlockTickReport::default()
                };
            }
        };
        let mut journal_failed = false;
        let can_fanout = cpu_resources.is_some_and(|resources| resources.cpu_limit() > 1)
            && region_plans.as_ref().is_some_and(|plans| {
                plans.len() > 1
                    && plans
                        .iter()
                        .map(|planned| planned.region)
                        .collect::<HashSet<_>>()
                        .len()
                        == plans.len()
                    && plans.iter().all(|planned| {
                        !planned.plan.edits.is_empty()
                            && scheduled_block_region_plan_fits(planned.region, &planned.plan)
                            && resident_block_edit_inputs(
                                &planned.plan.edits,
                                &planned.plan.preconditions,
                                table,
                            )
                            .is_some()
                    })
            });
        if can_fanout {
            let plans = region_plans.take().expect("fanout plans exist");
            match commit_scheduled_block_region_fanout(
                sessions,
                world_read,
                world_mutation,
                config.block_light.clone(),
                world_tick,
                cpu_resources.expect("fanout requires CPU resources"),
                plans,
                #[cfg(test)]
                probe.clone(),
            )
            .await
            {
                Ok((additional, fallback_due)) => {
                    append_resident_block_outcome(&mut outcome, additional);
                    due = fallback_due;
                }
                Err(()) => journal_failed = true,
            }
        }
        if let Some(region_plans) = region_plans {
            for planned in region_plans {
                let region_due = planned.due;
                let current_snapshot =
                    world_read.snapshot_chunks(&scheduled_block_planning_chunks(&region_due));
                let planned = match plan_scheduled_block_region_off_owner(
                    Arc::clone(&config.blocks),
                    current_snapshot,
                    planned.region,
                    region_due,
                    protection.clone(),
                    cpu_resources,
                )
                .await
                {
                    Ok(planned) => planned,
                    Err(()) => break,
                };
                let region_due = planned.due;
                let plan = planned.plan;
                if plan.edits.is_empty()
                    && scheduled_block_region_plan_fits(planned.region, &plan)
                    && let Some((edits, preconditions)) =
                        resident_block_edit_inputs(&plan.edits, &plan.preconditions, table)
                {
                    let result = world_mutation.apply_scheduled_block_tick_plan_conditionally(
                        &mc_world::ResidentScheduledBlockTickPlan {
                            consumed_ticks: &region_due,
                            edits: &edits,
                            preconditions: &preconditions,
                            light_table: table,
                            leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                        },
                    );
                    match result {
                        mc_world::ResidentBlockEditBatchResult::Applied(_)
                        | mc_world::ResidentBlockEditBatchResult::Stale => continue,
                        mc_world::ResidentBlockEditBatchResult::Missing
                        | mc_world::ResidentBlockEditBatchResult::CrossRegion => {}
                    }
                }
                let resident_result = if scheduled_block_region_plan_fits(planned.region, &plan)
                    && let Some((edits, preconditions)) =
                        resident_block_edit_inputs(&plan.edits, &plan.preconditions, table)
                {
                    Some(
                        world_mutation.apply_scheduled_block_tick_plan_conditionally(
                            &mc_world::ResidentScheduledBlockTickPlan {
                                consumed_ticks: &region_due,
                                edits: &edits,
                                preconditions: &preconditions,
                                light_table: table,
                                leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                            },
                        ),
                    )
                } else {
                    None
                };
                match resident_result {
                    Some(mc_world::ResidentBlockEditBatchResult::Applied(applied)) => {
                        let additional = simulation::resident_block_edit_result_outcome(
                            mc_world::ResidentBlockEditBatchResult::Applied(applied),
                        )
                        .expect("applied resident block edits have an outcome");
                        append_resident_block_outcome(&mut outcome, additional);
                        continue;
                    }
                    Some(mc_world::ResidentBlockEditBatchResult::Stale) => continue,
                    None
                    | Some(mc_world::ResidentBlockEditBatchResult::Missing)
                    | Some(mc_world::ResidentBlockEditBatchResult::CrossRegion) => {}
                }

                let Some((edits, preconditions)) =
                    resident_block_edit_inputs(&plan.edits, &plan.preconditions, table)
                else {
                    continue;
                };
                match commit_cross_region_scheduled_block_tick(
                    sessions,
                    world_mutation,
                    world_tick,
                    ResidentBlockCommit {
                        edits: &edits,
                        preconditions: &preconditions,
                        consumed_block_ticks: &region_due,
                        consumed_fluid_ticks: &[],
                        scheduled_fluid_ticks: &[],
                        light_table: table,
                        leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                    },
                )
                .await
                {
                    Ok(Some(boundary_outcome)) => {
                        append_resident_block_outcome(&mut outcome, boundary_outcome);
                    }
                    Ok(None) => {}
                    Err(()) => {
                        journal_failed = true;
                        break;
                    }
                }
            }
        }
        if journal_failed {
            outcome = BlockEditBatchOutcome::default();
            due.clear();
        }
    } else if contains_comparator_tick {
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "scheduled special block tick claim",
            Instant::now(),
            world.lock().await,
        );
        if !claim_scheduled_block_ticks(&mut storage, &due, world_tick) {
            due.clear();
        } else {
            due_claimed_by_coordinator = true;
        }
    }
    let hopper_context = HopperTransferContext {
        blocks: config.blocks.as_ref(),
        items: config.items.as_ref(),
        tags: config.tags.as_ref(),
        recipes: config.recipes.as_slice(),
        sessions,
    };
    if !due_claimed_by_coordinator && !due.is_empty() {
        match ResidentWorldJournalWave::begin(sessions).await {
            Err(()) => due.clear(),
            Ok(mut wave) => {
                let mut coordinator_due = Vec::with_capacity(due.len());
                for tick in due.drain(..) {
                    let Some(plan) =
                        resident_hopper_cooldown_plan(&config.blocks, &snapshot, &tick, world_tick)
                    else {
                        coordinator_due.push(tick);
                        continue;
                    };
                    match wave.commit_hopper(world_mutation, &tick, &plan) {
                        mc_world::ResidentHopperTransferCommitResult::Applied => {}
                        mc_world::ResidentHopperTransferCommitResult::Missing
                        | mc_world::ResidentHopperTransferCommitResult::Stale
                        | mc_world::ResidentHopperTransferCommitResult::CrossRegion => {
                            coordinator_due.push(tick);
                        }
                    }
                }
                due = coordinator_due;
                let mut coordinator_due = Vec::with_capacity(due.len());
                for tick in due.drain(..) {
                    let Some(planned) = plan_resident_hopper_transfer(
                        &hopper_context,
                        &config.blocks,
                        &snapshot,
                        &planning_chunks,
                        &tick,
                        world_tick,
                    ) else {
                        coordinator_due.push(tick);
                        continue;
                    };
                    match wave.commit_hopper(world_mutation, &tick, &planned.plan) {
                        mc_world::ResidentHopperTransferCommitResult::Applied => {
                            #[cfg(test)]
                            RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT
                                .with(|count| count.set(count.get() + 1));
                            if planned.result.moved {
                                applied_mutations += 1;
                            }
                            hopper_updates.extend(planned.result.updates);
                        }
                        mc_world::ResidentHopperTransferCommitResult::Missing
                        | mc_world::ResidentHopperTransferCommitResult::Stale
                        | mc_world::ResidentHopperTransferCommitResult::CrossRegion => {
                            coordinator_due.push(tick);
                        }
                    }
                }
                due = coordinator_due;
                if wave
                    .finish(sessions, world_read, world_mutation, world_tick)
                    .await
                    .is_err()
                {
                    due.clear();
                    applied_mutations = 0;
                    hopper_updates.clear();
                }
            }
        }
    }
    let plan = plan_scheduled_block_tick_edits(config, &snapshot, &due, protection.as_deref());
    #[cfg(test)]
    if !due.is_empty() && plan.is_some() && world.try_lock().is_ok() {
        SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(count.get() + 1));
    }

    if let Some(plan) = plan.filter(|_| !due.is_empty()) {
        if due_claimed_by_coordinator {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "scheduled block tick commit",
                Instant::now(),
                world.lock().await,
            );
            if let Some(additional) =
                commit_scheduled_block_tick_plan(&mut storage, table, world_tick, &plan, &due)
            {
                append_resident_block_outcome(&mut outcome, additional);
            }
        } else {
            if let Ok(Some(additional)) = commit_scheduled_block_tick_coordinator(
                sessions,
                world,
                world_mutation,
                table,
                world_tick,
                &plan,
                &due,
            )
            .await
            {
                append_resident_block_outcome(&mut outcome, additional);
            }
        }
        due.clear();
    } else if !due.is_empty() {
        if !due.is_empty() && !due_claimed_by_coordinator {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "scheduled special block tick fallback claim",
                Instant::now(),
                world.lock().await,
            );
            if !claim_scheduled_block_ticks(&mut storage, &due, world_tick) {
                due.clear();
            }
        }
        for tick in &due {
            let mut tick_outcome = BlockEditBatchOutcome::default();
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "scheduled special block tick apply",
                Instant::now(),
                world.lock().await,
            );
            let Some(state_id) = storage.get_cached_block(tick.pos) else {
                continue;
            };
            let Some(state) = config.blocks.by_id(state_id) else {
                continue;
            };
            if state.block.id != tick.block {
                continue;
            }
            if let Some(result) =
                scheduled_hopper_transfer(&hopper_context, &mut storage, tick.pos, state_id)
            {
                if result.moved {
                    applied_mutations += 1;
                }
                for update in &result.updates {
                    schedule_comparator_ticks_for_hopper_update(
                        &config.blocks,
                        &mut storage,
                        update,
                        world_tick.saturating_add(COMPARATOR_TICK_DELAY_TICKS),
                    );
                }
                hopper_updates.extend(result.updates);
                if let Err(err) = storage.schedule_block_tick(ScheduledBlockTick::new(
                    tick.pos,
                    state.block.id.clone(),
                    world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
                    0,
                )) {
                    warn!(error = %err, pos = ?tick.pos, "hopper transfer tick scheduling failed");
                }
                continue;
            }
            let Some(edits) =
                scheduled_block_tick_edits(&config.blocks, &mut storage, tick.pos, state_id)
            else {
                continue;
            };
            for edit in edits {
                apply_block_edit_to_storage(&mut storage, table, &edit, &mut tick_outcome);
            }
            schedule_leaf_ticks_near_applied(&mut storage, world_tick, &tick_outcome.applied);
            drop(storage);
            append_resident_block_outcome(&mut outcome, tick_outcome);
        }
    }
    for update in &hopper_updates {
        let dispatches = match update {
            HopperTransferUpdate::Chest { position, slots } => {
                sessions
                    .server_chest_slot_dispatches(*position, slots.clone())
                    .1
            }
            HopperTransferUpdate::Furnace { position, slots } => {
                sessions
                    .server_furnace_slot_dispatches(*position, slots.clone())
                    .1
            }
            HopperTransferUpdate::Campfire { .. } => Vec::new(),
        };
        hopper_container_dispatches.extend(dispatches);
    }
    dispatch_visibility_commands(hopper_container_dispatches);
    let mut hopper_visible_chunks = HashSet::new();
    for update in hopper_updates {
        match update {
            HopperTransferUpdate::Chest { .. } | HopperTransferUpdate::Furnace { .. } => {}
            HopperTransferUpdate::Campfire { position, cooking } => {
                hopper_visible_chunks
                    .insert((position.x.div_euclid(16), position.z.div_euclid(16)));
                dispatch_campfire_block_entity_update(
                    &config.items,
                    sessions,
                    None,
                    position,
                    &cooking,
                );
            }
        }
    }
    if !hopper_visible_chunks.is_empty() {
        sessions.invalidate_prepared_chunks(&hopper_visible_chunks);
    }
    let budget_exhausted = drained >= budget;
    if budget_exhausted {
        warn!(
            world_tick,
            drained, budget, "scheduled block tick budget exhausted"
        );
    }

    if !outcome.applied.is_empty() || applied_mutations > 0 {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates =
                collect_server_origin_light_updates(world, sessions, table, &outcome).await;
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
        }
        debug!(
            world_tick,
            drained,
            applied = outcome.applied.len() + applied_mutations,
            "scheduled block ticks applied work"
        );
    }
    ScheduledBlockTickReport {
        drained,
        applied: outcome.applied.len() + applied_mutations,
        budget,
        budget_exhausted,
    }
}

async fn land_falling_blocks_owned(
    authority: &simulation::SimulationAuthority,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    candidates: &[LandedFallingBlock],
) -> usize {
    let Some(world) = config.world.as_ref() else {
        return 0;
    };
    if candidates.is_empty() {
        return 0;
    }

    let owned_world_read = if world_read.is_none() {
        Some(world.lock().await.read_view())
    } else {
        None
    };
    let world_read = world_read
        .or(owned_world_read.as_ref())
        .expect("world-backed falling blocks have a read view");
    let chunks = falling_block_landing_chunks(candidates);
    let snapshot = world_read.snapshot_chunks(&chunks);
    let plan = plan_falling_block_landings(
        config.loot.as_ref(),
        config.items.as_ref(),
        config.item_facts.as_ref(),
        config.blocks.as_ref(),
        config.block_facts.as_ref(),
        &snapshot,
        candidates,
    );
    #[cfg(test)]
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| count.set(count.get() + 1));
    if plan.placements.is_empty() && plan.blocked_ids.is_empty() {
        return 0;
    }

    let FallingBlockLandingPlan {
        placements,
        blocked_ids,
        drops,
        preconditions,
    } = plan;
    let table = config.block_light.as_deref();
    let mut outcome = BlockEditBatchOutcome::default();
    let mut landed_ids = blocked_ids;
    {
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "falling block landing commit",
            Instant::now(),
            world.lock().await,
        );
        if !snapshot_read_preconditions_are_current(&storage, &preconditions) {
            return 0;
        }
        for placement in placements {
            let applied_before = outcome.applied.len();
            apply_block_edit_to_storage(&mut storage, table, &placement.edit, &mut outcome);
            if outcome.applied.len() > applied_before {
                landed_ids.push(placement.id);
            }
        }
        schedule_leaf_ticks_near_applied(
            &mut storage,
            sessions.simulation_tick(),
            &outcome.applied,
        );
    }

    if let Some(entity_type_id) = item_entity_type_id(&config.entity_types) {
        for drop in drops {
            dispatch_visibility_commands(sessions.spawn_item_drop_owned(
                authority,
                entity_type_id,
                drop.position,
                drop.stack,
            ));
        }
    } else if !drops.is_empty() {
        debug!(
            count = drops.len(),
            "falling block drops ignored: item entity type unavailable"
        );
    }

    if outcome.applied.is_empty() {
        if !landed_ids.is_empty() {
            sessions.remove_landed_falling_blocks(&landed_ids);
        }
        return 0;
    }
    sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
    broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
    if let Some(table) = table
        && !outcome.light_edit_chunks.is_empty()
    {
        let light_updates =
            collect_server_origin_light_updates(world, sessions, table, &outcome).await;
        if !light_updates.is_empty() {
            sessions.invalidate_prepared_chunks(&light_update_chunks(&light_updates));
            broadcast_light_updates_to_sessions(sessions, &light_updates, None);
        }
    }
    if !landed_ids.is_empty() {
        sessions.remove_landed_falling_blocks(&landed_ids);
    }
    outcome.applied.len()
}

fn schedule_fluid_ticks_near_applied(
    storage: &mut mc_world::WorldStorage,
    facts: &mc_data::block_facts::BlockFactsTable,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) -> usize {
    plan_fluid_ticks_near_applied(storage, facts, world_tick, applied)
        .into_iter()
        .filter(|tick| storage.schedule_fluid_tick(tick.clone()).unwrap_or(false))
        .count()
}

fn push_unique_block_edit(edits: &mut Vec<BlockEdit>, edit: BlockEdit) {
    if edits.iter().any(|existing| existing.pos == edit.pos) {
        return;
    }
    edits.push(edit);
}

fn schedule_leaf_ticks_near_applied(
    storage: &mut mc_world::WorldStorage,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) -> HashSet<ChunkPos> {
    let blocks = storage.registry_arc();
    let mut positions = HashSet::new();
    for edit in applied {
        for pos in fluid_neighbour_positions(edit.pos) {
            let Some(state_id) = storage.get_cached_block(pos) else {
                continue;
            };
            let Some(state) = blocks.by_id(state_id) else {
                continue;
            };
            if state.block.id.path().ends_with("_leaves") {
                positions.insert(pos);
            }
        }
    }

    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
    let trigger_tick = world_tick.saturating_add(1);
    let mut touched_chunks = HashSet::new();
    for pos in positions {
        let Some(state_id) = storage.get_cached_block(pos) else {
            continue;
        };
        let Some(state) = blocks.by_id(state_id) else {
            continue;
        };
        if storage
            .schedule_block_tick(ScheduledBlockTick::new(
                pos,
                state.block.id.clone(),
                trigger_tick,
                0,
            ))
            .unwrap_or(false)
        {
            touched_chunks.insert(ChunkPos {
                x: pos.x.div_euclid(SECTION_DIM as i32),
                z: pos.z.div_euclid(SECTION_DIM as i32),
            });
        }
    }
    touched_chunks
}

fn coordinator_random_tick_journal_positions(
    storage: &mc_world::WorldStorage,
    edits: &[BlockEdit],
) -> Vec<ChunkPos> {
    let blocks = storage.registry_arc();
    let planned_states = edits
        .iter()
        .map(|edit| (edit.pos, edit.new_state))
        .collect::<HashMap<_, _>>();
    let mut positions = HashSet::new();
    for edit in edits {
        positions.insert(ChunkPos {
            x: edit.pos.x.div_euclid(SECTION_DIM as i32),
            z: edit.pos.z.div_euclid(SECTION_DIM as i32),
        });
        for neighbour in fluid_neighbour_positions(edit.pos) {
            let current_is_leaf = storage
                .get_cached_block(neighbour)
                .and_then(|state_id| blocks.by_id(state_id))
                .is_some_and(|state| state.block.id.path().ends_with("_leaves"));
            let planned_is_leaf = planned_states
                .get(&neighbour)
                .and_then(|state_id| blocks.by_id(*state_id))
                .is_some_and(|state| state.block.id.path().ends_with("_leaves"));
            if current_is_leaf || planned_is_leaf {
                positions.insert(ChunkPos {
                    x: neighbour.x.div_euclid(SECTION_DIM as i32),
                    z: neighbour.z.div_euclid(SECTION_DIM as i32),
                });
            }
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions
}

fn fluid_neighbour_positions(pos: mc_world::BlockPos) -> [mc_world::BlockPos; 6] {
    [
        mc_world::BlockPos {
            x: pos.x + 1,
            ..pos
        },
        mc_world::BlockPos {
            x: pos.x - 1,
            ..pos
        },
        mc_world::BlockPos {
            z: pos.z + 1,
            ..pos
        },
        mc_world::BlockPos {
            z: pos.z - 1,
            ..pos
        },
        mc_world::BlockPos {
            y: pos.y + 1,
            ..pos
        },
        mc_world::BlockPos {
            y: pos.y - 1,
            ..pos
        },
    ]
}

fn named_block_default(
    blocks: &mc_world::BlockRegistry,
    name: &str,
) -> Option<mc_world::BlockStateId> {
    blocks
        .block(&Identifier::parse(name).expect("static identifier"))
        .map(|block| block.default)
}

async fn collect_server_origin_light_updates(
    world: &WorldHandle,
    sessions: &SessionRegistry,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> Vec<OutboundLightUpdate> {
    let sources = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "capture server relight sources",
            Instant::now(),
            world.lock().await,
        );
        capture_incremental_light_sources(&storage, table, outcome)
    };
    #[cfg(test)]
    sessions.pause_before_server_relight_compute_for_test();
    #[cfg(not(test))]
    let _ = sessions;
    let updates = compute_incremental_light_updates(&sources, table, outcome);

    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "publish server relight result",
        Instant::now(),
        world.lock().await,
    );
    if incremental_light_sources_are_current(&storage, &sources) {
        persist_baked_light_updates(&mut storage, &updates);
        updates
    } else {
        collect_full_light_updates_for_current_world(&mut storage, table, outcome)
    }
}

#[cfg(test)]
thread_local! {
    static OUTBOUND_LIGHT_UPDATE_ENCODING_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static OUTBOUND_LIGHT_NEIGHBOURHOOD_CAPTURE_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static RANDOM_TICK_PLANNING_COMPLETION_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static SCHEDULED_FLUID_PLANNING_WITHOUT_WRITER_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

fn block_state_property<'a>(state: &'a mc_world::BlockState, name: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value.as_str()))
}

fn sibling_state_with_property(
    blocks: &mc_world::BlockRegistry,
    state: &mc_world::BlockState,
    name: &str,
    value: &str,
) -> Option<mc_world::BlockStateId> {
    let mut props = state.properties.clone();
    let (_, current) = props.iter_mut().find(|(key, _)| key == name)?;
    *current = value.to_string();
    blocks.by_name_and_props(&state.block.id, &props)
}

fn sibling_state_with_bool_property(
    blocks: &mc_world::BlockRegistry,
    state: &mc_world::BlockState,
    name: &str,
    value: bool,
) -> Option<mc_world::BlockStateId> {
    sibling_state_with_property(blocks, state, name, if value { "true" } else { "false" })
}

fn crop_state_with_age(
    blocks: &mc_world::BlockRegistry,
    crop: &Identifier,
    age: u8,
) -> Option<mc_world::BlockStateId> {
    blocks.by_name_and_props(crop, &[("age".to_string(), age.to_string())])
}

async fn refresh_player_water_state(state: Option<&InteractionState>, pose: &mut PlayerPose) {
    if let Some(state) = state {
        let (in_water, eye_in_water) = player_water_overlap(state, *pose).await;
        pose.in_water = in_water;
        pose.eye_in_water = eye_in_water;
    } else {
        pose.in_water = false;
        pose.eye_in_water = false;
    }
    pose.swimming = pose.in_water && pose.sprinting && (pose.input.forward || pose.eye_in_water);
}

fn publish_player_air_supply(
    sessions: &SessionRegistry,
    session_id: SessionId,
    breathing: PlayerBreathingState,
) {
    dispatch_visibility_commands(
        sessions
            .broadcast_player_entity_data_including_self(session_id, vec![breathing.metadata()]),
    );
}

async fn player_water_overlap(state: &InteractionState, pose: PlayerPose) -> (bool, bool) {
    let half_width = 0.3;
    let snapshot = player_body_block_snapshot(state, pose, half_width);
    player_water_overlap_in_snapshot(&state.block_facts, &snapshot, pose)
}

async fn player_pose_collides_with_solid(
    state: Option<&InteractionState>,
    pose: PlayerPose,
) -> bool {
    player_pose_collides_with_solid_using_context(state, pose, pose).await
}

async fn player_pose_collides_with_solid_using_context(
    state: Option<&InteractionState>,
    pose: PlayerPose,
    context_pose: PlayerPose,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    let half_width = 0.3;
    let snapshot = player_body_block_snapshot(state, pose, half_width);
    let feet = &state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT];
    let leather_boots = !feet.is_empty()
        && state
            .items
            .name_of(feet.item_id)
            .is_some_and(|name| name.as_str() == "minecraft:leather_boots");
    player_pose_collides_with_solid_in_snapshot_with_context(
        &state.block_facts,
        &state.blocks,
        &snapshot,
        pose,
        PlayerCollisionContext::from_pose(context_pose, leather_boots),
    )
}

fn player_body_block_snapshot(
    state: &InteractionState,
    pose: PlayerPose,
    half_width: f64,
) -> mc_world::WorldReadSnapshot {
    let min_cx = ((pose.x - half_width).floor() as i32).div_euclid(16);
    let max_cx = ((pose.x + half_width).floor() as i32).div_euclid(16);
    let min_cz = ((pose.z - half_width).floor() as i32).div_euclid(16);
    let max_cz = ((pose.z + half_width).floor() as i32).div_euclid(16);
    let mut chunks = Vec::with_capacity(4);
    for cx in min_cx..=max_cx {
        for cz in min_cz..=max_cz {
            chunks.push(ChunkPos { x: cx, z: cz });
        }
    }
    state.world_read.snapshot_chunks(&chunks)
}

#[allow(clippy::too_many_arguments)]
async fn correct_player_collision<W>(
    state: Option<&InteractionState>,
    writer: &mut W,
    compression: Compression,
    old_pose: PlayerPose,
    new_pose: PlayerPose,
    current_tick: u64,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !player_pose_collides_with_solid_using_context(state, new_pose, old_pose).await
        || player_pose_collides_with_solid(state, old_pose).await
    {
        return Ok(false);
    }
    let teleport_id = next_player_teleport_id(next_teleport_id);
    send_player_position_sync(writer, compression, teleport_id, old_pose).await?;
    *pending_teleport = Some(PendingTeleport::new(teleport_id, current_tick));
    Ok(true)
}

async fn send_player_position_sync<W>(
    writer: &mut W,
    compression: Compression,
    teleport_id: i32,
    pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &SynchronizePlayerPosition {
            teleport_id,
            x: pose.x,
            y: pose.y,
            z: pose.z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: pose.yaw,
            pitch: pose.pitch,
            relative_flags: 0,
        },
        compression,
    )
    .await
}

async fn resend_pending_teleport_if_due<W>(
    writer: &mut W,
    compression: Compression,
    pending: &mut Option<PendingTeleport>,
    next_teleport_id: &mut i32,
    pose: PlayerPose,
    current_tick: u64,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(current) = *pending else {
        return Ok(false);
    };
    if current_tick.saturating_sub(current.sent_tick) <= TELEPORT_RESEND_DELAY_TICKS {
        return Ok(false);
    }

    let teleport_id = next_player_teleport_id(next_teleport_id);
    send_player_position_sync(writer, compression, teleport_id, pose).await?;
    *pending = Some(PendingTeleport::new(teleport_id, current_tick));
    Ok(true)
}

async fn maybe_trample_farmland<W>(
    state: &mut InteractionState,
    writer: &mut W,
    old_pose: PlayerPose,
    new_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(pos) = farmland_trample_pos(old_pose, new_pose) else {
        return Ok(());
    };
    let dirt = Identifier::parse("minecraft:dirt").expect("static identifier");
    let Some(dirt_state) = state.blocks.block(&dirt).map(|block| block.default) else {
        return Ok(());
    };
    let expected_farmland = {
        let snapshot = state.world_read.snapshot_chunks(&[ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        }]);
        snapshot
            .get_cached_block(pos)
            .filter(|state_id| {
                state.blocks.by_id(*state_id).is_some_and(|block_state| {
                    block_state.block.id.as_str() == "minecraft:farmland"
                })
            })
            .zip(snapshot.block_mutation_token(pos))
    };
    if let Some((expected_state, expected_token)) = expected_farmland {
        let _ = apply_visible_block_edit_batch_conditionally(
            state,
            writer,
            &[BlockEdit {
                pos,
                new_state: dirt_state,
            }],
            &[BlockEditPrecondition {
                pos,
                expected_state,
                expected_token,
            }],
            &[],
        )
        .await?;
    }
    Ok(())
}

async fn interact_with_bed<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    sequence: i32,
    pos: mc_world::BlockPos,
    respawn_pose: &mut PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some((new_respawn_pose, canonical_bed)) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, pos)
    else {
        return Ok(false);
    };

    if bed_sleep_is_obstructed(
        &state.world_read,
        &state.blocks,
        state.block_light.as_deref(),
        canonical_bed,
    ) {
        write_block_ack(writer, state.compression, sequence).await?;
        write_packet(
            writer,
            &ClientboundSystemChat {
                content_nbt: text_component_nbt("This bed is obstructed")?,
                overlay: true,
            },
            state.compression,
        )
        .await?;
        return Ok(true);
    }

    state
        .simulation
        .commit_respawn_pose(new_respawn_pose)
        .await
        .map_err(|error| {
            warn!(?error, ?pos, "respawn point owner commit failed");
            ConnectionError::RuntimeUnavailable {
                operation: "committing respawn point",
            }
        })?;
    *respawn_pose = new_respawn_pose;
    write_block_ack(writer, state.compression, sequence).await?;
    let hostile_nearby = state
        .sessions
        .has_rest_preventing_hostile_near_bed(canonical_bed);
    if bed_sleep_is_blocked_by_monster(game_mode, hostile_nearby) {
        write_packet(
            writer,
            &ClientboundSystemChat {
                content_nbt: text_component_nbt("You may not rest now; monsters are nearby")?,
                overlay: true,
            },
            state.compression,
        )
        .await?;
        return Ok(true);
    }
    match state
        .sessions
        .begin_sleep_at(state.session_id, canonical_bed)
    {
        SleepOutcome::Skipped { new_time, sleepers } => {
            let mut dispatches = state.sessions.broadcast_player_entity_data_including_self(
                state.session_id,
                vec![player_pose_entity_data(EntityPose::Sleeping)],
            );
            dispatches.extend(
                state
                    .sessions
                    .completed_sleep_dispatches(sleepers, Some(new_time)),
            );
            dispatch_visibility_commands(dispatches);
            send_command_feedback(
                writer,
                state.compression,
                "Respawn point set; skipped to morning",
            )
            .await?;
        }
        SleepOutcome::Daytime => {
            send_command_feedback(writer, state.compression, "Respawn point set").await?;
        }
        SleepOutcome::Occupied => {
            write_packet(
                writer,
                &ClientboundSystemChat {
                    content_nbt: text_component_nbt("This bed is occupied")?,
                    overlay: true,
                },
                state.compression,
            )
            .await?;
        }
        SleepOutcome::Waiting { sleeping, required } => {
            if set_bed_occupied(state, writer, canonical_bed, true).await? == Some(false) {
                state.sessions.cancel_sleep_reservation(state.session_id);
                return Err(ConnectionError::RuntimeUnavailable {
                    operation: "marking occupied bed",
                });
            }
            if state.sessions.sleeping_bed(state.session_id) != Some(canonical_bed) {
                let _ = set_bed_occupied(state, writer, canonical_bed, false).await?;
                send_command_feedback(writer, state.compression, "Respawn point set").await?;
                return Ok(true);
            }
            dispatch_visibility_commands(
                state.sessions.broadcast_player_entity_data_including_self(
                    state.session_id,
                    vec![player_pose_entity_data(EntityPose::Sleeping)],
                ),
            );
            send_command_feedback(
                writer,
                state.compression,
                &format!("Respawn point set; {sleeping}/{required} players sleeping"),
            )
            .await?;
        }
        SleepOutcome::Inactive => {
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "entering player sleep",
            });
        }
    }
    Ok(true)
}

async fn set_bed_occupied<W>(
    state: &mut InteractionState,
    writer: &mut W,
    canonical_bed: mc_world::BlockPos,
    occupied: bool,
) -> Result<Option<bool>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some((edits, preconditions)) =
        plan_bed_occupied_edits(&state.world_read, &state.blocks, canonical_bed, occupied)
    else {
        return Ok(None);
    };
    if edits.is_empty() {
        return Ok(Some(true));
    }
    Ok(Some(
        apply_visible_block_edit_batch_conditionally(state, writer, &edits, &preconditions, &[])
            .await?
            .is_some(),
    ))
}

fn player_pose_entity_data(pose: EntityPose) -> EntityDataValue {
    EntityDataValue::Pose {
        index: ENTITY_DATA_POSE_INDEX,
        pose,
    }
}

#[allow(clippy::too_many_arguments)]
async fn wake_player_from_bed<W>(
    state: &mut InteractionState,
    writer: &mut W,
    compression: Compression,
    simulation: &SimulationHandle,
    bed: mc_world::BlockPos,
    player_pose: &mut PlayerPose,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<Option<GameMode>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(release) = release_staged_sleep_bed(state, writer, bed).await? else {
        return Ok(None);
    };
    match release {
        SleepBedRelease::Completed => {}
        SleepBedRelease::Rejected { rollback_mode } => return Ok(rollback_mode),
    }
    let mut wake_pose = safe_bed_wake_pose(
        &state.world_read,
        &state.blocks,
        &state.block_facts,
        bed,
        *player_pose,
    );
    refresh_player_water_state(Some(state), &mut wake_pose).await;
    commit_authoritative_player_pose(simulation, wake_pose).await?;
    *player_pose = wake_pose;
    let teleport_id = next_player_teleport_id(next_teleport_id);
    send_player_position_sync(writer, compression, teleport_id, wake_pose).await?;
    *pending_teleport = Some(PendingTeleport::new(
        teleport_id,
        state.sessions.simulation_tick(),
    ));
    Ok(None)
}

enum SleepBedRelease {
    Completed,
    Rejected { rollback_mode: Option<GameMode> },
}

async fn release_staged_sleep_bed<W>(
    state: &mut InteractionState,
    writer: &mut W,
    bed: mc_world::BlockPos,
) -> Result<Option<SleepBedRelease>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(token) = state.sessions.claim_sleep_wake(state.session_id, bed) else {
        return Ok(None);
    };
    match set_bed_occupied(state, writer, bed, false).await {
        Ok(Some(true)) => {
            let Some(completed) = state.sessions.complete_sleep_wake(token) else {
                return Ok(None);
            };
            dispatch_visibility_commands(completed.dispatches);
            Ok(Some(SleepBedRelease::Completed))
        }
        Ok(None | Some(false)) => {
            let rollback = state.sessions.reject_sleep_wake(token);
            Ok(Some(SleepBedRelease::Rejected {
                rollback_mode: rollback,
            }))
        }
        Err(error) => {
            state.sessions.reject_sleep_wake(token);
            Err(error)
        }
    }
}

async fn release_disconnected_sleep_bed<W>(
    state: &mut InteractionState,
    writer: &mut W,
    bed: mc_world::BlockPos,
) where
    W: AsyncWriteExt + Unpin,
{
    #[cfg(test)]
    {
        let _ = release_staged_sleep_bed(state, writer, bed).await;
    }
    #[cfg(not(test))]
    {
        let _ = writer;
        let Some(token) = state.sessions.claim_sleep_wake(state.session_id, bed) else {
            return;
        };
        let Some((edits, preconditions)) =
            plan_bed_occupied_edits(&state.world_read, &state.blocks, bed, false)
        else {
            state.sessions.reject_sleep_wake(token);
            return;
        };
        let committed = if edits.is_empty() {
            true
        } else {
            matches!(
                state
                    .simulation
                    .apply_block_edits_with_scheduled_ticks(edits, preconditions, Vec::new())
                    .await,
                Ok(Some(_))
            )
        };
        if committed {
            if let Some(completed) = state.sessions.complete_sleep_wake(token) {
                dispatch_visibility_commands(completed.dispatches);
            }
        } else {
            state.sessions.reject_sleep_wake(token);
        }
    }
}

async fn interact_with_toggle_block<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let pos = mc_world::BlockPos { x, y, z };
    let Some(plan) =
        plan_loaded_toggle_block_interaction(state, pos, state.sessions.simulation_tick())
    else {
        return Ok(false);
    };
    let _ = apply_player_block_edit_batch_conditionally(
        state,
        writer,
        sequence,
        &plan.edits,
        &plan.preconditions,
        &plan.scheduled_block_ticks,
    )
    .await?;
    Ok(true)
}

fn plan_loaded_toggle_block_interaction(
    state: &InteractionState,
    pos: mc_world::BlockPos,
    world_tick: u64,
) -> Option<ToggleBlockPlan> {
    let protection = state.script_zones.as_ref().map(|zones| {
        zones.protection_snapshot().unwrap_or_else(|error| {
            warn!(
                ?error,
                "zone protection snapshot unavailable; denying piston movement"
            );
            crate::script::ZoneProtectionSnapshot::unavailable()
        })
    });
    let centre = ChunkPos {
        x: pos.x.div_euclid(SECTION_DIM as i32),
        z: pos.z.div_euclid(SECTION_DIM as i32),
    };
    let mut chunks = Vec::with_capacity(9);
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(ChunkPos {
                x: centre.x + dx,
                z: centre.z + dz,
            });
        }
    }
    let snapshot = state.world_read.snapshot_chunks(&chunks);
    let clicked = snapshot.get_cached_block(pos)?;
    plan_toggle_block_interaction_with_protection(
        &state.blocks,
        &snapshot,
        pos,
        clicked,
        world_tick,
        protection.as_ref(),
    )
}

fn adjacent_block_positions(pos: mc_world::BlockPos) -> [mc_world::BlockPos; 6] {
    [
        mc_world::BlockPos {
            x: pos.x + 1,
            ..pos
        },
        mc_world::BlockPos {
            x: pos.x - 1,
            ..pos
        },
        mc_world::BlockPos {
            y: pos.y + 1,
            ..pos
        },
        mc_world::BlockPos {
            y: pos.y - 1,
            ..pos
        },
        mc_world::BlockPos {
            z: pos.z + 1,
            ..pos
        },
        mc_world::BlockPos {
            z: pos.z - 1,
            ..pos
        },
    ]
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn block_edit_changes_light(
    table: &BlockLightTable,
    previous: mc_world::BlockStateId,
    new_state: mc_world::BlockStateId,
) -> bool {
    table.emission(previous.0).unwrap_or(0) != table.emission(new_state.0).unwrap_or(0)
        || table.opacity(previous.0).unwrap_or(0) != table.opacity(new_state.0).unwrap_or(0)
        || table.propagates_sky(previous.0).unwrap_or(true)
            != table.propagates_sky(new_state.0).unwrap_or(true)
}

pub(crate) fn air_state_id(registry: &mc_world::BlockRegistry) -> mc_world::BlockStateId {
    let air_id = mc_data::Identifier::parse("minecraft:air").expect("static identifier");
    registry
        .block(&air_id)
        .map(|b| b.default)
        .unwrap_or(mc_world::BlockStateId(0))
}

fn fluid_state_ids(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    kind: FluidKind,
    fallback: Option<mc_world::BlockStateId>,
) -> Vec<mc_world::BlockStateId> {
    let mut states = blocks
        .states()
        .filter(|state| {
            facts
                .fluid(state.id.0)
                .is_some_and(|fluid| fluid.kind == kind)
        })
        .map(|state| state.id)
        .collect::<Vec<_>>();
    if states.is_empty()
        && let Some(fallback) = fallback
    {
        states.push(fallback);
    }
    states.sort_unstable_by_key(|state| state.0);
    states.dedup();
    states
}

fn published_block_precondition(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> Option<BlockEditPrecondition> {
    let snapshot = state.world_read.snapshot_chunks(&[ChunkPos {
        x: position.x.div_euclid(SECTION_DIM as i32),
        z: position.z.div_euclid(SECTION_DIM as i32),
    }]);
    Some(BlockEditPrecondition {
        pos: position,
        expected_state: snapshot.get_cached_block(position)?,
        expected_token: snapshot.block_mutation_token(position)?,
    })
}

async fn schedule_fluid_ticks_for_interaction(
    state: &InteractionState,
    applied: &[AppliedBlockEdit],
) {
    let current_tick = state.sessions.simulation_tick();
    if let Err(error) = state
        .simulation
        .schedule_fluid_ticks_near_applied(
            applied.to_vec(),
            Arc::clone(&state.block_facts),
            current_tick,
        )
        .await
    {
        warn!(?error, "simulation fluid tick scheduling rejected");
    }
}

async fn handle_use_item<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    action: ServerboundUseItem,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival {
        return ack_use_item_noop(
            writer,
            state.compression,
            action.sequence,
            "non_survival_mode",
        )
        .await;
    }
    if survival_state.is_dead() {
        return ack_use_item_noop(writer, state.compression, action.sequence, "dead_player").await;
    }

    if start_shield_use(state, action.hand) {
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    let held_slot = hand_inventory_slot(state, action.hand);
    if is_bow_item(state, held_slot) {
        let held_item_id = state.inventory.slots[held_slot].item_id;
        state.pending_break = None;
        state.pending_use = Some(PendingUse {
            started_tick: state.sessions.simulation_tick(),
            required_ticks: item_use_ticks(Duration::from_secs(60)),
            held_hotbar_slot: state.selected_hotbar_slot,
            held_slot,
            held_item_id,
            kind: UseKind::Bow,
        });
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    if survival_state.food >= SurvivalState::MAX_FOOD {
        return ack_use_item_noop(writer, state.compression, action.sequence, "full_food").await;
    }

    let Some((held_item_id, rule, required_time)) = held_food_use(state, held_slot) else {
        return ack_use_item_noop(
            writer,
            state.compression,
            action.sequence,
            "unsupported_item",
        )
        .await;
    };

    state.pending_break = None;
    state.pending_use = Some(PendingUse {
        started_tick: state.sessions.simulation_tick(),
        required_ticks: item_use_ticks(required_time),
        held_hotbar_slot: state.selected_hotbar_slot,
        held_slot,
        held_item_id,
        kind: UseKind::Food(rule),
    });
    write_block_ack(writer, state.compression, action.sequence).await
}

async fn complete_food_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    pending: PendingUse,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if survival_state.is_dead()
        || survival_state.food >= SurvivalState::MAX_FOOD
        || !pending_use_matches(state, &pending)
    {
        return Ok(());
    }

    let UseKind::Food(food_rule) = &pending.kind else {
        return Ok(());
    };
    let committed = match state
        .simulation
        .commit_food_use(FoodUsePlan {
            held_slot: pending.held_slot,
            expected_held: state.inventory.slots[pending.held_slot].clone(),
            expected_survival: *survival_state,
            food: food_rule.food,
            saturation: food_rule.saturation,
        })
        .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            debug!("food use rejected because player state changed");
            return Ok(());
        }
        Err(error) => {
            debug!(?error, "simulation food use request rejected");
            return Ok(());
        }
    };
    state.inventory = committed.inventory;
    *survival_state = committed.survival;
    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    write_packet(writer, &survival_state.as_packet(), state.compression).await
}

async fn tick_pending_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    current_tick: u64,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival {
        state.pending_use = None;
        clear_shield_use(state);
        return Ok(());
    }
    refresh_shield_use_state(state);
    let Some(pending) = state.pending_use else {
        return Ok(());
    };
    if survival_state.is_dead() || !pending_use_matches(state, &pending) {
        state.pending_use = None;
        return Ok(());
    }
    if pending_use_is_complete(&pending, current_tick) {
        state.pending_use = None;
        complete_food_use(state, writer, survival_state, pending).await?;
    }
    Ok(())
}

async fn replan_after_movement<W>(
    writer: &mut W,
    compression: Compression,
    chunk_stream: &mut Option<ChunkStreamState>,
    interaction: Option<&mut InteractionState>,
    old_center: (i32, i32),
    new_center: (i32, i32),
    direction_yaw: f32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if old_center == new_center {
        if let Some(stream) = chunk_stream.as_mut() {
            let _ = stream.replan_center(new_center.0, new_center.1, direction_yaw);
        }
        return Ok(());
    }
    write_packet(
        writer,
        &SetCenterChunk {
            chunk_x: new_center.0,
            chunk_z: new_center.1,
        },
        compression,
    )
    .await?;
    if let Some(stream) = chunk_stream.as_mut() {
        let unloads = stream.replan_center(new_center.0, new_center.1, direction_yaw);
        let mut interaction = interaction;
        for (chunk_x, chunk_z) in unloads {
            if let Some(state) = interaction.as_deref_mut() {
                state.light_cache.remove(ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                });
            }
            write_packet(writer, &ForgetLevelChunk { chunk_x, chunk_z }, compression).await?;
        }
    }
    debug!(
        old_cx = old_center.0,
        old_cz = old_center.1,
        new_cx = new_center.0,
        new_cz = new_center.1,
        "chunk view center updated from movement"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_accepted_absolute_movement<W>(
    writer: &mut W,
    compression: Compression,
    interaction: &mut Option<&mut InteractionState>,
    chunk_stream: &mut Option<ChunkStreamState>,
    simulation: &SimulationHandle,
    script_zone_observer: &mut Option<ScriptZoneObserver>,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    game_mode: GameMode,
    player_pose: &mut PlayerPose,
    movement: AcceptedAbsoluteMovement,
    current_tick: u64,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let movement = normalize_absolute_player_movement(movement)?;
    let old_center = player_pose.chunk_pos();
    let old_pose = *player_pose;
    let mut new_pose = *player_pose;
    new_pose.x = movement.x;
    new_pose.y = movement.y;
    new_pose.z = movement.z;
    if let Some((yaw, pitch)) = movement.yaw_pitch {
        new_pose.yaw = yaw;
        new_pose.pitch = pitch;
    }
    new_pose.flags = movement.flags;
    refresh_player_water_state(interaction.as_deref(), &mut new_pose).await;
    refresh_player_fall_state(old_pose, &mut new_pose);
    if correct_player_collision(
        interaction.as_deref(),
        writer,
        compression,
        old_pose,
        new_pose,
        current_tick,
        next_teleport_id,
        pending_teleport,
    )
    .await?
    {
        *player_pose = old_pose;
        return Ok(());
    }

    *player_pose = new_pose;
    let exhaustion = if game_mode == GameMode::Survival {
        movement_exhaustion(old_pose, *player_pose)
    } else {
        0.0
    };
    let committed_pose =
        commit_authoritative_player_movement(simulation, *player_pose, exhaustion).await?;
    if let Some(observer) = script_zone_observer.as_mut() {
        observer.observe(*player_pose).await;
    }
    if game_mode == GameMode::Survival {
        committed_pose.apply_resources_to(survival_state);
        if committed_pose.resources_changed {
            write_packet(writer, &survival_state.as_packet(), compression).await?;
        }
    }
    if game_mode == GameMode::Survival
        && let Some(state) = interaction.as_deref_mut()
    {
        maybe_trample_farmland(state, writer, old_pose, *player_pose).await?;
    }
    if game_mode == GameMode::Survival {
        apply_fall_damage(
            interaction.as_deref_mut(),
            writer,
            compression,
            survival_state,
            xp_state,
            old_pose,
            *player_pose,
        )
        .await?;
    }
    let new_center = player_pose.chunk_pos();
    replan_after_movement(
        writer,
        compression,
        chunk_stream,
        interaction.as_deref_mut(),
        old_center,
        new_center,
        player_pose.yaw,
    )
    .await?;
    Ok(())
}

async fn commit_authoritative_player_pose(
    simulation: &SimulationHandle,
    pose: PlayerPose,
) -> Result<(), ConnectionError> {
    commit_authoritative_player_movement(simulation, pose, 0.0)
        .await
        .map(drop)
}

async fn commit_authoritative_player_movement(
    simulation: &SimulationHandle,
    pose: PlayerPose,
    exhaustion: f32,
) -> Result<CommittedPlayerPose, ConnectionError> {
    simulation
        .commit_player_pose(pose, exhaustion)
        .await
        .map_err(|error| {
            warn!(?error, "simulation player pose commit failed");
            ConnectionError::RuntimeUnavailable {
                operation: "committing player pose",
            }
        })
}

#[cfg(test)]
async fn settle_disconnected_cursor(
    interaction: &mut InteractionState,
    player_save_state: &Arc<Mutex<PlayerPersistedState>>,
) -> bool {
    if interaction.carried_item.is_empty() {
        return true;
    }
    let pose = {
        let state = player_save_state.lock().unwrap_or_else(|poisoned| {
            warn!("player persistence mutex was poisoned while settling cursor");
            poisoned.into_inner()
        });
        state.pose
    };
    match settle_player_inventory_returns(interaction, InventoryReturnPlan::cursor(pose)).await {
        Ok(()) => true,
        Err(error) => {
            warn!(?error, "cursor settlement deferred");
            false
        }
    }
}

async fn settle_disconnected_inventory(
    interaction: &mut InteractionState,
    player_save_state: &Arc<Mutex<PlayerPersistedState>>,
) -> bool {
    let (pose, owner_has_crafting_table_input, owner_enchanting_table_input, owner_merchant_input) = {
        let state = player_save_state.lock().unwrap_or_else(|poisoned| {
            warn!("player persistence mutex was poisoned while settling disconnect inventory");
            poisoned.into_inner()
        });
        (
            state.pose,
            state.crafting_table_input.is_some(),
            state.enchanting_table_input.clone(),
            state.merchant_input.clone(),
        )
    };
    let active = interaction.active_container.take();
    let crafting_table_input = match &active {
        Some(ActiveContainer::CraftingTable(window)) => Some(window.input.clone()),
        Some(ActiveContainer::Stonecutter(window)) => Some(stonecutter_input_array(&window.input)),
        _ => None,
    };
    let return_crafting_table_input = owner_has_crafting_table_input
        || crafting_table_input
            .as_ref()
            .is_some_and(|input| crafting_table_input_projection(input).is_some());
    let enchanting_table_input = match &active {
        Some(ActiveContainer::EnchantingTable(window)) => Some(window.inputs.clone()),
        _ => owner_enchanting_table_input.map(|input| *input),
    };
    let merchant_input = match &active {
        Some(ActiveContainer::Merchant(window)) => Some(window.inputs.clone()),
        _ => owner_merchant_input.map(|input| *input),
    };
    match &active {
        Some(ActiveContainer::Furnace(window)) => {
            interaction
                .sessions
                .unregister_furnace_viewer(interaction.session_id, window.position);
        }
        Some(ActiveContainer::Chest(window)) => {
            interaction
                .sessions
                .unregister_chest_viewer(interaction.session_id, window.position());
        }
        _ => {}
    }

    match settle_player_inventory_returns(
        interaction,
        InventoryReturnPlan::disconnect(
            enchanting_table_input.as_ref(),
            merchant_input.as_ref(),
            crafting_table_input.as_ref(),
            return_crafting_table_input,
            pose,
        ),
    )
    .await
    {
        Ok(()) => true,
        Err(error) => {
            if active.as_ref().is_some_and(|active| {
                matches!(
                    active,
                    ActiveContainer::CraftingTable(_)
                        | ActiveContainer::EnchantingTable(_)
                        | ActiveContainer::Stonecutter(_)
                        | ActiveContainer::Merchant(_)
                )
            }) {
                interaction.active_container = active;
            }
            warn!(?error, "disconnect inventory settlement deferred");
            false
        }
    }
}

async fn recv_outbound_command(
    rx: &mut mpsc::Receiver<OutboundCommand>,
    pending: &mut VecDeque<OutboundCommand>,
) -> Option<OutboundCommand> {
    if let Some(command) = pending.pop_front() {
        Some(command)
    } else {
        rx.recv().await
    }
}

fn collect_block_delta_batch(
    mut deltas: Vec<BlockDelta>,
    rx: &mut mpsc::Receiver<OutboundCommand>,
    pending: &mut VecDeque<OutboundCommand>,
) -> Vec<BlockDelta> {
    while let Ok(command) = rx.try_recv() {
        match command {
            OutboundCommand::BlockDeltas(mut more) => deltas.append(&mut more),
            other => {
                pending.push_back(other);
                break;
            }
        }
    }
    deltas
}

fn collect_light_update_batch(
    mut updates: Vec<OutboundLightUpdate>,
    rx: &mut mpsc::Receiver<OutboundCommand>,
    pending: &mut VecDeque<OutboundCommand>,
) -> Vec<OutboundLightUpdate> {
    while let Ok(command) = rx.try_recv() {
        match command {
            OutboundCommand::LightUpdates(mut more) => updates.append(&mut more),
            other => {
                pending.push_back(other);
                break;
            }
        }
    }
    updates
}

fn take_entity_movement_write_turn(
    mut movements: Vec<ServerEntityMove>,
) -> (Vec<ServerEntityMove>, Option<Vec<ServerEntityMove>>) {
    let remaining = (movements.len() > ENTITY_MOVEMENTS_PER_WRITE_TURN)
        .then(|| movements.split_off(ENTITY_MOVEMENTS_PER_WRITE_TURN));
    (movements, remaining)
}

fn outbound_queue_at_shed_pressure(
    rx: &mpsc::Receiver<OutboundCommand>,
    pending: &VecDeque<OutboundCommand>,
) -> Option<(usize, usize, usize)> {
    let queued = rx.len() + pending.len();
    let channel_capacity = rx.max_capacity();
    if channel_capacity == 0 {
        return None;
    }
    let threshold = (channel_capacity * SLOW_CLIENT_OUTBOUND_PRESSURE_NUMERATOR)
        .div_ceil(SLOW_CLIENT_OUTBOUND_PRESSURE_DENOMINATOR)
        .max(1);
    if queued >= threshold {
        Some((queued, channel_capacity, threshold))
    } else {
        None
    }
}

fn outbound_command_queue_capacity(config: &ServerConfig) -> usize {
    config
        .chunk_pipeline
        .chunk_result_queue_size
        .max(16)
        .max((config.max_players as usize).saturating_mul(OUTBOUND_COMMANDS_PER_PLAYER_BURST))
}

fn active_furnace_window_at(
    active: &mut Option<ActiveContainer>,
    position: mc_world::BlockPos,
) -> Option<&mut FurnaceWindow> {
    match active.as_mut() {
        Some(ActiveContainer::Furnace(window)) if window.position == position => Some(window),
        _ => None,
    }
}

fn active_chest_window_at(
    active: &mut Option<ActiveContainer>,
    position: mc_world::BlockPos,
) -> Option<&mut ChestWindow> {
    match active.as_mut() {
        Some(ActiveContainer::Chest(window)) if window.position() == position => Some(window),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundWriteOutcome {
    Sent,
    TimedOut,
}

struct SlowClientWriteGuard<W> {
    writer: W,
    blocked: Option<oneshot::Sender<()>>,
}

impl<W> SlowClientWriteGuard<W> {
    fn new(writer: W) -> (Self, oneshot::Receiver<()>) {
        let (blocked, blocked_rx) = oneshot::channel();
        (
            Self {
                writer,
                blocked: Some(blocked),
            },
            blocked_rx,
        )
    }

    fn record_blocked(&mut self) {
        if let Some(blocked) = self.blocked.take() {
            let _ = blocked.send(());
        }
    }
}

impl<W> AsyncWrite for SlowClientWriteGuard<W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_write(cx, buf);
        if result.is_pending() {
            this.record_blocked();
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_flush(cx);
        if result.is_pending() {
            this.record_blocked();
        }
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_shutdown(cx);
        if result.is_pending() {
            this.record_blocked();
        }
        result
    }
}

struct KeepAliveTracker {
    next_id: i64,
    pending_id: Option<i64>,
    pending_since: Option<Instant>,
    last_inbound_at: Instant,
}

impl KeepAliveTracker {
    fn new() -> Self {
        Self {
            next_id: 0,
            pending_id: None,
            pending_since: None,
            last_inbound_at: Instant::now(),
        }
    }

    fn record_request(&mut self) -> Option<i64> {
        if self.pending_id.is_some() {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.pending_id = Some(self.next_id);
        self.pending_since = Some(Instant::now());
        Some(self.next_id)
    }

    fn record_response(&mut self, id: i64) -> bool {
        if self.pending_id != Some(id) {
            return false;
        }
        self.pending_id = None;
        self.pending_since = None;
        true
    }

    fn pending_id(&self) -> Option<i64> {
        self.pending_id
    }

    fn pending_elapsed(&self) -> Option<Duration> {
        self.pending_since.map(|started| started.elapsed())
    }

    fn record_inbound_activity(&mut self) {
        self.last_inbound_at = Instant::now();
    }

    fn timed_out(&self, timeout: Duration) -> Option<Duration> {
        let pending_elapsed = self.pending_elapsed()?;
        (pending_elapsed > timeout && self.last_inbound_at.elapsed() > timeout)
            .then_some(pending_elapsed)
    }
}

async fn slow_client_outbound_write_timeout<F>(
    write: F,
    blocked: oneshot::Receiver<()>,
    timeout: Duration,
) -> Result<OutboundWriteOutcome, ConnectionError>
where
    F: Future<Output = Result<(), ConnectionError>>,
{
    tokio::pin!(write);
    tokio::pin!(blocked);

    tokio::select! {
        biased;
        result = write.as_mut() => {
            result?;
            Ok(OutboundWriteOutcome::Sent)
        }
        blocked = blocked.as_mut() => {
            if blocked.is_err() {
                write.await?;
                return Ok(OutboundWriteOutcome::Sent);
            }
            match tokio::time::timeout(timeout, write.as_mut()).await {
                Ok(result) => {
                    result?;
                    Ok(OutboundWriteOutcome::Sent)
                }
                Err(_) => Ok(OutboundWriteOutcome::TimedOut),
            }
        }
    }
}

async fn slow_client_chunk_stream_step_timeout<F>(
    sessions: &SessionRegistry,
    session_id: SessionId,
    step: F,
    timeout: Duration,
) -> Result<Option<ChunkStreamStep>, ConnectionError>
where
    F: Future<Output = Result<ChunkStreamStep, ConnectionError>>,
{
    match tokio::time::timeout(timeout, step).await {
        Ok(result) => result.map(Some),
        Err(_) => {
            sessions.record_slow_client_write_timeout();
            warn!(
                session_id,
                timeout_ms = timeout.as_millis() as u64,
                "slow client chunk stream write timed out; closing play session"
            );
            Ok(None)
        }
    }
}

async fn wait_for_chunk_stream_wake(
    progress: Arc<tokio::sync::Notify>,
    sessions: &SessionRegistry,
    prepared_generation: u64,
    memory_changes: Option<
        &mut tokio::sync::watch::Receiver<crate::memory_pressure::MemoryPressureObservation>,
    >,
) {
    let memory_changed = async {
        let Some(memory_changes) = memory_changes else {
            std::future::pending::<()>().await;
            return;
        };
        if memory_changes.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
        () = progress.notified() => {}
        () = sessions.wait_for_prepared_change(prepared_generation) => {}
        () = memory_changed => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn play_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    interaction: Option<&mut InteractionState>,
    chunk_stream: Option<ChunkStreamState>,
    runtime_control: Option<RuntimeControlHandle>,
    chunk_pipeline_resources: ChunkPipelineResources,
    sessions: Arc<SessionRegistry>,
    simulation: SimulationHandle,
    config: &ServerConfig,
    session_id: SessionId,
    loader_eligible: bool,
    player_pose: PlayerPose,
    respawn_pose: PlayerPose,
    respawn: ClientboundRespawn,
    permissions: CommandPermissions,
    survival_state: SurvivalState,
    xp_state: XpState,
    game_mode: GameMode,
    outbound_rx: mpsc::Receiver<OutboundCommand>,
    server_view_distance: i32,
    player_uuid: String,
    player_name: String,
    extension: Option<ExtensionEventSink>,
    extension_player_id: PlayerId,
    scripts: Option<ScriptEventSink>,
    script_zones: Option<PluginZoneAdapter>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let result = play_loop_inner(
        reader,
        writer,
        buf,
        compression,
        interaction,
        chunk_stream,
        runtime_control,
        chunk_pipeline_resources,
        Arc::clone(&sessions),
        simulation,
        config,
        session_id,
        loader_eligible,
        player_pose,
        respawn_pose,
        respawn,
        permissions,
        survival_state,
        xp_state,
        game_mode,
        outbound_rx,
        server_view_distance,
        player_uuid,
        player_name,
        extension,
        extension_player_id,
        scripts,
        script_zones,
    )
    .await;

    if let Err(ConnectionError::WriteTimeout { timeout }) = result {
        sessions.record_slow_client_write_timeout();
        warn!(
            session_id,
            timeout_ms = timeout.as_millis() as u64,
            "outbound socket write timed out; closing play session"
        );
        return Ok(());
    }

    result
}

#[allow(clippy::too_many_arguments)]
async fn play_loop_inner<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    mut interaction: Option<&mut InteractionState>,
    mut chunk_stream: Option<ChunkStreamState>,
    runtime_control: Option<RuntimeControlHandle>,
    chunk_pipeline_resources: ChunkPipelineResources,
    sessions: Arc<SessionRegistry>,
    simulation: SimulationHandle,
    config: &ServerConfig,
    session_id: SessionId,
    loader_eligible: bool,
    mut player_pose: PlayerPose,
    mut respawn_pose: PlayerPose,
    respawn: ClientboundRespawn,
    permissions: CommandPermissions,
    mut survival_state: SurvivalState,
    mut xp_state: XpState,
    mut game_mode: GameMode,
    mut outbound_rx: mpsc::Receiver<OutboundCommand>,
    server_view_distance: i32,
    player_uuid: String,
    player_name: String,
    extension: Option<ExtensionEventSink>,
    extension_player_id: PlayerId,
    scripts: Option<ScriptEventSink>,
    script_zones: Option<PluginZoneAdapter>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut ticker = interval(KEEPALIVE_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first `tick()` resolves immediately; drop it so we don't
    // race-send a keepalive before the client has processed initial
    // Play packets and the first chunk.
    ticker.tick().await;
    let mut world_time_ticker = interval(WORLD_TIME_SYNC_PERIOD);
    world_time_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    world_time_ticker.tick().await;

    let mut keepalive = KeepAliveTracker::new();
    let mut script_zone_observer = script_zones.clone().map(|zones| ScriptZoneObserver {
        zones,
        player_id: ScriptPlayerId::new(session_id),
        uuid: player_uuid.clone(),
        username: player_name.clone(),
        permissions,
        dimension: respawn.dimension_name.to_string(),
        revision: 0,
    });
    let script_gameplay_events = scripts.as_ref().map(|sink| {
        ScriptGameplayEventPublisher::new(
            sink.clone(),
            ScriptPlayerId::new(session_id),
            player_uuid.clone(),
            player_name.clone(),
            permissions,
            respawn.dimension_name.to_string(),
        )
        .with_zones(script_zones.clone())
    });
    if let Some(observer) = script_zone_observer.as_mut() {
        observer.observe(player_pose).await;
    }
    let mut food_tick_timer: u32 = 0;
    let mut breathing_state = PlayerBreathingState::default();
    let mut client_load = ClientLoadGate::default();
    let mut next_teleport_id: i32 = 2;
    let mut pending_teleport = Some(PendingTeleport::new(1, sessions.simulation_tick()));
    let mut client_brand: Option<String> = None;
    let mut client_preferences: Option<ClientPreferences> = None;
    let mut effective_client_view_distance = server_view_distance;
    let mut pending_outbound = VecDeque::new();
    let mut simulation_ticks = sessions.subscribe_simulation_ticks();
    send_world_time(writer, compression, &sessions).await?;
    write_packet(writer, &survival_state.as_packet(), compression).await?;
    write_packet(writer, &xp_state.as_packet(), compression).await?;
    let mut chunk_stream_needs_step = chunk_stream
        .as_ref()
        .is_some_and(|stream| !stream.is_complete());
    let mut chunk_prepared_generation = sessions.prepared_change_generation();
    let mut memory_pressure_changes = runtime_control
        .as_ref()
        .map(RuntimeControlHandle::subscribe_memory_pressure);

    loop {
        let mut stream_finished = false;
        if chunk_stream_needs_step
            && outbound_queue_at_shed_pressure(&outbound_rx, &pending_outbound).is_none()
            && let (Some(stream), Some(state)) = (chunk_stream.as_mut(), interaction.as_deref_mut())
            && !stream.is_complete()
        {
            chunk_prepared_generation = sessions.prepared_change_generation();
            chunk_stream_needs_step = false;
            for _ in 0..CHUNK_STREAM_STEPS_PER_TURN {
                if stream.is_complete() {
                    stream_finished = true;
                    break;
                }
                let Some(step) = slow_client_chunk_stream_step_timeout(
                    &sessions,
                    session_id,
                    stream.step(writer, &mut state.light_cache),
                    SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT,
                )
                .await?
                else {
                    return Ok(());
                };
                match step {
                    ChunkStreamStep::Progress => {
                        stream_finished = stream.is_complete();
                    }
                    ChunkStreamStep::Complete => {
                        stream_finished = true;
                        break;
                    }
                }
            }
        }
        if stream_finished && let Some(stream) = chunk_stream.as_mut() {
            stream.log_summary_once();
        }
        let chunk_stream_waiting = chunk_stream
            .as_ref()
            .is_some_and(|stream| !stream.is_complete());
        let chunk_progress_notify = chunk_stream
            .as_ref()
            .filter(|stream| !stream.is_complete())
            .map(ChunkStreamState::progress_notify);
        if let Some(stream) = chunk_stream.as_ref()
            && stream.has_immediate_work()
            && let Some(notify) = chunk_progress_notify.as_ref()
        {
            notify.notify_one();
        }
        let chunk_progress_sessions = Arc::clone(&sessions);

        tokio::select! {
            command = recv_outbound_command(&mut outbound_rx, &mut pending_outbound) => {
                let mut close_session = false;
                let (mut guarded_writer, blocked) = SlowClientWriteGuard::new(&mut *writer);
                let outcome = slow_client_outbound_write_timeout(async {
                    let writer = &mut guarded_writer;
                    match command {
                    Some(OutboundCommand::BlockDeltas(deltas)) => {
                        let deltas = collect_block_delta_batch(deltas, &mut outbound_rx, &mut pending_outbound);
                        let projection = sessions
                            .loader_block_projection(session_id, &config.blocks);
                        send_block_deltas(writer, compression, &deltas, projection.as_ref()).await?;
                    }
                    Some(OutboundCommand::LightUpdates(updates)) => {
                        if let Some(state) = interaction.as_deref_mut() {
                            let updates = collect_light_update_batch(updates, &mut outbound_rx, &mut pending_outbound);
                            send_light_updates(state, writer, &updates).await?;
                        }
                    }
                    Some(OutboundCommand::SpawnPlayer(player)) => {
                        send_player_spawn(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::MovePlayer(player)) => {
                        send_player_move(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::DespawnPlayer(player)) => {
                        send_player_despawn(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::SpawnEntity(entity)) => {
                        send_entity_spawn(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::SpawnEntities(mut entities)) => {
                        if entities.len() > ENTITY_SPAWNS_PER_WRITE_TURN {
                            let remaining = entities.split_off(ENTITY_SPAWNS_PER_WRITE_TURN);
                            pending_outbound.push_front(OutboundCommand::SpawnEntities(remaining));
                        }
                        for entity in &entities {
                            send_entity_spawn(writer, compression, entity).await?;
                        }
                    }
                    Some(OutboundCommand::UpdateEntityData(entity)) => {
                        send_entity_data(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::UpdateEntityHealth(entity)) => {
                        send_entity_health(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::MoveEntityRelative(movement)) => {
                        send_entity_relative_move(writer, compression, &movement).await?;
                    }
                    Some(OutboundCommand::MoveEntitiesRelative(movements)) => {
                        let (movements, remaining) =
                            take_entity_movement_write_turn(movements);
                        if let Some(remaining) = remaining {
                            pending_outbound
                                .push_front(OutboundCommand::MoveEntitiesRelative(remaining));
                        }
                        for movement in &movements {
                            send_entity_relative_move(writer, compression, movement).await?;
                        }
                    }
                    Some(OutboundCommand::EntityEvent { entity_id, event_id }) => {
                        write_packet(writer, &EntityEvent { entity_id, event_id }, compression)
                            .await?;
                    }
                    Some(OutboundCommand::LevelEvent(event)) => {
                        write_packet(writer, &event, compression).await?;
                    }
                    Some(OutboundCommand::DamagePlayer { damage }) => {
                        if sessions.player_accepts_damage(session_id) {
                            apply_player_damage(
                                interaction.as_deref_mut(),
                                writer,
                                compression,
                                &mut survival_state,
                                &mut xp_state,
                                game_mode,
                                PlayerDamageApplication {
                                    player_pose,
                                    request: damage,
                                },
                            )
                            .await?;
                        }
                    }
                    Some(OutboundCommand::PlayerDamageCommitted {
                        publication,
                        hurt_event,
                    }) => {
                        let applied = apply_player_damage_publication(
                            interaction.as_deref_mut(),
                            &mut survival_state,
                            &mut xp_state,
                            *publication,
                        );
                        if applied.survival_changed {
                            write_packet(writer, &survival_state.as_packet(), compression).await?;
                        }
                        if let Some(state) = interaction.as_deref_mut() {
                            if applied.died {
                                write_inventory_content(state, writer).await?;
                            } else if !applied.changed_slots.is_empty() {
                                write_inventory_slot_updates(
                                    state,
                                    writer,
                                    applied.changed_slots,
                                )
                                .await?;
                            }
                        }
                        if applied.xp_changed {
                            write_packet(writer, &xp_state.as_packet(), compression).await?;
                        }
                        if let Some(cooldown) = applied.shield_cooldown {
                            write_packet(
                                writer,
                                &ClientboundCooldown {
                                    cooldown_group: cooldown.cooldown_group,
                                    duration: cooldown.duration,
                                },
                                compression,
                            )
                            .await?;
                        }
                        if let Some(knockback) = applied.knockback {
                            write_packet(
                                writer,
                                &SetEntityMotion {
                                    entity_id: i32::try_from(session_id).unwrap_or(i32::MAX),
                                    movement: player_melee_knockback(knockback),
                                },
                                compression,
                            )
                            .await?;
                        }
                        if applied.fresh_hurt {
                            write_packet(
                                writer,
                                &EntityEvent {
                                    entity_id: hurt_event.entity_id,
                                    event_id: 2,
                                },
                                compression,
                            )
                            .await?;
                        }
                    }
                    Some(OutboundCommand::TakeItemEntity {
                        item_entity_id,
                        player_entity_id,
                        amount,
                    }) => {
                        send_take_item_entity(
                            writer,
                            compression,
                            item_entity_id,
                            player_entity_id,
                            amount,
                        ).await?;
                    }
                    Some(OutboundCommand::PickupCandidates(candidates)) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && game_mode != GameMode::Spectator
                            && !survival_state.is_dead()
                        {
                            pickup_candidate_entities(
                                state,
                                writer,
                                &mut xp_state,
                                candidates,
                                script_gameplay_events.as_ref(),
                                player_pose,
                                game_mode,
                            )
                            .await?;
                        }
                    }
                    Some(OutboundCommand::DespawnEntity(entity)) => {
                        send_entity_despawn(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::AnimatePlayer { entity_id }) => {
                        send_player_animation(writer, compression, entity_id).await?;
                    }
                    Some(OutboundCommand::PlayerEntityData { entity_id, values }) => {
                        write_packet(
                            writer,
                            &ClientboundSetEntityData { entity_id, values },
                            compression,
                        )
                        .await?;
                    }
                    Some(OutboundCommand::BlockEntityData { position, block_entity_type, nbt }) => {
                        write_packet(
                            writer,
                            &ClientboundBlockEntityData {
                                position: pack_block_pos(position.x, position.y, position.z),
                                block_entity_type,
                                nbt,
                            },
                            compression,
                        )
                        .await?;
                    }
                    Some(OutboundCommand::FurnaceSlots {
                        position,
                        state_id,
                        slots,
                    }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(window) = active_furnace_window_at(
                                &mut state.active_container,
                                position,
                            )
                        {
                            write_container_slots(
                                writer,
                                compression,
                                window.container_id,
                                state_id,
                                slots.iter().cloned(),
                            )
                            .await?;
                            window.state_id = state_id;
                        }
                    }
                    Some(OutboundCommand::FurnaceData { position, changed }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(ActiveContainer::Furnace(window)) = state.active_container.as_ref()
                            && window.position == position
                        {
                            write_furnace_data_changes(writer, compression, window, &changed).await?;
                        }
                    }
                    Some(OutboundCommand::ChestSlots {
                        position,
                        state_id,
                        slots,
                    }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(window) = active_chest_window_at(
                                &mut state.active_container,
                                position,
                            )
                        {
                            write_container_slots(
                                writer,
                                compression,
                                window.container_id,
                                state_id,
                                slots.iter().cloned(),
                            )
                            .await?;
                            window.state_id = state_id;
                        }
                    }
                    Some(OutboundCommand::CustomPayload { channel, payload }) => {
                        write_packet(
                            writer,
                            &ClientboundCustomPayload {
                                payload: CustomPayload::Unknown { channel, payload },
                            },
                            compression,
                        )
                        .await?;
                    }
                    Some(OutboundCommand::SystemChat { message }) => {
                        write_packet(
                            writer,
                            &ClientboundSystemChat {
                                content_nbt: text_component_nbt(&message)?,
                                overlay: false,
                            },
                            compression,
                        )
                        .await?;
                    }
                    Some(OutboundCommand::WorldTime { world_time }) => {
                        write_packet(writer, &clientbound_world_time(world_time), compression)
                            .await?;
                    }
                    Some(OutboundCommand::WakeFromBed { bed }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(previous) = wake_player_from_bed(
                                state,
                                writer,
                                compression,
                                &simulation,
                                bed,
                                &mut player_pose,
                                &mut next_teleport_id,
                                &mut pending_teleport,
                            )
                            .await?
                        {
                            game_mode = previous;
                            write_packet(
                                writer,
                                &GameEvent {
                                    event: GameEvent::EVENT_CHANGE_GAME_MODE,
                                    value: previous.id() as f32,
                                },
                                compression,
                            )
                            .await?;
                            write_packet(
                                writer,
                                &player_abilities_for_mode(previous),
                                compression,
                            )
                            .await?;
                        }
                    }
                    Some(OutboundCommand::DisconnectPlayer { reason }) => {
                        write_packet(
                            writer,
                            &PlayDisconnect {
                                reason_nbt: text_component_nbt(&reason)?,
                            },
                            compression,
                        )
                        .await?;
                        close_session = true;
                    }
                    Some(OutboundCommand::OpenScriptMenu(request)) => {
                        if game_mode != GameMode::Spectator
                            && !survival_state.is_dead()
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            open_script_menu(state, writer, player_pose, request).await?;
                        }
                    }
                    Some(OutboundCommand::CloseScriptMenu(request)) => {
                        if let Some(state) = interaction.as_deref_mut() {
                            close_script_menu(state, writer, request).await?;
                        }
                    }
                    Some(OutboundCommand::ScriptPlayerTeleport(command)) => {
                        apply_script_player_teleport(
                            command,
                            writer,
                            compression,
                            &mut interaction,
                            &mut chunk_stream,
                            &simulation,
                            &mut script_zone_observer,
                            &sessions,
                            &mut player_pose,
                            &mut next_teleport_id,
                            &mut pending_teleport,
                        )
                        .await?;
                    }
                    Some(OutboundCommand::ScriptPlayerInventoryTransaction(command)) => {
                        let result = match command.begin_commit() {
                            Some(_transaction_guard) => match interaction.as_deref_mut() {
                                Some(state) => commit_session_owner_script_player_inventory(
                                    state,
                                    command.transaction(),
                                ),
                                None => Err(
                                    mc_script::ScriptPlayerInventoryFailure::RuntimeUnavailable,
                                ),
                            },
                            None => {
                                Err(mc_script::ScriptPlayerInventoryFailure::PlayerUnavailable)
                            }
                        };
                        let committed = result.is_ok();
                        command.complete(result);
                        if committed
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            write_inventory_content(state, writer).await?;
                        }
                    }
                    Some(OutboundCommand::LoaderItemGrant(command)) => {
                        let result = match command.begin_commit() {
                            Some(_transaction_guard) => match interaction.as_deref_mut() {
                                Some(state) => {
                                    commit_session_owner_loader_item_grant(state, command.stack())
                                }
                                None => Err(
                                    mc_script::ScriptPlayerInventoryFailure::RuntimeUnavailable,
                                ),
                            },
                            None => {
                                Err(mc_script::ScriptPlayerInventoryFailure::PlayerUnavailable)
                            }
                        };
                        let committed = result.is_ok();
                        command.complete(result);
                        if committed
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            write_inventory_content(state, writer).await?;
                        }
                    }
                    Some(OutboundCommand::AuthoritativeInventory {
                        inventory,
                        carried_item,
                    }) => {
                        if let Some(state) = interaction.as_deref_mut() {
                            state.inventory = *inventory;
                            state.carried_item = carried_item;
                            write_inventory_content(state, writer).await?;
                        }
                    }
                    Some(OutboundCommand::Explosion(mut packet)) => {
                        if game_mode != GameMode::Survival {
                            packet.knockback = None;
                        }
                        write_packet(writer, &packet, compression).await?;
                    }
                    None => close_session = true,
                    }
                    Ok(())
                }, blocked, SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT).await?;
                if outcome == OutboundWriteOutcome::TimedOut {
                    sessions.record_slow_client_write_timeout();
                    warn!(
                        session_id,
                        timeout_ms = SLOW_CLIENT_OUTBOUND_WRITE_TIMEOUT.as_millis() as u64,
                        "slow client outbound write timed out; closing play session"
                    );
                    return Ok(());
                }
                if close_session {
                    return Ok(());
                }
            }
            _ = ticker.tick() => {
                if let Some(elapsed) = keepalive.timed_out(KEEPALIVE_TIMEOUT) {
                    warn!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        "client missed keepalive deadline; closing"
                    );
                    return Ok(());
                }
                if let Some(keepalive_id) = keepalive.record_request() {
                    write_packet(
                        writer,
                        &ClientboundKeepAlive { id: keepalive_id },
                        compression,
                    )
                    .await?;
                }
            }
            changed = simulation_ticks.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let current_tick = *simulation_ticks.borrow_and_update();
                if client_load.tick() {
                    let completed_respawn_load = sessions.mark_client_loaded(session_id);
                    debug!(completed_respawn_load, "client load timeout elapsed");
                }
                if resend_pending_teleport_if_due(
                    writer,
                    compression,
                    &mut pending_teleport,
                    &mut next_teleport_id,
                    player_pose,
                    current_tick,
                )
                .await?
                {
                    debug!(
                        teleport_id = pending_teleport.map(|pending| pending.id),
                        "unconfirmed teleport synchronization resent"
                    );
                }
                let (next_breathing, breathing_tick) = breathing_state.tick(
                    player_pose.eye_in_water,
                    player_can_drown(game_mode, survival_state.is_dead()),
                );
                let client_has_loaded = client_load.has_loaded();
                let breathing_requires_damage_commit =
                    client_has_loaded && breathing_tick.drowning_damage > 0.0;
                if !breathing_requires_damage_commit {
                    breathing_state = next_breathing;
                    if breathing_tick.air_changed {
                        publish_player_air_supply(&sessions, session_id, breathing_state);
                    }
                }

                let mut breathing_damage_committed = false;
                if matches!(game_mode, GameMode::Survival | GameMode::Adventure) {
                    let mut updated_survival = survival_state;
                    let health_tick = if game_mode == GameMode::Survival {
                        updated_survival.tick_health(&mut food_tick_timer)
                    } else {
                        food_tick_timer = 0;
                        SurvivalHealthTick::Unchanged
                    };
                    if client_has_loaded
                        && let SurvivalHealthTick::StarvationDamage(amount) = health_tick
                    {
                        updated_survival.apply_damage(survival_damage_after_equipment(
                            interaction.as_deref(),
                            amount,
                            PlayerDamageKind::Starvation,
                        ));
                    }
                    if client_has_loaded && breathing_tick.drowning_damage > 0.0 {
                        updated_survival.apply_damage(survival_damage_after_equipment(
                            interaction.as_deref(),
                            breathing_tick.drowning_damage,
                            PlayerDamageKind::Drowning,
                        ));
                    }
                    let health_changed = match health_tick {
                        SurvivalHealthTick::Unchanged => false,
                        SurvivalHealthTick::StarvationDamage(_) => client_has_loaded,
                        _ => true,
                    };
                    if health_changed || breathing_requires_damage_commit
                    {
                        if let Some(state) = interaction.as_deref_mut() {
                            let expected_inventory = state.inventory.clone();
                            let updated_xp = xp_state.clone();
                            breathing_damage_committed = commit_player_survival_update(
                                state,
                                writer,
                                &mut survival_state,
                                &mut xp_state,
                                expected_inventory,
                                updated_survival,
                                updated_xp,
                                None,
                                true,
                                player_pose,
                            )
                            .await?;
                        } else {
                            survival_state = updated_survival;
                            write_packet(writer, &survival_state.as_packet(), compression).await?;
                            breathing_damage_committed = true;
                        }
                    }
                    if client_has_loaded
                        && game_mode == GameMode::Survival
                        && current_tick.is_multiple_of(20)
                    {
                        apply_contact_block_damage(
                            interaction.as_deref_mut(),
                            writer,
                            compression,
                            &mut survival_state,
                            &mut xp_state,
                            game_mode,
                            player_pose,
                        )
                        .await?;
                    }
                } else {
                    food_tick_timer = 0;
                }
                if breathing_requires_damage_commit && breathing_damage_committed {
                    breathing_state = next_breathing;
                    publish_player_air_supply(&sessions, session_id, breathing_state);
                }
                if let Some(state) = interaction.as_deref_mut() {
                    tick_delayed_break(
                        state,
                        writer,
                        script_gameplay_events.as_ref(),
                        game_mode,
                        &mut survival_state,
                        &mut xp_state,
                        player_pose,
                        current_tick,
                    )
                    .await?;
                    tick_pending_use(
                        state,
                        writer,
                        game_mode,
                        &mut survival_state,
                        current_tick,
                    )
                    .await?;
                }
            }
            _ = world_time_ticker.tick() => {
                send_world_time(writer, compression, &sessions).await?;
            }
            () = config.shutdown.notified() => {
                info!("shutdown requested; closing play session");
                return Ok(());
            }
            result = read_frame(reader, buf, compression) => {
                let frame = result?;
                keepalive.record_inbound_activity();
                if frame.id == ServerboundKeepAlive::ID {
                    let mut body = frame.body;
                    let echo = ServerboundKeepAlive::decode(&mut body)?;
                    if !keepalive.record_response(echo.id) {
                        warn!(
                            expected = ?keepalive.pending_id(),
                            received = echo.id,
                            "keepalive id mismatch"
                        );
                    }
                } else if frame.id == ConfirmTeleportation::ID {
                    let mut body = frame.body;
                    let confirm = ConfirmTeleportation::decode(&mut body)?;
                    match confirm_pending_teleport(&mut pending_teleport, confirm.teleport_id) {
                        TeleportConfirmResult::Confirmed => {
                            debug!(teleport_id = confirm.teleport_id, "teleport confirmed");
                        }
                        TeleportConfirmResult::Mismatched { expected } => {
                            warn!(
                                expected,
                                received = confirm.teleport_id,
                                "teleport confirmation id mismatch"
                            );
                        }
                        TeleportConfirmResult::Unexpected => {
                            debug!(teleport_id = confirm.teleport_id, "unexpected teleport confirmation ignored");
                        }
                    }
                } else if frame.id == ServerboundMovePlayerPos::ID {
                    if !client_load.has_loaded() {
                        continue;
                    }
                    if guard_pending_teleport_movement(
                        &pending_teleport,
                        "ServerboundMovePlayerPos",
                    ) {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPos::decode(&mut body)?;
                    handle_accepted_absolute_movement(
                        writer,
                        compression,
                        &mut interaction,
                        &mut chunk_stream,
                        &simulation,
                        &mut script_zone_observer,
                        &mut survival_state,
                        &mut xp_state,
                        game_mode,
                        &mut player_pose,
                        AcceptedAbsoluteMovement {
                            x: movement.x,
                            y: movement.y,
                            z: movement.z,
                            yaw_pitch: None,
                            flags: movement.flags,
                        },
                        sessions.simulation_tick(),
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundMovePlayerPosRot::ID {
                    if !client_load.has_loaded() {
                        continue;
                    }
                    if guard_pending_teleport_movement(
                        &pending_teleport,
                        "ServerboundMovePlayerPosRot",
                    ) {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPosRot::decode(&mut body)?;
                    handle_accepted_absolute_movement(
                        writer,
                        compression,
                        &mut interaction,
                        &mut chunk_stream,
                        &simulation,
                        &mut script_zone_observer,
                        &mut survival_state,
                        &mut xp_state,
                        game_mode,
                        &mut player_pose,
                        AcceptedAbsoluteMovement {
                            x: movement.x,
                            y: movement.y,
                            z: movement.z,
                            yaw_pitch: Some((movement.yaw, movement.pitch)),
                            flags: movement.flags,
                        },
                        sessions.simulation_tick(),
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundMovePlayerRot::ID {
                    if !client_load.has_loaded() {
                        continue;
                    }
                    if guard_pending_teleport_movement(
                        &pending_teleport,
                        "ServerboundMovePlayerRot",
                    ) {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerRot::decode(&mut body)?;
                    validate_player_rotation(movement.yaw, movement.pitch)?;
                    player_pose.yaw = movement.yaw;
                    player_pose.pitch = movement.pitch;
                    player_pose.flags = movement.flags;
                    commit_authoritative_player_pose(&simulation, player_pose).await?;
                    let center = player_pose.chunk_pos();
                    replan_after_movement(
                        writer,
                        compression,
                        &mut chunk_stream,
                        interaction.as_deref_mut(),
                        center,
                        center,
                        player_pose.yaw,
                    )
                    .await?;
                } else if frame.id == ServerboundMovePlayerStatusOnly::ID {
                    if !client_load.has_loaded() {
                        continue;
                    }
                    if guard_pending_teleport_movement(
                        &pending_teleport,
                        "ServerboundMovePlayerStatusOnly",
                    ) {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerStatusOnly::decode(&mut body)?;
                    player_pose.flags = movement.flags;
                    commit_authoritative_player_pose(&simulation, player_pose).await?;
                } else if frame.id == ServerboundPlayerAction::ID {
                    let mut body = frame.body;
                    let action = ServerboundPlayerAction::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_player_action(
                            state,
                            writer,
                            script_gameplay_events.as_ref(),
                            game_mode,
                            &mut survival_state,
                            &mut xp_state,
                            player_pose,
                            action,
                        )
                        .await?;
                    } else {
                        debug!(
                            action = ?action.action,
                            sequence = action.sequence,
                            "PlayerAction ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundPlayerCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundPlayerCommand::decode(&mut body)?;
                    match command.action {
                        PlayerCommandAction::StartSprinting => player_pose.sprinting = true,
                        PlayerCommandAction::StopSprinting => player_pose.sprinting = false,
                        PlayerCommandAction::PressShiftKey => player_pose.shifting = true,
                        PlayerCommandAction::ReleaseShiftKey => player_pose.shifting = false,
                        PlayerCommandAction::StopSleeping => {
                            if let Some(bed) = sessions.request_sleep_wake(session_id)
                                && let Some(state) = interaction.as_deref_mut()
                                && let Some(previous) = wake_player_from_bed(
                                        state,
                                        writer,
                                        compression,
                                        &simulation,
                                        bed,
                                        &mut player_pose,
                                        &mut next_teleport_id,
                                        &mut pending_teleport,
                                    )
                                    .await?
                            {
                                game_mode = previous;
                                write_packet(
                                    writer,
                                    &GameEvent {
                                        event: GameEvent::EVENT_CHANGE_GAME_MODE,
                                        value: previous.id() as f32,
                                    },
                                    compression,
                                )
                                .await?;
                                write_packet(
                                    writer,
                                    &player_abilities_for_mode(previous),
                                    compression,
                                )
                                .await?;
                            }
                        }
                        _ => {}
                    }
                    refresh_player_water_state(interaction.as_deref(), &mut player_pose).await;
                    commit_authoritative_player_pose(&simulation, player_pose).await?;
                } else if frame.id == ServerboundPlayerInput::ID {
                    let mut body = frame.body;
                    let input = ServerboundPlayerInput::decode(&mut body)?.input;
                    player_pose.input = input;
                    player_pose.sprinting = input.sprint;
                    player_pose.shifting = input.shift;
                    refresh_player_water_state(interaction.as_deref(), &mut player_pose).await;
                    commit_authoritative_player_pose(&simulation, player_pose).await?;
                } else if frame.id == ServerboundUseItemOn::ID {
                    let mut body = frame.body;
                    let use_on = ServerboundUseItemOn::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item_on(
                            state,
                            writer,
                            script_gameplay_events.as_ref(),
                            game_mode,
                            survival_state,
                            &xp_state,
                            player_pose,
                            &mut respawn_pose,
                            use_on,
                        )
                        .await?;
                    } else {
                        debug!(
                            sequence = use_on.sequence,
                            "UseItemOn ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundUseItem::ID {
                    let mut body = frame.body;
                    let use_item = ServerboundUseItem::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item(state, writer, game_mode, &mut survival_state, use_item)
                            .await?;
                    } else {
                        debug!(
                            sequence = use_item.sequence,
                            "UseItem ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundSignUpdate::ID {
                    let mut body = frame.body;
                    let sign_update = ServerboundSignUpdate::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_sign_update(state, writer, sign_update).await?;
                    } else {
                        debug!("SignUpdate ignored — no world configured");
                    }
                } else if frame.id == ServerboundAttack::ID {
                    let mut body = frame.body;
                    let attack = ServerboundAttack::decode(&mut body)?;
                    if !client_load.has_loaded() {
                        debug!(entity_id = attack.entity_id, "Attack ignored while client is loading");
                    } else if let Some(state) = interaction.as_deref_mut() {
                        handle_attack(
                            state,
                            writer,
                            game_mode,
                            &mut survival_state,
                            &mut xp_state,
                            player_pose,
                            attack,
                        )
                            .await?;
                    } else {
                        debug!(
                            entity_id = attack.entity_id,
                            "Attack ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundInteract::ID {
                    let mut body = frame.body;
                    let interact = ServerboundInteract::decode(&mut body)?;
                    if !client_load.has_loaded() {
                        debug!(entity_id = interact.entity_id, "Interact ignored while client is loading");
                    } else if let Some(state) = interaction.as_deref_mut() {
                        handle_interact(state, writer, script_gameplay_events.as_ref(), interact)
                            .await?;
                    } else {
                        debug!(
                            entity_id = interact.entity_id,
                            "Interact ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundSwing::ID {
                    let mut body = frame.body;
                    let _ = ServerboundSwing::decode(&mut body)?;
                } else if frame.id == ServerboundPlaceRecipe::ID {
                    let mut body = frame.body;
                    let recipe = ServerboundPlaceRecipe::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_place_recipe(
                            state,
                            writer,
                            script_gameplay_events.as_ref(),
                            player_pose,
                            game_mode,
                            survival_state,
                            recipe,
                        )
                        .await?;
                    } else {
                        debug!(
                            recipe = recipe.recipe_display_id,
                            "PlaceRecipe ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundSelectTrade::ID {
                    let mut body = frame.body;
                    let selection = ServerboundSelectTrade::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_select_trade(state, writer, selection).await?;
                    } else {
                        debug!(offer_index = selection.offer_index, "SelectTrade ignored - no world configured");
                    }
                } else if frame.id == ServerboundContainerButtonClick::ID {
                    let mut body = frame.body;
                    let click = ServerboundContainerButtonClick::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_container_button_click(
                            state,
                            writer,
                            game_mode,
                            &mut survival_state,
                            &mut xp_state,
                            player_pose,
                            click,
                        )
                        .await?;
                    } else {
                        debug!(
                            container_id = click.container_id,
                            button_id = click.button_id,
                            "ContainerButtonClick ignored - no world configured"
                        );
                    }
                } else if frame.id == ServerboundContainerClick::ID {
                    let mut body = frame.body;
                    let click = ServerboundContainerClick::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_container_click(
                            state,
                            writer,
                            ContainerClickContext {
                                game_mode,
                                survival_state,
                                xp_state: &xp_state,
                                player_pose,
                                script_events: script_gameplay_events.as_ref(),
                                scripts: scripts.as_ref(),
                                script_player_id: ScriptPlayerId::new(session_id),
                                script_context: script_player_context_from_values(
                                    &player_uuid,
                                    &player_name,
                                    permissions,
                                    player_pose,
                                ),
                            },
                            click,
                        )
                        .await?;
                    } else {
                        debug!(
                            container_id = click.container_id,
                            slot = click.slot_num,
                            "ContainerClick ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundContainerClose::ID {
                    let mut body = frame.body;
                    let close = ServerboundContainerClose::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        let script_close = state.active_container.as_ref().and_then(|active| {
                            let ActiveContainer::Script(window) = active else {
                                return None;
                            };
                            Some(client_close_matches(window.container_id, close.container_id))
                        });
                        let should_store = state
                            .active_container
                            .as_ref()
                            .is_some_and(|active| active.container_id() == close.container_id);
                        if script_close == Some(true) || (script_close.is_none() && should_store) {
                            store_active_container(state, player_pose).await?;
                        } else if script_close == Some(false) {
                            let Some(ActiveContainer::Script(window)) =
                                state.active_container.take()
                            else {
                                unreachable!(
                                    "script close classification requires a script window"
                                )
                            };
                            write_script_menu_content(state, writer, &window).await?;
                            state.active_container = Some(ActiveContainer::Script(window));
                        } else if close.container_id == 0 {
                            store_inventory_crafting_inputs(state, player_pose).await?;
                        }
                    }
                    debug!(container_id = close.container_id, "container close acknowledged");
                } else if frame.id == ServerboundRecipeBookChangeSettings::ID {
                    let mut body = frame.body;
                    let settings = ServerboundRecipeBookChangeSettings::decode(&mut body)?;
                    debug!(
                        book_type = ?settings.book_type,
                        open = settings.is_open,
                        filtering = settings.is_filtering,
                        "recipe book settings noted"
                    );
                } else if frame.id == ServerboundRecipeBookSeenRecipe::ID {
                    let mut body = frame.body;
                    let seen = ServerboundRecipeBookSeenRecipe::decode(&mut body)?;
                    debug!(recipe = seen.recipe_display_id, "recipe book seen recipe noted");
                } else if frame.id == ServerboundSetCarriedItem::ID {
                    let mut body = frame.body;
                    let pick = ServerboundSetCarriedItem::decode(&mut body)?;
                    if (0..=8).contains(&pick.slot) {
                        let slot = pick.slot as u8;
                        simulation
                            .commit_selected_hotbar_slot(slot)
                            .await
                            .map_err(|error| {
                                warn!(?error, slot, "hotbar selection owner commit failed");
                                ConnectionError::RuntimeUnavailable {
                                    operation: "committing hotbar selection",
                                }
                            })?;
                        if let Some(state) = interaction.as_deref_mut() {
                            state.pending_break = None;
                            state.pending_use = None;
                            clear_shield_use(state);
                            state.selected_hotbar_slot = slot;
                            debug!(slot, "hotbar selection updated");
                        }
                    } else {
                        debug!(slot = pick.slot, "invalid hotbar selection ignored");
                    }
                } else if frame.id == ServerboundClientCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundClientCommand::decode(&mut body)?;
                    let was_dead = survival_state.is_dead();
                    handle_client_command(
                        writer,
                        compression,
                        interaction.as_deref_mut(),
                        &mut chunk_stream,
                        &mut player_pose,
                        respawn_pose,
                        &mut survival_state,
                        &mut xp_state,
                        &respawn,
                        &mut next_teleport_id,
                        &mut pending_teleport,
                        sessions.simulation_tick(),
                        command,
                    )
                    .await?;
                    if was_dead && !survival_state.is_dead() {
                        client_load.restart_after_respawn();
                        if breathing_state.reset() {
                            publish_player_air_supply(&sessions, session_id, breathing_state);
                        }
                    }
                    commit_authoritative_player_pose(&simulation, player_pose).await?;
                } else if frame.id == ServerboundClientInformation::ID {
                    let mut body = frame.body;
                    let information = ServerboundClientInformation::decode(&mut body)?.information;
                    let preferences = ClientPreferences::from_packet(
                        information,
                        server_view_distance,
                        client_brand.clone(),
                    );
                    debug!(
                        language = %preferences.language,
                        requested_view_distance = preferences.requested_view_distance,
                        clamped_view_distance = preferences.clamped_view_distance,
                        chat_visibility = ?preferences.chat_visibility,
                        chat_colors = preferences.chat_colors,
                        model_customisation = preferences.model_customisation,
                        main_hand = ?preferences.main_hand,
                        text_filtering_enabled = preferences.text_filtering_enabled,
                        allows_listing = preferences.allows_listing,
                        particle_status = ?preferences.particle_status,
                        brand = ?preferences.brand,
                        "client information updated"
                    );
                    if preferences.clamped_view_distance != effective_client_view_distance {
                        effective_client_view_distance = preferences.clamped_view_distance;
                        if let Some(stream) = chunk_stream.as_mut() {
                            let unloads = stream.replan_view_distance(
                                effective_client_view_distance,
                                player_pose.yaw,
                            );
                            for (chunk_x, chunk_z) in unloads {
                                write_packet(writer, &ForgetLevelChunk { chunk_x, chunk_z }, compression)
                                    .await?;
                            }
                        }
                    }
                    client_preferences = Some(preferences);
                } else if frame.id == ServerboundCustomPayload::ID {
                    match classify_play_custom_payload(frame.body)? {
                        PlayCustomPayloadAction::Brand(brand) => {
                            debug!(brand = %brand, "client brand noted");
                            if let Some(preferences) = client_preferences.as_mut() {
                                preferences.brand = Some(brand.clone());
                            }
                            let brand_for_event = brand.clone();
                            client_brand = Some(brand);
                            if let Some(extension) = extension.as_ref() {
                                extension.enqueue_event(InboundEvent::ClientBrand {
                                    player_id: extension_player_id,
                                    brand: brand_for_event,
                                });
                            }
                        }
                        PlayCustomPayloadAction::LoaderInteraction(payload) => {
                            if let Err(error) = session::route_client_loader_interaction(
                                scripts.as_ref(),
                                session_id,
                                loader_eligible,
                                config.loader_manifest.as_deref(),
                                payload.as_ref(),
                            )
                            .await
                            {
                                debug!(
                                    ?error,
                                    player_id = session_id,
                                    "Loader interaction rejected"
                                );
                            }
                        }
                        PlayCustomPayloadAction::Unknown { channel, payload } => {
                            if let Some(extension) = extension.as_ref() {
                                extension.enqueue_custom_payload(
                                    extension_player_id,
                                    ProtocolPhase::Play,
                                    &channel,
                                    payload.as_ref(),
                                );
                            } else {
                                debug!(
                                    channel = %channel,
                                    len = payload.len(),
                                    "custom payload ignored"
                                );
                            }
                        }
                        PlayCustomPayloadAction::Oversized { len } => {
                            warn!(
                                len,
                                max = DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES,
                                "oversized custom payload rejected before decode"
                            );
                        }
                    }
                } else if frame.id == ServerboundResourcePack::ID {
                    let mut body = frame.body;
                    let status = ServerboundResourcePack::decode(&mut body)?.status;
                    debug!(
                        id = %status.id,
                        action = ?status.action,
                        terminal = status.action.is_terminal(),
                        "resource-pack status noted"
                    );
                } else if frame.id == ServerboundChatAck::ID {
                    let mut body = frame.body;
                    let ack = ServerboundChatAck::decode(&mut body)?;
                    debug!(offset = ack.offset, "chat acknowledgement ignored");
                } else if frame.id == ServerboundChunkBatchReceived::ID {
                    let mut body = frame.body;
                    let packet = ServerboundChunkBatchReceived::decode(&mut body)?;
                    debug!(
                        desired_chunks_per_tick = packet.desired_chunks_per_tick,
                        "client chunk-batch preference noted"
                    );
                } else if frame.id == ServerboundClientTickEnd::ID {
                    let mut body = frame.body;
                    let _ = ServerboundClientTickEnd::decode(&mut body)?;
                } else if frame.id == ServerboundPlayerLoaded::ID {
                    let mut body = frame.body;
                    let _ = ServerboundPlayerLoaded::decode(&mut body)?;
                    client_load.acknowledge();
                    let completed_respawn_load = sessions.mark_client_loaded(session_id);
                    debug!(completed_respawn_load, "client reported player loaded");
                } else if frame.id == ServerboundCommandSuggestion::ID {
                    let mut body = frame.body;
                    let request = ServerboundCommandSuggestion::decode(&mut body)?;
                    let plugin_command_roots = scripts
                        .as_ref()
                        .map_or_else(Vec::new, ScriptEventSink::player_command_roots);
                    let operator_plugin_command_roots = scripts
                        .as_ref()
                        .map_or_else(Vec::new, ScriptEventSink::operator_command_roots);
                    let suggestions = command_suggestions_with_plugin_roots(
                        &request.command,
                        permissions,
                        &plugin_command_roots,
                        &operator_plugin_command_roots,
                    );
                    debug!(
                        request_id = request.id,
                        command = %request.command,
                        count = suggestions.suggestions.len(),
                        "command suggestions requested"
                    );
                    write_packet(
                        writer,
                        &ClientboundCommandSuggestions {
                            id: request.id,
                            start: suggestions.start,
                            length: suggestions.length,
                            suggestions: suggestions
                                .suggestions
                                .into_iter()
                                .map(|text| mc_protocol::packets::play::CommandSuggestionEntry {
                                    text,
                                    tooltip_nbt: None,
                                })
                                .collect(),
                        },
                        compression,
                    )
                    .await?;
                } else if frame.id == ServerboundChat::ID {
                    let mut body = frame.body;
                    let chat = ServerboundChat::decode(&mut body)?;
                    if let Some(scripts) = scripts.as_ref() {
                        scripts.enqueue_event(ScriptEvent::player_chat_with_context(
                            ScriptPlayerId::new(session_id),
                            chat.message.clone(),
                            script_player_context_from_values(
                                &player_uuid,
                                &player_name,
                                permissions,
                                player_pose,
                            ),
                        ));
                    }
                    dispatch_visibility_commands(sessions.broadcast_system_chat(format!(
                        "<{}> {}",
                        player_name, chat.message
                    )));
                } else if frame.id == ServerboundChatCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundChatCommand::decode(&mut body)?;
                    if let Some(scripts) = scripts.as_ref() {
                        match scripts.enqueue_player_command_with_context(
                            session_id,
                            script_player_context_from_values(
                                &player_uuid,
                                &player_name,
                                permissions,
                                player_pose,
                            ),
                            &command.command,
                        ) {
                            mc_script::PlayerCommandAdmission::Enqueued => {
                                debug!(command = %command.command, "player command routed to Lua plugin");
                                continue;
                            }
                            mc_script::PlayerCommandAdmission::Dropped => {
                                debug!(
                                    command = %command.command,
                                    "player command dropped because the Lua event queue is full"
                                );
                                continue;
                            }
                            mc_script::PlayerCommandAdmission::PermissionDenied => {
                                send_command_feedback(
                                    writer,
                                    compression,
                                    command_error_message(CommandError::PermissionDenied),
                                )
                                .await?;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    execute_player_command(
                        writer,
                        compression,
                        &command.command,
                        permissions,
                        &mut game_mode,
                        &mut survival_state,
                        &mut xp_state,
                        config,
                        &sessions,
                        &simulation,
                        interaction.as_deref_mut(),
                        &mut player_pose,
                        runtime_control.as_ref(),
                        &chunk_pipeline_resources,
                        &mut chunk_stream,
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundChangeGameMode::ID {
                    let mut body = frame.body;
                    let command = ServerboundChangeGameMode::decode(&mut body)?;
                    prepare_game_mode_transition(
                        interaction.as_deref_mut(),
                        game_mode,
                        command.mode,
                        permissions,
                    );
                    apply_game_mode(
                        writer,
                        compression,
                        &simulation,
                        &mut game_mode,
                        command.mode,
                        permissions,
                    )
                    .await?;
                } else {
                    debug!(
                        id = format!("{:#04x}", frame.id),
                        "play packet ignored"
                    );
                }
            }
            () = async {
                if let Some(notify) = chunk_progress_notify {
                    wait_for_chunk_stream_wake(
                        notify,
                        chunk_progress_sessions.as_ref(),
                        chunk_prepared_generation,
                        memory_pressure_changes.as_mut(),
                    )
                    .await;
                }
            }, if chunk_stream_waiting => {
                chunk_stream_needs_step = true;
            }
        }
    }
}

fn text_component_nbt(text: &str) -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_string(), Tag::String(text.to_string()))]),
    )?;
    Ok(out)
}

fn session_admission_message(error: &SessionAdmissionError) -> &'static str {
    match error {
        SessionAdmissionError::ServerFull { .. } => "Server is full",
        SessionAdmissionError::DuplicateProfile { .. } => "This player is already connected",
    }
}
#[cfg(test)]
mod campfire_output_recovery_tests {
    use super::*;

    struct CampfireRuntime {
        config: Arc<ServerConfig>,
        sessions: Arc<SessionRegistry>,
        simulation: simulation::SimulationHandle,
        owner: simulation::SimulationOwner,
    }

    fn campfire_test_blocks() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:campfire").unwrap(),
                    properties: BTreeMap::from([(
                        "lit".to_string(),
                        vec!["true".to_string(), "false".to_string()],
                    )]),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: BTreeMap::from([("lit".to_string(), "true".to_string())]),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    fn campfire_test_config(
        root: &std::path::Path,
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
        entity_types: Arc<EntityTypeRegistry>,
        storage: mc_world::WorldStorage,
    ) -> Arc<ServerConfig> {
        let _ = root;
        Arc::new(ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "campfire recovery test".into(),
            max_players: 1,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: Some(Arc::new(tokio::sync::Mutex::new(storage))),
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items,
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types,
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: RandomTickPolicy::default(),
            command_permissions: crate::server::CommandPermissionConfig::new(
                Vec::<String>::new(),
                true,
            ),
            loader_manifest: None,
            shutdown: crate::server::ShutdownHandle::default(),
        })
    }

    fn create_campfire_runtime(root: &std::path::Path) -> CampfireRuntime {
        std::fs::create_dir_all(root.join("region")).unwrap();
        let blocks = campfire_test_blocks();
        let items = Arc::new(mc_data::items::solaris_required_items());
        let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let input = items
            .id_of(&Identifier::parse("minecraft:porkchop").unwrap())
            .expect("required porkchop");
        let result = items
            .id_of(&Identifier::parse("minecraft:cooked_porkchop").unwrap())
            .expect("required cooked porkchop");
        let mut cooking = CampfireCookingState::default();
        assert!(cooking.insert(ItemStack::new(input, 1), ItemStack::new(result, 1), 1));
        let bytes = campfire_block_entity_persistent_bytes(
            "minecraft:campfire",
            position,
            &items,
            &cooking,
        )
        .unwrap();
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            chunk_position,
            mc_world::BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk
            .set_block(1, 64, 1, mc_world::BlockStateId(1))
            .unwrap();
        chunk.block_entities.insert(position, bytes);
        chunk.mark_dirty();
        let mut storage = mc_world::WorldStorage::open(root, Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        storage
            .insert_generated_chunk(chunk_position, chunk)
            .unwrap();
        assert_eq!(storage.flush_dirty().unwrap(), 1);

        let (entity_journal, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(root).unwrap();
        assert!(entity_pending.is_empty());
        let sessions = Arc::new(SessionRegistry::new_with_entity_owner_journal(
            1,
            Box::new(entity_journal),
        ));
        let (world_journal, world_pending) =
            world_journal::WorldChunkJournal::open(root, Arc::clone(&blocks), Arc::clone(&items))
                .unwrap();
        assert!(world_pending.is_empty());
        sessions.install_world_chunk_journal(world_journal);
        assert!(sessions.restore_campfire_cooking(position, cooking));
        let (simulation, owner) = simulation::simulation_channel();
        let config = campfire_test_config(root, blocks, items, entity_types, storage);
        CampfireRuntime {
            config,
            sessions,
            simulation,
            owner,
        }
    }

    async fn reopen_campfire_runtime(root: &std::path::Path) -> (CampfireRuntime, usize) {
        let blocks = campfire_test_blocks();
        let items = Arc::new(mc_data::items::solaris_required_items());
        let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
        let mut storage = mc_world::WorldStorage::open(root, Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        let (world_journal, world_pending) =
            world_journal::WorldChunkJournal::open(root, Arc::clone(&blocks), Arc::clone(&items))
                .unwrap();
        for chunk in world_journal.decode_pending(&world_pending).unwrap() {
            storage.replay_journal_chunk(chunk).unwrap();
        }
        let (entity_journal, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(root).unwrap();
        let sessions = Arc::new(SessionRegistry::new_with_entity_owner_journal(
            1,
            Box::new(entity_journal),
        ));
        sessions.install_world_chunk_journal(world_journal);
        let (simulation, owner) = simulation::simulation_channel();
        let persisted = persistence::load_persisted_entities(root, &items, &entity_types).unwrap();
        let replayed = persistence::replay_regional_commit_decisions(persisted, &entity_pending)
            .expect("persisted campfire entity journal replays");
        let expected = replayed.records.len();
        assert_eq!(
            owner.restore_persisted_entities(&sessions, replayed),
            expected
        );
        let config = campfire_test_config(root, blocks, items, entity_types, storage);
        hydrate_persisted_campfire_cooking_strict(&config, &sessions)
            .await
            .unwrap();
        let recovered = recover_pending_campfire_outputs(&config, &sessions, &owner)
            .await
            .unwrap();
        (
            CampfireRuntime {
                config,
                sessions,
                simulation,
                owner,
            },
            recovered,
        )
    }

    async fn checkpoint_campfire_runtime(runtime: &mut CampfireRuntime) {
        let shutdown = crate::server::ShutdownHandle::default();
        let mut checkpoint = Box::pin(crate::server::save_periodic_checkpoint(
            &runtime.config,
            &runtime.sessions,
            &runtime.simulation,
            &shutdown,
        ));
        let command_ready = tokio::select! {
            report = &mut checkpoint => {
                panic!("checkpoint completed before its simulation barrier: {report:?}")
            }
            ready = runtime.owner.wait_for_command() => ready,
        };
        assert!(command_ready, "simulation command channel closed");
        assert_eq!(
            runtime
                .owner
                .process_tick_with_world(&runtime.sessions, runtime.config.world.as_ref(), None, 1,)
                .processed,
            1
        );
        let report = checkpoint.await.expect("checkpoint was not superseded");
        assert!(report.is_ok(), "checkpoint errors: {:?}", report.errors);
    }

    fn pending_output_from_world_journal(
        root: &std::path::Path,
        position: mc_world::BlockPos,
    ) -> PendingCampfireOutput {
        let blocks = campfire_test_blocks();
        let items = Arc::new(mc_data::items::solaris_required_items());
        let (journal, pending) =
            world_journal::WorldChunkJournal::open(root, blocks, Arc::clone(&items)).unwrap();
        let chunks = journal.decode_pending(&pending).unwrap();
        let bytes = chunks
            .iter()
            .rev()
            .find_map(|chunk| chunk.block_entities.get(&position))
            .expect("pending world image contains campfire");
        let cooking = campfire_cooking_state_from_persistent_nbt_strict(
            bytes,
            &[],
            &items,
            &TagsData::default(),
        )
        .unwrap()
        .expect("pending-only campfire is retained");
        assert!(cooking.slots.iter().all(Option::is_none));
        assert_eq!(cooking.pending_outputs.len(), 1);
        cooking.pending_outputs[0].clone()
    }

    async fn assert_one_output_and_no_intent(
        runtime: &CampfireRuntime,
        expected: &PendingCampfireOutput,
    ) {
        let records = runtime.sessions.persisted_entity_save_snapshot().0.records;
        let matching = records
            .iter()
            .filter(|record| record.snapshot.uuid == expected.uuid)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        assert_eq!(
            matching[0].snapshot.item_stack.as_ref(),
            Some(&expected.stack)
        );

        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut world = runtime.config.world.as_ref().unwrap().lock().await;
        let chunk = match world.cached_chunk_snapshot(chunk_position) {
            Some(chunk) => chunk,
            None => world
                .get_chunk_without_generation(chunk_position)
                .unwrap()
                .expect("persisted campfire chunk"),
        };
        let bytes = chunk.block_entities.get(&position).unwrap().clone();
        drop(world);
        let mut cursor = std::io::Cursor::new(bytes);
        let tag = mc_nbt::read_network(&mut cursor).unwrap();
        let Some(Tag::List(items)) = compound_field(&tag, "Items") else {
            panic!("campfire Items list missing");
        };
        assert!(items.elements.is_empty(), "completed input resurrected");
        assert!(
            pending_campfire_outputs_from_nbt(&tag, &runtime.config.items)
                .unwrap()
                .is_empty()
        );
    }

    async fn abort_runtime_at_gate(
        runtime: CampfireRuntime,
        gate: tokio::sync::oneshot::Receiver<()>,
    ) {
        let task = tokio::spawn({
            let config = Arc::clone(&runtime.config);
            let sessions = Arc::clone(&runtime.sessions);
            async move {
                runtime
                    .owner
                    .run_campfire_cooking_ticks(&config, &sessions, None, None)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(3), gate)
            .await
            .expect("campfire crash gate was not reached")
            .expect("campfire crash gate sender dropped");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn pending_campfire_output_round_trips_full_stack_and_identity() {
        let items = mc_data::items::solaris_required_items();
        let cooked_porkchop = items
            .id_of(&Identifier::parse("minecraft:cooked_porkchop").unwrap())
            .expect("required cooked porkchop");
        let sharpness = Identifier::parse("minecraft:sharpness").unwrap();
        let position = mc_world::BlockPos { x: -7, y: 64, z: 9 };
        let output = PendingCampfireOutput {
            uuid: campfire_output_uuid(41, position, 2),
            stack: EntityItemStack::new(cooked_porkchop, 2)
                .with_damage(5)
                .with_enchantment(sharpness, 3),
        };
        let cooking = CampfireCookingState {
            pending_outputs: vec![output.clone()],
            ..CampfireCookingState::default()
        };

        let bytes = campfire_block_entity_persistent_bytes(
            "minecraft:campfire",
            position,
            &items,
            &cooking,
        )
        .expect("encode pending output");
        let decoded = campfire_cooking_state_from_persistent_nbt_strict(
            &bytes,
            &[],
            &items,
            &TagsData::default(),
        )
        .expect("decode pending output")
        .expect("pending-only campfire is retained");

        assert_eq!(decoded.pending_outputs, vec![output]);
    }

    #[test]
    fn pending_campfire_output_uuid_is_stable_and_slot_specific() {
        let position = mc_world::BlockPos { x: 1, y: 70, z: -3 };

        assert_eq!(
            campfire_output_uuid(7, position, 0),
            campfire_output_uuid(7, position, 0)
        );
        assert_ne!(
            campfire_output_uuid(7, position, 0),
            campfire_output_uuid(7, position, 1)
        );
        assert_ne!(
            campfire_output_uuid(7, position, 0),
            campfire_output_uuid(8, position, 0)
        );
    }

    #[tokio::test]
    async fn campfire_completion_persists_intent_before_entity_materialization() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = create_campfire_runtime(tmp.path());
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        runtime
            .sessions
            .install_campfire_d1_probe_for_test(reached_tx, resume_rx);
        let task = tokio::spawn({
            let config = Arc::clone(&runtime.config);
            let sessions = Arc::clone(&runtime.sessions);
            async move {
                runtime
                    .owner
                    .run_campfire_cooking_ticks(&config, &sessions, None, None)
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(3), reached_rx)
            .await
            .expect("D1 gate was not reached")
            .expect("D1 gate sender dropped");
        let output =
            pending_output_from_world_journal(tmp.path(), mc_world::BlockPos { x: 1, y: 64, z: 1 });
        let (_, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(entity_pending.is_empty());
        assert_eq!(output.stack.count, 1);

        resume_tx.send(()).unwrap();
        let report = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("campfire tick did not finish after D1 release")
            .unwrap();
        assert_eq!(report.dropped, 1);
    }

    #[tokio::test]
    async fn restart_materializes_pending_campfire_output_once() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = create_campfire_runtime(tmp.path());
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (_resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        runtime
            .sessions
            .install_campfire_d1_probe_for_test(reached_tx, resume_rx);
        abort_runtime_at_gate(runtime, reached_rx).await;
        let expected =
            pending_output_from_world_journal(tmp.path(), mc_world::BlockPos { x: 1, y: 64, z: 1 });

        let (first_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
        assert_eq!(recovered, 1);
        assert_one_output_and_no_intent(&first_restart, &expected).await;
        drop(first_restart);

        let (second_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
        assert_eq!(recovered, 0);
        assert_one_output_and_no_intent(&second_restart, &expected).await;
    }

    #[tokio::test]
    async fn restart_after_entity_commit_before_world_ack_deduplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = create_campfire_runtime(tmp.path());
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (_resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        runtime
            .sessions
            .install_campfire_entity_probe_for_test(reached_tx, resume_rx);
        abort_runtime_at_gate(runtime, reached_rx).await;
        let expected =
            pending_output_from_world_journal(tmp.path(), mc_world::BlockPos { x: 1, y: 64, z: 1 });
        let (_, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        let replayed = persistence::replay_regional_commit_decisions(
            PersistedEntityCheckpoint::new(0, Vec::<PersistedEntityRecord>::new()),
            &entity_pending,
        )
        .expect("committed campfire entity journal replays");
        assert_eq!(replayed.records.len(), 1);
        assert_eq!(replayed.records[0].snapshot.uuid, expected.uuid);

        let (first_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
        assert_eq!(recovered, 1);
        assert_one_output_and_no_intent(&first_restart, &expected).await;
        drop(first_restart);

        let (second_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
        assert_eq!(recovered, 0);
        assert_one_output_and_no_intent(&second_restart, &expected).await;
    }

    #[tokio::test]
    async fn successful_d2_checkpoint_does_not_resurrect_campfire_output() {
        let tmp = tempfile::tempdir().unwrap();
        let mut runtime = create_campfire_runtime(tmp.path());

        let report = runtime
            .owner
            .run_campfire_cooking_ticks(&runtime.config, &runtime.sessions, None, None)
            .await;
        assert_eq!(report.dropped, 1);
        let records = runtime.sessions.persisted_entity_save_snapshot().0.records;
        assert_eq!(records.len(), 1);
        let expected = PendingCampfireOutput {
            uuid: records[0].snapshot.uuid,
            stack: records[0]
                .snapshot
                .item_stack
                .clone()
                .expect("campfire output item stack"),
        };

        let (_, world_pending) = world_journal::WorldChunkJournal::open(
            tmp.path(),
            Arc::clone(&runtime.config.blocks),
            Arc::clone(&runtime.config.items),
        )
        .unwrap();
        assert_eq!(world_pending.len(), 2, "D1 and D2 must precede checkpoint");
        let (_, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert_eq!(entity_pending.len(), 1, "E must precede checkpoint");

        checkpoint_campfire_runtime(&mut runtime).await;

        let (_, world_pending) = world_journal::WorldChunkJournal::open(
            tmp.path(),
            Arc::clone(&runtime.config.blocks),
            Arc::clone(&runtime.config.items),
        )
        .unwrap();
        assert!(world_pending.is_empty());
        let (_, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert_eq!(
            entity_pending.len(),
            1,
            "entity checkpoint cleanup stays memory-only"
        );
        let checkpoint = persistence::load_persisted_entities(
            tmp.path(),
            runtime.config.items.as_ref(),
            runtime.config.entity_types.as_ref(),
        )
        .unwrap();
        let replayed = persistence::replay_regional_commit_decisions(checkpoint, &entity_pending)
            .expect("entity checkpoint watermark filters old campfire output");
        assert_eq!(replayed.records.len(), 1);
        assert_eq!(replayed.records[0].snapshot.uuid, expected.uuid);
        drop(runtime);

        let (_, entity_pending) =
            persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(
            entity_pending.is_empty(),
            "normal shutdown compacts checkpointed entity WAL"
        );

        let (restarted, recovered) = reopen_campfire_runtime(tmp.path()).await;
        assert_eq!(recovered, 0);
        assert_one_output_and_no_intent(&restarted, &expected).await;
    }
}

#[cfg(test)]
mod tests;
