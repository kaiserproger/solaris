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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes, BytesMut};
use mc_data::block_facts::{FluidKind, FluidStateFacts};
use mc_data::block_light::BlockLightTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_data::{Registry, VanillaData};
use mc_entity::{
    AttributeKind, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, EntityStore,
    EntityView, GoalState, PathingBudget, PathingProbe, PathingProbeResult, Rotation, SpawnEntity,
    Vec3,
};
use mc_extension::DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES;
use mc_nbt::{ListTag, Tag};
use mc_protocol::codec::{DEFAULT_MAX_STRING_LEN, Identifier, ReadMc};
use mc_protocol::frame::{Compression, encode_frame};
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockEntityInfo, BlockUpdate, ChunkHeightmap, ClientCommandAction,
    ClientboundBlockEntityData, ClientboundChangeDifficulty, ClientboundCommandSuggestions,
    ClientboundContainerSetContent, ClientboundContainerSetData, ClientboundContainerSetSlot,
    ClientboundInitializeBorder, ClientboundKeepAlive, ClientboundOpenScreen,
    ClientboundOpenSignEditor, ClientboundRespawn, ClientboundSetEntityData,
    ClientboundSetExperience, ClientboundSetHealth, ClientboundSetHeldSlot, ClientboundSetTime,
    ClientboundSystemChat, ClientboundTakeItemEntity, ConfirmTeleportation, ContainerInput,
    Direction, ENTITY_DATA_POSE_INDEX, ENTITY_DATA_SHARED_FLAGS_INDEX, EntityAnimation,
    EntityAnimationAction, EntityDataValue, EntityEvent, EntityPose, EntityPositionSync,
    EntityVec3, ForgetLevelChunk, GameEvent, GameMode, HashedStack, ITEM_ENTITY_DATA_ITEM_INDEX,
    InteractionHand, ItemStack, LIVING_ENTITY_DATA_FLAGS_INDEX, LIVING_ENTITY_FLAG_OFF_HAND,
    LIVING_ENTITY_FLAG_USING_ITEM, LevelChunkWithLight, LightData, LightUpdate, LoginPlay,
    MoveEntityPosRot, MovePlayerFlags, PlayDisconnect, PlayerActionKind, PlayerCommandAction,
    PlayerInfoActions, PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate, PlayerInput,
    PositionMoveRotation, RemoveEntities, RotateHead, SectionBlockChange, SectionBlocksUpdate,
    ServerboundAttack, ServerboundChangeGameMode, ServerboundChatAck, ServerboundChatCommand,
    ServerboundChunkBatchReceived, ServerboundClientCommand, ServerboundClientInformation,
    ServerboundClientTickEnd, ServerboundCommandSuggestion, ServerboundContainerClick,
    ServerboundContainerClose, ServerboundCustomPayload, ServerboundInteract, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerPosRot, ServerboundMovePlayerRot,
    ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe, ServerboundPlayerAction,
    ServerboundPlayerCommand, ServerboundPlayerInput, ServerboundPlayerLoaded,
    ServerboundRecipeBookChangeSettings, ServerboundRecipeBookSeenRecipe, ServerboundResourcePack,
    ServerboundSetCarriedItem, ServerboundSignUpdate, ServerboundSwing, ServerboundUseItem,
    ServerboundUseItemOn, SetCenterChunk, SetDefaultSpawnPosition, SetEntityMotion,
    SynchronizePlayerPosition, pack_section_pos, pack_section_relative_pos, unpack_block_pos,
};
use mc_protocol::packets::{CustomPayload, Packet};
use mc_world::light::{
    ChunkLight, LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::wire::{client_heightmaps, encode_chunk_data, encode_chunk_light};
use mc_world::{
    BlockRegistry, BlockStateId, ChestBlockEntity, Chunk, ChunkPos, FurnaceBlockEntity,
    FurnaceSlot, ScheduledFluidTick,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use crate::chunk_pipeline::ChunkPipelineResources;
use crate::connection::{read_frame, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;
use crate::server::{ServerConfig, WorldHandle};
use crate::{
    ChunkPipelinePolicy, ChunkPipelineStopReason, ChunkPriority, ChunkRequest, ChunkScheduler,
};

mod block_wire;
mod chunk_stream;
pub(crate) mod commands;
mod containers;
mod inventory;
mod item_blocks;
pub(crate) mod persistence;
mod plants;
mod recipes;
mod session;
mod spawn;
mod survival;
mod wire_entities;

pub(crate) use chunk_stream::{
    passive_entity_passable_blocks, passive_herd_fallback_surface_blocks,
};
pub(crate) use session::SessionRegistry;

use block_wire::{
    BlockDelta, broadcast_block_deltas, broadcast_block_deltas_to_sessions,
    broadcast_light_updates, broadcast_light_updates_to_sessions, send_block_deltas,
    send_light_updates,
};
#[cfg(test)]
use block_wire::{BlockDeltaPacket, plan_block_delta_packets};
#[cfg(test)]
use chunk_stream::{ChunkBuildTiming, ChunkWriteTiming, plan_passive_herd, spiral_chunks};
use chunk_stream::{
    ChunkStreamState, ChunkStreamStep, PreparedChunkFrame, desired_chunk_set, herd_uuid,
    passable_block_name,
};
use commands::{
    AdminCommand, CommandError, CommandPermissions, DebugCommand, SurvivalCommand,
    command_suggestions, command_tree_packet, parse_admin_command, player_abilities_for_mode,
};
#[cfg(test)]
use commands::{parse_debug_command, parse_gamemode_command};
use containers::{
    ActiveContainer, ChestView, ChestWindow, CraftingTableWindow, FurnaceKind, FurnaceWindow,
    chest_menu_title_nbt, crafting_menu_title_nbt, find_campfire_recipe_for_item,
    find_smelting_recipe_for_item, furnace_kind_for_state, furnace_menu_title_for_state,
    furnace_menu_title_nbt, is_barrel_state, is_chest_state, is_crafting_table_state, is_fuel_item,
    is_furnace_state, next_container_id, store_active_container,
    unsupported_survival_station_for_state,
};
#[cfg(test)]
use inventory::{ArmorStats, armor_reduced_damage};
use inventory::{
    PlayerInventory, armor_entry_for_item, armor_slot_for_kind, can_stack, damage_equipped_armor,
    item_max_stack, survival_damage_after_armor,
};
use item_blocks::ItemToBlockTable;
use persistence::{
    PersistedEntityRecord, PlayerPersistedState, SpawnState, XpState, load_player_state,
    save_player_state,
};
#[cfg(test)]
use plants::bonemeal_growth_edit;
use plants::{
    bonemeal_growth_edits, next_crop_growth_state, sapling_tree_edits, stem_fruit_edits,
    sweet_berry_harvest, vertical_plant_growth_edit,
};
use recipes::{craft_recipe, ingredient_accepts_item};
use session::{
    OutboundCommand, OutboundLightUpdate, PlayerEntitySnapshot, ServerEntityMove,
    ServerEntitySnapshot, SessionAdmissionError, SessionId, SessionRegistration,
    VisibilityDispatch, dispatch_visibility_commands, entity_aabb, within_block_reach,
    within_entity_reach,
};
#[cfg(test)]
use spawn::spawn_chunk_pos;
#[cfg(test)]
use spawn::spawn_y_from_chunk;
use spawn::{chunk_pos_from_coords, pack_block_pos, spawn_dimension, spawn_position};
use survival::{
    PendingBreak, PendingUse, SurvivalState, UseKind, arrow_entity_type_id, block_break_is_denied,
    block_drop_stacks, bow_draw_power, consume_arrow, damage_held_bow_after_shot,
    damage_held_tool_after_mining, damage_held_weapon_after_attack, entity_item_stack,
    falling_block_entity_type_id, held_attack_damage, held_food_use, held_item_id, is_bow_item,
    is_hostile_entity, item_entity_type_id, max_tool_damage_for_path, mining_time_for_target,
    mob_drop_stack, mob_drop_stack_from, mob_xp_value, pending_break_is_complete,
    pending_break_matches, pending_use_is_complete, pending_use_matches, xp_orb_entity_type_id,
};
#[cfg(test)]
use survival::{
    attack_damage_for_item, block_drop_stacks_from, fallback_mining_time, food_rule_for_item,
    is_durability_tool_path,
};
use wire_entities::{
    send_entity_data, send_entity_despawn, send_entity_relative_move, send_entity_spawn,
    send_player_animation, send_player_despawn, send_player_move, send_player_spawn,
    send_take_item_entity,
};

thread_local! {
    static CHUNK_LIGHT_WORKSPACE: RefCell<LightWorkspace> = RefCell::new(LightWorkspace::new());
}

/// How often we ping the client. Vanilla's value.
pub const KEEPALIVE_PERIOD: Duration = Duration::from_secs(15);
/// How long we wait for the client's echo before disconnecting. Vanilla's value.
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PENDING_TELEPORT_RESYNCS: u8 = 3;
const SHIELD_ACTIVATION_DELAY_TICKS: u64 = 5;
const SHIELD_FRONT_ARC_DOT_MIN: f64 = 0.5;
const SHIELD_FALLBACK_MAX_DAMAGE: i32 = 336;

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
const CAMPFIRE_COOKING_SLOT_COUNT: usize = 4;
const SIGN_BLOCK_ENTITY_TYPE_ID: i32 = 7;
const CAMPFIRE_BLOCK_ENTITY_TYPE_ID: i32 = 33;

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
        Arc::clone(&config.items),
        Arc::clone(&config.loot),
    );
}
const ENTITY_MOVE_SEND_INTERVAL_TICKS: u64 = 1;
const WORLD_TIME_SYNC_PERIOD: Duration = Duration::from_secs(1);
const ENTITY_SIMULATION_DISTANCE_CHUNKS: i32 = 8;
const ENTITY_PHYSICS_QUERY_BUDGET_PER_TICK: usize = 128;
const SURVIVAL_MINING_FALLBACK_TIME: Duration = Duration::from_millis(200);
const CRAFTING_MENU_TYPE_ID: i32 = 12;
const CRAFTING_MENU_SLOT_COUNT: usize = 46;
const CHEST_MENU_TYPE_ID: i32 = 2;
const DOUBLE_CHEST_MENU_TYPE_ID: i32 = 5;
const BLAST_FURNACE_MENU_TYPE_ID: i32 = 10;
const SINGLE_CHEST_STORAGE_SLOTS: usize = 27;
const PLAYER_CONTAINER_STORAGE_SLOTS: usize = 36;
#[derive(Debug, Clone, PartialEq, Eq)]
struct CampfireCookingEntry {
    input: ItemStack,
    result: ItemStack,
    ticks_remaining: u32,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CampfireCookingTick {
    completed: Vec<ItemStack>,
    changed: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CampfireCookingUpdates {
    completed: Vec<(mc_world::BlockPos, ItemStack)>,
    changed: Vec<(mc_world::BlockPos, CampfireCookingState)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CampfireCookingState {
    slots: [Option<CampfireCookingEntry>; CAMPFIRE_COOKING_SLOT_COUNT],
}

impl CampfireCookingState {
    fn insert(&mut self, input: ItemStack, result: ItemStack, cooking_time: u32) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(CampfireCookingEntry {
            input,
            result,
            ticks_remaining: cooking_time.max(1),
        });
        true
    }

    fn tick(&mut self) -> CampfireCookingTick {
        let mut tick = CampfireCookingTick::default();
        for slot in &mut self.slots {
            let Some(entry) = slot.as_mut() else {
                continue;
            };
            entry.ticks_remaining = entry.ticks_remaining.saturating_sub(1);
            if entry.ticks_remaining == 0 {
                let entry = slot.take().expect("entry existed before completion");
                tick.completed.push(entry.result);
                tick.changed = true;
            }
        }
        tick
    }

    fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

struct RegisteredSessionCleanup {
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    active: bool,
}

impl RegisteredSessionCleanup {
    fn new(sessions: Arc<SessionRegistry>, session_id: SessionId) -> Self {
        Self {
            sessions,
            session_id,
            active: true,
        }
    }

    fn unregister(mut self) {
        self.active = false;
        dispatch_visibility_commands(self.sessions.unregister(self.session_id));
    }
}

impl Drop for RegisteredSessionCleanup {
    fn drop(&mut self) {
        if self.active {
            dispatch_visibility_commands(self.sessions.unregister(self.session_id));
        }
    }
}
const FURNACE_MENU_TYPE_ID: i32 = 14;
const SMOKER_MENU_TYPE_ID: i32 = 22;
const FURNACE_CONTAINER_ID_MIN: i32 = 1;
const FURNACE_CONTAINER_ID_MAX: i32 = 100;
const FURNACE_MENU_SLOT_COUNT: usize = 39;
const FURNACE_FUEL_TICKS: i16 = 1600;
const DEFAULT_FURNACE_COOK_TICKS: i16 = 200;
const DEFAULT_FOOD_USE_DURATION: Duration = Duration::from_millis(1_600);
const HOSTILE_MELEE_RANGE: f64 = 1.8;
const HOSTILE_MELEE_VERTICAL_REACH: f64 = 2.25;
const HOSTILE_MELEE_COOLDOWN: Duration = Duration::from_secs(1);
const HOSTILE_FOLLOW_SPEED: f64 = 1.25;
const HOSTILE_WANDER_SPEED: f64 = 1.25;
const PASSIVE_WANDER_SPEED: f64 = 0.8;
const MAX_PASSIVE_SPAWNS_PER_CHUNK: usize = 6;
const MAX_HOSTILE_SPAWNS_PER_CHUNK: usize = 3;
const MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER: f64 = 0.5;
const PLAYER_ENTITY_ATTACK_COOLDOWN: Duration = Duration::from_millis(350);
const ENTITY_HURT_INVULNERABLE_TICKS: u64 = 6;
const ITEM_PICKUP_DELAY_TICKS: u64 = 4;
const ITEM_DESPAWN_AGE_TICKS: u64 = 6_000;
const ITEM_DESPAWN_SWEEP_BUDGET: usize = 256;
const ARROW_DESPAWN_AGE_TICKS: u64 = 1_200;
const ARROW_ENTITY_HIT_DAMAGE: f32 = 4.0;
const ARROW_ENTITY_HIT_KNOCKBACK: f64 = 0.6;
const CHUNK_STREAM_STEPS_PER_TURN: usize = 1;
const DEFAULT_FLUID_TICK_BUDGET: usize = 256;
const WATER_FLOW_DELAY_TICKS: u64 = 5;
const LAVA_FLOW_DELAY_TICKS: u64 = 30;

/// Default chunk radius around the player when no operator override is present.
pub const DEFAULT_VIEW_DISTANCE: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomTickPolicy {
    pub random_tick_speed: u32,
    pub chunk_budget: usize,
    pub fluid_tick_budget: usize,
    pub save_interval_ticks: u64,
    pub seed: u64,
}

impl Default for RandomTickPolicy {
    fn default() -> Self {
        Self {
            random_tick_speed: 3,
            chunk_budget: 64,
            fluid_tick_budget: DEFAULT_FLUID_TICK_BUDGET,
            save_interval_ticks: 20,
            seed: 0,
        }
    }
}

impl RandomTickPolicy {
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            random_tick_speed: self.random_tick_speed,
            chunk_budget: self.chunk_budget.max(1),
            fluid_tick_budget: self.fluid_tick_budget.max(1),
            save_interval_ticks: self.save_interval_ticks.max(1),
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
pub(crate) struct ScheduledFluidTickReport {
    pub(crate) drained: usize,
    pub(crate) applied: usize,
    pub(crate) budget: usize,
    pub(crate) budget_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct HerdSpawn {
    chunk: (i32, i32),
    slot: u8,
    entity_type_id: i32,
    entity_type_name: String,
    position: Vec3,
    hostile: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsQuery {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: mc_physics::Aabb,
    pub on_ground: bool,
    pub kind: EntityPhysicsKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityPhysicsKind {
    Default,
    ArrowProjectile,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsStep {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockEdit {
    pos: mc_world::BlockPos,
    new_state: mc_world::BlockStateId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FallingBlockStart {
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppliedBlockEdit {
    pos: mc_world::BlockPos,
    previous: mc_world::BlockStateId,
    new_state: mc_world::BlockStateId,
}

#[derive(Debug, Default)]
struct BlockEditBatchOutcome {
    applied: Vec<AppliedBlockEdit>,
    deltas: Vec<BlockDelta>,
    edit_chunks: HashSet<(i32, i32)>,
    light_edit_chunks: HashSet<(i32, i32)>,
    previous_light_chunks: HashMap<(i32, i32), ChunkLight>,
    cleared_campfires: Vec<mc_world::BlockPos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RandomTickSample {
    pub chunk: (i32, i32),
    pub pos: mc_world::BlockPos,
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

    fn shared_flags(self) -> i8 {
        let mut flags = 0_u8;
        if self.shifting {
            flags |= 0x02;
        }
        if self.sprinting {
            flags |= 0x08;
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

#[derive(Debug, Clone, Copy)]
struct AcceptedAbsoluteMovement {
    x: f64,
    y: f64,
    z: f64,
    yaw_pitch: Option<(f32, f32)>,
    flags: MovePlayerFlags,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    profile: &LoggedInProfile,
    config: &ServerConfig,
    sessions: Arc<SessionRegistry>,
    chunk_pipeline_resources: ChunkPipelineResources,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
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

    let (spawn_x, spawn_y, spawn_z) = spawn_position(config).await;
    let default_spawn_pose = PlayerPose::new(spawn_x, spawn_y, spawn_z);
    let world_root = if let Some(world) = config.world.as_ref() {
        let storage = world.lock().await;
        storage.world_root().map(std::path::Path::to_path_buf)
    } else {
        None
    };
    let default_player_state = PlayerPersistedState::new_default(default_spawn_pose);
    let player_state = if let Some(root) = world_root.as_deref() {
        match load_player_state(
            root,
            profile.uuid,
            &config.items,
            default_player_state.clone(),
        ) {
            Ok(Some(state)) => {
                info!(player = %profile.name, state = %state, "loaded player state");
                state
            }
            Ok(None) => default_player_state,
            Err(err) => {
                warn!(player = %profile.name, error = %err, "player state load failed; using defaults");
                default_player_state
            }
        }
    } else {
        default_player_state
    };

    let (spawn_cx, spawn_cz) = player_state.pose.chunk_pos();
    let (outbound_tx, outbound_rx) =
        mpsc::channel(config.chunk_pipeline.chunk_result_queue_size.max(16));
    let initial_desired = if config.world.is_some() {
        desired_chunk_set(spawn_cx, spawn_cz, config.view_distance)
    } else {
        HashSet::new()
    };
    let initial_pose = player_state.pose;
    let (session_id, visibility) = match sessions.try_register(SessionRegistration {
        profile,
        center: (spawn_cx, spawn_cz),
        view_distance: config.view_distance,
        desired: initial_desired,
        tx: outbound_tx,
        pose: initial_pose,
        max_sessions: config.max_players as usize,
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
    let session_cleanup = RegisteredSessionCleanup::new(Arc::clone(&sessions), session_id);

    let permissions = config.command_permissions.permissions_for(profile);

    // 1. Login (Play).
    let login = LoginPlay {
        entity_id: i32::try_from(session_id).unwrap_or(i32::MAX),
        is_hardcore: false,
        dimension_names: dim_names.to_vec(),
        max_players: config.max_players.min(i32::MAX as u32) as i32,
        view_distance: config.view_distance,
        simulation_distance: config.view_distance,
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
    write_packet(writer, &command_tree_packet(permissions), compression).await?;
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
        Some(ChunkStreamState::new(
            Arc::clone(world),
            Arc::new(biomes.clone()),
            Arc::clone(&config.blocks),
            config.block_light.as_ref().map(Arc::clone),
            Arc::clone(&config.items),
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
        ))
    });
    if config.world.is_some() && chunk_stream.is_none() {
        warn!("worldgen/biome registry missing; skipping chunk emission");
    }
    let player_save_state = Arc::new(Mutex::new(player_state.clone()));
    sessions.register_player_persistence(session_id, Arc::clone(&player_save_state));
    let result = async {
        if let Some(stream) = chunk_stream.as_mut()
            && stream.step(writer, &mut light_cache).await? == ChunkStreamStep::Complete
        {
            stream.log_summary_once();
        }

        // 6. Seed an empty server-authoritative player inventory. Test
        //    and dev-only inventory mutation goes through explicit
        //    debug commands; normal survival no longer gets a starter kit.
        let initial_inventory = player_state.inventory.clone();
        write_packet(
            writer,
            &ClientboundContainerSetContent {
                container_id: 0,
                state_id: 1,
                items: initial_inventory.as_wire_list(),
                carried_item: ItemStack::EMPTY,
            },
            compression,
        )
        .await?;

        let mut recipes = (*config.recipes).clone();
        if recipes.is_empty() {
            recipes = mc_data::recipes::solaris_required_recipes();
        }

        // 7. Play loop. Runs until the connection drops or the client
        //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`. The
        //    interaction state passes the M5.d/M5.e/M6.f break/place
        //    handlers everything they need to mutate the world and emit
        //    relight + container packets back to the client.
        let mut interaction = config.world.as_ref().map(|world| InteractionState {
            world: Arc::clone(world),
            blocks: Arc::clone(&config.blocks),
            block_light: config.block_light.as_ref().map(Arc::clone),
            block_facts: Arc::clone(&config.block_facts),
            water: passive_herd_water,
            sessions: Arc::clone(&sessions),
            session_id,
            workspace: LightWorkspace::new(),
            light_cache: std::mem::take(&mut light_cache),
            compression,
            selected_hotbar_slot: player_state.selected_hotbar_slot,
            inventory: initial_inventory,
            carried_item: ItemStack::EMPTY,
            inventory_state_id: 1,
            items: Arc::clone(&config.items),
            item_facts: Arc::clone(&config.item_facts),
            entity_types: Arc::clone(&config.entity_types),
            item_to_block: ItemToBlockTable::build(&config.items, &config.blocks),
            tags: Arc::clone(&config.tags),
            recipes,
            loot: Arc::clone(&config.loot),
            next_container_id: FURNACE_CONTAINER_ID_MIN,
            active_container: None,
            pending_break: None,
            pending_use: None,
            pending_sign_edit: None,
            shield_use: None,
            last_hostile_damage_at: None,
            last_entity_attack_at: None,
        });
        play_loop(
            reader,
            writer,
            buf,
            compression,
            interaction.as_mut(),
            chunk_stream,
            Arc::clone(&sessions),
            config,
            session_id,
            initial_pose,
            player_state.spawn.pose(),
            respawn,
            permissions,
            player_state.survival,
            player_state.xp,
            player_state.game_mode,
            Some(Arc::clone(&player_save_state)),
            outbound_rx,
            config.view_distance,
        )
        .await
    }
    .await;

    if let Some(root) = world_root.as_deref() {
        let snapshot = player_save_state
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!(
                    player = %profile.name,
                    "player persistence mutex was poisoned before save; recovering state"
                );
                poisoned.into_inner()
            })
            .clone();
        match save_player_state(root, profile.uuid, &config.items, &snapshot) {
            Ok(()) => info!(player = %profile.name, state = %snapshot, "saved player state"),
            Err(err) => warn!(player = %profile.name, error = %err, "player state save failed"),
        }
    }

    session_cleanup.unregister();
    result
}

/// Per-connection state the M5.d / M5.e / M6 interaction handlers
/// carry.
struct InteractionState {
    world: WorldHandle,
    blocks: Arc<mc_world::BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    water: Option<mc_world::BlockStateId>,
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    /// Reused across all interaction-driven relight computes for
    /// the lifetime of the connection (same amortisation pattern
    /// as `emit_chunks_around`).
    workspace: LightWorkspace,
    /// M9.a: per-chunk computed light, populated during the spawn
    /// burst and mutated in place by [`apply_block_change_to_light`]
    /// on every edit. Replaces the M5-era pattern of recomputing
    /// the full 3×3 neighbourhood for each affected chunk on every
    /// break/place.
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
    /// M6.e: per-vanilla, the server bumps this counter on every
    /// inventory mutation it ships to the client; the client uses
    /// it to detect desyncs. Starts at 1 (after the seed
    /// ContainerSetContent on login).
    inventory_state_id: i32,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
    entity_types: Arc<EntityTypeRegistry>,
    /// Registry-derived item→default-block resolver. Built once from
    /// vanilla item/block registries at construction time.
    item_to_block: ItemToBlockTable,
    tags: Arc<TagsData>,
    recipes: Vec<mc_data::recipes::Recipe>,
    loot: Arc<mc_data::loot::LootTables>,
    next_container_id: i32,
    active_container: Option<ActiveContainer>,
    pending_break: Option<PendingBreak>,
    pending_use: Option<PendingUse>,
    pending_sign_edit: Option<mc_world::BlockPos>,
    shield_use: Option<ShieldUseState>,
    last_hostile_damage_at: Option<Instant>,
    last_entity_attack_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShieldUseState {
    hand: mc_protocol::packets::play::InteractionHand,
    started_tick: u64,
    slot: usize,
    stack: ItemStack,
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
    Ignored { channel: String, payload_len: usize },
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

    Ok(PlayCustomPayloadAction::Ignored {
        channel: channel.as_str().to_string(),
        payload_len: body.remaining(),
    })
}

fn clamp_client_view_distance(requested: i8, server_view_distance: i32) -> i32 {
    i32::from(requested).clamp(2, server_view_distance.max(2))
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
    let (x, y, z) = unpack_block_pos(position);
    let update = {
        let mut storage = state.world.lock().await;
        match storage.get_block(mc_world::BlockPos { x, y, z }) {
            Ok(Some(state_id)) => Some(BlockUpdate {
                position,
                state_id: state_id.0 as i32,
            }),
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, x, y, z, "block resync read failed");
                None
            }
        }
    };
    if let Some(update) = update {
        write_packet(writer, &update, state.compression).await?;
    }
    write_block_ack(writer, state.compression, sequence).await
}

fn crafting_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        10..=36 => Some(9 + (menu_slot - 10)),
        37..=45 => Some(36 + (menu_slot - 37)),
        _ => None,
    }
}

fn shaped_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    shaped: &mc_data::recipes::ShapedRecipe,
) -> bool {
    let height = shaped.pattern.len();
    let width = shaped
        .pattern
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    if height == 0 || width == 0 || height > 3 || width > 3 {
        return false;
    }

    for top in 0..=(3 - height) {
        'left: for left in 0..=(3 - width) {
            for row in 0..3 {
                for col in 0..3 {
                    let stack = &input[row * 3 + col];
                    let ingredient =
                        if row >= top && row < top + height && col >= left && col < left + width {
                            shaped
                                .pattern
                                .get(row - top)
                                .and_then(|pattern_row| pattern_row.chars().nth(col - left))
                                .filter(|ch| *ch != ' ')
                                .and_then(|ch| shaped.key.get(&ch))
                        } else {
                            None
                        };
                    match ingredient {
                        Some(ingredient)
                            if !stack.is_empty()
                                && ingredient_accepts_item(
                                    items,
                                    tags,
                                    stack.item_id,
                                    ingredient,
                                ) => {}
                        None if stack.is_empty() => {}
                        _ => continue 'left,
                    }
                }
            }
            return true;
        }
    }
    false
}

fn shapeless_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    shapeless: &mc_data::recipes::ShapelessRecipe,
) -> bool {
    let stacks: Vec<_> = input.iter().filter(|stack| !stack.is_empty()).collect();
    if stacks.len() != shapeless.ingredients.len() {
        return false;
    }
    let mut used = vec![false; shapeless.ingredients.len()];
    for stack in stacks {
        let Some((idx, _)) = shapeless
            .ingredients
            .iter()
            .enumerate()
            .find(|(idx, ingredient)| {
                !used[*idx] && ingredient_accepts_item(items, tags, stack.item_id, ingredient)
            })
        else {
            return false;
        };
        used[idx] = true;
    }
    true
}

fn crafting_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    recipe: &mc_data::recipes::Recipe,
) -> bool {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            shaped_recipe_matches(items, tags, input, shaped)
        }
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            shapeless_recipe_matches(items, tags, input, shapeless)
        }
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_)
        | mc_data::recipes::RecipeKind::CampfireCooking(_) => false,
    }
}

fn crafting_result_from_input(
    items: &ItemRegistry,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    input: &[ItemStack; 9],
) -> ItemStack {
    recipes
        .iter()
        .find(|recipe| crafting_recipe_matches(items, tags, input, recipe))
        .and_then(|recipe| {
            let item_id = items.id_of(&recipe.result.item)?;
            let count = i32::try_from(recipe.result.count).ok()?;
            (count > 0).then(|| ItemStack::new(item_id, count))
        })
        .unwrap_or(ItemStack::EMPTY)
}

fn refresh_crafting_result(state: &InteractionState, window: &mut CraftingTableWindow) {
    window.result =
        crafting_result_from_input(&state.items, &state.tags, &state.recipes, &window.input);
}

fn inventory_crafting_input(inventory: &PlayerInventory) -> [ItemStack; 9] {
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = inventory.slots[1].clone();
    input[1] = inventory.slots[2].clone();
    input[3] = inventory.slots[3].clone();
    input[4] = inventory.slots[4].clone();
    input
}

fn refresh_inventory_crafting_result(state: &mut InteractionState) {
    let input = inventory_crafting_input(&state.inventory);
    state.inventory.slots[0] =
        crafting_result_from_input(&state.items, &state.tags, &state.recipes, &input);
}

fn crafting_wire_items(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(CRAFTING_MENU_SLOT_COUNT);
    items.push(window.result.clone());
    items.extend(window.input.iter().cloned());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
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

fn crafting_menu_stack(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0 => Some(window.result.clone()),
        1..=9 => Some(window.input[menu_slot - 1].clone()),
        _ => crafting_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_crafting_menu_stack(
    window: &mut CraftingTableWindow,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        1..=9 => {
            window.input[menu_slot - 1] = stack;
            true
        }
        _ => {
            let Some(slot) = crafting_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn can_place_in_crafting_menu_slot(
    state: &InteractionState,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => false,
        1..=9 => true,
        _ => crafting_player_slot(menu_slot)
            .is_some_and(|slot| can_place_in_player_slot(state, slot, stack)),
    }
}

fn apply_crafting_swap_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT || menu_slot == 0 {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if crafting_player_slot(menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = crafting_menu_stack(window, &state.inventory, menu_slot) else {
        return false;
    };
    let swap = state.inventory.slots[player_slot].clone();
    let can_place_swap = can_place_in_crafting_menu_slot(state, menu_slot, &swap);
    let can_place_clicked = can_place_in_player_slot(state, player_slot, &clicked);
    let Some((new_clicked, new_swap)) =
        apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
    else {
        return false;
    };
    if !set_crafting_menu_stack(window, &mut state.inventory, menu_slot, new_clicked) {
        return false;
    }
    state.inventory.slots[player_slot] = new_swap;
    refresh_crafting_result(state, window);
    true
}

fn apply_crafting_throw_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT || menu_slot == 0 {
        return None;
    }
    let (stack, dropped) = apply_regular_throw_slot(
        crafting_menu_stack(window, &state.inventory, menu_slot)?,
        button,
    )?;
    if !set_crafting_menu_stack(window, &mut state.inventory, menu_slot, stack) {
        return None;
    }
    refresh_crafting_result(state, window);
    Some(dropped)
}

fn crafting_remainder_for_item(state: &InteractionState, item_id: u32) -> Option<ItemStack> {
    let name = state.items.name_of(item_id)?;
    let bucket = Identifier::parse("minecraft:bucket").expect("static identifier");
    if name.path().ends_with("_bucket") || name.as_str() == "minecraft:milk_bucket" {
        state
            .items
            .id_of(&bucket)
            .map(|bucket_id| ItemStack::new(bucket_id, 1))
    } else {
        None
    }
}

fn consume_crafting_ingredients(state: &mut InteractionState, window: &mut CraftingTableWindow) {
    let consumed: Vec<_> = window
        .input
        .iter()
        .map(|stack| (!stack.is_empty()).then_some(stack.item_id))
        .collect();
    for (idx, item_id) in consumed.into_iter().enumerate() {
        let Some(item_id) = item_id else {
            continue;
        };
        window.input[idx].count -= 1;
        if window.input[idx].count <= 0 {
            window.input[idx] =
                crafting_remainder_for_item(state, item_id).unwrap_or(ItemStack::EMPTY);
        } else if let Some(remainder) = crafting_remainder_for_item(state, item_id) {
            let max_stack = item_max_stack(&state.item_facts, &state.items, &remainder);
            let (remaining, _) = state.inventory.merge_stack(remainder, max_stack);
            if !remaining.is_empty() {
                debug!(
                    item_id = remaining.item_id,
                    count = remaining.count,
                    "dropping crafting remainder because inventory is full"
                );
            }
        }
    }
    refresh_crafting_result(state, window);
}

fn consume_inventory_crafting_ingredients(state: &mut InteractionState) {
    for slot in 1..=4 {
        let item_id = (!state.inventory.slots[slot].is_empty())
            .then_some(state.inventory.slots[slot].item_id);
        let Some(item_id) = item_id else {
            continue;
        };
        state.inventory.slots[slot].count -= 1;
        if state.inventory.slots[slot].count <= 0 {
            state.inventory.slots[slot] =
                crafting_remainder_for_item(state, item_id).unwrap_or(ItemStack::EMPTY);
        } else if let Some(remainder) = crafting_remainder_for_item(state, item_id) {
            let max_stack = item_max_stack(&state.item_facts, &state.items, &remainder);
            let (remaining, _) = state.inventory.merge_stack(remainder, max_stack);
            if !remaining.is_empty() {
                debug!(
                    item_id = remaining.item_id,
                    count = remaining.count,
                    "dropping inventory crafting remainder because inventory is full"
                );
            }
        }
    }
    refresh_inventory_crafting_result(state);
}

fn store_inventory_crafting_inputs(state: &mut InteractionState, player_pose: PlayerPose) {
    state.inventory.slots[0] = ItemStack::EMPTY;
    for slot in 1..=4 {
        let stack = std::mem::replace(&mut state.inventory.slots[slot], ItemStack::EMPTY);
        if stack.is_empty() {
            continue;
        }
        let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
        let (remaining, _) = state.inventory.merge_stack(stack, max_stack);
        if !remaining.is_empty() {
            dispatch_inventory_drop(state, player_pose, remaining);
        }
    }
    refresh_inventory_crafting_result(state);
}

fn take_inventory_crafting_result(state: &mut InteractionState) -> bool {
    refresh_inventory_crafting_result(state);
    let result = state.inventory.slots[0].clone();
    if result.is_empty() {
        return false;
    }
    let max_stack = item_max_stack(&state.item_facts, &state.items, &result);
    if state.carried_item.is_empty() {
        state.carried_item = result;
        consume_inventory_crafting_ingredients(state);
        return true;
    }
    if can_stack(&state.carried_item, &result)
        && state.carried_item.count + result.count <= max_stack
    {
        state.carried_item.count += result.count;
        consume_inventory_crafting_ingredients(state);
        return true;
    }
    false
}

fn take_crafting_result(state: &mut InteractionState, window: &mut CraftingTableWindow) -> bool {
    let result = window.result.clone();
    if result.is_empty() {
        return false;
    }
    let max_stack = item_max_stack(&state.item_facts, &state.items, &result);
    if state.carried_item.is_empty() {
        state.carried_item = result;
        consume_crafting_ingredients(state, window);
        return true;
    }
    if can_stack(&state.carried_item, &result)
        && state.carried_item.count + result.count <= max_stack
    {
        state.carried_item.count += result.count;
        consume_crafting_ingredients(state, window);
        return true;
    }
    false
}

fn apply_crafting_pickup_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    if menu_slot == 0 {
        return take_crafting_result(state, window);
    }
    let Some(slot_stack) = crafting_menu_stack(window, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = pickup_click_max_stack(state, &slot_stack);
    let can_place_cursor = can_place_in_crafting_menu_slot(state, menu_slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        &mut state.carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    let changed = set_crafting_menu_stack(window, &mut state.inventory, menu_slot, new_slot);
    if changed {
        refresh_crafting_result(state, window);
    }
    changed
}

fn apply_crafting_quick_move_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
) -> bool {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT {
        return false;
    }
    if menu_slot == 0 {
        let result = window.result.clone();
        if result.is_empty() {
            return false;
        }
        let max_stack = item_max_stack(&state.item_facts, &state.items, &result);
        let mut merged = state.inventory.clone();
        let (remaining, _) = merged.merge_stack(result.clone(), max_stack);
        if !remaining.is_empty() {
            return false;
        }
        state.inventory = merged;
        consume_crafting_ingredients(state, window);
        return true;
    }
    let Some(player_slot) = crafting_player_slot(menu_slot) else {
        return false;
    };
    let original = state.inventory.slots[player_slot].clone();
    if original.is_empty() {
        return false;
    }
    state.inventory.slots[player_slot] = ItemStack::EMPTY;
    let remaining = state.inventory.merge_stack_into_ranges(
        original.clone(),
        &[9..=35, 36..=44],
        item_max_stack(&state.item_facts, &state.items, &original),
    );
    state.inventory.slots[player_slot] = remaining;
    state.inventory.slots[player_slot] != original
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
    let clicked = {
        let mut storage = state.world.lock().await;
        storage.get_block(position).map_err(|err| {
            warn!(error = %err, x, y, z, "crafting table use target read failed");
            err
        })?
    };
    if !clicked.is_some_and(|block_state| is_crafting_table_state(state, block_state)) {
        return Ok(false);
    }

    store_active_container(state, player_pose);
    let mut window = CraftingTableWindow::new(next_container_id(state));
    refresh_crafting_result(state, &mut window);
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
    state.active_container = Some(ActiveContainer::CraftingTable(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn handle_crafting_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: CraftingTableWindow,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<CraftingTableWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if packet.state_id != window.state_id {
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let before_window = window.clone();
    let mut dropped = None;
    let changed = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => {
            apply_crafting_pickup_click(state, &mut window, slot, button)
        }
        ContainerClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click(state, button);
            dropped.is_some()
        }
        ContainerClickAction::QuickMove { slot } => {
            apply_crafting_quick_move_click(state, &mut window, slot)
        }
        ContainerClickAction::Swap { slot, button } => {
            apply_crafting_swap_click(state, &mut window, slot, button)
        }
        ContainerClickAction::Throw { slot, button } => {
            if item_entity_type_id(&state.entity_types).is_some() {
                dropped = apply_crafting_throw_click(state, &mut window, slot, button);
                dropped.is_some()
            } else {
                false
            }
        }
        ContainerClickAction::Unsupported => false,
    };
    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        window = before_window;
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    if changed {
        window.state_id = window.state_id.wrapping_add(1);
    }
    if let Some(stack) = dropped {
        dispatch_inventory_drop(state, player_pose, stack);
    }
    write_crafting_content(state, writer, &window).await?;
    Ok(window)
}

fn furnace_slot_to_stack(slot: &FurnaceSlot) -> ItemStack {
    if slot.is_empty() {
        ItemStack::EMPTY
    } else {
        ItemStack {
            count: slot.count,
            item_id: slot.item_id,
            damage: slot.damage,
        }
    }
}

fn stack_to_furnace_slot(stack: &ItemStack) -> FurnaceSlot {
    if stack.is_empty() {
        FurnaceSlot::EMPTY
    } else {
        FurnaceSlot {
            count: stack.count,
            item_id: stack.item_id,
            damage: stack.damage,
        }
    }
}

fn furnace_data_values(furnace: &FurnaceBlockEntity) -> [(i16, i16); 4] {
    [
        (0, furnace.burn_remaining),
        (1, furnace.burn_total),
        (2, furnace.cook_progress),
        (3, furnace.cook_total),
    ]
}

fn furnace_wire_items(furnace: &FurnaceBlockEntity, inventory: &PlayerInventory) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(FURNACE_MENU_SLOT_COUNT);
    items.extend(furnace.slots.iter().map(furnace_slot_to_stack));
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

fn furnace_slot_stacks(furnace: &FurnaceBlockEntity) -> [ItemStack; 3] {
    std::array::from_fn(|slot| furnace_slot_to_stack(&furnace.slots[slot]))
}

fn chest_player_slot(storage_slots: usize, menu_slot: usize) -> Option<usize> {
    let main_end = storage_slots + 26;
    let hotbar_start = storage_slots + 27;
    let hotbar_end = storage_slots + 35;
    match menu_slot {
        slot if (storage_slots..=main_end).contains(&slot) => Some(9 + (slot - storage_slots)),
        slot if (hotbar_start..=hotbar_end).contains(&slot) => Some(36 + (slot - hotbar_start)),
        _ => None,
    }
}

fn chest_menu_stack(
    view: &ChestView,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    let storage_slots = view.storage_slots();
    if menu_slot < storage_slots {
        let chest = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        return Some(furnace_slot_to_stack(&view.chests[chest].slots[slot]));
    }
    chest_player_slot(storage_slots, menu_slot).map(|slot| inventory.slots[slot].clone())
}

fn set_chest_menu_stack(
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    let storage_slots = view.storage_slots();
    if menu_slot < storage_slots {
        let chest = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        view.chests[chest].slots[slot] = stack_to_furnace_slot(&stack);
        return true;
    }
    let Some(slot) = chest_player_slot(storage_slots, menu_slot) else {
        return false;
    };
    inventory.slots[slot] = stack;
    true
}

fn chest_wire_items(view: &ChestView, inventory: &PlayerInventory) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(view.storage_slots() + PLAYER_CONTAINER_STORAGE_SLOTS);
    for chest in &view.chests {
        items.extend(chest.slots.iter().map(furnace_slot_to_stack));
    }
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

fn chest_slot_stacks(view: &ChestView) -> Vec<ItemStack> {
    view.chests
        .iter()
        .flat_map(|chest| chest.slots.iter().map(furnace_slot_to_stack))
        .collect()
}

fn adjacent_chest_positions(position: mc_world::BlockPos) -> [mc_world::BlockPos; 4] {
    [
        mc_world::BlockPos {
            x: position.x - 1,
            y: position.y,
            z: position.z,
        },
        mc_world::BlockPos {
            x: position.x + 1,
            y: position.y,
            z: position.z,
        },
        mc_world::BlockPos {
            x: position.x,
            y: position.y,
            z: position.z - 1,
        },
        mc_world::BlockPos {
            x: position.x,
            y: position.y,
            z: position.z + 1,
        },
    ]
}

async fn load_chest_view(
    state: &InteractionState,
    window: &ChestWindow,
) -> Result<ChestView, ConnectionError> {
    let mut storage = state.world.lock().await;
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
    let (positions, title) = {
        let mut storage = state.world.lock().await;
        let clicked = storage
            .get_block(position)
            .map_err(|err| {
                warn!(error = %err, x, y, z, "chest use target read failed");
                err
            })
            .ok()
            .flatten();
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
                let neighbour_state = storage
                    .get_block(neighbour)
                    .map_err(|err| {
                        warn!(error = %err, ?neighbour, "adjacent chest read failed");
                        err
                    })
                    .ok()
                    .flatten();
                if neighbour_state.is_some_and(|block_state| is_chest_state(state, block_state)) {
                    positions.push(neighbour);
                    break;
                }
            }
        }
        positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
        (positions, title)
    };

    store_active_container(state, player_pose);
    let container_id = next_container_id(state);
    let mut window = ChestWindow::new(positions, container_id);
    window.state_id = state
        .sessions
        .register_chest_viewer(state.session_id, window.position());
    let view = load_chest_view(state, &window).await?;
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
        let mut storage = state.world.lock().await;
        let clicked = storage
            .get_block(position)
            .map_err(|err| {
                warn!(error = %err, x, y, z, "furnace use target read failed");
                err
            })
            .ok()
            .flatten();
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

    store_active_container(state, player_pose);
    let container_id = next_container_id(state);
    let mut window = FurnaceWindow::new(position, container_id, kind);
    window.state_id = state
        .sessions
        .register_furnace_viewer(state.session_id, window.position);
    let furnace = {
        let mut storage = state.world.lock().await;
        storage.furnace_block_entity(position).map_err(|err| {
            warn!(error = %err, x, y, z, "furnace state read failed");
            err
        })?
    }
    .unwrap_or_default();
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
        let mut storage = state.world.lock().await;
        let clicked = storage
            .get_block(position)
            .map_err(|err| {
                warn!(error = %err, x, y, z, "station use target read failed");
                err
            })?
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

    let active_crafting = if packet.container_id == 0 {
        None
    } else {
        match state.active_container.take() {
            Some(ActiveContainer::CraftingTable(window))
                if window.container_id == packet.container_id =>
            {
                Some(window)
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

    if let Some(changed) = craft_recipe(state, &recipe, packet.use_max_items) {
        write_inventory_slot_updates(state, writer, changed).await?;
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
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: 0,
            state_id: state.inventory_state_id,
            items: state.inventory.as_wire_list(),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

fn take_from_slot(slot: &mut ItemStack, count: i32) -> ItemStack {
    if slot.is_empty() || count <= 0 {
        return ItemStack::EMPTY;
    }
    let moved = slot.count.min(count);
    let mut out = slot.clone();
    out.count = moved;
    slot.count -= moved;
    if slot.count <= 0 {
        *slot = ItemStack::EMPTY;
    }
    out
}

fn decrement_cursor(cursor: &mut ItemStack) {
    cursor.count -= 1;
    if cursor.count <= 0 {
        *cursor = ItemStack::EMPTY;
    }
}

fn hotbar_swap_slot(button: i8) -> Option<usize> {
    (0..=8)
        .contains(&button)
        .then_some(PlayerInventory::HOTBAR_BASE + button as usize)
}

fn player_swap_slot(button: i8) -> Option<usize> {
    hotbar_swap_slot(button).or_else(|| (button == 40).then_some(45))
}

fn take_throw_stack(slot: &mut ItemStack, button: i8) -> Option<ItemStack> {
    match button {
        0 => (!slot.is_empty()).then(|| take_from_slot(slot, 1)),
        1 => (!slot.is_empty()).then(|| std::mem::take(slot)),
        _ => None,
    }
}

fn pickup_click_max_stack(state: &InteractionState, slot_stack: &ItemStack) -> i32 {
    let stack = if state.carried_item.is_empty() {
        slot_stack
    } else {
        &state.carried_item
    };
    item_max_stack(&state.item_facts, &state.items, stack)
}

fn apply_regular_pickup_slot(
    carried_item: &mut ItemStack,
    slot_stack: ItemStack,
    button: i8,
    max_stack: i32,
    can_place_carried: bool,
) -> Option<ItemStack> {
    if !(button == 0 || button == 1) {
        return None;
    }

    let cursor = carried_item.clone();
    if button == 0 {
        if cursor.is_empty() {
            if slot_stack.is_empty() {
                return None;
            }
            *carried_item = slot_stack;
            return Some(ItemStack::EMPTY);
        }
        if !can_place_carried {
            return None;
        }
        if slot_stack.is_empty() {
            *carried_item = ItemStack::EMPTY;
            return Some(cursor);
        }
        if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
            let moved = (max_stack - slot_stack.count).min(cursor.count);
            if moved <= 0 {
                return None;
            }
            let mut new_slot = slot_stack;
            new_slot.count += moved;
            carried_item.count -= moved;
            if carried_item.count <= 0 {
                *carried_item = ItemStack::EMPTY;
            }
            return Some(new_slot);
        }
        *carried_item = slot_stack;
        return Some(cursor);
    }

    if cursor.is_empty() {
        if slot_stack.is_empty() {
            return None;
        }
        let moved = (slot_stack.count + 1) / 2;
        let mut new_cursor = slot_stack.clone();
        new_cursor.count = moved;
        let mut remaining = slot_stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            remaining = ItemStack::EMPTY;
        }
        *carried_item = new_cursor;
        return Some(remaining);
    }
    if !can_place_carried {
        return None;
    }
    if slot_stack.is_empty() {
        let mut one = cursor;
        one.count = 1;
        decrement_cursor(carried_item);
        return Some(one);
    }
    if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
        let mut new_slot = slot_stack;
        new_slot.count += 1;
        decrement_cursor(carried_item);
        return Some(new_slot);
    }
    None
}

fn apply_regular_swap_slot(
    clicked: ItemStack,
    swap: ItemStack,
    can_place_swap: bool,
    can_place_clicked: bool,
) -> Option<(ItemStack, ItemStack)> {
    (can_place_swap && can_place_clicked).then_some((swap, clicked))
}

fn apply_regular_throw_slot(slot_stack: ItemStack, button: i8) -> Option<(ItemStack, ItemStack)> {
    let mut stack = slot_stack;
    let dropped = take_throw_stack(&mut stack, button)?;
    Some((stack, dropped))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerClickAction {
    Pickup { slot: usize, button: i8 },
    OutsidePickup { button: i8 },
    QuickMove { slot: usize },
    Swap { slot: usize, button: i8 },
    Throw { slot: usize, button: i8 },
    Unsupported,
}

fn classify_container_click(packet: &ServerboundContainerClick) -> ContainerClickAction {
    if packet.slot_num < 0 {
        return match packet.container_input {
            ContainerInput::Pickup => ContainerClickAction::OutsidePickup {
                button: packet.button_num,
            },
            _ => ContainerClickAction::Unsupported,
        };
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

fn dispatch_inventory_drop(state: &InteractionState, player_pose: PlayerPose, stack: ItemStack) {
    if stack.is_empty() {
        return;
    }
    let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
        debug!("inventory drop ignored: item entity type unavailable");
        return;
    };
    dispatch_visibility_commands(state.sessions.spawn_item_drop(
        entity_type_id,
        Vec3::new(player_pose.x, player_pose.y + 1.0, player_pose.z),
        entity_item_stack(stack),
    ));
}

fn is_campfire_state(state: &InteractionState, block_state: mc_world::BlockStateId) -> bool {
    state.blocks.by_id(block_state).is_some_and(|block_state| {
        matches!(
            block_state.block.id.as_str(),
            "minecraft:campfire" | "minecraft:soul_campfire"
        )
    })
}

fn hand_inventory_slot(state: &InteractionState, hand: InteractionHand) -> usize {
    match hand {
        InteractionHand::MainHand => {
            PlayerInventory::HOTBAR_BASE + state.selected_hotbar_slot as usize
        }
        InteractionHand::OffHand => 45,
    }
}

fn campfire_result_stack(
    state: &InteractionState,
    recipe: &mc_data::recipes::Recipe,
) -> Option<ItemStack> {
    let item_id = state.items.id_of(&recipe.result.item)?;
    let count = i32::try_from(recipe.result.count).ok()?;
    (count > 0).then(|| ItemStack::new(item_id, count))
}

fn campfire_block_entity_update_nbt(
    items: &ItemRegistry,
    cooking: &CampfireCookingState,
) -> Option<Tag> {
    let mut item_tags = Vec::new();
    for (slot, entry) in cooking.slots.iter().enumerate() {
        let Some(entry) = entry else {
            continue;
        };
        let name = items.name_of(entry.input.item_id)?;
        item_tags.push(Tag::Compound(vec![
            ("Slot".into(), Tag::Int(slot as i32)),
            ("id".into(), Tag::String(name.as_str().to_string())),
            ("count".into(), Tag::Int(entry.input.count)),
        ]));
    }
    Some(Tag::Compound(vec![(
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                mc_nbt::tag_type::END
            } else {
                mc_nbt::tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    )]))
}

async fn send_campfire_block_entity_update<W>(
    state: &InteractionState,
    writer: &mut W,
    position: mc_world::BlockPos,
    cooking: &CampfireCookingState,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(nbt) = campfire_block_entity_update_nbt(&state.items, cooking) else {
        warn!(
            ?position,
            "campfire block entity update skipped for unknown item id"
        );
        return Ok(());
    };
    write_packet(
        writer,
        &ClientboundBlockEntityData {
            position: pack_block_pos(position.x, position.y, position.z),
            block_entity_type: CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
            nbt: nbt.clone(),
        },
        state.compression,
    )
    .await?;
    dispatch_visibility_commands(state.sessions.block_entity_data_dispatches(
        position,
        Some(state.session_id),
        CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
        nbt,
    ));
    Ok(())
}

fn dispatch_campfire_block_entity_update(
    state: &InteractionState,
    position: mc_world::BlockPos,
    cooking: &CampfireCookingState,
) {
    let Some(nbt) = campfire_block_entity_update_nbt(&state.items, cooking) else {
        warn!(
            ?position,
            "campfire block entity update skipped for unknown item id"
        );
        return;
    };
    dispatch_visibility_commands(state.sessions.block_entity_data_dispatches(
        position,
        None,
        CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
        nbt,
    ));
}

async fn handle_campfire_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    _game_mode: GameMode,
    sequence: i32,
    position: mc_world::BlockPos,
    hand: InteractionHand,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let clicked = {
        let mut storage = state.world.lock().await;
        match storage.get_block(position) {
            Ok(Some(current)) => current,
            Ok(None) => return Ok(false),
            Err(err) => {
                warn!(error = %err, x = position.x, y = position.y, z = position.z, "campfire clicked read failed");
                return Ok(false);
            }
        }
    };
    if !is_campfire_state(state, clicked) {
        return Ok(false);
    }

    let slot = hand_inventory_slot(state, hand);
    let held = state.inventory.slots[slot].clone();
    if held.is_empty() {
        return Ok(false);
    }
    let Some(recipe) = find_campfire_recipe_for_item(state, held.item_id) else {
        return Ok(false);
    };
    let Some(result) = campfire_result_stack(state, &recipe) else {
        return Ok(false);
    };
    let cooking_time = match &recipe.kind {
        mc_data::recipes::RecipeKind::CampfireCooking(smelting) => smelting.cooking_time,
        _ => return Ok(false),
    };
    let input = ItemStack {
        count: 1,
        item_id: held.item_id,
        damage: held.damage,
    };
    let Some(cooking) =
        state
            .sessions
            .insert_campfire_cooking(position, input, result, cooking_time)
    else {
        return Ok(false);
    };

    let held = &mut state.inventory.slots[slot];
    held.count = held.count.saturating_sub(1);
    if held.count <= 0 {
        *held = ItemStack::EMPTY;
    }
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    write_packet(
        writer,
        &ClientboundContainerSetSlot {
            container_id: 0,
            state_id: state.inventory_state_id,
            slot: slot as i16,
            item_stack: state.inventory.slots[slot].clone(),
        },
        state.compression,
    )
    .await?;
    send_campfire_block_entity_update(state, writer, position, &cooking).await?;
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn tick_campfire_cooking(state: &mut InteractionState) {
    let updates = state.sessions.tick_campfire_cooking();
    for (position, cooking) in updates.changed {
        let still_campfire = campfire_block_still_present(state, position).await;
        if still_campfire {
            dispatch_campfire_block_entity_update(state, position, &cooking);
        }
    }
    let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
        if !updates.completed.is_empty() {
            debug!(
                count = updates.completed.len(),
                "campfire drops ignored: item entity type unavailable"
            );
        }
        return;
    };
    for (position, stack) in updates.completed {
        let still_campfire = campfire_block_still_present(state, position).await;
        if !still_campfire {
            continue;
        }
        dispatch_visibility_commands(state.sessions.spawn_item_drop(
            entity_type_id,
            Vec3::new(
                position.x as f64 + 0.5,
                position.y as f64 + 1.0,
                position.z as f64 + 0.5,
            ),
            entity_item_stack(stack),
        ));
    }
}

async fn campfire_block_still_present(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> bool {
    let mut storage = state.world.lock().await;
    storage
        .get_block(position)
        .ok()
        .flatten()
        .is_some_and(|block_state| is_campfire_state(state, block_state))
}

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

async fn drop_inventory_on_death<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let drops = take_death_inventory_drops(&mut state.inventory, &mut state.carried_item);
    if drops.is_empty() {
        return Ok(());
    }
    for stack in drops {
        dispatch_inventory_drop(state, player_pose, stack);
    }
    write_inventory_content(state, writer).await
}

async fn reset_xp_on_death<W>(
    state: Option<&InteractionState>,
    xp_state: &mut XpState,
    writer: &mut W,
    compression: Compression,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let dropped_xp = recoverable_death_xp(xp_state);
    if dropped_xp > 0
        && let Some(state) = state
        && let Some(entity_type_id) = xp_orb_entity_type_id(&state.entity_types)
    {
        dispatch_visibility_commands(state.sessions.spawn_xp_orb(
            entity_type_id,
            Vec3::new(player_pose.x, player_pose.y, player_pose.z),
            dropped_xp,
        ));
    }
    if xp_state.reset() {
        write_packet(writer, &xp_state.as_packet(), compression).await?;
    }
    Ok(())
}

fn recoverable_death_xp(xp_state: &XpState) -> i32 {
    xp_state.level.saturating_mul(7).clamp(0, 100)
}

fn apply_pickup_click(state: &mut InteractionState, slot: usize, button: i8) -> bool {
    if slot >= state.inventory.slots.len() || !(button == 0 || button == 1) {
        return false;
    }
    if slot == 0 {
        return take_inventory_crafting_result(state);
    }

    let slot_stack = state.inventory.slots[slot].clone();
    let cursor = state.carried_item.clone();
    let max_stack = pickup_click_max_stack(state, &slot_stack);
    let can_place_cursor = can_place_in_player_slot(state, slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        &mut state.carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    state.inventory.slots[slot] = new_slot;
    if (1..=4).contains(&slot) {
        refresh_inventory_crafting_result(state);
    }
    true
}

fn apply_swap_click(state: &mut InteractionState, slot: usize, button: i8) -> bool {
    if slot == 0 || slot >= state.inventory.slots.len() {
        return false;
    }
    let Some(swap_slot) = player_swap_slot(button) else {
        return false;
    };
    if slot == swap_slot {
        return false;
    }
    let clicked = state.inventory.slots[slot].clone();
    let swap = state.inventory.slots[swap_slot].clone();
    let can_place_swap = can_place_in_player_slot(state, slot, &swap);
    let can_place_clicked = can_place_in_player_slot(state, swap_slot, &clicked);
    let Some((new_clicked, new_swap)) =
        apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
    else {
        return false;
    };
    state.inventory.slots[slot] = new_clicked;
    state.inventory.slots[swap_slot] = new_swap;
    if (1..=4).contains(&slot) || (1..=4).contains(&swap_slot) {
        refresh_inventory_crafting_result(state);
    }
    true
}

fn apply_throw_click(state: &mut InteractionState, slot: usize, button: i8) -> Option<ItemStack> {
    if slot == 0 || slot >= state.inventory.slots.len() {
        return None;
    }
    let (new_slot, dropped) =
        apply_regular_throw_slot(state.inventory.slots[slot].clone(), button)?;
    state.inventory.slots[slot] = new_slot;
    if (1..=4).contains(&slot) {
        refresh_inventory_crafting_result(state);
    }
    Some(dropped)
}

fn apply_outside_pickup_click(state: &mut InteractionState, button: i8) -> Option<ItemStack> {
    if state.carried_item.is_empty() {
        return None;
    }
    match button {
        0 => Some(std::mem::take(&mut state.carried_item)),
        1 => {
            let mut dropped = state.carried_item.clone();
            dropped.count = 1;
            decrement_cursor(&mut state.carried_item);
            Some(dropped)
        }
        _ => None,
    }
}

fn can_place_in_player_slot(state: &InteractionState, slot: usize, stack: &ItemStack) -> bool {
    if stack.is_empty() {
        return true;
    }
    match slot {
        5..=8 => armor_entry_for_item(&state.items, stack.item_id)
            .is_some_and(|entry| armor_slot_for_kind(entry.slot) == slot),
        _ => true,
    }
}

fn apply_quick_move_click(state: &mut InteractionState, slot: usize) -> bool {
    if slot >= state.inventory.slots.len() || state.inventory.slots[slot].is_empty() {
        return false;
    }
    if slot == 0 {
        let result = state.inventory.slots[0].clone();
        let max_stack = item_max_stack(&state.item_facts, &state.items, &result);
        let mut merged = state.inventory.clone();
        let (remaining, _) = merged.merge_stack(result, max_stack);
        if !remaining.is_empty() {
            return false;
        }
        state.inventory = merged;
        consume_inventory_crafting_ingredients(state);
        return true;
    }

    let original = state.inventory.slots[slot].clone();
    let max_stack = item_max_stack(&state.item_facts, &state.items, &original);
    if !(5..=8).contains(&slot)
        && let Some(entry) = armor_entry_for_item(&state.items, original.item_id)
    {
        let armor_slot = armor_slot_for_kind(entry.slot);
        if state.inventory.slots[armor_slot].is_empty() {
            let mut equipped = original.clone();
            equipped.count = 1;
            state.inventory.slots[armor_slot] = equipped;
            if original.count <= 1 {
                state.inventory.slots[slot] = ItemStack::EMPTY;
            } else {
                state.inventory.slots[slot].count -= 1;
            }
            if (1..=4).contains(&slot) {
                refresh_inventory_crafting_result(state);
            }
            return true;
        }
    }

    state.inventory.slots[slot] = ItemStack::EMPTY;
    let remaining = if (36..=44).contains(&slot) {
        state
            .inventory
            .merge_stack_into_ranges(original.clone(), &[9..=35], max_stack)
    } else {
        state
            .inventory
            .merge_stack_into_ranges(original.clone(), &[36..=44, 9..=35], max_stack)
    };
    state.inventory.slots[slot] = remaining;
    if (1..=4).contains(&slot) {
        refresh_inventory_crafting_result(state);
    }
    state.inventory.slots[slot] != original
}

fn furnace_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        3..=29 => Some(9 + (menu_slot - 3)),
        30..=38 => Some(36 + (menu_slot - 30)),
        _ => None,
    }
}

fn furnace_menu_stack(
    furnace: &FurnaceBlockEntity,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0..=2 => Some(furnace_slot_to_stack(&furnace.slots[menu_slot])),
        _ => furnace_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_furnace_menu_stack(
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        0..=2 => {
            furnace.slots[menu_slot] = stack_to_furnace_slot(&stack);
            true
        }
        _ => {
            let Some(slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn can_place_in_furnace_menu_slot(
    state: &InteractionState,
    kind: FurnaceKind,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => find_smelting_recipe_for_item(state, kind, stack.item_id).is_some(),
        1 => is_fuel_item(state, stack.item_id),
        2 => false,
        3..=38 => true,
        _ => false,
    }
}

fn apply_furnace_swap_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || menu_slot == 2 {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if furnace_player_slot(menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = furnace_menu_stack(furnace, &state.inventory, menu_slot) else {
        return false;
    };
    let swap = state.inventory.slots[player_slot].clone();
    let can_place_swap = can_place_in_furnace_menu_slot(state, kind, menu_slot, &swap);
    let can_place_clicked = can_place_in_player_slot(state, player_slot, &clicked);
    let Some((new_clicked, new_swap)) =
        apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
    else {
        return false;
    };
    if !set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, new_clicked) {
        return false;
    }
    state.inventory.slots[player_slot] = new_swap;
    true
}

fn apply_furnace_throw_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || menu_slot == 2 {
        return None;
    }
    let (stack, dropped) = apply_regular_throw_slot(
        furnace_menu_stack(furnace, &state.inventory, menu_slot)?,
        button,
    )?;
    if !set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, stack) {
        return None;
    }
    Some(dropped)
}

fn apply_furnace_pickup_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = furnace_menu_stack(furnace, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = pickup_click_max_stack(state, &slot_stack);

    if menu_slot == 2 && !slot_stack.is_empty() {
        if cursor.is_empty() {
            state.carried_item = slot_stack;
            return set_furnace_menu_stack(
                furnace,
                &mut state.inventory,
                menu_slot,
                ItemStack::EMPTY,
            );
        }
        if can_stack(&cursor, &slot_stack) && cursor.count < max_stack {
            let moved = (max_stack - state.carried_item.count).min(slot_stack.count);
            state.carried_item.count += moved;
            let mut remaining = slot_stack;
            remaining.count -= moved;
            if remaining.count <= 0 {
                remaining = ItemStack::EMPTY;
            }
            return set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, remaining);
        }
        return false;
    }

    let can_place_cursor = can_place_in_furnace_menu_slot(state, kind, menu_slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        &mut state.carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, new_slot)
}

fn merge_stack_into_furnace_slot(
    state: &InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    menu_slot: usize,
    stack: ItemStack,
) -> ItemStack {
    if stack.is_empty() || !can_place_in_furnace_menu_slot(state, kind, menu_slot, &stack) {
        return stack;
    }
    let target = &mut furnace.slots[menu_slot];
    let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
    if target.is_empty() {
        let moved = stack.count.min(max_stack);
        let mut moved_stack = stack.clone();
        moved_stack.count = moved;
        *target = stack_to_furnace_slot(&moved_stack);
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else if can_stack(&furnace_slot_to_stack(target), &stack) && target.count < max_stack {
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

fn apply_furnace_quick_move_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
    menu_slot: usize,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT {
        return false;
    }
    match menu_slot {
        0..=2 => {
            let original = furnace_slot_to_stack(&furnace.slots[menu_slot]);
            if original.is_empty() {
                return false;
            }
            let max_stack = item_max_stack(&state.item_facts, &state.items, &original);
            let (remaining, _) = state.inventory.merge_stack(original.clone(), max_stack);
            furnace.slots[menu_slot] = stack_to_furnace_slot(&remaining);
            remaining != original
        }
        _ => {
            let Some(player_slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            let original = state.inventory.slots[player_slot].clone();
            if original.is_empty() {
                return false;
            }
            let target = if find_smelting_recipe_for_item(state, kind, original.item_id).is_some() {
                Some(0)
            } else if is_fuel_item(state, original.item_id) {
                Some(1)
            } else {
                None
            };
            let Some(target) = target else {
                return false;
            };
            state.inventory.slots[player_slot] = ItemStack::EMPTY;
            let remaining =
                merge_stack_into_furnace_slot(state, furnace, kind, target, original.clone());
            state.inventory.slots[player_slot] = remaining;
            state.inventory.slots[player_slot] != original
        }
    }
}

fn can_place_in_chest_menu_slot(
    state: &InteractionState,
    storage_slots: usize,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    if menu_slot < storage_slots {
        true
    } else {
        chest_player_slot(storage_slots, menu_slot)
            .is_some_and(|slot| can_place_in_player_slot(state, slot, stack))
    }
}

fn apply_chest_pickup_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
    button: i8,
) -> bool {
    let menu_slot_count = view.storage_slots() + PLAYER_CONTAINER_STORAGE_SLOTS;
    if menu_slot >= menu_slot_count || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = chest_menu_stack(view, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = pickup_click_max_stack(state, &slot_stack);
    let can_place_cursor =
        can_place_in_chest_menu_slot(state, view.storage_slots(), menu_slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        &mut state.carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    set_chest_menu_stack(view, &mut state.inventory, menu_slot, new_slot)
}

fn merge_stack_into_chest(
    view: &mut ChestView,
    state: &InteractionState,
    mut stack: ItemStack,
) -> ItemStack {
    let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
    for chest in &mut view.chests {
        for slot in &mut chest.slots {
            if can_stack(&furnace_slot_to_stack(slot), &stack) && slot.count < max_stack {
                let moved = (max_stack - slot.count).min(stack.count);
                slot.count += moved;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }
    }
    for chest in &mut view.chests {
        for slot in &mut chest.slots {
            if !slot.is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            *slot = stack_to_furnace_slot(&moved_stack);
            stack.count -= moved;
            if stack.count <= 0 {
                return ItemStack::EMPTY;
            }
        }
    }
    stack
}

fn apply_chest_quick_move_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
) -> bool {
    let storage_slots = view.storage_slots();
    if menu_slot >= storage_slots + PLAYER_CONTAINER_STORAGE_SLOTS {
        return false;
    }
    if menu_slot < storage_slots {
        let chest_idx = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let local_slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        let original = furnace_slot_to_stack(&view.chests[chest_idx].slots[local_slot]);
        if original.is_empty() {
            return false;
        }
        let max_stack = item_max_stack(&state.item_facts, &state.items, &original);
        let (remaining, _) = state.inventory.merge_stack(original.clone(), max_stack);
        view.chests[chest_idx].slots[local_slot] = stack_to_furnace_slot(&remaining);
        remaining != original
    } else {
        let Some(player_slot) = chest_player_slot(storage_slots, menu_slot) else {
            return false;
        };
        let original = state.inventory.slots[player_slot].clone();
        if original.is_empty() {
            return false;
        }
        state.inventory.slots[player_slot] = ItemStack::EMPTY;
        let remaining = merge_stack_into_chest(view, state, original.clone());
        state.inventory.slots[player_slot] = remaining;
        state.inventory.slots[player_slot] != original
    }
}

fn apply_chest_swap_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
    button: i8,
) -> bool {
    let storage_slots = view.storage_slots();
    if menu_slot >= storage_slots + PLAYER_CONTAINER_STORAGE_SLOTS {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if chest_player_slot(storage_slots, menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = chest_menu_stack(view, &state.inventory, menu_slot) else {
        return false;
    };
    let swap = state.inventory.slots[player_slot].clone();
    let can_place_swap = can_place_in_chest_menu_slot(state, storage_slots, menu_slot, &swap);
    let can_place_clicked = can_place_in_player_slot(state, player_slot, &clicked);
    let Some((new_clicked, new_swap)) =
        apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
    else {
        return false;
    };
    if !set_chest_menu_stack(view, &mut state.inventory, menu_slot, new_clicked) {
        return false;
    }
    state.inventory.slots[player_slot] = new_swap;
    true
}

fn apply_chest_throw_click(
    state: &mut InteractionState,
    view: &mut ChestView,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if menu_slot >= view.storage_slots() + PLAYER_CONTAINER_STORAGE_SLOTS {
        return None;
    }
    let (stack, dropped) =
        apply_regular_throw_slot(chest_menu_stack(view, &state.inventory, menu_slot)?, button)?;
    if !set_chest_menu_stack(view, &mut state.inventory, menu_slot, stack) {
        return None;
    }
    Some(dropped)
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
    let mut view;
    let mut dropped = None;
    let changed;
    {
        let world = Arc::clone(&state.world);
        let mut storage = world.lock().await;
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
        view = ChestView { chests };
        let authoritative_state_id = state.sessions.chest_state_id(window.position());
        if window.state_id != authoritative_state_id {
            window.state_id = authoritative_state_id;
        }
        if packet.state_id != window.state_id {
            drop(storage);
            write_chest_content(state, writer, &window, &view).await?;
            return Ok(window);
        }
        let before_inventory = state.inventory.clone();
        let before_carried_item = state.carried_item.clone();
        let before_view = view.clone();
        changed = match classify_container_click(&packet) {
            ContainerClickAction::Pickup { slot, button } => {
                apply_chest_pickup_click(state, &mut view, slot, button)
            }
            ContainerClickAction::OutsidePickup { button } => {
                dropped = apply_outside_pickup_click(state, button);
                dropped.is_some()
            }
            ContainerClickAction::QuickMove { slot } => {
                apply_chest_quick_move_click(state, &mut view, slot)
            }
            ContainerClickAction::Swap { slot, button } => {
                apply_chest_swap_click(state, &mut view, slot, button)
            }
            ContainerClickAction::Throw { slot, button } => {
                if item_entity_type_id(&state.entity_types).is_some() {
                    dropped = apply_chest_throw_click(state, &mut view, slot, button);
                    dropped.is_some()
                } else {
                    false
                }
            }
            ContainerClickAction::Unsupported => false,
        };
        if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
            state.inventory = before_inventory;
            state.carried_item = before_carried_item;
            view = before_view;
            drop(storage);
            write_chest_content(state, writer, &window, &view).await?;
            return Ok(window);
        }
        if changed {
            window.state_id = window.state_id.wrapping_add(1);
            for (&position, chest) in window.positions.iter().zip(&view.chests) {
                storage
                    .set_chest_block_entity(position, chest.clone())
                    .map_err(|err| {
                        warn!(error = %err, ?position, "chest state write failed");
                        err
                    })?;
            }
        }
    }
    if changed {
        let (state_id, dispatches) = state.sessions.chest_slot_dispatches(
            window.position(),
            state.session_id,
            chest_slot_stacks(&view),
        );
        window.state_id = state_id;
        dispatch_visibility_commands(dispatches);
    }
    if let Some(stack) = dropped {
        dispatch_inventory_drop(state, player_pose, stack);
    }
    write_chest_content(state, writer, &window, &view).await?;
    Ok(window)
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
    let mut furnace;
    let mut dropped = None;
    let changed;
    {
        let world = Arc::clone(&state.world);
        let mut storage = world.lock().await;
        furnace = storage
            .furnace_block_entity(window.position)
            .map_err(|err| {
                warn!(error = %err, ?window.position, "furnace state read failed");
                err
            })?
            .unwrap_or_default();
        let authoritative_state_id = state.sessions.furnace_state_id(window.position);
        if window.state_id != authoritative_state_id {
            window.state_id = authoritative_state_id;
        }
        if packet.state_id != window.state_id {
            drop(storage);
            write_furnace_content(state, writer, &window, &furnace).await?;
            return Ok(window);
        }
        let before_inventory = state.inventory.clone();
        let before_carried_item = state.carried_item.clone();
        let before_furnace = furnace.clone();
        changed = match classify_container_click(&packet) {
            ContainerClickAction::Pickup { slot, button } => {
                apply_furnace_pickup_click(state, &mut furnace, window.kind, slot, button)
            }
            ContainerClickAction::OutsidePickup { button } => {
                dropped = apply_outside_pickup_click(state, button);
                dropped.is_some()
            }
            ContainerClickAction::QuickMove { slot } => {
                apply_furnace_quick_move_click(state, &mut furnace, window.kind, slot)
            }
            ContainerClickAction::Swap { slot, button } => {
                apply_furnace_swap_click(state, &mut furnace, window.kind, slot, button)
            }
            ContainerClickAction::Throw { slot, button } => {
                if item_entity_type_id(&state.entity_types).is_some() {
                    dropped = apply_furnace_throw_click(state, &mut furnace, slot, button);
                    dropped.is_some()
                } else {
                    false
                }
            }
            ContainerClickAction::Unsupported => false,
        };
        if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
            state.inventory = before_inventory;
            state.carried_item = before_carried_item;
            furnace = before_furnace;
            drop(storage);
            write_furnace_content(state, writer, &window, &furnace).await?;
            return Ok(window);
        }
        if changed {
            window.state_id = window.state_id.wrapping_add(1);
            storage
                .set_furnace_block_entity(window.position, furnace.clone())
                .map_err(|err| {
                    warn!(error = %err, ?window.position, "furnace state write failed");
                    err
                })?;
        }
    }
    if changed {
        let (state_id, dispatches) = state.sessions.furnace_slot_dispatches(
            window.position,
            state.session_id,
            furnace_slot_stacks(&furnace),
        );
        window.state_id = state_id;
        dispatch_visibility_commands(dispatches);
    }
    if let Some(stack) = dropped {
        dispatch_inventory_drop(state, player_pose, stack);
    }
    write_furnace_content(state, writer, &window, &furnace).await?;
    Ok(window)
}

fn furnace_output_room(furnace: &FurnaceBlockEntity, item_id: u32, count: i32) -> bool {
    let output = furnace_slot_to_stack(&furnace.slots[2]);
    output.is_empty()
        || output.item_id == item_id && output.damage.is_none() && output.count + count <= 64
}

fn add_furnace_output(furnace: &mut FurnaceBlockEntity, item_id: u32, count: i32) {
    if furnace.slots[2].is_empty() {
        furnace.slots[2] = stack_to_furnace_slot(&ItemStack::new(item_id, count));
    } else {
        furnace.slots[2].count += count;
    }
}

fn decrement_furnace_slot(stack: &mut FurnaceSlot) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = FurnaceSlot::EMPTY;
    }
}

fn tick_furnace(
    state: &InteractionState,
    furnace: &mut FurnaceBlockEntity,
    kind: FurnaceKind,
) -> (bool, Vec<(i16, i16)>) {
    let before_slots = furnace.slots.clone();
    let before_data = furnace_data_values(furnace);

    if furnace.burn_remaining > 0 {
        furnace.burn_remaining -= 1;
    }

    let input = furnace_slot_to_stack(&furnace.slots[0]);
    let recipe = (!input.is_empty())
        .then(|| find_smelting_recipe_for_item(state, kind, input.item_id))
        .flatten();
    let Some(recipe) = recipe else {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    };
    let Some(output_item_id) = state.items.id_of(&recipe.result.item) else {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    };
    let output_count = i32::try_from(recipe.result.count).unwrap_or(0);
    let cooking_time = match &recipe.kind {
        mc_data::recipes::RecipeKind::Smelting(smelting)
        | mc_data::recipes::RecipeKind::Blasting(smelting)
        | mc_data::recipes::RecipeKind::Smoking(smelting)
        | mc_data::recipes::RecipeKind::CampfireCooking(smelting) => smelting.cooking_time,
        _ => DEFAULT_FURNACE_COOK_TICKS as u32,
    };
    furnace.cook_total = i16::try_from(cooking_time)
        .unwrap_or(DEFAULT_FURNACE_COOK_TICKS)
        .max(1);

    if output_count <= 0 || !furnace_output_room(furnace, output_item_id, output_count) {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    }

    if furnace.burn_remaining <= 0
        && !furnace.slots[1].is_empty()
        && is_fuel_item(state, furnace.slots[1].item_id)
    {
        decrement_furnace_slot(&mut furnace.slots[1]);
        furnace.burn_total = FURNACE_FUEL_TICKS;
        furnace.burn_remaining = FURNACE_FUEL_TICKS;
    }

    if furnace.burn_remaining > 0 {
        furnace.cook_progress += 1;
        if furnace.cook_progress >= furnace.cook_total {
            decrement_furnace_slot(&mut furnace.slots[0]);
            add_furnace_output(furnace, output_item_id, output_count);
            furnace.cook_progress = 0;
        }
    } else {
        furnace.cook_progress = 0;
    }

    let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
    (furnace.slots != before_slots, changed_data)
}

fn changed_furnace_data(before: [(i16, i16); 4], after: [(i16, i16); 4]) -> Vec<(i16, i16)> {
    before
        .into_iter()
        .zip(after)
        .filter_map(|(before, after)| (before != after).then_some(after))
        .collect()
}

async fn tick_active_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(active) = state.active_container.take() else {
        return Ok(());
    };
    match active {
        ActiveContainer::Furnace(mut window) => {
            if !state
                .sessions
                .is_furnace_tick_owner(window.position, state.session_id)
            {
                state.active_container = Some(ActiveContainer::Furnace(window));
                return Ok(());
            }
            let mut furnace = {
                let mut storage = state.world.lock().await;
                storage
                    .furnace_block_entity(window.position)
                    .map_err(|err| {
                        warn!(error = %err, ?window.position, "furnace state read failed");
                        err
                    })?
            }
            .unwrap_or_default();
            let (slots_changed, data_changed) = tick_furnace(state, &mut furnace, window.kind);
            if slots_changed || !data_changed.is_empty() {
                let mut storage = state.world.lock().await;
                storage
                    .set_furnace_block_entity(window.position, furnace.clone())
                    .map_err(|err| {
                        warn!(error = %err, ?window.position, "furnace state write failed");
                        err
                    })?;
            }
            if slots_changed {
                let (state_id, dispatches) = state.sessions.furnace_slot_dispatches(
                    window.position,
                    state.session_id,
                    furnace_slot_stacks(&furnace),
                );
                window.state_id = state_id;
                dispatch_visibility_commands(dispatches);
                write_furnace_content(state, writer, &window, &furnace).await?;
            }
            if !data_changed.is_empty() {
                write_furnace_data_changes(writer, state.compression, &window, &data_changed)
                    .await?;
            }
            if !data_changed.is_empty() {
                dispatch_visibility_commands(state.sessions.furnace_data_dispatches(
                    window.position,
                    state.session_id,
                    data_changed,
                ));
            }
            state.active_container = Some(ActiveContainer::Furnace(window));
        }
        ActiveContainer::CraftingTable(window) => {
            state.active_container = Some(ActiveContainer::CraftingTable(window));
        }
        ActiveContainer::Chest(window) => {
            state.active_container = Some(ActiveContainer::Chest(window));
        }
    }
    Ok(())
}

async fn handle_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    packet: ServerboundContainerClick,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    state.pending_use = None;
    clear_shield_use(state);
    if game_mode == GameMode::Spectator
        || matches!(game_mode, GameMode::Survival | GameMode::Adventure) && survival_state.is_dead()
    {
        if packet.container_id == 0 {
            write_inventory_content(state, writer).await?;
        } else if let Some(active) = state.active_container.take() {
            match active {
                ActiveContainer::Furnace(window) => {
                    let furnace = {
                        let mut storage = state.world.lock().await;
                        storage
                            .furnace_block_entity(window.position)
                            .map_err(|err| {
                                warn!(error = %err, ?window.position, "furnace state read failed");
                                err
                            })?
                    }
                    .unwrap_or_default();
                    write_furnace_content(state, writer, &window, &furnace).await?;
                    state.active_container = Some(ActiveContainer::Furnace(window));
                }
                ActiveContainer::CraftingTable(window) => {
                    write_crafting_content(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::CraftingTable(window));
                }
                ActiveContainer::Chest(window) => {
                    let view = load_chest_view(state, &window).await?;
                    write_chest_content(state, writer, &window, &view).await?;
                    state.active_container = Some(ActiveContainer::Chest(window));
                }
            }
        }
        return Ok(());
    }

    if packet.container_id != 0 {
        let Some(active) = state.active_container.take() else {
            write_inventory_content(state, writer).await?;
            return Ok(());
        };
        match active {
            ActiveContainer::CraftingTable(crafting)
                if crafting.container_id == packet.container_id =>
            {
                let crafting =
                    handle_crafting_container_click(state, writer, crafting, player_pose, packet)
                        .await?;
                state.active_container = Some(ActiveContainer::CraftingTable(crafting));
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

    if packet.state_id != state.inventory_state_id {
        debug!(
            client_state = packet.state_id,
            server_state = state.inventory_state_id,
            "container click resynced stale state"
        );
        write_inventory_content(state, writer).await?;
        return Ok(());
    }

    let before_inventory = state.inventory.clone();
    let before_carried_item = state.carried_item.clone();
    let mut dropped = None;
    let changed = match classify_container_click(&packet) {
        ContainerClickAction::Pickup { slot, button } => apply_pickup_click(state, slot, button),
        ContainerClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click(state, button);
            dropped.is_some()
        }
        ContainerClickAction::QuickMove { slot } => apply_quick_move_click(state, slot),
        ContainerClickAction::Swap { slot, button } => apply_swap_click(state, slot, button),
        ContainerClickAction::Throw { slot, button } => {
            if item_entity_type_id(&state.entity_types).is_some() {
                dropped = apply_throw_click(state, slot, button);
                dropped.is_some()
            } else {
                false
            }
        }
        ContainerClickAction::Unsupported => false,
    };
    if !client_carried_item_matches(&packet.carried_item, &state.carried_item) {
        debug!("container click resynced mismatched carried item");
        state.inventory = before_inventory;
        state.carried_item = before_carried_item;
        write_inventory_content(state, writer).await?;
        return Ok(());
    }

    if !changed {
        debug!(
            slot = packet.slot_num,
            button = packet.button_num,
            input = ?packet.container_input,
            "container click unsupported or no-op; resyncing"
        );
    }
    if let Some(stack) = dropped {
        dispatch_inventory_drop(state, player_pose, stack);
    }
    write_inventory_content(state, writer).await
}

async fn pickup_nearby_items<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let player_position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state.sessions.nearby_item_entities(player_position, 2.25);
    for entity in candidates {
        let Some(stack) = entity.item_stack else {
            continue;
        };
        let probe = ItemStack {
            item_id: stack.item_id,
            count: stack.count,
            damage: stack.damage,
        };
        let max_stack = item_max_stack(&state.item_facts, &state.items, &probe);
        let (remaining, changed) = state.inventory.clone().merge_pickup_stack(probe, max_stack);
        let requested = stack.count - remaining.count;
        if requested <= 0 || changed.is_empty() {
            continue;
        }
        let Some(claimed) =
            state
                .sessions
                .claim_item_pickup(entity.id, state.session_id, requested)
        else {
            continue;
        };
        let picked = ItemStack {
            item_id: claimed.stack.item_id,
            count: claimed.stack.count,
            damage: claimed.stack.damage,
        };
        let max_stack = item_max_stack(&state.item_facts, &state.items, &picked);
        let (remaining, changed) = state.inventory.merge_pickup_stack(picked, max_stack);
        debug_assert!(
            remaining.is_empty(),
            "claimed pickup should fit probed inventory space"
        );
        if changed.is_empty() {
            continue;
        }
        write_inventory_slot_updates(state, writer, changed).await?;
        dispatch_visibility_commands(claimed.dispatches);
    }
    Ok(())
}

async fn pickup_nearby_arrows<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let arrow = Identifier::parse("minecraft:arrow").expect("static identifier");
    let Some(item_id) = state.items.id_of(&arrow) else {
        return Ok(());
    };
    let player_position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state.sessions.nearby_grounded_arrows(player_position, 2.25);
    for entity in candidates {
        let probe = ItemStack::new(item_id, 1);
        let max_stack = item_max_stack(&state.item_facts, &state.items, &probe);
        let (remaining, changed) = state.inventory.clone().merge_pickup_stack(probe, max_stack);
        if !remaining.is_empty() || changed.is_empty() {
            continue;
        }
        let Some(dispatches) = state
            .sessions
            .claim_arrow_pickup(entity.id, state.session_id)
        else {
            continue;
        };
        let picked = ItemStack::new(item_id, 1);
        let max_stack = item_max_stack(&state.item_facts, &state.items, &picked);
        let (remaining, changed) = state.inventory.merge_pickup_stack(picked, max_stack);
        debug_assert!(
            remaining.is_empty(),
            "claimed arrow should fit probed inventory space"
        );
        if changed.is_empty() {
            continue;
        }
        write_inventory_slot_updates(state, writer, changed).await?;
        dispatch_visibility_commands(dispatches);
    }
    Ok(())
}

async fn pickup_nearby_xp<W>(
    state: &mut InteractionState,
    writer: &mut W,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let player_position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state
        .sessions
        .nearby_experience_entities(player_position, 2.25);
    let mut changed = false;
    for entity in candidates {
        let Some(value) = entity.experience_value else {
            continue;
        };
        let Some(dispatches) =
            state
                .sessions
                .remove_picked_item(entity.id, state.session_id, value)
        else {
            continue;
        };
        dispatch_visibility_commands(dispatches);
        changed |= xp_state.add_points(value);
    }
    if changed {
        write_packet(writer, &xp_state.as_packet(), state.compression).await?;
    }
    Ok(())
}

async fn handle_interact(
    state: &mut InteractionState,
    packet: ServerboundInteract,
) -> Result<(), ConnectionError> {
    state.pending_break = None;
    debug!(entity_id = packet.entity_id, "entity interaction ignored");
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
    let Some(entity) = state.sessions.server_entity_snapshot(entity_id) else {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for unknown entity"
        );
        return Ok(());
    };
    if game_mode == GameMode::Survival
        && !within_entity_reach(
            player_pose,
            entity.position,
            entity_aabb(&entity.type_name),
            game_mode,
        )
    {
        debug!(
            entity_id = packet.entity_id,
            "survival entity attack ignored: target out of reach"
        );
        return Ok(());
    }
    if entity.item_stack.is_some() {
        return Ok(());
    }
    let now = Instant::now();
    if game_mode == GameMode::Survival
        && state
            .last_entity_attack_at
            .is_some_and(|last| now.duration_since(last) < PLAYER_ENTITY_ATTACK_COOLDOWN)
    {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored during cooldown"
        );
        return Ok(());
    }
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    state.last_entity_attack_at = Some(now);

    let Some(damage) = state
        .sessions
        .damage_server_entity(entity_id, held_attack_damage(state))
    else {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for non-living entity"
        );
        return Ok(());
    };
    if weapon_attacks_damage_held_item(game_mode) {
        damage_held_weapon_after_attack(state, writer).await?;
    }
    if game_mode == GameMode::Survival
        && survival_state.add_exhaustion(SurvivalState::ENTITY_ATTACK_EXHAUSTION)
    {
        write_packet(writer, &survival_state.as_packet(), state.compression).await?;
    }
    if !damage.killed {
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

    let Some((entity, despawn)) = state.sessions.remove_server_entity(entity_id) else {
        debug!(
            entity_id = packet.entity_id,
            "killed entity disappeared before despawn"
        );
        return Ok(());
    };
    dispatch_visibility_commands(despawn);

    if let (Some(drop), Some(entity_type_id)) = (
        mob_drop_stack(state, &entity.type_name),
        item_entity_type_id(&state.entity_types),
    ) {
        dispatch_visibility_commands(state.sessions.spawn_item_drop(
            entity_type_id,
            entity.position,
            entity_item_stack(drop),
        ));
        pickup_nearby_items(state, writer, player_pose).await?;
    }
    if let Some(entity_type_id) = xp_orb_entity_type_id(&state.entity_types) {
        dispatch_visibility_commands(state.sessions.spawn_xp_orb(
            entity_type_id,
            entity.position,
            mob_xp_value(&entity.type_name),
        ));
        pickup_nearby_xp(state, writer, xp_state, player_pose).await?;
    }
    Ok(())
}

async fn complete_block_break<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    position: i64,
    drop_items: bool,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (x, y, z) = unpack_block_pos(position);
    let air = air_state_id(&state.blocks);
    let replacement = break_replacement_state(state, x, y, z, air).await;
    let pos = mc_world::BlockPos { x, y, z };
    let (prev, edits) = {
        let mut storage = state.world.lock().await;
        let prev = storage.get_block(pos).ok().flatten();
        let edits = if prev.is_some_and(|state_id| block_break_is_denied(&state.blocks, state_id)) {
            Vec::new()
        } else {
            prev.map(|state_id| {
                plan_break_block_edits(&state.blocks, &mut storage, pos, state_id, replacement, air)
            })
            .unwrap_or_else(|| {
                vec![BlockEdit {
                    pos,
                    new_state: replacement,
                }]
            })
        };
        (prev, edits)
    };

    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    let outcome = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
    let changed = !outcome.applied.is_empty();
    if changed {
        schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;
        start_falling_blocks_after_edits(state, writer, &outcome.applied).await?;
    }
    if drop_items
        && let (Some(prev), Some(entity_type_id)) = (prev, item_entity_type_id(&state.entity_types))
    {
        let drops = block_drop_stacks(state, prev);
        if !drops.is_empty() {
            for drop in drops {
                dispatch_visibility_commands(state.sessions.spawn_item_drop(
                    entity_type_id,
                    Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5),
                    entity_item_stack(drop),
                ));
            }
            pickup_nearby_items(state, writer, player_pose).await?;
        }
    }
    Ok(changed)
}

fn plan_break_block_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state_id: mc_world::BlockStateId,
    replacement: mc_world::BlockStateId,
    air: mc_world::BlockStateId,
) -> Vec<BlockEdit> {
    let mut edits = vec![BlockEdit {
        pos,
        new_state: replacement,
    }];
    let Some(state) = blocks.by_id(state_id) else {
        return edits;
    };
    if state.block.id.path().ends_with("_door") {
        let other_y = match block_state_property(state, "half") {
            Some("lower") => pos.y + 1,
            Some("upper") => pos.y - 1,
            _ => return edits,
        };
        let other_pos = mc_world::BlockPos { y: other_y, ..pos };
        if let Ok(Some(other_state_id)) = storage.get_block(other_pos)
            && let Some(other_state) = blocks.by_id(other_state_id)
            && other_state.block.id == state.block.id
        {
            edits.push(BlockEdit {
                pos: other_pos,
                new_state: air,
            });
        }
    }
    append_vertical_support_cascade(blocks, storage, &mut edits, pos, air);
    edits
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
    let starts = {
        let mut storage = state.world.lock().await;
        collect_falling_block_starts(
            &state.blocks,
            &state.block_facts,
            &mut storage,
            applied,
            air,
        )
    };
    if starts.is_empty() {
        return Ok(());
    }

    let removal_edits = starts
        .iter()
        .map(|start| BlockEdit {
            pos: start.pos,
            new_state: air,
        })
        .collect::<Vec<_>>();
    let outcome = apply_visible_block_edit_batch(state, writer, &removal_edits).await?;
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

fn collect_falling_block_starts(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    applied: &[AppliedBlockEdit],
    air: mc_world::BlockStateId,
) -> Vec<FallingBlockStart> {
    let mut seen = HashSet::new();
    let mut starts = Vec::new();
    for edit in applied {
        let pos = mc_world::BlockPos {
            y: edit.pos.y + 1,
            ..edit.pos
        };
        if !seen.insert(pos) || !falling_block_can_enter(facts, edit.new_state, air) {
            continue;
        }
        let Ok(Some(state)) = storage.get_block(pos) else {
            continue;
        };
        if is_falling_block_state(blocks, state) {
            starts.push(FallingBlockStart { pos, state });
        }
    }
    starts
}

fn is_falling_block_state(
    blocks: &mc_world::BlockRegistry,
    state_id: mc_world::BlockStateId,
) -> bool {
    blocks.by_id(state_id).is_some_and(|state| {
        matches!(
            state.block.id.path(),
            "sand" | "red_sand" | "gravel" | "anvil" | "chipped_anvil" | "damaged_anvil"
        )
    })
}

fn falling_block_can_enter(
    facts: &mc_data::block_facts::BlockFactsTable,
    state: mc_world::BlockStateId,
    air: mc_world::BlockStateId,
) -> bool {
    state == air || facts.fluid(state.0).is_some()
}

fn append_vertical_support_cascade(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    edits: &mut Vec<BlockEdit>,
    base: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) {
    let mut y = base.y + 1;
    loop {
        let pos = mc_world::BlockPos { y, ..base };
        let Ok(Some(state_id)) = storage.get_block(pos) else {
            break;
        };
        let Some(state) = blocks.by_id(state_id) else {
            break;
        };
        if !is_vertical_support_cascade_block(state.block.id.path()) {
            break;
        }
        edits.push(BlockEdit {
            pos,
            new_state: air,
        });
        y += 1;
    }
}

fn is_vertical_support_cascade_block(path: &str) -> bool {
    matches!(path, "sugar_cane" | "cactus" | "bamboo")
}

/// M5.d/M22.b: handle serverbound block-destroy actions. Creative keeps the
/// historical instant edit path; survival now requires a server-timed start/stop
/// pair before the shared mutation back-half can run.
async fn handle_player_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
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
        if game_mode == GameMode::Survival
            && let Some(pending) = state.pending_use.take()
            && matches!(pending.kind, UseKind::Bow)
            && pending_use_matches(state, &pending)
            && bow_draw_power(pending.started_at) > 0.0
            && let Some(entity_type_id) = arrow_entity_type_id(&state.entity_types)
            && let Some(slot) = consume_arrow(state)
        {
            let power = bow_draw_power(pending.started_at);
            let position = arrow_spawn_position(player_pose);
            let velocity = arrow_velocity(player_pose, power);
            let rotation = Rotation {
                yaw: player_pose.yaw,
                pitch: player_pose.pitch,
                head_yaw: player_pose.yaw,
            };
            dispatch_visibility_commands(state.sessions.spawn_arrow(
                Some(state.session_id),
                entity_type_id,
                position,
                velocity,
                rotation,
            ));
            let slot_value = state.inventory.slots[slot].clone();
            write_inventory_slot_updates(state, writer, vec![(slot, slot_value)]).await?;
            damage_held_bow_after_shot(state, writer).await?;
        } else {
            state.pending_use = None;
        }
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if !is_destroy {
        // DROP_*, RELEASE_USE_ITEM, SWAP_ITEM_WITH_OFFHAND, STAB —
        // all out of scope for M5. Ack so the client doesn't hang
        // on a prediction.
        debug!(
            action = ?action.action,
            sequence = action.sequence,
            "non-destroy player action ignored"
        );
        write_block_ack(writer, state.compression, action.sequence).await?;
        return Ok(());
    }

    if game_mode == GameMode::Survival && survival_state.is_dead() {
        state.pending_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "survival block break ignored for dead player"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    if game_mode == GameMode::Survival
        && !within_block_reach(player_pose, action.position, game_mode)
    {
        state.pending_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "survival block break ignored: target out of reach"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    match game_mode {
        GameMode::Creative => {
            state.pending_break = None;
            state.pending_use = None;
            if matches!(action.action, PlayerActionKind::AbortDestroyBlock) {
                return write_block_ack(writer, state.compression, action.sequence).await;
            }
            complete_block_break(
                state,
                writer,
                action.sequence,
                action.position,
                false,
                player_pose,
            )
            .await
            .map(|_| ())
        }
        GameMode::Survival => match action.action {
            PlayerActionKind::StartDestroyBlock => {
                let required_time = mining_time_for_target(state, action.position).await;
                state.pending_break = Some(PendingBreak {
                    position: action.position,
                    direction: action.direction,
                    started_at: Instant::now(),
                    required_time,
                    held_hotbar_slot: state.selected_hotbar_slot,
                    held_item_id: held_item_id(state),
                });
                write_block_ack(writer, state.compression, action.sequence).await
            }
            PlayerActionKind::AbortDestroyBlock => {
                state.pending_break = None;
                write_block_ack(writer, state.compression, action.sequence).await
            }
            PlayerActionKind::StopDestroyBlock => {
                let can_complete = state.pending_break.as_ref().is_some_and(|pending| {
                    pending_break_matches(state, pending, &action)
                        && pending_break_is_complete(pending, Instant::now())
                });
                state.pending_break = None;
                if can_complete {
                    let changed = complete_block_break(
                        state,
                        writer,
                        action.sequence,
                        action.position,
                        true,
                        player_pose,
                    )
                    .await?;
                    if changed {
                        if survival_state.add_exhaustion(SurvivalState::BLOCK_BREAK_EXHAUSTION) {
                            write_packet(writer, &survival_state.as_packet(), state.compression)
                                .await?;
                        }
                        damage_held_tool_after_mining(state, writer).await
                    } else {
                        Ok(())
                    }
                } else {
                    debug!(
                        sequence = action.sequence,
                        "survival block break rejected before completion"
                    );
                    write_block_resync_then_ack(state, writer, action.position, action.sequence)
                        .await
                }
            }
            _ => write_block_ack(writer, state.compression, action.sequence).await,
        },
        GameMode::Adventure | GameMode::Spectator => {
            state.pending_break = None;
            state.pending_use = None;
            debug!(
                mode = ?game_mode,
                sequence = action.sequence,
                "block break denied outside survival/creative"
            );
            write_block_ack(writer, state.compression, action.sequence).await
        }
    }
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

fn player_horizontal_look_direction(yaw: f32) -> Vec3 {
    let yaw = f64::from(yaw).to_radians();
    Vec3::new(-yaw.sin(), 0.0, yaw.cos())
}

fn shield_hand_slot(
    state: &InteractionState,
    hand: mc_protocol::packets::play::InteractionHand,
) -> usize {
    match hand {
        mc_protocol::packets::play::InteractionHand::MainHand => {
            PlayerInventory::HOTBAR_BASE + state.selected_hotbar_slot as usize
        }
        mc_protocol::packets::play::InteractionHand::OffHand => 45,
    }
}

fn stack_is_shield(state: &InteractionState, stack: &ItemStack) -> bool {
    !stack.is_empty()
        && state
            .items
            .name_of(stack.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:shield")
}

fn shield_use_flags(shield_use: Option<&ShieldUseState>) -> i8 {
    let Some(shield_use) = shield_use else {
        return 0;
    };
    let mut flags = LIVING_ENTITY_FLAG_USING_ITEM;
    if shield_use.hand == InteractionHand::OffHand {
        flags |= LIVING_ENTITY_FLAG_OFF_HAND;
    }
    flags
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
    let slot = shield_hand_slot(state, hand);
    let stack = state.inventory.slots[slot].clone();
    let Some(shield_use) = shield_use_from_stack(
        hand,
        slot,
        stack,
        state.sessions.world_time(),
        stack_is_shield(state, &state.inventory.slots[slot]),
    ) else {
        return false;
    };
    state.pending_break = None;
    state.pending_use = None;
    state.shield_use = Some(shield_use);
    dispatch_shield_use_metadata(state);
    true
}

fn shield_use_from_stack(
    hand: mc_protocol::packets::play::InteractionHand,
    slot: usize,
    stack: ItemStack,
    current_tick: u64,
    is_shield: bool,
) -> Option<ShieldUseState> {
    is_shield.then_some(ShieldUseState {
        hand,
        started_tick: current_tick,
        slot,
        stack,
    })
}

fn refresh_shield_use_state(state: &mut InteractionState) {
    let Some(shield_use) = &state.shield_use else {
        return;
    };
    if shield_hand_slot(state, shield_use.hand) != shield_use.slot
        || state.inventory.slots[shield_use.slot] != shield_use.stack
        || !stack_is_shield(state, &state.inventory.slots[shield_use.slot])
    {
        clear_shield_use(state);
    }
}

fn shield_durability_damage(blocked_damage: f32) -> i32 {
    if blocked_damage < 3.0 {
        return 0;
    }
    if !blocked_damage.is_finite() {
        return i32::MAX;
    }
    let scaled = blocked_damage.max(0.0).floor();
    if scaled >= (i32::MAX - 1) as f32 {
        i32::MAX
    } else {
        (scaled as i32).saturating_add(1).max(1)
    }
}

fn damage_active_shield(
    state: &mut InteractionState,
    blocked_damage: f32,
) -> Option<(usize, ItemStack)> {
    let shield_use = state.shield_use.clone()?;
    if shield_hand_slot(state, shield_use.hand) != shield_use.slot
        || !stack_is_shield(state, &state.inventory.slots[shield_use.slot])
    {
        clear_shield_use(state);
        return None;
    }

    let max_damage = state
        .items
        .name_of(state.inventory.slots[shield_use.slot].item_id)
        .and_then(|item| state.item_facts.get(item))
        .and_then(|facts| facts.max_damage)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(SHIELD_FALLBACK_MAX_DAMAGE)
        .max(1);

    let durability_damage = shield_durability_damage(blocked_damage);
    if durability_damage <= 0 {
        return None;
    }

    let stack = &mut state.inventory.slots[shield_use.slot];
    let next_damage = stack.damage.unwrap_or(0).saturating_add(durability_damage);
    if next_damage >= max_damage {
        *stack = ItemStack::EMPTY;
        clear_shield_use(state);
    } else {
        stack.damage = Some(next_damage);
        if let Some(active) = &mut state.shield_use {
            active.stack = stack.clone();
        }
    }
    Some((
        shield_use.slot,
        state.inventory.slots[shield_use.slot].clone(),
    ))
}

fn shield_blocks_damage(
    player_position: Vec3,
    player_yaw: f32,
    source_origin: Option<Vec3>,
    current_tick: u64,
    activation_delay_ticks: u64,
    shield_use: Option<&ShieldUseState>,
) -> bool {
    let Some(shield_use) = shield_use else {
        return false;
    };
    if current_tick.saturating_sub(shield_use.started_tick) < activation_delay_ticks {
        return false;
    }
    let Some(source_origin) = source_origin else {
        return false;
    };
    let incoming = Vec3::new(
        source_origin.x - player_position.x,
        0.0,
        source_origin.z - player_position.z,
    );
    let incoming_len = (incoming.x * incoming.x + incoming.z * incoming.z).sqrt();
    if incoming_len <= f64::EPSILON {
        return false;
    }
    let look = player_horizontal_look_direction(player_yaw);
    let dot = (look.x * incoming.x + look.z * incoming.z) / incoming_len;
    dot >= SHIELD_FRONT_ARC_DOT_MIN
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
        SHIELD_ACTIVATION_DELAY_TICKS,
        state.shield_use.as_ref(),
    )
}

fn weapon_attacks_damage_held_item(game_mode: GameMode) -> bool {
    game_mode == GameMode::Survival
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

pub(crate) async fn run_random_ticks(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_tick: u64,
) -> RandomTickReport {
    let policy = config.random_tick.normalized();
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

    let table = config.block_light.as_deref();
    let mut sampled = 0usize;
    let mut eligible = 0usize;
    let mut outcome = BlockEditBatchOutcome::default();
    {
        let mut storage = world.lock().await;
        for sample in &samples {
            let Some(state) = storage.get_cached_block(sample.pos) else {
                continue;
            };
            sampled += 1;
            let Some(family) = config.block_facts.random_tick_family(state.0) else {
                continue;
            };
            eligible += 1;
            if let Some(edits) = random_tick_edit(
                &config.blocks,
                &config.block_facts,
                &mut storage,
                sample.pos,
                state,
                family,
            ) {
                for edit in &edits {
                    apply_block_edit_to_storage(&mut storage, table, edit, &mut outcome);
                }
            }
        }
    }

    if !outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates = {
                let mut storage = world.lock().await;
                collect_incremental_light_updates_for_applied_edits(&mut storage, table, &outcome)
            };
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&outcome.light_edit_chunks);
                broadcast_light_updates_to_sessions(sessions, &light_updates, None);
            }
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

pub(crate) async fn run_scheduled_fluid_ticks(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_tick: u64,
) -> ScheduledFluidTickReport {
    let policy = config.random_tick.normalized();
    let Some(world) = config.world.as_ref() else {
        return ScheduledFluidTickReport {
            budget: policy.fluid_tick_budget,
            ..ScheduledFluidTickReport::default()
        };
    };
    let loaded_chunks = sessions.loaded_chunks_sorted();
    if loaded_chunks.is_empty() {
        return ScheduledFluidTickReport {
            budget: policy.fluid_tick_budget,
            ..ScheduledFluidTickReport::default()
        };
    }

    let table = config.block_light.as_deref();
    let mut drained = 0usize;
    let mut outcome = BlockEditBatchOutcome::default();
    {
        let mut storage = world.lock().await;
        for &(cx, cz) in &loaded_chunks {
            if drained >= policy.fluid_tick_budget {
                break;
            }
            let cpos = ChunkPos { x: cx, z: cz };
            let remaining = policy.fluid_tick_budget - drained;
            let due = storage.drain_due_cached_fluid_ticks(cpos, world_tick, remaining);
            drained += due.len();
            for tick in due {
                let Some(state) = storage.get_cached_block(tick.pos) else {
                    continue;
                };
                let Some(fluid) = config.block_facts.fluid(state.0) else {
                    continue;
                };
                if fluid_identifier(fluid.kind) != tick.fluid {
                    continue;
                }
                let edits = fluid_tick_edits(
                    &config.blocks,
                    &config.block_facts,
                    &mut storage,
                    tick.pos,
                    state,
                    fluid,
                );
                for edit in edits {
                    apply_block_edit_to_storage(&mut storage, table, &edit, &mut outcome);
                }
            }
        }
        schedule_fluid_ticks_near_applied(
            &mut storage,
            &config.block_facts,
            world_tick,
            &outcome.applied,
        );
    }
    let budget_exhausted = drained >= policy.fluid_tick_budget;
    if budget_exhausted {
        warn!(
            world_tick,
            drained,
            budget = policy.fluid_tick_budget,
            "scheduled fluid tick budget exhausted"
        );
    }

    if !outcome.applied.is_empty() {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        broadcast_block_deltas_to_sessions(sessions, &outcome.edit_chunks, &outcome.deltas, None);
        if let Some(table) = table
            && !outcome.light_edit_chunks.is_empty()
        {
            let light_updates = {
                let mut storage = world.lock().await;
                collect_incremental_light_updates_for_applied_edits(&mut storage, table, &outcome)
            };
            if !light_updates.is_empty() {
                sessions.invalidate_prepared_chunks(&outcome.light_edit_chunks);
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
        budget: policy.fluid_tick_budget,
        budget_exhausted,
    }
}

fn fluid_tick_edits(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    let mut edits = fluid_interaction_edits(blocks, facts, storage, pos, fluid);
    if !edits.is_empty() {
        return edits;
    }

    if !fluid.source
        && let Some(new_state) = supported_flow_state(blocks, facts, storage, pos, fluid)
        && new_state != state
    {
        edits.push(BlockEdit { pos, new_state });
        return edits;
    }

    edits.extend(fluid_spread_edits(blocks, facts, storage, pos, fluid));
    edits
}

fn fluid_interaction_edits(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    let mut edits = Vec::new();
    for neighbour in fluid_neighbour_positions(pos) {
        let Ok(Some(neighbour_state)) = storage.get_block(neighbour) else {
            continue;
        };
        let Some(other) = facts.fluid(neighbour_state.0) else {
            continue;
        };
        if other.kind == fluid.kind {
            continue;
        }
        match (fluid.kind, other.kind) {
            (FluidKind::Water, FluidKind::Lava) => {
                if let Some(new_state) = lava_contact_result(blocks, other, pos, neighbour) {
                    push_unique_block_edit(
                        &mut edits,
                        BlockEdit {
                            pos: neighbour,
                            new_state,
                        },
                    );
                }
            }
            (FluidKind::Lava, FluidKind::Water) => {
                if let Some(new_state) = lava_contact_result(blocks, fluid, neighbour, pos) {
                    push_unique_block_edit(&mut edits, BlockEdit { pos, new_state });
                }
            }
            _ => {}
        }
    }
    edits
}

fn push_unique_block_edit(edits: &mut Vec<BlockEdit>, edit: BlockEdit) {
    if edits.iter().any(|existing| existing.pos == edit.pos) {
        return;
    }
    edits.push(edit);
}

fn lava_contact_result(
    blocks: &mc_world::BlockRegistry,
    lava: FluidStateFacts,
    water_pos: mc_world::BlockPos,
    lava_pos: mc_world::BlockPos,
) -> Option<mc_world::BlockStateId> {
    if lava.source {
        return named_block_default(blocks, "minecraft:obsidian");
    }
    if water_pos.y > lava_pos.y {
        named_block_default(blocks, "minecraft:stone")
    } else {
        named_block_default(blocks, "minecraft:cobblestone")
    }
}

fn supported_flow_state(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    fluid: FluidStateFacts,
) -> Option<mc_world::BlockStateId> {
    let above = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if storage
        .get_block(above)
        .ok()
        .flatten()
        .and_then(|state| facts.fluid(state.0))
        .is_some_and(|above| above.kind == fluid.kind)
    {
        return fluid_state_with_level(blocks, fluid.kind, 1);
    }

    let next_level = horizontal_fluid_neighbours(pos)
        .into_iter()
        .filter_map(|neighbour| {
            let state = storage.get_block(neighbour).ok().flatten()?;
            let other = facts.fluid(state.0)?;
            (other.kind == fluid.kind && fluid_has_source_path(facts, storage, neighbour, other, 0))
                .then_some(other)
        })
        .map(|other| other.level.saturating_add(1))
        .min();

    match next_level {
        Some(level) if level <= max_flow_level(fluid.kind) => {
            fluid_state_with_level(blocks, fluid.kind, level)
        }
        _ => Some(air_state_id(blocks)),
    }
}

fn fluid_has_source_path(
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    fluid: FluidStateFacts,
    depth: u8,
) -> bool {
    if fluid.source {
        return true;
    }
    if depth > max_flow_level(fluid.kind).saturating_add(1) {
        return false;
    }

    let above = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if storage
        .get_block(above)
        .ok()
        .flatten()
        .and_then(|state| facts.fluid(state.0))
        .is_some_and(|above_fluid| {
            above_fluid.kind == fluid.kind
                && fluid_has_source_path(
                    facts,
                    storage,
                    above,
                    above_fluid,
                    depth.saturating_add(1),
                )
        })
    {
        return true;
    }

    for neighbour in horizontal_fluid_neighbours(pos) {
        let support = storage
            .get_block(neighbour)
            .ok()
            .flatten()
            .and_then(|state| facts.fluid(state.0))
            .and_then(|other| {
                (other.kind == fluid.kind && other.level < fluid.level)
                    .then_some((neighbour, other))
            });
        let Some((support_pos, support_fluid)) = support else {
            continue;
        };
        if fluid_has_source_path(
            facts,
            storage,
            support_pos,
            support_fluid,
            depth.saturating_add(1),
        ) {
            return true;
        }
    }
    false
}

fn fluid_spread_edits(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    let next_level = if fluid.source { 1 } else { fluid.level + 1 };
    if next_level > max_flow_level(fluid.kind) {
        return Vec::new();
    }
    let Some(next_state) = fluid_state_with_level(blocks, fluid.kind, next_level) else {
        return Vec::new();
    };
    let below = mc_world::BlockPos {
        y: pos.y - 1,
        ..pos
    };
    if can_flow_into(blocks, facts, storage, below, fluid.kind, next_level) {
        return vec![BlockEdit {
            pos: below,
            new_state: next_state,
        }];
    }

    horizontal_fluid_neighbours(pos)
        .into_iter()
        .filter(|&target| can_flow_into(blocks, facts, storage, target, fluid.kind, next_level))
        .map(|target| BlockEdit {
            pos: target,
            new_state: next_state,
        })
        .collect()
}

fn can_flow_into(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    kind: FluidKind,
    new_level: u8,
) -> bool {
    let Ok(Some(state)) = storage.get_block(pos) else {
        return false;
    };
    if state == air_state_id(blocks) {
        return true;
    }
    facts
        .fluid(state.0)
        .is_some_and(|fluid| fluid.kind == kind && !fluid.source && fluid.level > new_level)
}

fn schedule_fluid_ticks_near_applied(
    storage: &mut mc_world::WorldStorage,
    facts: &mc_data::block_facts::BlockFactsTable,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) {
    let mut positions = HashSet::new();
    for edit in applied {
        positions.insert(edit.pos);
        for pos in fluid_neighbour_positions(edit.pos) {
            positions.insert(pos);
        }
    }
    for pos in positions {
        let Ok(Some(state)) = storage.get_block(pos) else {
            continue;
        };
        let Some(fluid) = facts.fluid(state.0) else {
            continue;
        };
        let delay = fluid_tick_delay(fluid.kind);
        let _ = storage.schedule_fluid_tick(ScheduledFluidTick::new(
            pos,
            fluid_identifier(fluid.kind),
            world_tick.wrapping_add(delay),
            0,
        ));
    }
}

fn fluid_tick_delay(kind: FluidKind) -> u64 {
    match kind {
        FluidKind::Water => WATER_FLOW_DELAY_TICKS,
        FluidKind::Lava => LAVA_FLOW_DELAY_TICKS,
    }
}

fn max_flow_level(kind: FluidKind) -> u8 {
    match kind {
        FluidKind::Water => 7,
        FluidKind::Lava => 3,
    }
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

fn horizontal_fluid_neighbours(pos: mc_world::BlockPos) -> [mc_world::BlockPos; 4] {
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

fn random_tick_edit(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    family: mc_data::block_facts::RandomTickFamily,
) -> Option<Vec<BlockEdit>> {
    match family {
        mc_data::block_facts::RandomTickFamily::Crop => next_crop_growth_state(blocks, state)
            .map(|new_state| vec![BlockEdit { pos, new_state }])
            .or_else(|| stem_fruit_edits(blocks, storage, pos, state))
            .or_else(|| {
                vertical_plant_growth_edit(blocks, storage, pos, state).map(|edit| vec![edit])
            }),
        mc_data::block_facts::RandomTickFamily::Farmland => {
            next_farmland_state(blocks, facts, storage, pos, state)
                .map(|new_state| vec![BlockEdit { pos, new_state }])
        }
        mc_data::block_facts::RandomTickFamily::Fire => {
            next_fire_state(blocks, state).map(|new_state| vec![BlockEdit { pos, new_state }])
        }
        mc_data::block_facts::RandomTickFamily::Grass => {
            next_grass_edit(blocks, storage, pos, state).map(|edit| vec![edit])
        }
        mc_data::block_facts::RandomTickFamily::Leaves => {
            next_leaf_decay_state(blocks, state).map(|new_state| vec![BlockEdit { pos, new_state }])
        }
        mc_data::block_facts::RandomTickFamily::Sapling => {
            sapling_tree_edits(blocks, storage, pos, state)
        }
    }
}

fn collect_incremental_light_updates_for_applied_edits(
    storage: &mut mc_world::WorldStorage,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> Vec<OutboundLightUpdate> {
    let mut cache = LightCache::new();
    let mut updates = Vec::new();

    for edit in &outcome.applied {
        if !block_edit_changes_light(table, edit.previous, edit.new_state) {
            continue;
        }
        let centre_pos = ChunkPos {
            x: edit.pos.x.div_euclid(16),
            z: edit.pos.z.div_euclid(16),
        };
        seed_background_light_cache(
            &mut cache,
            storage,
            centre_pos,
            &outcome.previous_light_chunks,
        );
        if !cache.contains(centre_pos) {
            warn!(
                x = edit.pos.x,
                y = edit.pos.y,
                z = edit.pos.z,
                "background relight skipped; missing baked light seed"
            );
            continue;
        }

        let mut chunks: HashMap<(i32, i32), Arc<Chunk>> = HashMap::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let pos = ChunkPos {
                    x: centre_pos.x + dx,
                    z: centre_pos.z + dz,
                };
                if let Some(chunk) = storage.cached_chunk_snapshot(pos) {
                    chunks.insert((pos.x, pos.z), chunk);
                }
            }
        }
        if !chunks.contains_key(&(centre_pos.x, centre_pos.z)) {
            continue;
        }

        let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                refs[(dz + 1) as usize][(dx + 1) as usize] = chunks
                    .get(&(centre_pos.x + dx, centre_pos.z + dz))
                    .map(|chunk| chunk.as_ref());
            }
        }

        let touched = apply_block_change_to_light(
            &mut cache,
            &refs,
            table,
            centre_pos,
            edit.pos.x.rem_euclid(16) as u8,
            edit.pos.y,
            edit.pos.z.rem_euclid(16) as u8,
            edit.previous,
            edit.new_state,
        );
        let mut edit_updates = Vec::new();
        for pos in touched {
            let Some(light) = cache.get(pos) else {
                continue;
            };
            let wire = encode_chunk_light(light);
            edit_updates.push(OutboundLightUpdate {
                pos,
                light: light.clone(),
                wire: LightData {
                    sky_y_mask: wire.sky_y_mask,
                    block_y_mask: wire.block_y_mask,
                    empty_sky_y_mask: wire.empty_sky_y_mask,
                    empty_block_y_mask: wire.empty_block_y_mask,
                    sky_updates: wire.sky_updates,
                    block_updates: wire.block_updates,
                },
            });
        }
        merge_light_updates(&mut updates, edit_updates);
    }

    persist_baked_light_updates(storage, &updates);
    updates
}

fn seed_background_light_cache(
    cache: &mut LightCache,
    storage: &mc_world::WorldStorage,
    centre_pos: ChunkPos,
    previous_lights: &HashMap<(i32, i32), ChunkLight>,
) {
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            let pos = ChunkPos {
                x: centre_pos.x + dx,
                z: centre_pos.z + dz,
            };
            if cache.contains(pos) {
                continue;
            }
            if let Some(light) = previous_lights.get(&(pos.x, pos.z)) {
                cache.insert(pos, light.clone());
                continue;
            }
            if let Some(chunk) = storage.cached_chunk_snapshot(pos)
                && let Some(light) = ChunkLight::from_section_lights(&chunk.section_lights)
            {
                cache.insert(pos, light);
            }
        }
    }
}

fn persist_baked_light_update(
    storage: &mut mc_world::WorldStorage,
    pos: ChunkPos,
    light: &ChunkLight,
) {
    if storage.cached_chunk_snapshot(pos).is_none() {
        return;
    }
    match storage.get_chunk_mut(pos) {
        Ok(Some(chunk)) => chunk.set_baked_light(light),
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, cx = pos.x, cz = pos.z, "baked light update write failed");
        }
    }
}

fn persist_baked_light_updates(
    storage: &mut mc_world::WorldStorage,
    updates: &[OutboundLightUpdate],
) {
    for update in updates {
        persist_baked_light_update(storage, update.pos, &update.light);
    }
}

fn next_leaf_decay_state(
    blocks: &mc_world::BlockRegistry,
    state: mc_world::BlockStateId,
) -> Option<mc_world::BlockStateId> {
    let current = blocks.by_id(state)?;
    if !current.block.id.path().ends_with("_leaves") {
        return None;
    }
    if block_state_property(current, "persistent") == Some("true") {
        return None;
    }
    let distance = block_state_property(current, "distance")?
        .parse::<u8>()
        .ok()?;
    (distance >= 7).then(|| air_state_id(blocks))
}

fn next_fire_state(
    blocks: &mc_world::BlockRegistry,
    state: mc_world::BlockStateId,
) -> Option<mc_world::BlockStateId> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() == "minecraft:soul_fire" {
        return Some(air_state_id(blocks));
    }
    if current.block.id.as_str() != "minecraft:fire" {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if age >= 15 {
        return Some(air_state_id(blocks));
    }
    sibling_state_with_property(blocks, current, "age", &(age + 1).to_string())
}

fn next_grass_edit(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<BlockEdit> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:grass_block" {
        return None;
    }
    if !block_above_allows_grass(blocks, storage, pos) {
        let dirt = Identifier::parse("minecraft:dirt").expect("static identifier");
        return blocks.block(&dirt).map(|block| BlockEdit {
            pos,
            new_state: block.default,
        });
    }
    let grass_state = blocks
        .block(&Identifier::parse("minecraft:grass_block").expect("static identifier"))
        .map(|block| block.default)?;
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let target = mc_world::BlockPos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                    z: pos.z + dz,
                };
                if block_is(blocks, storage, target, "minecraft:dirt")
                    && block_above_allows_grass(blocks, storage, target)
                {
                    return Some(BlockEdit {
                        pos: target,
                        new_state: grass_state,
                    });
                }
            }
        }
    }
    None
}

fn block_above_allows_grass(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
) -> bool {
    storage
        .get_cached_block(mc_world::BlockPos {
            x: pos.x,
            y: pos.y + 1,
            z: pos.z,
        })
        .and_then(|state| blocks.by_id(state))
        .is_none_or(|state| {
            matches!(
                state.block.id.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:short_grass"
                    | "minecraft:tall_grass"
            )
        })
}

fn block_is(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    name: &str,
) -> bool {
    storage
        .get_cached_block(pos)
        .and_then(|state| blocks.by_id(state))
        .is_some_and(|state| state.block.id.as_str() == name)
}

fn next_farmland_state(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<mc_world::BlockStateId> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:farmland" {
        return None;
    }
    let moisture = block_state_property(current, "moisture")?
        .parse::<u8>()
        .ok()?;
    if farmland_has_nearby_water(blocks, storage, pos) {
        return (moisture < 7)
            .then(|| farmland_state_with_moisture(blocks, 7))
            .flatten();
    }
    if moisture > 0 {
        return farmland_state_with_moisture(blocks, moisture - 1);
    }
    if farmland_has_crop_above(facts, storage, pos) {
        return None;
    }
    let dirt = Identifier::parse("minecraft:dirt").expect("static identifier");
    blocks.block(&dirt).map(|block| block.default)
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

fn fluid_identifier(kind: FluidKind) -> Identifier {
    Identifier::parse(match kind {
        FluidKind::Water => "minecraft:water",
        FluidKind::Lava => "minecraft:lava",
    })
    .expect("static identifier")
}

fn fluid_state_with_level(
    blocks: &mc_world::BlockRegistry,
    kind: FluidKind,
    level: u8,
) -> Option<mc_world::BlockStateId> {
    blocks.by_name_and_props(
        &fluid_identifier(kind),
        &[("level".to_string(), level.to_string())],
    )
}

fn farmland_state_with_moisture(
    blocks: &mc_world::BlockRegistry,
    moisture: u8,
) -> Option<mc_world::BlockStateId> {
    let farmland = Identifier::parse("minecraft:farmland").expect("static identifier");
    blocks.by_name_and_props(&farmland, &[("moisture".to_string(), moisture.to_string())])
}

fn farmland_has_crop_above(
    facts: &mc_data::block_facts::BlockFactsTable,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
) -> bool {
    storage
        .get_cached_block(mc_world::BlockPos {
            x: pos.x,
            y: pos.y + 1,
            z: pos.z,
        })
        .and_then(|state| facts.random_tick_family(state.0))
        == Some(mc_data::block_facts::RandomTickFamily::Crop)
}

fn farmland_has_nearby_water(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
) -> bool {
    for x in (pos.x - 4)..=(pos.x + 4) {
        for z in (pos.z - 4)..=(pos.z + 4) {
            for y in pos.y..=(pos.y + 1) {
                let Some(state) = storage.get_cached_block(mc_world::BlockPos { x, y, z }) else {
                    continue;
                };
                if blocks
                    .by_id(state)
                    .is_some_and(|state| state.block.id.as_str() == "minecraft:water")
                {
                    return true;
                }
            }
        }
    }
    false
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

async fn player_water_overlap(state: &InteractionState, pose: PlayerPose) -> (bool, bool) {
    let half_width = 0.3;
    let min_x = (pose.x - half_width).floor() as i32;
    let max_x = (pose.x + half_width).floor() as i32;
    let min_z = (pose.z - half_width).floor() as i32;
    let max_z = (pose.z + half_width).floor() as i32;
    let min_y = pose.y.floor() as i32;
    let max_y = (pose.y + 1.8).floor() as i32;
    let eye_pos = mc_world::BlockPos {
        x: pose.x.floor() as i32,
        y: (pose.y + 1.62).floor() as i32,
        z: pose.z.floor() as i32,
    };
    let mut in_water = false;
    let mut eye_in_water = false;
    let mut storage = state.world.lock().await;
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let pos = mc_world::BlockPos { x, y, z };
                let water = storage
                    .get_block(pos)
                    .ok()
                    .flatten()
                    .is_some_and(|state_id| state_is_water(&state.block_facts, state_id));
                in_water |= water;
                eye_in_water |= water && pos == eye_pos;
            }
        }
    }
    (in_water, eye_in_water)
}

fn refresh_player_fall_state(old_pose: PlayerPose, new_pose: &mut PlayerPose) {
    if new_pose.in_water || new_pose.flags.on_ground {
        new_pose.fall_start_y = new_pose.y;
    } else if old_pose.flags.on_ground || old_pose.in_water {
        new_pose.fall_start_y = old_pose.y.max(new_pose.y);
    } else {
        new_pose.fall_start_y = old_pose.fall_start_y.max(new_pose.y);
    }
}

async fn player_pose_collides_with_solid(
    state: Option<&InteractionState>,
    pose: PlayerPose,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    let half_width = 0.3;
    let min_x = (pose.x - half_width).floor() as i32;
    let max_x = (pose.x + half_width).floor() as i32;
    let min_y = pose.y.floor() as i32;
    let max_y = (pose.y + 1.8 - 1.0e-6).floor() as i32;
    let min_z = (pose.z - half_width).floor() as i32;
    let max_z = (pose.z + half_width).floor() as i32;
    let mut storage = state.world.lock().await;
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let solid = storage
                    .get_block(mc_world::BlockPos { x, y, z })
                    .ok()
                    .flatten()
                    .is_some_and(|state_id| player_collision_state_is_solid(state, state_id));
                if solid {
                    return true;
                }
            }
        }
    }
    false
}

fn player_collision_state_is_solid(
    state: &InteractionState,
    state_id: mc_world::BlockStateId,
) -> bool {
    if state.block_facts.fluid(state_id.0).is_some() {
        return false;
    }
    state
        .blocks
        .by_id(state_id)
        .is_some_and(|block_state| !passable_block_name(block_state.block.id.as_str()))
}

async fn correct_player_collision<W>(
    state: Option<&InteractionState>,
    writer: &mut W,
    compression: Compression,
    old_pose: PlayerPose,
    new_pose: PlayerPose,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !player_pose_collides_with_solid(state, new_pose).await {
        return Ok(false);
    }
    let teleport_id = next_player_teleport_id(next_teleport_id);
    send_player_position_sync(writer, compression, teleport_id, old_pose).await?;
    *pending_teleport = Some(PendingTeleport::new(teleport_id, old_pose));
    Ok(true)
}

#[derive(Clone, Copy, Debug)]
struct PendingTeleport {
    id: i32,
    pose: PlayerPose,
    resyncs_sent: u8,
}

impl PendingTeleport {
    fn new(id: i32, pose: PlayerPose) -> Self {
        Self {
            id,
            pose,
            resyncs_sent: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeleportConfirmResult {
    Confirmed,
    Mismatched { expected: i32 },
    Unexpected,
}

fn confirm_pending_teleport(
    pending: &mut Option<PendingTeleport>,
    teleport_id: i32,
) -> TeleportConfirmResult {
    let Some(current) = *pending else {
        return TeleportConfirmResult::Unexpected;
    };
    if current.id != teleport_id {
        return TeleportConfirmResult::Mismatched {
            expected: current.id,
        };
    }
    *pending = None;
    TeleportConfirmResult::Confirmed
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

async fn resync_pending_teleport<W>(
    writer: &mut W,
    compression: Compression,
    pending: PendingTeleport,
    packet: &'static str,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        teleport_id = pending.id,
        packet, "movement ignored until teleport is confirmed; resyncing"
    );
    send_player_position_sync(writer, compression, pending.id, pending.pose).await
}

async fn guard_pending_teleport_movement<W>(
    pending: &mut Option<PendingTeleport>,
    writer: &mut W,
    compression: Compression,
    packet: &'static str,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(current) = pending.as_mut() else {
        return Ok(false);
    };
    if current.resyncs_sent < MAX_PENDING_TELEPORT_RESYNCS {
        current.resyncs_sent += 1;
        resync_pending_teleport(writer, compression, *current, packet).await?;
    } else {
        debug!(
            teleport_id = current.id,
            resyncs_sent = current.resyncs_sent,
            packet,
            "movement ignored until teleport is confirmed; resync already sent"
        );
    }
    Ok(true)
}

fn next_player_teleport_id(next_teleport_id: &mut i32) -> i32 {
    let teleport_id = (*next_teleport_id).max(1);
    *next_teleport_id = if teleport_id == i32::MAX {
        1
    } else {
        teleport_id + 1
    };
    teleport_id
}

fn state_is_water(
    facts: &mc_data::block_facts::BlockFactsTable,
    state_id: mc_world::BlockStateId,
) -> bool {
    facts
        .fluid(state_id.0)
        .is_some_and(|fluid| fluid.kind == FluidKind::Water)
}

fn farmland_trample_pos(old_pose: PlayerPose, new_pose: PlayerPose) -> Option<mc_world::BlockPos> {
    if old_pose.in_water || new_pose.in_water {
        return None;
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground || old_pose.y - new_pose.y <= 0.5 {
        return None;
    }
    Some(mc_world::BlockPos {
        x: new_pose.x.floor() as i32,
        y: (new_pose.y - 0.01).floor() as i32,
        z: new_pose.z.floor() as i32,
    })
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
    let is_farmland = {
        let mut storage = state.world.lock().await;
        storage
            .get_block(pos)
            .ok()
            .flatten()
            .and_then(|state_id| state.blocks.by_id(state_id))
            .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:farmland")
    };
    if is_farmland {
        let _ = apply_visible_block_edit_batch(
            state,
            writer,
            &[BlockEdit {
                pos,
                new_state: dirt_state,
            }],
        )
        .await?;
    }
    Ok(())
}

async fn apply_fall_damage<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    old_pose: PlayerPose,
    new_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if old_pose.in_water || new_pose.in_water {
        return Ok(());
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground {
        return Ok(());
    }
    let damage = fall_damage_amount(old_pose, new_pose);
    if damage <= 0.0 || survival_state.is_dead() {
        return Ok(());
    }
    let was_dead = survival_state.is_dead();
    survival_state.apply_damage(damage);
    write_packet(writer, &survival_state.as_packet(), compression).await?;
    if !was_dead
        && survival_state.is_dead()
        && let Some(state) = state
    {
        state.pending_break = None;
        state.pending_use = None;
        clear_shield_use(state);
        drop_inventory_on_death(state, writer, new_pose).await?;
        reset_xp_on_death(Some(state), xp_state, writer, compression, new_pose).await?;
    }
    Ok(())
}

struct ProjectilePlayerDamage {
    player_pose: PlayerPose,
    amount: f32,
    source_origin: Option<Vec3>,
}

async fn apply_projectile_player_damage<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    game_mode: GameMode,
    damage: ProjectilePlayerDamage,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival || damage.amount <= 0.0 || survival_state.is_dead() {
        return Ok(());
    }
    let mut state = state;
    if let Some(state) = state.as_deref_mut()
        && shield_blocks_current_damage(state, damage.player_pose, damage.source_origin)
    {
        if let Some(changed) = damage_active_shield(state, damage.amount) {
            write_inventory_slot_updates(state, writer, vec![changed]).await?;
        }
        return Ok(());
    }
    let applied_damage = survival_damage_after_armor(state.as_deref(), damage.amount);
    let was_dead = survival_state.is_dead();
    if applied_damage > 0.0 {
        survival_state.apply_damage(applied_damage);
    }
    let armor_changed = if applied_damage > 0.0 {
        state
            .as_deref_mut()
            .map(|state| damage_equipped_armor(state, damage.amount))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if !armor_changed.is_empty()
        && let Some(state) = state.as_deref_mut()
    {
        write_inventory_slot_updates(state, writer, armor_changed).await?;
    }
    write_packet(writer, &survival_state.as_packet(), compression).await?;
    if !was_dead
        && survival_state.is_dead()
        && let Some(state) = state
    {
        state.pending_break = None;
        state.pending_use = None;
        clear_shield_use(state);
        drop_inventory_on_death(state, writer, damage.player_pose).await?;
        reset_xp_on_death(
            Some(state),
            xp_state,
            writer,
            compression,
            damage.player_pose,
        )
        .await?;
    }
    Ok(())
}

fn fall_damage_amount(old_pose: PlayerPose, new_pose: PlayerPose) -> f32 {
    if old_pose.in_water || new_pose.in_water {
        return 0.0;
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground {
        return 0.0;
    }
    ((old_pose.fall_start_y - new_pose.y).max(0.0) - 3.0)
        .floor()
        .max(0.0) as f32
}

async fn interact_with_bed<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    pos: mc_world::BlockPos,
    respawn_pose: &mut PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let clicked = {
        let mut storage = state.world.lock().await;
        match storage.get_block(pos) {
            Ok(Some(state_id)) => state_id,
            Ok(None) => return Ok(false),
            Err(err) => {
                warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "bed use target read failed");
                return write_block_ack(writer, state.compression, sequence)
                    .await
                    .map(|()| true);
            }
        }
    };
    let Some(block_state) = state.blocks.by_id(clicked) else {
        return Ok(false);
    };
    if !block_state.block.id.path().ends_with("_bed") {
        return Ok(false);
    }

    *respawn_pose = bed_respawn_pose(pos, block_state);
    write_block_ack(writer, state.compression, sequence).await?;
    match plan_sleep_skip(
        state.sessions.world_time(),
        state.sessions.active_session_count(),
    ) {
        SleepPlan::SkipTo(new_time) => {
            send_player_pose(
                writer,
                state.compression,
                state.session_id,
                EntityPose::Sleeping,
            )
            .await?;
            state.sessions.set_world_time(new_time);
            send_world_time(writer, state.compression, &state.sessions).await?;
            send_player_pose(
                writer,
                state.compression,
                state.session_id,
                EntityPose::Standing,
            )
            .await?;
            send_command_feedback(
                writer,
                state.compression,
                "Respawn point set; skipped to morning",
            )
            .await?;
        }
        SleepPlan::Daytime => {
            send_command_feedback(writer, state.compression, "Respawn point set").await?;
        }
        SleepPlan::MultiplayerDeferred => {
            send_command_feedback(
                writer,
                state.compression,
                "Respawn point set; multiplayer sleep quorum is not implemented",
            )
            .await?;
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SleepPlan {
    SkipTo(u64),
    Daytime,
    MultiplayerDeferred,
}

fn plan_sleep_skip(world_time: u64, active_sessions: usize) -> SleepPlan {
    if world_time % DAY_LENGTH_TICKS < NIGHT_START_TICK {
        return SleepPlan::Daytime;
    }
    if active_sessions == 1 {
        SleepPlan::SkipTo(next_morning_time(world_time))
    } else {
        SleepPlan::MultiplayerDeferred
    }
}

fn next_morning_time(world_time: u64) -> u64 {
    let day = world_time / DAY_LENGTH_TICKS;
    day.saturating_add(1)
        .saturating_mul(DAY_LENGTH_TICKS)
        .saturating_add(DAY_START_TICK)
}

async fn send_player_pose<W>(
    writer: &mut W,
    compression: Compression,
    session_id: SessionId,
    pose: EntityPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundSetEntityData {
            entity_id: i32::try_from(session_id).unwrap_or(i32::MAX),
            values: vec![EntityDataValue::Pose {
                index: ENTITY_DATA_POSE_INDEX,
                pose,
            }],
        },
        compression,
    )
    .await
}

fn bed_respawn_pose(pos: mc_world::BlockPos, state: &mc_world::BlockState) -> PlayerPose {
    let mut pose = PlayerPose::new(
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 1.0,
        f64::from(pos.z) + 0.5,
    );
    pose.yaw = block_state_property(state, "facing")
        .map(yaw_for_horizontal_facing)
        .unwrap_or(0.0);
    pose
}

fn yaw_for_horizontal_facing(facing: &str) -> f32 {
    match facing {
        "north" => 180.0,
        "south" => 0.0,
        "west" => 90.0,
        "east" => -90.0,
        _ => 0.0,
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
    let edits = {
        let mut storage = state.world.lock().await;
        let clicked = match storage.get_block(pos) {
            Ok(Some(state)) => state,
            Ok(None) => return Ok(false),
            Err(err) => {
                warn!(error = %err, x, y, z, "interactive block read failed");
                return write_block_ack(writer, state.compression, sequence)
                    .await
                    .map(|()| true);
            }
        };
        plan_toggle_block_edits(&state.blocks, &mut storage, pos, clicked)
    };
    let Some(edits) = edits else {
        return Ok(false);
    };
    let _ = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
    Ok(true)
}

fn plan_toggle_block_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state_id: mc_world::BlockStateId,
) -> Option<Vec<BlockEdit>> {
    let state = blocks.by_id(state_id)?;
    if state.block.id.path().ends_with("_door") && block_state_property(state, "half").is_some() {
        return plan_door_toggle_edits(blocks, storage, pos, state);
    }
    if let Some(open) = toggled_bool_state(blocks, state, "open") {
        return Some(vec![BlockEdit {
            pos,
            new_state: open,
        }]);
    }
    toggled_bool_state(blocks, state, "powered").map(|new_state| vec![BlockEdit { pos, new_state }])
}

fn plan_door_toggle_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: &mc_world::BlockState,
) -> Option<Vec<BlockEdit>> {
    let new_open = block_state_property(state, "open")? != "true";
    let mut edits = Vec::with_capacity(2);
    edits.push(BlockEdit {
        pos,
        new_state: sibling_state_with_bool_property(blocks, state, "open", new_open)?,
    });
    let other_y = match block_state_property(state, "half")? {
        "lower" => pos.y + 1,
        "upper" => pos.y - 1,
        _ => return Some(edits),
    };
    let other_pos = mc_world::BlockPos { y: other_y, ..pos };
    if let Ok(Some(other_state_id)) = storage.get_block(other_pos)
        && let Some(other_state) = blocks.by_id(other_state_id)
        && other_state.block.id == state.block.id
        && let Some(new_state) =
            sibling_state_with_bool_property(blocks, other_state, "open", new_open)
    {
        edits.push(BlockEdit {
            pos: other_pos,
            new_state,
        });
    }
    Some(edits)
}

fn toggled_bool_state(
    blocks: &mc_world::BlockRegistry,
    state: &mc_world::BlockState,
    name: &str,
) -> Option<mc_world::BlockStateId> {
    let next = match block_state_property(state, name)? {
        "true" => false,
        "false" => true,
        _ => return None,
    };
    sibling_state_with_bool_property(blocks, state, name, next)
}

fn sample_random_tick_positions(
    policy: RandomTickPolicy,
    world_tick: u64,
    chunks: &[(i32, i32)],
) -> Vec<RandomTickSample> {
    let policy = policy.normalized();
    if !policy.is_enabled() || chunks.is_empty() {
        return Vec::new();
    }
    let chunk_count = chunks.len();
    let budget = policy.chunk_budget.min(chunk_count);
    let start = (world_tick as usize) % chunk_count;
    let mut samples = Vec::with_capacity(budget * policy.random_tick_speed as usize);
    for offset in 0..budget {
        let chunk = chunks[(start + offset) % chunk_count];
        for sample_idx in 0..policy.random_tick_speed {
            let hash = splitmix64(
                policy.seed
                    ^ world_tick
                    ^ ((chunk.0 as i64 as u64) << 32)
                    ^ (chunk.1 as i64 as u64)
                    ^ ((offset as u64) << 48)
                    ^ u64::from(sample_idx),
            );
            let local_x = (hash & 0xF) as i32;
            let local_z = ((hash >> 4) & 0xF) as i32;
            let height = (mc_world::MAX_Y - mc_world::MIN_Y) as u64;
            let y = mc_world::MIN_Y + ((hash >> 8) % height) as i32;
            samples.push(RandomTickSample {
                chunk,
                pos: mc_world::BlockPos {
                    x: chunk.0 * 16 + local_x,
                    y,
                    z: chunk.1 * 16 + local_z,
                },
            });
        }
    }
    samples
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

async fn apply_block_edit_batch_to_world(
    state: &mut InteractionState,
    edits: &[BlockEdit],
) -> BlockEditBatchOutcome {
    let table = state.block_light.as_ref().map(Arc::clone);
    let mut outcome = BlockEditBatchOutcome::default();

    let mut storage = state.world.lock().await;
    for edit in edits {
        apply_block_edit_to_storage(&mut storage, table.as_deref(), edit, &mut outcome);
    }
    drop(storage);

    for applied in &outcome.applied {
        if !replaced_campfire_with_non_campfire(state, applied) {
            continue;
        }
        if state.sessions.clear_campfire_cooking(applied.pos) {
            outcome.cleared_campfires.push(applied.pos);
        }
    }

    outcome
}

fn replaced_campfire_with_non_campfire(state: &InteractionState, edit: &AppliedBlockEdit) -> bool {
    is_campfire_state(state, edit.previous) && !is_campfire_state(state, edit.new_state)
}

fn apply_block_edit_to_storage(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edit: &BlockEdit,
    outcome: &mut BlockEditBatchOutcome,
) {
    let pos = edit.pos;
    let chunk_pos = ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    let previous_light = if table.is_some() {
        match storage.get_chunk(chunk_pos) {
            Ok(Some(chunk)) => ChunkLight::from_section_lights(&chunk.section_lights),
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, cx = chunk_pos.x, cz = chunk_pos.z, "pre-edit baked light read failed");
                None
            }
        }
    } else {
        None
    };
    match storage.set_block_at(pos, edit.new_state) {
        Ok(Some(previous)) if previous != edit.new_state => {
            if let Some(table) = table
                && let Err(err) = storage.update_highest_opaque_at(pos, table)
            {
                warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "highest-opaque heightmap update failed");
            }
            outcome.applied.push(AppliedBlockEdit {
                pos,
                previous,
                new_state: edit.new_state,
            });
            outcome.deltas.push(BlockDelta {
                x: pos.x,
                y: pos.y,
                z: pos.z,
                state_id: edit.new_state,
            });
            let chunk = (pos.x.div_euclid(16), pos.z.div_euclid(16));
            outcome.edit_chunks.insert(chunk);
            let changes_light = table
                .is_some_and(|table| block_edit_changes_light(table, previous, edit.new_state));
            if changes_light {
                outcome.light_edit_chunks.insert(chunk);
                if let Some(light) = previous_light {
                    outcome.previous_light_chunks.entry(chunk).or_insert(light);
                }
            } else if let Some(light) = previous_light {
                match storage.get_chunk_mut(chunk_pos) {
                    Ok(Some(chunk)) => chunk.set_baked_light(&light),
                    Ok(None) => {}
                    Err(err) => {
                        warn!(error = %err, cx = chunk_pos.x, cz = chunk_pos.z, "light-inert edit baked light restore failed");
                    }
                }
            }
        }
        Ok(Some(_)) | Ok(None) => {}
        Err(err) => {
            warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "set_block_at failed; skipping edit");
        }
    }
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

async fn apply_visible_block_edit_batch<W>(
    state: &mut InteractionState,
    writer: &mut W,
    edits: &[BlockEdit],
) -> Result<BlockEditBatchOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let table = state.block_light.as_ref().map(Arc::clone);
    let outcome = apply_block_edit_batch_to_world(state, edits).await;

    if outcome.applied.is_empty() {
        return Ok(outcome);
    }

    state
        .sessions
        .invalidate_prepared_chunks(&outcome.edit_chunks);
    send_block_deltas(writer, state.compression, &outcome.deltas).await?;
    broadcast_block_deltas(
        state,
        &outcome.edit_chunks,
        &outcome.deltas,
        Some(state.session_id),
    );
    for pos in &outcome.cleared_campfires {
        dispatch_campfire_block_entity_update(state, *pos, &CampfireCookingState::default());
    }

    if let Some(table) = table {
        let mut light_updates = Vec::new();
        for edit in &outcome.applied {
            merge_light_updates(
                &mut light_updates,
                collect_incremental_relight(state, &table, edit).await?,
            );
        }
        {
            let mut storage = state.world.lock().await;
            persist_baked_light_updates(&mut storage, &light_updates);
        }
        let light_chunks: HashSet<_> = light_updates
            .iter()
            .map(|update| (update.pos.x, update.pos.z))
            .collect();
        state.sessions.invalidate_prepared_chunks(&light_chunks);
        send_light_updates(state, writer, &light_updates).await?;
        broadcast_light_updates(state, &light_updates, Some(state.session_id));
    }

    Ok(outcome)
}

async fn apply_player_block_edit_batch<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    edits: &[BlockEdit],
) -> Result<BlockEditBatchOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let outcome = apply_visible_block_edit_batch(state, writer, edits).await?;

    if outcome.applied.is_empty() {
        write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
        return Ok(outcome);
    }

    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    Ok(outcome)
}

fn merge_light_updates(target: &mut Vec<OutboundLightUpdate>, updates: Vec<OutboundLightUpdate>) {
    for update in updates {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.pos == update.pos)
        {
            *existing = update;
        } else {
            target.push(update);
        }
    }
}

/// M9: incremental relight. Pulls the post-edit 3×3 chunk
/// neighbourhood out of storage once, runs a bounded BFS that
/// mutates the per-chunk cached light in place, and emits one
/// `LightUpdate` per chunk whose stored arrays changed.
///
/// Falls back to a single-chunk full recompute when the cache
/// hasn't been pre-warmed (e.g. an edit lands before the spawn
/// burst's `build_chunk_packet` got to that chunk) — same coverage
/// as the old `send_relight_around` for the centre tile, but
/// without the 5× cost.
async fn collect_incremental_relight(
    state: &mut InteractionState,
    table: &BlockLightTable,
    edit: &AppliedBlockEdit,
) -> Result<Vec<OutboundLightUpdate>, ConnectionError> {
    let cx = edit.pos.x.div_euclid(16);
    let cz = edit.pos.z.div_euclid(16);

    // 1. Pull the 3×3 chunks around the edit out of storage. The
    //    edit has already been applied, so these are post-edit.
    let mut chunks: HashMap<(i32, i32), Arc<Chunk>> = HashMap::new();
    {
        let storage = state.world.lock().await;
        for dcz in -1i32..=1 {
            for dcx in -1i32..=1 {
                let pos = ChunkPos {
                    x: cx + dcx,
                    z: cz + dcz,
                };
                if let Some(chunk) = storage.cached_chunk_snapshot(pos) {
                    chunks.insert((cx + dcx, cz + dcz), chunk);
                }
            }
        }
    }

    let centre_pos = ChunkPos { x: cx, z: cz };

    // 2. If the edit chunk isn't in the cache yet (rare: edit before
    //    spawn burst reached it), seed it via a single full compute.
    if !state.light_cache.contains(centre_pos) {
        let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                refs[(dz + 1) as usize][(dx + 1) as usize] =
                    chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
            }
        }
        let Some(centre) = refs[1][1] else {
            return Ok(Vec::new()); // edit chunk vanished from storage — nothing to relight
        };
        let _ = centre;
        let light = compute_chunk_light_in(&mut state.workspace, refs, table);
        state.light_cache.insert(centre_pos, light);
    }

    // 3. Build the 3×3 reference array for the incremental update.
    let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            refs[(dz + 1) as usize][(dx + 1) as usize] =
                chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
        }
    }

    let local_x = edit.pos.x.rem_euclid(16) as u8;
    let local_z = edit.pos.z.rem_euclid(16) as u8;

    let touched = apply_block_change_to_light(
        &mut state.light_cache,
        &refs,
        table,
        centre_pos,
        local_x,
        edit.pos.y,
        local_z,
        edit.previous,
        edit.new_state,
    );

    // 4. Collect one LightUpdate per chunk whose cached light changed.
    let mut updates = Vec::new();
    for pos in touched {
        let Some(light) = state.light_cache.get(pos) else {
            continue;
        };
        let wire = encode_chunk_light(light);
        let light_data = LightData {
            sky_y_mask: wire.sky_y_mask,
            block_y_mask: wire.block_y_mask,
            empty_sky_y_mask: wire.empty_sky_y_mask,
            empty_block_y_mask: wire.empty_block_y_mask,
            sky_updates: wire.sky_updates,
            block_updates: wire.block_updates,
        };
        updates.push(OutboundLightUpdate {
            pos,
            light: light.clone(),
            wire: light_data.clone(),
        });
    }
    Ok(updates)
}

fn air_state_id(registry: &mc_world::BlockRegistry) -> mc_world::BlockStateId {
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

async fn break_replacement_state(
    state: &InteractionState,
    x: i32,
    y: i32,
    z: i32,
    air: mc_world::BlockStateId,
) -> mc_world::BlockStateId {
    let pos = mc_world::BlockPos { x, y, z };
    let mut storage = state.world.lock().await;
    let neighbours = [
        (x, y + 1, z),
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ];
    let neighbour_states = neighbours.map(|(x, y, z)| {
        storage
            .get_block(mc_world::BlockPos { x, y, z })
            .ok()
            .flatten()
    });
    for fluid in neighbour_states
        .into_iter()
        .flatten()
        .filter_map(|state_id| state.block_facts.fluid(state_id.0))
    {
        if let Some(flow_state) =
            supported_flow_state(&state.blocks, &state.block_facts, &mut storage, pos, fluid)
        {
            return flow_state;
        }
    }

    if let Some(water) = state.water
        && neighbour_states
            .into_iter()
            .any(|state| state == Some(water))
    {
        return water;
    }

    air
}

async fn handle_bucket_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    direction: Direction,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot).clone();
    if held.is_empty() {
        return Ok(false);
    }

    if let Some(kind) = state.item_to_block.bucket_fluid_kind(held.item_id) {
        let Some(source_state) = state.item_to_block.fluid_source_state(kind) else {
            return Ok(false);
        };
        let Some(empty_bucket) = state.item_to_block.empty_bucket_item() else {
            return Ok(false);
        };
        let (dx, dy, dz) = direction.normal();
        let target = mc_world::BlockPos {
            x: clicked_pos.x + dx,
            y: clicked_pos.y + dy,
            z: clicked_pos.z + dz,
        };
        let air = air_state_id(&state.blocks);
        let target_is_air = {
            let mut storage = state.world.lock().await;
            matches!(storage.get_block(target), Ok(Some(state_id)) if state_id == air)
        };
        if !target_is_air {
            return Ok(false);
        }

        let inventory_update = (game_mode == GameMode::Survival)
            .then(|| plan_bucket_replacement(&state.inventory, held_slot, empty_bucket, 16))
            .flatten();
        if game_mode == GameMode::Survival && inventory_update.is_none() {
            return Ok(false);
        }

        let edits = [BlockEdit {
            pos: target,
            new_state: source_state,
        }];
        let outcome = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
        if outcome.applied.is_empty() {
            return Ok(true);
        }
        schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;
        if let Some((inventory, changed)) = inventory_update {
            state.inventory = inventory;
            write_inventory_slot_updates(state, writer, changed).await?;
        }
        dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
        return Ok(true);
    }

    if Some(held.item_id) != state.item_to_block.empty_bucket_item() {
        return Ok(false);
    }
    let clicked_state = {
        let mut storage = state.world.lock().await;
        match storage.get_block(clicked_pos) {
            Ok(Some(state_id)) => state_id,
            Ok(None) => return Ok(false),
            Err(err) => {
                warn!(error = %err, x = clicked_pos.x, y = clicked_pos.y, z = clicked_pos.z, "bucket pickup read failed");
                return write_block_ack(writer, state.compression, sequence)
                    .await
                    .map(|()| true);
            }
        }
    };
    let Some(fluid) = state.block_facts.fluid(clicked_state.0) else {
        return Ok(false);
    };
    if !fluid.source {
        return Ok(false);
    }
    let Some(filled_bucket) = state.item_to_block.filled_bucket_item(fluid.kind) else {
        return Ok(false);
    };
    let inventory_update = (game_mode == GameMode::Survival)
        .then(|| plan_bucket_replacement(&state.inventory, held_slot, filled_bucket, 1))
        .flatten();
    if game_mode == GameMode::Survival && inventory_update.is_none() {
        return Ok(false);
    }

    let edits = [BlockEdit {
        pos: clicked_pos,
        new_state: air_state_id(&state.blocks),
    }];
    let outcome = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
    if outcome.applied.is_empty() {
        return Ok(true);
    }
    schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;
    if let Some((inventory, changed)) = inventory_update {
        state.inventory = inventory;
        write_inventory_slot_updates(state, writer, changed).await?;
    }
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    Ok(true)
}

fn plan_bucket_replacement(
    inventory: &PlayerInventory,
    held_slot: u8,
    replacement_item: u32,
    _replacement_max_stack: i32,
) -> Option<(PlayerInventory, Vec<(usize, ItemStack)>)> {
    let mut inventory = inventory.clone();
    let wire_slot = PlayerInventory::HOTBAR_BASE + held_slot as usize;
    let mut changed = Vec::new();
    let held = inventory.held_mut(held_slot);
    if held.count > 1 {
        return None;
    }

    *held = ItemStack {
        item_id: replacement_item,
        count: 1,
        damage: None,
    };
    changed.push((wire_slot, held.clone()));
    Some((inventory, changed))
}

async fn schedule_fluid_ticks_for_interaction(
    state: &InteractionState,
    applied: &[AppliedBlockEdit],
) {
    let mut storage = state.world.lock().await;
    let current_tick = state.sessions.simulation_tick();
    schedule_fluid_ticks_near_applied(&mut storage, &state.block_facts, current_tick, applied);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseItemOnOutcome {
    Handled,
    NoOp { reason: UseItemOnNoOpReason },
    PlaceBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseItemOnNoOpReason {
    DeadPlayer,
    UnsupportedGameMode,
    OutOfReach,
    EmptyHeldItem,
    ClickedCellUnavailable,
    TargetBlockedOrUnplaceable,
    PlacementPlanRejected,
}

#[derive(Debug, Clone, Copy)]
struct UseItemOnTarget {
    clicked_pos: mc_world::BlockPos,
    coords: (i32, i32, i32),
}

#[derive(Debug, Clone, Copy)]
enum UseItemOnResyncLookup {
    AuthoritativeLookup,
    LoadedOnly,
}

#[derive(Debug, Clone, Copy)]
struct UseItemOnResyncOptions {
    lookup: UseItemOnResyncLookup,
    resync_held_bucket: bool,
}

impl UseItemOnResyncOptions {
    const AUTHORITATIVE_WITH_BUCKET: Self = Self {
        lookup: UseItemOnResyncLookup::AuthoritativeLookup,
        resync_held_bucket: true,
    };

    const LOADED_ONLY_BLOCKS: Self = Self {
        lookup: UseItemOnResyncLookup::LoadedOnly,
        resync_held_bucket: false,
    };
}

async fn handle_use_item_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    respawn_pose: &mut PlayerPose,
    action: ServerboundUseItemOn,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_use = None;
    clear_shield_use(state);

    let preflight = classify_use_item_on_preflight(game_mode, survival_state, player_pose, &action);
    if let UseItemOnOutcome::NoOp { reason } = preflight
        && reason != UseItemOnNoOpReason::OutOfReach
    {
        return ack_use_item_on_noop(writer, state.compression, action.sequence, reason).await;
    }

    let (cx, cy, cz) = unpack_block_pos(action.position);
    let clicked_pos = mc_world::BlockPos {
        x: cx,
        y: cy,
        z: cz,
    };
    let target = UseItemOnTarget {
        clicked_pos,
        coords: (cx, cy, cz),
    };

    if let UseItemOnOutcome::NoOp {
        reason: UseItemOnNoOpReason::OutOfReach,
    } = preflight
    {
        let (dx, dy, dz) = action.direction.normal();
        return reject_use_item_on_with_resync(
            state,
            writer,
            action.sequence,
            target.clicked_pos,
            mc_world::BlockPos {
                x: cx + dx,
                y: cy + dy,
                z: cz + dz,
            },
            UseItemOnNoOpReason::OutOfReach,
            UseItemOnResyncOptions::LOADED_ONLY_BLOCKS,
        )
        .await;
    }

    match handle_use_item_on_interactions(
        state,
        writer,
        game_mode,
        player_pose,
        respawn_pose,
        &action,
        target,
    )
    .await?
    {
        UseItemOnOutcome::Handled => Ok(()),
        UseItemOnOutcome::NoOp { reason } => {
            ack_use_item_on_noop(writer, state.compression, action.sequence, reason).await
        }
        UseItemOnOutcome::PlaceBlock => {
            handle_block_item_placement(
                state,
                writer,
                player_pose,
                target.clicked_pos,
                &action,
                target.coords,
            )
            .await
        }
    }
}

fn classify_use_item_on_preflight(
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    action: &ServerboundUseItemOn,
) -> UseItemOnOutcome {
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            sequence = action.sequence,
            "survival block placement ignored for dead player"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::DeadPlayer,
        };
    }

    if !matches!(game_mode, GameMode::Creative | GameMode::Survival) {
        debug!(
            mode = ?game_mode,
            sequence = action.sequence,
            "block placement denied outside creative/survival"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::UnsupportedGameMode,
        };
    }

    if game_mode == GameMode::Survival
        && !within_block_reach(player_pose, action.position, game_mode)
    {
        debug!(
            sequence = action.sequence,
            "survival block placement ignored: target out of reach"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        };
    }

    UseItemOnOutcome::PlaceBlock
}

async fn handle_use_item_on_interactions<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    player_pose: PlayerPose,
    respawn_pose: &mut PlayerPose,
    action: &ServerboundUseItemOn,
    target: UseItemOnTarget,
) -> Result<UseItemOnOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (cx, cy, cz) = target.coords;
    if !player_pose.shifting {
        if open_crafting_table_container(state, writer, player_pose, action.sequence, cx, cy, cz)
            .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_furnace_container(state, writer, player_pose, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_chest_container(state, writer, player_pose, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
        if reject_unsupported_survival_station_use(state, writer, action.sequence, cx, cy, cz)
            .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if interact_with_bed(
            state,
            writer,
            action.sequence,
            mc_world::BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
            respawn_pose,
        )
        .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if interact_with_toggle_block(state, writer, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
    }

    if handle_campfire_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.hand,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_bucket_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.direction,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_plant_use_on(
        state,
        writer,
        action.sequence,
        target.clicked_pos,
        player_pose,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }

    Ok(UseItemOnOutcome::PlaceBlock)
}

/// M6.f/M23 follow-up: resolve the placed block via the player's currently-held
/// hotbar slot through the item→block table. Drops the placement silently (still
/// acking) if the held stack is empty, if the held item has no block mapping
/// (e.g. food, tool), or if the target cell is non-air. On success decrements
/// the held stack and emits `ContainerSetSlot` so the client sees the new count.
async fn handle_block_item_placement<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    clicked_pos: mc_world::BlockPos,
    action: &ServerboundUseItemOn,
    (cx, cy, cz): (i32, i32, i32),
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let sequence = action.sequence;

    let (dx, dy, dz) = action.direction.normal();
    let (tx, ty, tz) = (cx + dx, cy + dy, cz + dz);

    let air = air_state_id(&state.blocks);

    // M6.f: resolve the placed block from the held item.
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot).clone();
    if held.is_empty() {
        debug!(
            sequence = action.sequence,
            held_item = held.item_id,
            held_count = held.count,
            "UseItemOn: held item is empty or not placeable; skipping"
        );
        return reject_use_item_on_with_resync(
            state,
            writer,
            sequence,
            clicked_pos,
            mc_world::BlockPos {
                x: tx,
                y: ty,
                z: tz,
            },
            UseItemOnNoOpReason::EmptyHeldItem,
            UseItemOnResyncOptions::AUTHORITATIVE_WITH_BUCKET,
        )
        .await;
    };

    if handle_bonemeal_use_on(
        state,
        writer,
        action.sequence,
        clicked_pos,
        held_slot,
        held.item_id,
    )
    .await?
    {
        return Ok(());
    }

    // Validate: target cell must currently be air. Crop items also
    // inspect the clicked block because seeds place the crop above
    // their supporting soil instead of mapping item name to block name.
    let placement_result = 'placement: {
        let mut storage = state.world.lock().await;
        let clicked = match storage.get_block(mc_world::BlockPos {
            x: cx,
            y: cy,
            z: cz,
        }) {
            Ok(Some(current)) => current,
            Ok(None) => {
                debug!(
                    x = cx,
                    y = cy,
                    z = cz,
                    "UseItemOn clicked cell absent; skipping placement"
                );
                break 'placement Err(UseItemOnNoOpReason::ClickedCellUnavailable);
            }
            Err(err) => {
                warn!(error = %err, x = cx, y = cy, z = cz, "UseItemOn clicked read failed");
                break 'placement Err(UseItemOnNoOpReason::ClickedCellUnavailable);
            }
        };
        let target_is_air = match storage.get_block(mc_world::BlockPos {
            x: tx,
            y: ty,
            z: tz,
        }) {
            Ok(Some(current)) => current == air,
            Ok(None) => false,
            Err(err) => {
                warn!(error = %err, x = tx, y = ty, z = tz, "UseItemOn target read failed");
                false
            }
        };
        if !target_is_air {
            Ok(None)
        } else {
            Ok(state.item_to_block.resolve_for_use_on(
                &state.items,
                held.item_id,
                clicked,
                action.direction,
                &state.blocks,
            ))
        }
    };
    let placed_state = match placement_result {
        Ok(Some(placed_state)) => placed_state,
        Ok(None) => {
            debug!(
                x = tx,
                y = ty,
                z = tz,
                held_item = held.item_id,
                "UseItemOn target invalid or held item not placeable; skipping placement"
            );
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                mc_world::BlockPos {
                    x: tx,
                    y: ty,
                    z: tz,
                },
                UseItemOnNoOpReason::TargetBlockedOrUnplaceable,
                UseItemOnResyncOptions::AUTHORITATIVE_WITH_BUCKET,
            )
            .await;
        }
        Err(reason) => {
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                mc_world::BlockPos {
                    x: tx,
                    y: ty,
                    z: tz,
                },
                reason,
                UseItemOnResyncOptions::AUTHORITATIVE_WITH_BUCKET,
            )
            .await;
        }
    };

    let Some(edits) = plan_place_block_edits(
        state,
        mc_world::BlockPos {
            x: tx,
            y: ty,
            z: tz,
        },
        placed_state,
        player_pose,
        action.direction,
    )
    .await
    else {
        return reject_use_item_on_with_resync(
            state,
            writer,
            sequence,
            clicked_pos,
            mc_world::BlockPos {
                x: tx,
                y: ty,
                z: tz,
            },
            UseItemOnNoOpReason::PlacementPlanRejected,
            UseItemOnResyncOptions::AUTHORITATIVE_WITH_BUCKET,
        )
        .await;
    };
    let outcome = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
    if outcome.applied.is_empty() {
        return Ok(());
    }
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));

    // M6.f: decrement the held stack's count + tell the client the
    // new slot contents. Empty stacks ship as `count == 0`.
    {
        let held = state.inventory.held_mut(held_slot);
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
    }
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    let new_slot_value = state.inventory.held(held_slot).clone();
    write_packet(
        writer,
        &ClientboundContainerSetSlot {
            container_id: 0,
            state_id: state.inventory_state_id,
            slot: (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            item_stack: new_slot_value,
        },
        state.compression,
    )
    .await?;
    if let Some(pos) = placed_sign_edit_position(&state.blocks, &outcome.applied) {
        state.pending_sign_edit = Some(pos);
        write_packet(
            writer,
            &ClientboundOpenSignEditor {
                position: pack_block_pos(pos.x, pos.y, pos.z),
                is_front_text: true,
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

async fn reject_use_item_on_with_resync<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    target_pos: mc_world::BlockPos,
    reason: UseItemOnNoOpReason,
    options: UseItemOnResyncOptions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let held_slot_resync = if options.resync_held_bucket {
        bucket_held_slot_resync(state)
    } else {
        None
    };
    let updates = {
        let mut storage = state.world.lock().await;
        [clicked_pos, target_pos]
            .into_iter()
            .filter_map(|pos| {
                let state_id = match options.lookup {
                    UseItemOnResyncLookup::AuthoritativeLookup => match storage.get_block(pos) {
                        Ok(Some(state_id)) => Some(state_id),
                        Ok(None) => None,
                        Err(err) => {
                            warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "UseItemOn resync read failed");
                            None
                        }
                    },
                    UseItemOnResyncLookup::LoadedOnly => storage.get_cached_block(pos),
                }?;
                Some(BlockUpdate {
                    position: pack_block_pos(pos.x, pos.y, pos.z),
                    state_id: state_id.0 as i32,
                })
            })
            .collect::<Vec<_>>()
    };
    for update in updates {
        write_packet(writer, &update, state.compression).await?;
    }
    if let Some((slot, item_stack)) = held_slot_resync {
        state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
        write_packet(
            writer,
            &ClientboundContainerSetSlot {
                container_id: 0,
                state_id: state.inventory_state_id,
                slot,
                item_stack,
            },
            state.compression,
        )
        .await?;
    }
    ack_use_item_on_noop(writer, state.compression, sequence, reason).await
}

fn bucket_held_slot_resync(state: &InteractionState) -> Option<(i16, ItemStack)> {
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot);
    if held.is_empty() {
        return None;
    }
    let is_bucket = state
        .item_to_block
        .bucket_fluid_kind(held.item_id)
        .is_some()
        || Some(held.item_id) == state.item_to_block.empty_bucket_item();
    is_bucket.then(|| {
        (
            (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            held.clone(),
        )
    })
}

async fn ack_use_item_on_noop<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
    reason: UseItemOnNoOpReason,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(sequence, reason = ?reason, "UseItemOn noop acknowledged");
    write_block_ack(writer, compression, sequence).await
}

async fn ack_use_item_noop<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
    reason: &'static str,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(sequence, reason, "UseItem noop acknowledged");
    write_block_ack(writer, compression, sequence).await
}

async fn handle_bonemeal_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    held_slot: u8,
    held_item_id: u32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let bone_meal = Identifier::parse("minecraft:bone_meal").expect("static identifier");
    if state.items.id_of(&bone_meal) != Some(held_item_id) {
        return Ok(false);
    }

    let edits = {
        let mut storage = state.world.lock().await;
        match storage.get_block(clicked_pos) {
            Ok(Some(current)) => {
                bonemeal_growth_edits(&state.blocks, &mut storage, clicked_pos, current)
            }
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, x = clicked_pos.x, y = clicked_pos.y, z = clicked_pos.z, "bonemeal target read failed");
                None
            }
        }
    };

    let Some(edits) = edits else {
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(true);
    };

    let outcome = apply_player_block_edit_batch(state, writer, sequence, &edits).await?;
    if outcome.applied.is_empty() {
        return Ok(true);
    }

    let slot = PlayerInventory::HOTBAR_BASE + held_slot as usize;
    let slot_value = consume_bonemeal_after_growth(&mut state.inventory, held_slot, true)
        .expect("growth succeeded");
    write_inventory_slot_updates(state, writer, vec![(slot, slot_value)]).await?;
    Ok(true)
}

async fn handle_plant_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    position: mc_world::BlockPos,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let plan = {
        let mut storage = state.world.lock().await;
        match storage.get_block(position) {
            Ok(Some(current)) => {
                sweet_berry_harvest(&state.blocks, &state.items, position, current)
            }
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, x = position.x, y = position.y, z = position.z, "plant use target read failed");
                None
            }
        }
    };
    let Some((edit, drop)) = plan else {
        return Ok(false);
    };

    let outcome = apply_player_block_edit_batch(state, writer, sequence, &[edit]).await?;
    if outcome.applied.is_empty() {
        return Ok(true);
    }

    let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
        debug!("plant harvest drop ignored: item entity type unavailable");
        return Ok(true);
    };
    dispatch_visibility_commands(state.sessions.spawn_item_drop(
        entity_type_id,
        Vec3::new(
            position.x as f64 + 0.5,
            position.y as f64 + 0.5,
            position.z as f64 + 0.5,
        ),
        entity_item_stack(drop),
    ));
    pickup_nearby_items(state, writer, player_pose).await?;
    Ok(true)
}

fn consume_bonemeal_after_growth(
    inventory: &mut PlayerInventory,
    held_slot: u8,
    grew: bool,
) -> Option<ItemStack> {
    if !grew {
        return None;
    }
    let held = inventory.held_mut(held_slot);
    held.count = held.count.saturating_sub(1);
    if held.count <= 0 {
        *held = ItemStack::EMPTY;
    }
    Some(inventory.held(held_slot).clone())
}

async fn plan_place_block_edits(
    state: &InteractionState,
    pos: mc_world::BlockPos,
    placed_state: mc_world::BlockStateId,
    player_pose: PlayerPose,
    direction: Direction,
) -> Option<Vec<BlockEdit>> {
    let placed = state.blocks.by_id(placed_state)?;
    if let Some(new_state) = sign_placement_state(&state.blocks, placed, player_pose, direction) {
        return Some(vec![BlockEdit { pos, new_state }]);
    }
    if !placed.block.id.path().ends_with("_door") {
        let air = air_state_id(&state.blocks);
        let mut storage = state.world.lock().await;
        if placed.block.id.path() == "cactus"
            && cactus_has_side_neighbor(&state.blocks, &mut storage, pos)
        {
            return None;
        }
        let mut edits = vec![BlockEdit {
            pos,
            new_state: placed_state,
        }];
        append_cactus_side_neighbor_cascades(
            &state.blocks,
            &mut storage,
            &mut edits,
            pos,
            placed_state,
            air,
        );
        return Some(edits);
    }
    let upper_pos = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let air = air_state_id(&state.blocks);
    let upper_is_air = {
        let mut storage = state.world.lock().await;
        matches!(storage.get_block(upper_pos), Ok(Some(state_id)) if state_id == air)
    };
    if !upper_is_air {
        return None;
    }
    let facing = horizontal_facing_from_yaw(player_pose.yaw);
    let lower = door_half_state(&state.blocks, placed, "lower", facing)?;
    let upper = door_half_state(&state.blocks, placed, "upper", facing)?;
    Some(vec![
        BlockEdit {
            pos,
            new_state: lower,
        },
        BlockEdit {
            pos: upper_pos,
            new_state: upper,
        },
    ])
}

fn append_cactus_side_neighbor_cascades(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    edits: &mut Vec<BlockEdit>,
    placed: mc_world::BlockPos,
    placed_state: mc_world::BlockStateId,
    air: mc_world::BlockStateId,
) {
    if blocks
        .by_id(placed_state)
        .is_none_or(|state| !is_known_cactus_side_obstructor(state.block.id.path()))
    {
        return;
    }

    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let mut y = placed.y;
        loop {
            let pos = mc_world::BlockPos {
                x: placed.x + dx,
                y,
                z: placed.z + dz,
            };
            let Ok(Some(state_id)) = storage.get_block(pos) else {
                break;
            };
            let Some(state) = blocks.by_id(state_id) else {
                break;
            };
            if state.block.id.path() != "cactus" {
                break;
            }
            push_unique_block_edit(
                edits,
                BlockEdit {
                    pos,
                    new_state: air,
                },
            );
            y += 1;
        }
    }
}

fn cactus_has_side_neighbor(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    placed: mc_world::BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let pos = mc_world::BlockPos {
                x: placed.x + dx,
                y: placed.y,
                z: placed.z + dz,
            };
            let Ok(Some(state_id)) = storage.get_block(pos) else {
                return false;
            };
            blocks
                .by_id(state_id)
                .is_some_and(|state| state.block.id.path() == "cactus")
        })
}

fn is_known_cactus_side_obstructor(path: &str) -> bool {
    matches!(
        path,
        "stone"
            | "granite"
            | "polished_granite"
            | "diorite"
            | "polished_diorite"
            | "andesite"
            | "polished_andesite"
            | "deepslate"
            | "cobbled_deepslate"
            | "tuff"
            | "calcite"
            | "dripstone_block"
            | "grass_block"
            | "dirt"
            | "coarse_dirt"
            | "podzol"
            | "rooted_dirt"
            | "mud"
            | "clay"
            | "sand"
            | "red_sand"
            | "gravel"
            | "cobblestone"
            | "mossy_cobblestone"
            | "obsidian"
            | "crying_obsidian"
            | "bedrock"
            | "netherrack"
            | "basalt"
            | "smooth_basalt"
            | "blackstone"
            | "end_stone"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
    ) || path.ends_with("_planks")
        || path.ends_with("_log")
        || path.ends_with("_wood")
        || path.ends_with("_stem")
        || path.ends_with("_hyphae")
        || path.ends_with("_leaves")
        || path.ends_with("_wool")
        || path.ends_with("_terracotta")
        || path.ends_with("_concrete")
        || path.ends_with("_concrete_powder")
}

fn sign_placement_state(
    blocks: &mc_world::BlockRegistry,
    state: &mc_world::BlockState,
    player_pose: PlayerPose,
    direction: Direction,
) -> Option<mc_world::BlockStateId> {
    let path = state.block.id.path();
    if path.ends_with("_wall_sign") {
        return direction_to_horizontal_facing(direction)
            .and_then(|facing| sibling_state_with_property(blocks, state, "facing", facing));
    }
    if path.ends_with("_sign") && !path.ends_with("_hanging_sign") {
        return sibling_state_with_property(
            blocks,
            state,
            "rotation",
            &sign_rotation_from_yaw(player_pose.yaw).to_string(),
        );
    }
    None
}

fn placed_sign_edit_position(
    blocks: &mc_world::BlockRegistry,
    applied: &[AppliedBlockEdit],
) -> Option<mc_world::BlockPos> {
    applied.iter().find_map(|edit| {
        blocks
            .by_id(edit.new_state)
            .filter(|state| is_editable_sign_state(state))
            .map(|_| edit.pos)
    })
}

fn is_editable_sign_state(state: &mc_world::BlockState) -> bool {
    let path = state.block.id.path();
    path.ends_with("_sign") && !path.ends_with("_hanging_sign")
}

async fn handle_sign_update<W>(
    state: &mut InteractionState,
    writer: &mut W,
    packet: ServerboundSignUpdate,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (x, y, z) = unpack_block_pos(packet.position);
    let pos = mc_world::BlockPos { x, y, z };
    if state.pending_sign_edit != Some(pos) {
        debug!(
            ?pos,
            "sign update ignored without matching open editor state"
        );
        return Ok(());
    }
    let is_sign = {
        let mut storage = state.world.lock().await;
        match storage.get_block(pos) {
            Ok(Some(state_id)) => state
                .blocks
                .by_id(state_id)
                .is_some_and(is_editable_sign_state),
            Ok(None) => false,
            Err(err) => {
                warn!(error = %err, ?pos, "sign update target read failed");
                false
            }
        }
    };
    if !is_sign {
        debug!(?pos, "sign update ignored for non-sign block");
        return Ok(());
    }

    let update_tag = sign_block_entity_update_nbt(&packet.lines, packet.is_front_text);
    let persistent_tag = sign_block_entity_persistent_nbt(pos, &update_tag);
    let mut persistent_bytes = Vec::new();
    mc_nbt::write_network(&mut persistent_bytes, &persistent_tag)
        .map_err(mc_protocol::CodecError::from)?;
    {
        let mut storage = state.world.lock().await;
        if let Err(err) = storage.set_opaque_block_entity(pos, persistent_bytes) {
            warn!(error = %err, ?pos, "sign block entity save failed");
        }
    }

    write_packet(
        writer,
        &ClientboundBlockEntityData {
            position: packet.position,
            block_entity_type: SIGN_BLOCK_ENTITY_TYPE_ID,
            nbt: update_tag.clone(),
        },
        state.compression,
    )
    .await?;
    dispatch_visibility_commands(state.sessions.block_entity_data_dispatches(
        pos,
        Some(state.session_id),
        SIGN_BLOCK_ENTITY_TYPE_ID,
        update_tag,
    ));
    state.pending_sign_edit = None;
    debug!(?pos, lines = ?packet.lines, front = packet.is_front_text, "sign update accepted");
    Ok(())
}

fn sign_block_entity_update_nbt(lines: &[String], is_front_text: bool) -> Tag {
    let text = sign_text_nbt(lines);
    let empty = sign_text_nbt(&[]);
    Tag::Compound(vec![
        (
            "front_text".into(),
            if is_front_text {
                text.clone()
            } else {
                empty.clone()
            },
        ),
        ("back_text".into(), if is_front_text { empty } else { text }),
        ("is_waxed".into(), Tag::Byte(0)),
    ])
}

fn sign_block_entity_persistent_nbt(pos: mc_world::BlockPos, update_tag: &Tag) -> Tag {
    let Tag::Compound(fields) = update_tag else {
        unreachable!("sign update tag is always a compound")
    };
    let mut fields = fields.clone();
    fields.extend([
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("id".into(), Tag::String("minecraft:sign".into())),
    ]);
    Tag::Compound(fields)
}

fn sign_text_nbt(lines: &[String]) -> Tag {
    let messages = (0..4)
        .map(|idx| Tag::String(lines.get(idx).cloned().unwrap_or_default()))
        .collect();
    Tag::Compound(vec![
        (
            "messages".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: messages,
            }),
        ),
        ("color".into(), Tag::String("black".into())),
        ("has_glowing_text".into(), Tag::Byte(0)),
    ])
}

fn direction_to_horizontal_facing(direction: Direction) -> Option<&'static str> {
    match direction {
        Direction::North => Some("north"),
        Direction::South => Some("south"),
        Direction::West => Some("west"),
        Direction::East => Some("east"),
        Direction::Down | Direction::Up => None,
    }
}

fn sign_rotation_from_yaw(yaw: f32) -> u8 {
    ((yaw.rem_euclid(360.0) / 22.5).round() as i32).rem_euclid(16) as u8
}

fn door_half_state(
    blocks: &mc_world::BlockRegistry,
    state: &mc_world::BlockState,
    half: &str,
    facing: &str,
) -> Option<mc_world::BlockStateId> {
    let mut props = state.properties.clone();
    set_prop_if_present(&mut props, "half", half);
    set_prop_if_present(&mut props, "facing", facing);
    set_prop_if_present(&mut props, "open", "false");
    set_prop_if_present(&mut props, "powered", "false");
    blocks.by_name_and_props(&state.block.id, &props)
}

fn set_prop_if_present(props: &mut [(String, String)], name: &str, value: &str) {
    if let Some((_, current)) = props.iter_mut().find(|(key, _)| key == name) {
        *current = value.to_string();
    }
}

fn horizontal_facing_from_yaw(yaw: f32) -> &'static str {
    match ((yaw.rem_euclid(360.0) / 90.0).round() as i32).rem_euclid(4) {
        0 => "south",
        1 => "west",
        2 => "north",
        _ => "east",
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

    if action.hand != mc_protocol::packets::play::InteractionHand::MainHand {
        return ack_use_item_noop(writer, state.compression, action.sequence, "offhand").await;
    }

    if is_bow_item(state) {
        let Some(held_item_id) = held_item_id(state) else {
            return ack_use_item_noop(
                writer,
                state.compression,
                action.sequence,
                "bow_without_item",
            )
            .await;
        };
        state.pending_break = None;
        state.pending_use = Some(PendingUse {
            started_at: Instant::now(),
            required_time: Duration::from_secs(60),
            held_hotbar_slot: state.selected_hotbar_slot,
            held_item_id,
            kind: UseKind::Bow,
        });
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    if survival_state.food >= SurvivalState::MAX_FOOD {
        return ack_use_item_noop(writer, state.compression, action.sequence, "full_food").await;
    }

    let Some((held_item_id, rule, required_time)) = held_food_use(state) else {
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
        started_at: Instant::now(),
        required_time,
        held_hotbar_slot: state.selected_hotbar_slot,
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
    survival_state.add_food(food_rule.food, food_rule.saturation);
    let held_slot = state.selected_hotbar_slot;
    {
        let held = state.inventory.held_mut(held_slot);
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
    }
    let slot = PlayerInventory::HOTBAR_BASE + held_slot as usize;
    let slot_value = state.inventory.held(held_slot).clone();
    write_inventory_slot_updates(state, writer, vec![(slot, slot_value)]).await?;
    write_packet(writer, &survival_state.as_packet(), state.compression).await
}

async fn tick_pending_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
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
    if pending_use_is_complete(&pending, Instant::now()) {
        state.pending_use = None;
        complete_food_use(state, writer, survival_state, pending).await?;
    }
    Ok(())
}

async fn tick_hostile_pressure<W>(
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
    if game_mode != GameMode::Survival || survival_state.is_dead() {
        return Ok(());
    }
    let now = Instant::now();
    if state
        .last_hostile_damage_at
        .is_some_and(|last| now.duration_since(last) < HOSTILE_MELEE_COOLDOWN)
    {
        return Ok(());
    }
    let player_position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let Some(hostile) = state
        .sessions
        .nearby_hostile_entities(
            player_position,
            HOSTILE_MELEE_RANGE + HOSTILE_MELEE_VERTICAL_REACH,
        )
        .into_iter()
        .find(|hostile| hostile_can_melee_player(hostile, player_position))
    else {
        return Ok(());
    };
    write_packet(
        writer,
        &EntityAnimation {
            entity_id: hostile.id.0,
            action: EntityAnimationAction::SwingMainHand,
        },
        state.compression,
    )
    .await?;
    let attack_damage = hostile.attack_damage.unwrap_or(3.0) as f32;
    let blocked_by_shield =
        shield_blocks_current_damage(state, player_pose, Some(hostile.position));
    let damage = if blocked_by_shield {
        0.0
    } else {
        survival_damage_after_armor(Some(state), attack_damage)
    };
    let was_dead = survival_state.is_dead();
    if damage > 0.0 {
        survival_state.apply_damage(damage);
    }
    state.last_hostile_damage_at = Some(now);
    let armor_changed = if blocked_by_shield {
        damage_active_shield(state, attack_damage)
            .into_iter()
            .collect()
    } else if damage > 0.0 {
        damage_equipped_armor(state, attack_damage)
    } else {
        Vec::new()
    };
    if !armor_changed.is_empty() {
        write_inventory_slot_updates(state, writer, armor_changed).await?;
    }
    write_packet(writer, &survival_state.as_packet(), state.compression).await?;
    if !was_dead && survival_state.is_dead() {
        state.pending_break = None;
        state.pending_use = None;
        clear_shield_use(state);
        drop_inventory_on_death(state, writer, player_pose).await?;
        reset_xp_on_death(
            Some(state),
            xp_state,
            writer,
            state.compression,
            player_pose,
        )
        .await?;
    }
    Ok(())
}

fn hostile_can_melee_player(hostile: &ServerEntitySnapshot, player_position: Vec3) -> bool {
    if (player_position.y - hostile.position.y).abs() > HOSTILE_MELEE_VERTICAL_REACH {
        return false;
    }
    let to_player = Vec3::new(
        player_position.x - hostile.position.x,
        0.0,
        player_position.z - hostile.position.z,
    );
    let distance = (to_player.x * to_player.x + to_player.z * to_player.z).sqrt();
    if distance > HOSTILE_MELEE_RANGE {
        return false;
    }
    if distance < 0.05 {
        return true;
    }
    let speed =
        (hostile.velocity.x * hostile.velocity.x + hostile.velocity.z * hostile.velocity.z).sqrt();
    if speed < 0.01 {
        return false;
    }
    let dot =
        (hostile.velocity.x * to_player.x + hostile.velocity.z * to_player.z) / (speed * distance);
    dot > 0.35
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
    sessions: &SessionRegistry,
    session_id: SessionId,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    game_mode: GameMode,
    player_pose: &mut PlayerPose,
    movement: AcceptedAbsoluteMovement,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
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
        next_teleport_id,
        pending_teleport,
    )
    .await?
    {
        *player_pose = old_pose;
        return Ok(());
    }

    *player_pose = new_pose;
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
    dispatch_visibility_commands(sessions.update_pose(session_id, *player_pose));
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

fn sync_player_persistence(
    player_save_state: &Option<Arc<Mutex<PlayerPersistedState>>>,
    pose: PlayerPose,
    respawn_pose: PlayerPose,
    interaction: Option<&InteractionState>,
    survival: SurvivalState,
    xp: XpState,
    game_mode: GameMode,
) {
    let Some(player_save_state) = player_save_state else {
        return;
    };
    let mut state = player_save_state.lock().unwrap_or_else(|poisoned| {
        warn!("player persistence mutex was poisoned during sync; recovering state");
        poisoned.into_inner()
    });
    state.pose = pose;
    state.spawn = SpawnState::from_pose(respawn_pose);
    state.survival = survival;
    state.xp = xp;
    state.game_mode = game_mode;
    if let Some(interaction) = interaction {
        state.inventory = interaction.inventory.clone();
        state.selected_hotbar_slot = interaction.selected_hotbar_slot;
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

#[allow(clippy::too_many_arguments)]
async fn play_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    mut interaction: Option<&mut InteractionState>,
    mut chunk_stream: Option<ChunkStreamState>,
    sessions: Arc<SessionRegistry>,
    config: &ServerConfig,
    session_id: SessionId,
    mut player_pose: PlayerPose,
    mut respawn_pose: PlayerPose,
    respawn: ClientboundRespawn,
    permissions: CommandPermissions,
    mut survival_state: SurvivalState,
    mut xp_state: XpState,
    mut game_mode: GameMode,
    player_save_state: Option<Arc<Mutex<PlayerPersistedState>>>,
    mut outbound_rx: mpsc::Receiver<OutboundCommand>,
    server_view_distance: i32,
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
    let mut survival_ticker = interval(Duration::from_secs(1));
    survival_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    survival_ticker.tick().await;
    let mut furnace_ticker = interval(ENTITY_TICK_PERIOD);
    furnace_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    furnace_ticker.tick().await;
    let mut world_time_ticker = interval(WORLD_TIME_SYNC_PERIOD);
    world_time_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    world_time_ticker.tick().await;

    let mut next_id: i64 = 0;
    let mut last_response_at = Instant::now();
    let mut pending_id: Option<i64> = None;
    let mut survival_tick: u32 = 0;
    let mut next_teleport_id: i32 = 2;
    let mut pending_teleport = Some(PendingTeleport::new(1, player_pose));
    let mut client_brand: Option<String> = None;
    let mut client_preferences: Option<ClientPreferences> = None;
    let mut effective_client_view_distance = server_view_distance;
    let mut pending_outbound = VecDeque::new();
    send_world_time(writer, compression, &sessions).await?;
    write_packet(writer, &survival_state.as_packet(), compression).await?;
    write_packet(writer, &xp_state.as_packet(), compression).await?;

    loop {
        sync_player_persistence(
            &player_save_state,
            player_pose,
            respawn_pose,
            interaction.as_deref(),
            survival_state,
            xp_state.clone(),
            game_mode,
        );
        let mut stream_finished = false;
        if let (Some(stream), Some(state)) = (chunk_stream.as_mut(), interaction.as_deref_mut())
            && !stream.is_complete()
        {
            for _ in 0..CHUNK_STREAM_STEPS_PER_TURN {
                if stream.is_complete() {
                    stream_finished = true;
                    break;
                }
                match stream.step(writer, &mut state.light_cache).await? {
                    ChunkStreamStep::Progress => {
                        stream_finished = stream.is_complete();
                        if !stream_finished {
                            tokio::task::yield_now().await;
                        }
                    }
                    ChunkStreamStep::Complete => {
                        stream_finished = true;
                        break;
                    }
                }
            }
        }
        if stream_finished {
            if let Some(stream) = chunk_stream.as_mut() {
                stream.log_summary_once();
            }
            last_response_at = Instant::now();
            pending_id = None;
        }

        tokio::select! {
            command = recv_outbound_command(&mut outbound_rx, &mut pending_outbound) => {
                match command {
                    Some(OutboundCommand::BlockDeltas(deltas)) => {
                        let deltas = collect_block_delta_batch(deltas, &mut outbound_rx, &mut pending_outbound);
                        send_block_deltas(writer, compression, &deltas).await?;
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
                    Some(OutboundCommand::UpdateEntityData(entity)) => {
                        send_entity_data(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::MoveEntityRelative(movement)) => {
                        send_entity_relative_move(writer, compression, &movement).await?;
                    }
                    Some(OutboundCommand::EntityEvent { entity_id, event_id }) => {
                        write_packet(writer, &EntityEvent { entity_id, event_id }, compression)
                            .await?;
                    }
                    Some(OutboundCommand::DamagePlayer {
                        amount,
                        source_origin,
                    }) => {
                        apply_projectile_player_damage(
                            interaction.as_deref_mut(),
                            writer,
                            compression,
                            &mut survival_state,
                            &mut xp_state,
                            game_mode,
                            ProjectilePlayerDamage {
                                player_pose,
                                amount,
                                source_origin,
                            },
                        )
                        .await?;
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
                            && let Some(ActiveContainer::Furnace(mut window)) = state.active_container.take()
                        {
                            if window.position == position {
                                window.state_id = state_id;
                                write_container_slots(
                                    writer,
                                    compression,
                                    window.container_id,
                                    window.state_id,
                                    slots.iter().cloned(),
                                )
                                .await?;
                            }
                            state.active_container = Some(ActiveContainer::Furnace(window));
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
                            && let Some(ActiveContainer::Chest(mut window)) = state.active_container.take()
                        {
                            if window.position() == position {
                                window.state_id = state_id;
                                write_container_slots(
                                    writer,
                                    compression,
                                    window.container_id,
                                    window.state_id,
                                    slots.iter().cloned(),
                                )
                                .await?;
                            }
                            state.active_container = Some(ActiveContainer::Chest(window));
                        }
                    }
                    None => {}
                }
            }
            _ = ticker.tick(), if chunk_stream.as_ref().is_none_or(ChunkStreamState::is_complete) => {
                if last_response_at.elapsed() > KEEPALIVE_TIMEOUT {
                    warn!(
                        elapsed_ms = last_response_at.elapsed().as_millis() as u64,
                        "client missed keepalive deadline; closing"
                    );
                    return Ok(());
                }
                next_id = next_id.wrapping_add(1).max(1);
                pending_id = Some(next_id);
                write_packet(
                    writer,
                    &ClientboundKeepAlive { id: next_id },
                    compression,
                )
                .await?;
            }
            _ = survival_ticker.tick() => {
                if game_mode == GameMode::Survival {
                    survival_tick = survival_tick.wrapping_add(1);
                    let was_dead = survival_state.is_dead();
                    if survival_state.tick_health(survival_tick) {
                        if survival_state.is_dead()
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            state.pending_break = None;
                            state.pending_use = None;
                            clear_shield_use(state);
                        }
                        write_packet(writer, &survival_state.as_packet(), compression).await?;
                        if !was_dead
                            && survival_state.is_dead()
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            clear_shield_use(state);
                            drop_inventory_on_death(state, writer, player_pose).await?;
                            reset_xp_on_death(
                                Some(state),
                                &mut xp_state,
                                writer,
                                compression,
                                player_pose,
                            )
                            .await?;
                        }
                    }
                }
            }
            _ = furnace_ticker.tick() => {
                if let Some(state) = interaction.as_deref_mut() {
                    tick_active_container(state, writer).await?;
                    tick_campfire_cooking(state).await;
                    tick_pending_use(state, writer, game_mode, &mut survival_state).await?;
                    tick_hostile_pressure(
                        state,
                        writer,
                        game_mode,
                        &mut survival_state,
                        &mut xp_state,
                        player_pose,
                    )
                    .await?;
                    pickup_nearby_items(state, writer, player_pose).await?;
                    pickup_nearby_arrows(state, writer, player_pose).await?;
                    pickup_nearby_xp(state, writer, &mut xp_state, player_pose).await?;
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
                if frame.id == ServerboundKeepAlive::ID {
                    let mut body = frame.body;
                    let echo = ServerboundKeepAlive::decode(&mut body)?;
                    if pending_id == Some(echo.id) {
                        last_response_at = Instant::now();
                        pending_id = None;
                    } else {
                        warn!(
                            expected = ?pending_id,
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
                    if guard_pending_teleport_movement(
                        &mut pending_teleport,
                        writer,
                        compression,
                        "ServerboundMovePlayerPos",
                    )
                    .await?
                    {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPos::decode(&mut body)?;
                    handle_accepted_absolute_movement(
                        writer,
                        compression,
                        &mut interaction,
                        &mut chunk_stream,
                        sessions.as_ref(),
                        session_id,
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
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundMovePlayerPosRot::ID {
                    if guard_pending_teleport_movement(
                        &mut pending_teleport,
                        writer,
                        compression,
                        "ServerboundMovePlayerPosRot",
                    )
                    .await?
                    {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPosRot::decode(&mut body)?;
                    handle_accepted_absolute_movement(
                        writer,
                        compression,
                        &mut interaction,
                        &mut chunk_stream,
                        sessions.as_ref(),
                        session_id,
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
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundMovePlayerRot::ID {
                    if guard_pending_teleport_movement(
                        &mut pending_teleport,
                        writer,
                        compression,
                        "ServerboundMovePlayerRot",
                    )
                    .await?
                    {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerRot::decode(&mut body)?;
                    player_pose.yaw = movement.yaw;
                    player_pose.pitch = movement.pitch;
                    player_pose.flags = movement.flags;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
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
                    if guard_pending_teleport_movement(
                        &mut pending_teleport,
                        writer,
                        compression,
                        "ServerboundMovePlayerStatusOnly",
                    )
                    .await?
                    {
                        continue;
                    }
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerStatusOnly::decode(&mut body)?;
                    player_pose.flags = movement.flags;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundPlayerAction::ID {
                    let mut body = frame.body;
                    let action = ServerboundPlayerAction::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_player_action(
                            state,
                            writer,
                            game_mode,
                            &mut survival_state,
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
                            send_player_pose(writer, compression, session_id, EntityPose::Standing)
                                .await?;
                        }
                        _ => {}
                    }
                    refresh_player_water_state(interaction.as_deref(), &mut player_pose).await;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundPlayerInput::ID {
                    let mut body = frame.body;
                    let input = ServerboundPlayerInput::decode(&mut body)?.input;
                    player_pose.input = input;
                    player_pose.sprinting = input.sprint;
                    player_pose.shifting = input.shift;
                    refresh_player_water_state(interaction.as_deref(), &mut player_pose).await;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundUseItemOn::ID {
                    let mut body = frame.body;
                    let use_on = ServerboundUseItemOn::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item_on(
                            state,
                            writer,
                            game_mode,
                            survival_state,
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
                    if let Some(state) = interaction.as_deref_mut() {
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
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_interact(state, interact).await?;
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
                        handle_place_recipe(state, writer, game_mode, survival_state, recipe).await?;
                    } else {
                        debug!(
                            recipe = recipe.recipe_display_id,
                            "PlaceRecipe ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundContainerClick::ID {
                    let mut body = frame.body;
                    let click = ServerboundContainerClick::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_container_click(
                            state,
                            writer,
                            game_mode,
                            survival_state,
                            player_pose,
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
                        let should_store = state
                                .active_container
                                .as_ref()
                                .is_some_and(|active| active.container_id() == close.container_id);
                        if should_store {
                            store_active_container(state, player_pose);
                        } else if close.container_id == 0 {
                            store_inventory_crafting_inputs(state, player_pose);
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
                    let slot = pick.slot.clamp(0, 8) as u8;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.pending_break = None;
                        state.pending_use = None;
                        clear_shield_use(state);
                        state.selected_hotbar_slot = slot;
                        debug!(slot, "hotbar selection updated");
                    }
                } else if frame.id == ServerboundClientCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundClientCommand::decode(&mut body)?;
                    handle_client_command(
                        writer,
                        compression,
                        interaction.as_deref_mut(),
                        &mut chunk_stream,
                        &mut player_pose,
                        respawn_pose,
                        &mut survival_state,
                        &respawn,
                        &mut next_teleport_id,
                        &mut pending_teleport,
                        command,
                    )
                    .await?;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
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
                            client_brand = Some(brand);
                        }
                        PlayCustomPayloadAction::Ignored {
                            channel,
                            payload_len,
                        } => {
                            debug!(channel = %channel, len = payload_len, "custom payload ignored");
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
                    debug!("client reported player loaded");
                } else if frame.id == ServerboundCommandSuggestion::ID {
                    let mut body = frame.body;
                    let request = ServerboundCommandSuggestion::decode(&mut body)?;
                    let suggestions = command_suggestions(&request.command, permissions);
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
                } else if frame.id == ServerboundChatCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundChatCommand::decode(&mut body)?;
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
                        session_id,
                        interaction.as_deref_mut(),
                        &mut player_pose,
                        &mut chunk_stream,
                        &mut next_teleport_id,
                        &mut pending_teleport,
                    )
                    .await?;
                } else if frame.id == ServerboundChangeGameMode::ID {
                    let mut body = frame.body;
                    let command = ServerboundChangeGameMode::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.pending_break = None;
                        state.pending_use = None;
                    }
                    apply_game_mode(writer, compression, &mut game_mode, command.mode, permissions).await?;
                } else {
                    debug!(
                        id = format!("{:#04x}", frame.id),
                        "play packet ignored"
                    );
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(1)), if chunk_stream.as_ref().is_some_and(|stream| !stream.is_complete()) => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_player_command<W>(
    writer: &mut W,
    compression: Compression,
    raw: &str,
    permissions: CommandPermissions,
    game_mode: &mut GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    session_id: SessionId,
    mut interaction: Option<&mut InteractionState>,
    player_pose: &mut PlayerPose,
    chunk_stream: &mut Option<ChunkStreamState>,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let command = match parse_admin_command(raw, permissions) {
        Ok(command) => command,
        Err(err) => {
            send_command_feedback(writer, compression, command_error_message(err)).await?;
            debug!(command = %raw, error = ?err, "command rejected");
            return Ok(());
        }
    };

    match command {
        AdminCommand::GameMode(mode) => {
            if let Some(state) = interaction.as_deref_mut() {
                state.pending_break = None;
                state.pending_use = None;
            }
            apply_game_mode(writer, compression, game_mode, mode, permissions).await?;
            send_command_feedback(writer, compression, &format!("Set game mode to {mode:?}")).await
        }
        AdminCommand::Give { item, count } => {
            match apply_give_command(writer, interaction.as_deref_mut(), &item, count).await? {
                Ok(message) => send_command_feedback(writer, compression, &message).await,
                Err(message) => send_command_feedback(writer, compression, message).await,
            }
        }
        AdminCommand::SaveAll => {
            let report = crate::server::save_all(config, sessions).await;
            if report.is_ok() {
                send_command_feedback(
                    writer,
                    compression,
                    &format!(
                        "Saved {} players, {} entities, {} chunks",
                        report.players_saved, report.entities_saved, report.chunks_flushed
                    ),
                )
                .await
            } else {
                warn!(errors = report.errors.len(), "save-all command failed");
                send_command_feedback(writer, compression, "Save-all failed; see server log").await
            }
        }
        AdminCommand::Stop => {
            let report = crate::server::save_all(config, sessions).await;
            if report.is_ok() {
                config.shutdown.request();
                send_command_feedback(writer, compression, "Saved all state; stopping server")
                    .await?;
            } else {
                warn!(errors = report.errors.len(), "stop command save-all failed");
                send_command_feedback(writer, compression, "Stop aborted; save-all failed").await?;
            }
            Ok(())
        }
        AdminCommand::Teleport { x, y, z } => {
            let old_center = player_pose.chunk_pos();
            player_pose.x = x;
            player_pose.y = y;
            player_pose.z = z;
            if let Some(state) = interaction.as_deref_mut() {
                state.pending_break = None;
                state.pending_use = None;
            }
            let new_center = player_pose.chunk_pos();
            dispatch_visibility_commands(sessions.update_pose(session_id, *player_pose));
            let teleport_id = next_player_teleport_id(next_teleport_id);
            send_player_position_sync(writer, compression, teleport_id, *player_pose).await?;
            *pending_teleport = Some(PendingTeleport::new(teleport_id, *player_pose));
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
            send_command_feedback(writer, compression, &format!("Teleported to {x} {y} {z}")).await
        }
        AdminCommand::Kill => {
            let was_dead = survival_state.is_dead();
            survival_state.apply_damage(10000.0);
            write_packet(writer, &survival_state.as_packet(), compression).await?;
            if !was_dead
                && survival_state.is_dead()
                && let Some(state) = interaction.as_deref_mut()
            {
                state.pending_break = None;
                state.pending_use = None;
                clear_shield_use(state);
                drop_inventory_on_death(state, writer, *player_pose).await?;
                reset_xp_on_death(
                    interaction.as_deref(),
                    xp_state,
                    writer,
                    compression,
                    *player_pose,
                )
                .await?;
            }
            send_command_feedback(writer, compression, "Killed player").await
        }
        AdminCommand::Summon { entity, x, y, z } => {
            let Some(state) = interaction.as_deref_mut() else {
                send_command_feedback(
                    writer,
                    compression,
                    "Cannot summon before play state is ready",
                )
                .await?;
                return Ok(());
            };
            let Some(entity_type_id) = state.entity_types.id_of(&entity) else {
                send_command_feedback(writer, compression, "Unknown entity type").await?;
                return Ok(());
            };
            let position = Vec3::new(
                x.unwrap_or(player_pose.x),
                y.unwrap_or(player_pose.y),
                z.unwrap_or(player_pose.z),
            );
            dispatch_visibility_commands(sessions.spawn_command_entity(
                i32::try_from(entity_type_id).unwrap_or(i32::MAX),
                entity.to_string(),
                position,
            ));
            send_command_feedback(writer, compression, &format!("Summoned {entity}")).await
        }
        AdminCommand::TimeSet(time) => {
            sessions.set_world_time(time);
            send_world_time(writer, compression, sessions).await?;
            send_command_feedback(writer, compression, &format!("Set time to {time}")).await
        }
        AdminCommand::Debug(command) => {
            apply_debug_command(
                writer,
                compression,
                survival_state,
                xp_state,
                interaction,
                *player_pose,
                command,
                permissions,
            )
            .await?;
            send_command_feedback(writer, compression, "Debug command executed").await
        }
    }
}

async fn apply_give_command<W>(
    writer: &mut W,
    interaction: Option<&mut InteractionState>,
    item: &Identifier,
    count: i32,
) -> Result<Result<String, &'static str>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(state) = interaction else {
        debug!(%item, count, "give command rejected: no interaction state");
        return Ok(Err("Cannot give items before play state is ready"));
    };
    let Some(item_id) = state.items.id_of(item) else {
        debug!(%item, "give command rejected: item not in registry");
        return Ok(Err("Unknown item"));
    };
    let stack = ItemStack::new(item_id, count);
    let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
    let mut candidate = state.inventory.clone();
    let (leftover, changed) = candidate.merge_stack(stack, max_stack);
    if !leftover.is_empty() {
        debug!(%item, count, "give command rejected: inventory full");
        return Ok(Err("Not enough inventory space"));
    }
    state.inventory = candidate;
    write_inventory_slot_updates(state, writer, changed).await?;
    Ok(Ok(format!("Gave {count} of {item}")))
}

async fn send_command_feedback<W>(
    writer: &mut W,
    compression: Compression,
    message: &str,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundSystemChat {
            content_nbt: text_component_nbt(message)?,
            overlay: false,
        },
        compression,
    )
    .await
}

async fn send_world_time<W>(
    writer: &mut W,
    compression: Compression,
    sessions: &SessionRegistry,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &clientbound_session_world_time(sessions),
        compression,
    )
    .await
}

fn clientbound_session_world_time(sessions: &SessionRegistry) -> ClientboundSetTime {
    clientbound_world_time(sessions.world_time())
}

fn clientbound_world_time(time: u64) -> ClientboundSetTime {
    ClientboundSetTime {
        game_time: i64::try_from(time).unwrap_or(i64::MAX),
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

fn command_error_message(error: CommandError) -> &'static str {
    match error {
        CommandError::Unknown => "Unknown command",
        CommandError::PermissionDenied => "You do not have permission to use that command",
        CommandError::Usage(usage) => usage,
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_debug_command<W>(
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    mut interaction: Option<&mut InteractionState>,
    player_pose: PlayerPose,
    command: DebugCommand,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.op {
        debug!(command = ?command, "debug command denied for non-op player");
        return Ok(());
    }

    match command {
        DebugCommand::Survival(command) => {
            let result = apply_survival_command(
                writer,
                compression,
                survival_state,
                xp_state,
                interaction.as_deref_mut(),
                player_pose,
                command,
            )
            .await;
            if survival_state.is_dead()
                && let Some(state) = interaction.as_mut()
            {
                state.pending_break = None;
            }
            result
        }
        DebugCommand::Give {
            item,
            count,
            hotbar_slot,
        } => {
            let Some(state) = interaction else {
                debug!(%item, "debug give ignored — no interaction state");
                return Ok(());
            };
            let stack = if count <= 0 {
                ItemStack::EMPTY
            } else {
                let Some(item_id) = state.items.id_of(&item) else {
                    debug!(%item, "debug give ignored — item not in registry");
                    return Ok(());
                };
                ItemStack::new(item_id, count.min(i32::from(u8::MAX)))
            };
            state.inventory.set_hotbar(hotbar_slot, stack.clone());
            state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
            write_packet(
                writer,
                &ClientboundContainerSetSlot {
                    container_id: 0,
                    state_id: state.inventory_state_id,
                    slot: (PlayerInventory::HOTBAR_BASE + hotbar_slot as usize) as i16,
                    item_stack: stack,
                },
                compression,
            )
            .await
        }
    }
}

async fn apply_survival_command<W>(
    writer: &mut W,
    compression: Compression,
    state: &mut SurvivalState,
    xp_state: &mut XpState,
    mut interaction: Option<&mut InteractionState>,
    player_pose: PlayerPose,
    command: SurvivalCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut armor_changed = Vec::new();
    let was_dead = state.is_dead();
    match command {
        SurvivalCommand::Damage(amount) => {
            state.apply_damage(survival_damage_after_armor(interaction.as_deref(), amount));
            if amount > 0.0
                && let Some(interaction) = interaction.as_deref_mut()
            {
                armor_changed = damage_equipped_armor(interaction, amount);
            }
        }
        SurvivalCommand::Heal(amount) => state.heal(amount),
        SurvivalCommand::Feed { food, saturation } => state.add_food(food, saturation),
        SurvivalCommand::Exhaust(amount) => {
            state.add_exhaustion(amount);
        }
    }
    if state.is_dead() {
        debug!("player survival state reached death threshold");
    }
    write_packet(writer, &state.as_packet(), compression).await?;
    if !armor_changed.is_empty()
        && let Some(interaction) = interaction.as_deref_mut()
    {
        write_inventory_slot_updates(interaction, writer, armor_changed).await?;
    }
    if !was_dead
        && state.is_dead()
        && let Some(interaction) = interaction
    {
        interaction.pending_break = None;
        interaction.pending_use = None;
        clear_shield_use(interaction);
        drop_inventory_on_death(interaction, writer, player_pose).await?;
        reset_xp_on_death(
            Some(interaction),
            xp_state,
            writer,
            compression,
            player_pose,
        )
        .await?;
    }
    Ok(())
}

async fn apply_game_mode<W>(
    writer: &mut W,
    compression: Compression,
    current: &mut GameMode,
    requested: GameMode,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.can_change_game_mode() {
        debug!(mode = ?requested, "gamemode change denied for non-op player");
        return Ok(());
    }
    if *current == requested {
        return Ok(());
    }
    *current = requested;
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_CHANGE_GAME_MODE,
            value: requested.id() as f32,
        },
        compression,
    )
    .await?;
    write_packet(writer, &player_abilities_for_mode(requested), compression).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_client_command<W>(
    writer: &mut W,
    compression: Compression,
    interaction: Option<&mut InteractionState>,
    chunk_stream: &mut Option<ChunkStreamState>,
    player_pose: &mut PlayerPose,
    respawn_pose: PlayerPose,
    survival_state: &mut SurvivalState,
    respawn: &ClientboundRespawn,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
    command: ServerboundClientCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    match command.action {
        ClientCommandAction::PerformRespawn => {
            if !survival_state.is_dead() {
                return Ok(());
            }
            *survival_state = SurvivalState::FULL;
            if let Some(state) = interaction {
                state.pending_break = None;
            }
            *player_pose = respawn_pose;
            write_packet(writer, respawn, compression).await?;
            write_packet(
                writer,
                &GameEvent {
                    event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
                    value: 0.0,
                },
                compression,
            )
            .await?;
            write_packet(
                writer,
                &SetCenterChunk {
                    chunk_x: respawn_pose.chunk_pos().0,
                    chunk_z: respawn_pose.chunk_pos().1,
                },
                compression,
            )
            .await?;
            if let Some(stream) = chunk_stream.as_mut() {
                stream.replay_current_view(respawn_pose.yaw);
            }
            let teleport_id = next_player_teleport_id(next_teleport_id);
            send_player_position_sync(writer, compression, teleport_id, *player_pose).await?;
            *pending_teleport = Some(PendingTeleport::new(teleport_id, *player_pose));
            write_packet(writer, &survival_state.as_packet(), compression).await
        }
        ClientCommandAction::RequestStats | ClientCommandAction::RequestGameruleValues => {
            debug!(action = ?command.action, "client command ignored");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
