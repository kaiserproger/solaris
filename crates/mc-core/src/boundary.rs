//! Tick boundary contract between the core and the edge (network) layer.
//!
//! The edge decodes packets off-tick into [`PlayerInput`] values and submits
//! them; the core applies everything submitted before a tick starts in that
//! tick's input step. After commit the core returns a [`TickOutput`]: the
//! immutable snapshot of tick `t + 1` and the [`ChangeSet`] that produced it.
//! The edge builds every outbound packet from these two values only; it never
//! reads or writes core state any other way.

use std::sync::Arc;

pub use crate::entity_views::EntityViews;
use mc_data::item_stack::ItemStack;
use mc_script::ScriptClientViewKey;
pub use mc_world::chunk::{BlockPos, Chunk, ChunkPos};
use uuid::Uuid;

/// Simulation tick number. Tick `t` output describes the state after tick `t`.
pub type Tick = u64;

/// Network entity id as sent on the wire. Players and mobs share one id space.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct EntityId(pub i32);

/// Player identity for the lifetime of one connection, assigned by the edge.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ConnectionId(pub u64);

/// One player action decoded by the edge.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PlayerInput {
    /// Login finished; the core loads or creates the player and answers with
    /// [`PlayerChange::Joined`].
    Join {
        connection: ConnectionId,
        uuid: Uuid,
        name: String,
        /// Actual edge-authorized permission snapshot of the authenticated UUID.
        permission_level: u8,
    },
    /// Trusted edge notification after the connection accepted a Loader bind.
    PluginClientReady { connection: ConnectionId },
    /// Trusted edge admission of a Loader key request from this negotiated
    /// connection, resolved against the verified artifact index.
    LoaderViewRequest {
        connection: ConnectionId,
        owner: String,
        #[serde(with = "crate::input_log::view_key")]
        request_kind: ScriptClientViewKey,
    },

    /// Trusted edge snapshot of this live connection's actual permission level.
    PermissionLevel { connection: ConnectionId, level: u8 },
    /// Trusted complete persisted operator list, including offline operators.
    SpawnProtectionOperators { operators: Vec<Uuid> },
    /// Trusted edge admission of this live connection's negotiated client view
    /// radius (already clamped by the edge; the core additionally caps it at
    /// the configured server view). Recorded before [`PlayerInput::Join`] and
    /// re-sent on every Play ClientInformation: transient per-connection
    /// state, cleared on [`PlayerInput::Leave`].
    SetViewDistance {
        connection: ConnectionId,
        view_distance: u8,
    },
    /// Trusted edge snapshot of the client's locale (`ClientInformation.language`,
    /// e.g. `ru_ru`). Recorded before [`PlayerInput::Join`] from the configuration
    /// phase and re-sent on every Play ClientInformation: transient
    /// per-connection UI language, cleared on [`PlayerInput::Leave`].
    SetLocale {
        connection: ConnectionId,
        locale: String,
    },
    /// Connection closed for any reason; the core saves and removes the player.
    Leave { connection: ConnectionId },
    /// Client-reported movement; the core validates and may correct it.
    Move {
        connection: ConnectionId,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    /// Rotation-only movement; absent coordinates resolve against core state,
    /// not an older client pose cached before a server teleport.
    MoveRotation {
        connection: ConnectionId,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    MoveStatus {
        connection: ConnectionId,
        on_ground: bool,
    },
    ConfirmTeleport {
        connection: ConnectionId,
        teleport_id: i32,
    },
    /// Block dig action with the client's sequence number for acknowledgement.
    Dig {
        connection: ConnectionId,
        position: BlockPos,
        action: DigAction,
        sequence: i32,
    },
    /// Internal FIFO barrier for a read-only before-build preview. Not input
    /// state: the input log omits it.
    #[serde(skip)]
    BuildPreviewFence {
        fence: crate::build_precommit::PreviewFence,
    },
    /// Edge-resolved before-build approval bound to one frozen native plan.
    /// The plan is not serializable: the input log records an `Uncovered` marker.
    #[serde(skip)]
    AuthorizedBuild {
        authorization: crate::build_precommit::AuthorizedBuild,
    },
    /// Repair client prediction after a refusal; never a world/WAL mutation.
    BuildResync {
        request: crate::build_precommit::BuildResync,
    },
    /// Use the held item on a block face (placement or interaction).
    UseItemOn {
        connection: ConnectionId,
        hand: Hand,
        position: BlockPos,
        face: u8,
        cursor: [f32; 3],
        sequence: i32,
    },
    /// Use the held item without a block target (eat, drink, throw, start a bow).
    UseItem {
        connection: ConnectionId,
        hand: Hand,
        yaw: f32,
        pitch: f32,
        sequence: i32,
    },
    /// Stop using the item started by [`PlayerInput::UseItem`] (bow release,
    /// interrupted eating).
    ReleaseUseItem { connection: ConnectionId },
    /// Drop the selected stack (`whole_stack`) or one item from it.
    DropSelected {
        connection: ConnectionId,
        whole_stack: bool,
    },
    /// Swap main-hand and off-hand items.
    SwapHands { connection: ConnectionId },
    /// A creative client sets an inventory menu slot outright, or throws
    /// the stack (`slot` -1).
    CreativeSlot {
        connection: ConnectionId,
        slot: i16,
        stack: ItemStack,
    },
    /// The client's sneak key changed (vanilla `PlayerInput.shift`).
    Sneak {
        connection: ConnectionId,
        sneaking: bool,
    },
    /// Serverbound player command changed sprinting.
    Sprint {
        connection: ConnectionId,
        sprinting: bool,
    },
    /// Client requests elytra gliding; only the core can admit the transition.
    StartFallFlying { connection: ConnectionId },
    /// Authoritative edit/sign request for a writable book in a hotbar/offhand slot.
    EditBook {
        connection: ConnectionId,
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
    },
    /// Text submitted for the sign editor the core opened.
    SignUpdate {
        connection: ConnectionId,
        position: BlockPos,
        front: bool,
        lines: Vec<String>,
    },
    /// Attack an entity.
    Attack {
        connection: ConnectionId,
        target: EntityId,
    },
    /// Interact with an entity (breed, trade, mount).
    InteractEntity {
        connection: ConnectionId,
        target: EntityId,
        hand: Hand,
        /// Hit position relative to the entity's feet.
        location: [f64; 3],
    },
    /// Entity secondary-use retains its intent (dismount / unleash).
    InteractEntitySecondary {
        connection: ConnectionId,
        target: EntityId,
        hand: Hand,
        location: [f64; 3],
    },
    /// Full vanilla input bitset, retained by the rider for vehicle control.
    VehicleInput { connection: ConnectionId, flags: u8 },
    MoveVehicle {
        connection: ConnectionId,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    /// Selected hotbar slot changed.
    SelectSlot { connection: ConnectionId, slot: u8 },
    /// Vanilla pick block (middle click on a block).
    PickItemFromBlock {
        connection: ConnectionId,
        position: BlockPos,
    },
    /// Vanilla pick entity (middle click on an entity).
    PickItemFromEntity {
        connection: ConnectionId,
        target: EntityId,
    },
    /// Click in an open menu or the player inventory (window 0). The core
    /// validates against its authoritative menu at `state_id`; the client's
    /// predicted slots are hints only and are answered with authoritative
    /// [`PlayerChange::MenuContents`] on mismatch.
    ContainerClick {
        connection: ConnectionId,
        window: u8,
        state_id: i32,
        slot: i16,
        button: i8,
        mode: ClickMode,
    },
    /// Enchanting-table offer selection.
    ContainerButton {
        connection: ConnectionId,
        window: u8,
        button: i32,
    },
    /// Crafter enabled-slot state, from container_slot_state_changed.
    ContainerSlotState {
        connection: ConnectionId,
        window: u8,
        slot: i32,
        enabled: bool,
    },
    /// Merchant offer selected by the client; the tick fills payment slots.
    SelectTrade {
        connection: ConnectionId,
        offer: i32,
    },
    /// Beacon effect selection from the beacon menu confirm button.
    /// Zero-based `mob_effect` registry ids; `None` is an absent selection.
    /// Admission and payment belong to the tick.
    SetBeacon {
        connection: ConnectionId,
        primary: Option<u32>,
        secondary: Option<u32>,
    },
    /// Anvil text field update.
    RenameItem {
        connection: ConnectionId,
        name: String,
    },
    /// The client closed a menu.
    ContainerClose {
        connection: ConnectionId,
        window: u8,
    },
    /// Recipe-book placement request for the open crafting menu. `recipe` is
    /// the recipe display id, which is the recipe's index in the core's loaded
    /// recipe list; the edge announces recipe-book entries with the same ids.
    PlaceRecipe {
        connection: ConnectionId,
        window: u8,
        recipe: u32,
        make_all: bool,
    },
    /// Respawn after death.
    Respawn { connection: ConnectionId },
    /// Request the complete nonzero statistics snapshot.
    RequestStats { connection: ConnectionId },
    /// The client left its bed (serverbound player command `StopSleeping`).
    LeaveBed { connection: ConnectionId },
    /// Open the inventory of the mount currently ridden by this player.
    OpenMountInventory { connection: ConnectionId },
    /// Chat message, already validated for length and characters by the edge.
    Chat {
        connection: ConnectionId,
        message: String,
    },
    /// Slash command without the leading `/` (chat command packet).
    Command {
        connection: ConnectionId,
        command: String,
        /// Trusted edge authorization captured when this input was admitted.
        permission_level: u8,
    },
    /// A `custom_click_action` from a server-driven dialog ([`crate::dialog`]).
    /// `id` is the canonical `namespace:path` action id; `payload` is the
    /// client-built NBT (untrusted: owners re-validate in the tick).
    DialogClick {
        connection: ConnectionId,
        id: String,
        #[serde(with = "crate::input_log::nbt_option")]
        payload: Option<mc_nbt::Tag>,
    },
    /// Trusted stdin source; has op level four and no entity.
    ConsoleCommand(crate::command_console::ConsoleCommand),
    SeenAdvancements {
        connection: ConnectionId,
        tab: Option<String>,
    },
    /// Test-only: place an exact stack (with components) into an inventory
    /// slot. Unit tests use this because no protocol input can materialize a
    /// component stack (`/give` makes plain stacks; creative Clone copies
    /// existing slots). Never constructed outside `cfg(test)`.
    #[cfg(test)]
    TestGive {
        connection: ConnectionId,
        slot: usize,
        stack: mc_data::ItemStack,
    },
}

impl PlayerInput {
    /// Native player text that shares mute and chat-spam admission.
    #[must_use]
    pub fn is_communication(&self) -> bool {
        match self {
            Self::Chat { .. } => true,
            Self::Command { command, .. } => matches!(
                command.split_whitespace().next(),
                Some(
                    "me" | "msg"
                        | "tell"
                        | "w"
                        | "reply"
                        | "teammsg"
                        | "tm"
                        | "say"
                        | "tellraw"
                        | "title"
                )
            ),
            _ => false,
        }
    }

    pub fn connection(&self) -> ConnectionId {
        match self {
            Self::SpawnProtectionOperators { .. } => ConnectionId(0),
            Self::BuildPreviewFence { fence } => fence.connection(),
            Self::AuthorizedBuild { authorization } => authorization.connection(),
            Self::BuildResync { request } => request.connection(),
            Self::ConsoleCommand(_) => crate::command_console::CONNECTION,
            Self::Join { connection, .. }
            | Self::PluginClientReady { connection }
            | Self::LoaderViewRequest { connection, .. }
            | Self::PermissionLevel { connection, .. }
            | Self::SetViewDistance { connection, .. }
            | Self::SetLocale { connection, .. }
            | Self::Leave { connection }
            | Self::Move { connection, .. }
            | Self::MoveRotation { connection, .. }
            | Self::MoveStatus { connection, .. }
            | Self::ConfirmTeleport { connection, .. }
            | Self::Dig { connection, .. }
            | Self::UseItemOn { connection, .. }
            | Self::UseItem { connection, .. }
            | Self::ReleaseUseItem { connection }
            | Self::DropSelected { connection, .. }
            | Self::SwapHands { connection }
            | Self::CreativeSlot { connection, .. }
            | Self::Sneak { connection, .. }
            | Self::Sprint { connection, .. }
            | Self::StartFallFlying { connection }
            | Self::SignUpdate { connection, .. }
            | Self::EditBook { connection, .. }
            | Self::Attack { connection, .. }
            | Self::InteractEntity { connection, .. }
            | Self::InteractEntitySecondary { connection, .. }
            | Self::VehicleInput { connection, .. }
            | Self::MoveVehicle { connection, .. }
            | Self::SelectSlot { connection, .. }
            | Self::PickItemFromBlock { connection, .. }
            | Self::PickItemFromEntity { connection, .. }
            | Self::ContainerClick { connection, .. }
            | Self::ContainerButton { connection, .. }
            | Self::ContainerSlotState { connection, .. }
            | Self::SelectTrade { connection, .. }
            | Self::SetBeacon { connection, .. }
            | Self::RenameItem { connection, .. }
            | Self::ContainerClose { connection, .. }
            | Self::PlaceRecipe { connection, .. }
            | Self::SeenAdvancements { connection, .. }
            | Self::Respawn { connection }
            | Self::RequestStats { connection }
            | Self::LeaveBed { connection }
            | Self::OpenMountInventory { connection }
            | Self::Chat { connection, .. }
            | Self::DialogClick { connection, .. }
            | Self::Command { connection, .. } => *connection,
            #[cfg(test)]
            Self::TestGive { connection, .. } => *connection,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Hand {
    Main,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DigAction {
    Start,
    Cancel,
    Finish,
}

/// Vanilla container click modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClickMode {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

/// Inputs accepted for one tick, in arrival order per connection.
#[derive(Debug, Default)]
pub struct TickInput {
    pub players: Vec<PlayerInput>,
}

/// Result of one committed tick.
#[derive(Debug, Clone)]
pub struct TickOutput {
    pub tick: Tick,
    pub snapshot: Arc<WorldSnapshot>,
    pub changes: Arc<ChangeSet>,
    /// Server-side cost of this tick, carried by the output it describes so
    /// a sample can never be attributed to another tick.
    pub times: TickTimes,
}

impl TickOutput {
    /// The root publication owns every dimension's isolated projection.
    pub fn in_dimension(&self, dimension: crate::dimensions::Dimension) -> Option<&Self> {
        if self.snapshot.dimension == dimension {
            Some(self)
        } else {
            self.snapshot.dimensions.get(&dimension)
        }
    }
}

/// Server-side phase times for one tick in microseconds (bench/monitoring).
#[derive(Debug, Clone, Copy, Default)]
pub struct TickTimes {
    /// UNIX wall-clock nanoseconds sampled at the active owner iteration
    /// start, before checkpoint acknowledgements and input drains. Zero
    /// for outputs constructed outside the live loop. Wall-clock steps may
    /// move this backwards; durations use a monotonic clock.
    pub wall_start_unix_ns: u64,
    /// Full active owner iteration including drains, outbox and checkpoint
    /// capture, excluding pacing. Available only from `subscribe_tick_times`;
    /// the earlier world publication carries zero because work is ongoing.
    pub wall_us: u64,
    /// Player inputs, natural spawning and player/environment updates.
    pub input_us: u64,
    /// Chunk admission/eviction/hydration, resident services and respawn retries.
    pub chunks_us: u64,
    pub entity_us: u64,
    pub merge_us: u64,
    pub block_us: u64,
    /// Post-merge effects, block overlay folding and gameplay commit.
    pub commit_us: u64,
    /// Resident chunk list build for the snapshot.
    pub snapshot_us: u64,
    /// Entity wire views + chunk index build.
    pub views_us: u64,
    /// Snapshot metadata and reliable outbox construction (not edge fanout).
    pub publish_us: u64,
    /// Plugin admission/observations and read-only build previews.
    pub plugins_us: u64,
    /// Dimension routing/travel/globals, local return cleanup and phase boundaries.
    pub coordination_us: u64,
    /// Build the tick's native WAL post-images; no disk I/O.
    pub journal_build_us: u64,
    /// Seal plugin events and enqueue WAL, including bounded writer backpressure.
    pub journal_queue_us: u64,
    /// Capture/enqueue generated chunk saves; writer I/O remains off-tick.
    pub generated_us: u64,
    /// Acknowledgements/input drains before `total_us` begins.
    pub owner_pre_us: u64,
    /// Broadcast/reliable handoff and checkpoint capture after `total_us`.
    pub owner_post_us: u64,
    /// `run_tick` + WAL build/enqueue + generated capture, before the broadcast.
    pub total_us: u64,
}

/// Immutable world state after a tick. Shared read-only; never mutated after
/// publication. A reader may hold it for as long as it likes: the core never
/// reuses its storage (copy-on-write pages, see ADR 0013 "Data layout").
#[derive(Debug, Default)]
pub struct WorldSnapshot {
    pub tick: Tick,
    pub dimension: crate::dimensions::Dimension,
    /// Root output owns the other dimension projections from this same tick.
    pub dimensions: std::collections::BTreeMap<crate::dimensions::Dimension, TickOutput>,
    /// Day time in ticks (0..24000 cycle plus day count).
    pub day_time: i64,
    /// Client clock speed; zero when `advance_time` is disabled.
    pub day_time_rate: f32,
    pub raining: bool,
    pub rain_level: f32,
    pub thunder_level: f32,
    pub border: crate::command_border::Border,
    /// Every resident chunk, sorted by `(x, z)` for binary search. The list
    /// is shared with earlier snapshots until residency or a revision
    /// changes; unchanged chunks share their `Arc` either way.
    pub chunks: Arc<Vec<SnapshotChunk>>,
    /// Persistent identity and chunk indices of every simulated entity.
    /// Unchanged rows and spatial membership are shared with earlier snapshots.
    pub entities: EntityViews,
    pub raid_bars: Vec<crate::raids::RaidBar>,
    /// Derived POI labels sorted by unique anchor; never persisted gameplay entities.
    pub settlement_labels: Arc<Vec<crate::settlements::presentation::SettlementLabel>>,
}

/// One resident chunk in a snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotChunk {
    pub pos: ChunkPos,
    /// World-unique content revision: changes whenever the chunk's blocks,
    /// light-relevant content or block entities change, or it is (re)loaded.
    /// Equal revision at the same position means identical content, so
    /// readers may key derived data (encoded packets) on it.
    pub revision: u64,
    pub chunk: Arc<Chunk>,
}

/// Wire-relevant view of one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityView {
    pub id: EntityId,
    pub uuid: Uuid,
    /// Protocol entity type id (26.1.2 registry).
    pub type_id: u32,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
    /// Rarely changing state shared with the previous snapshot until it changes.
    pub appearance: Arc<EntityAppearance>,
}

/// Entity state the client renders besides motion. The edge maps it to
/// protocol entity data and equipment packets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityAppearance {
    /// Type-contract spawn data (projectile owner id, falling state, ...).
    pub spawn_data: i32,
    /// Stack carried by an item entity, or shown by an item frame.
    pub item: Option<ItemStack>,
    /// Vanilla orb denomination and number of merged pickups.
    pub experience_orb: Option<(i32, i32)>,
    /// Main hand, off hand, feet, legs, chest, head.
    pub equipment: [Option<ItemStack>; 6],
    /// Native animal equipment projects to vanilla slots 6 and 7.
    pub body_armor: Option<ItemStack>,
    pub saddle: Option<ItemStack>,
    pub passengers: Vec<EntityId>,
    pub leash_holder: Option<EntityId>,
    pub baby: bool,
    /// Vanilla pose ordinal (standing, sleeping, swimming, crouching, ...).
    pub pose: u8,
    /// Vanilla shared flags bit set (on fire, crouching, sprinting, ...).
    pub shared_flags: u8,
    pub health: f32,
    /// Visible vanilla effect particles, including ambient alpha in ARGB colors.
    pub effect_particles: Vec<EffectParticle>,
    pub effects_ambient: bool,
    /// Transient LivingEntity render counts; vanilla does not save them.
    pub arrow_count: i32,
    pub stinger_count: i32,
    /// Player absorption hearts; other living entities do not expose this accessor.
    pub absorption: f32,
    pub custom_name: Option<String>,
    /// Vanilla `CustomNameVisible` / Entity.DATA_CUSTOM_NAME_VISIBLE.
    pub custom_name_visible: bool,
    /// Type-specific values the client needs (variant, color, profession,
    /// sheared, saddled, ...), as `(protocol data index, value)`.
    pub extra: Vec<(u8, EntityDataValue)>,
}

/// Vanilla particle options produced by built-in status effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectParticle {
    EntityEffect { argb: i32 },
    TrialOmen,
    RaidOmen,
}

/// Value kinds used by type-specific entity data.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityDataValue {
    Byte(i8),
    VarInt(i32),
    VarLong(i64),
    Float(f32),
    Rotations([f32; 3]),
    Boolean(bool),
    WeatheringCopper(u8),
    CopperGolemState(u8),
    Direction(u8),
    PaintingVariant(u32),
    String(String),
    BlockState(u32),
    Item(Option<ItemStack>),
    /// Single particle serializer, used by AreaEffectCloud at index 10.
    EntityEffectParticle(i32),
    /// Vanilla optional unsigned integer serializer (firework attachment).
    OptionalUnsignedInt(Option<u32>),
    /// `Optional<BlockPos>` (the sleeping position).
    OptionalBlockPos(Option<[i32; 3]>),
    OptionalLivingEntityReference(Option<Uuid>),
    VillagerData {
        kind: i32,
        profession: i32,
        level: i32,
    },
}

/// Everything that changed in one tick, in commit order within each list.
/// Entity motion is not listed: readers compare the snapshot with what
/// they last delivered, so a dropped delivery is never lost.
#[derive(Debug, Default)]
pub struct ChangeSet {
    pub blocks: Vec<BlockChange>,
    pub chunks_loaded: Vec<ChunkPos>,
    pub chunks_unloaded: Vec<ChunkPos>,
    pub entities_spawned: Vec<EntityId>,
    /// Entities whose [`EntityAppearance`] changed this tick.
    pub entities_appearance_changed: Vec<EntityId>,
    pub entities_removed: Vec<EntityId>,
    /// One-shot visual events: swing, hurt, death animation, item pickup.
    pub entity_events: Vec<EntityEvent>,
    /// Blasts committed this tick (the edge sends the explode packet).
    pub explosions: Vec<ExplosionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityEvent {
    /// Verified vanilla entity status event (guardian beam, sonic, fangs).
    Status {
        entity: EntityId,
        event: i8,
    },
    Swing {
        entity: EntityId,
        hand: Hand,
    },
    Hurt {
        entity: EntityId,
    },
    /// Accepted damage with attribution retained even when its projectile was removed.
    Damage {
        entity: EntityId,
        source_type_id: i32,
        cause: Option<EntityId>,
        direct: Option<EntityId>,
        source_position: Option<[f64; 3]>,
    },
    Death {
        entity: EntityId,
    },
    SweepAttack {
        entity: EntityId,
        position: [f64; 3],
        yaw: f32,
    },
    /// Vanilla 26.1.2 uses sound packets, not the removed shield status bytes.
    ShieldBlock {
        entity: EntityId,
        disabled: bool,
    },
    GoldenDandelion {
        entity: EntityId,
        locked: bool,
    },
    /// Verified fill/empty sound, attached to the player using the bottle.
    Bottle {
        entity: EntityId,
        empty: bool,
        cauldron: bool,
    },
    /// `collector` picked up `count` from the item entity `item`.
    Pickup {
        item: EntityId,
        collector: EntityId,
        count: u8,
    },
}

/// One committed explosion, as clients see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplosionEvent {
    pub center: [f64; 3],
    pub radius: f32,
    /// Blocks the blast destroyed (their changes ride `blocks`).
    pub blocks: u32,
    /// Knockback velocity per hit player (blocks/tick).
    pub knockback: Vec<(EntityId, [f64; 3])>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockChange {
    pub position: BlockPos,
    /// Global block state id.
    pub state: u32,
}

/// Equipment-only player attributes; registry identity is resolved by the edge.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerAttribute {
    pub kind: &'static str,
    pub base: f64,
    pub modifiers: Vec<mc_data::item_components::ItemAttributeModifier>,
}

/// One `/title` packet for a connection (`ClientboundSetTitleText` & friends).
#[derive(Debug, Clone, PartialEq)]
pub enum TitleChange {
    Title(mc_nbt::Tag),
    Subtitle(mc_nbt::Tag),
    ActionBar(mc_nbt::Tag),
    Times {
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    },
    Clear {
        reset_times: bool,
    },
}

/// One `ClientboundBossEvent` operation for a custom boss bar.
/// `color`/`overlay` are vanilla `BossBarColor`/`BossBarOverlay` ordinals;
/// `flags` bit 0 darken sky, bit 1 play boss music, bit 2 create world fog.
#[derive(Debug, Clone, PartialEq)]
pub enum BossBarChange {
    Add {
        name: mc_nbt::Tag,
        progress: f32,
        color: i32,
        overlay: i32,
        flags: u8,
    },
    Remove,
    Progress(f32),
    Name(mc_nbt::Tag),
    Style {
        color: i32,
        overlay: i32,
    },
    Properties(u8),
}

/// Reliable results for the connection identified by the enclosing outbox queue key.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerChange {
    Map {
        update: crate::maps::MapUpdate,
    },
    Advancements {
        reset: bool,
        definitions: Arc<mc_data::advancements::AdvancementTree>,
        progress: serde_json::Value,
    },
    AdvancementsTab {
        tab: Option<String>,
    },
    RecipesUnlocked {
        recipes: Vec<String>,
        replace: bool,
    },
    VehicleCorrected {
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    },
    /// Retained projectile one-shot animation (for example firework status 17).
    ProjectileStatus {
        entity: EntityId,
        event: i8,
    },
    /// Vanilla projectile impact level event, with its registry-backed payload.
    ProjectileLevelEvent {
        position: BlockPos,
        event: i32,
        data: i32,
    },
    MobEffect {
        effect: crate::effects::ActiveEffect,
        /// Vanilla blends newly added effects, not updates or restored effects.
        blend: bool,
    },
    RemoveMobEffect {
        id: crate::effects::EffectId,
    },
    Absorption {
        amount: f32,
    },
    /// Authoritative air supply for this connection's player metadata.
    AirSupply {
        air: i32,
    },
    Scoreboard {
        event: crate::scoreboard::ScoreboardEvent,
    },
    /// Global online roster; entity visibility remains dimension-local.
    Roster {
        players: Arc<[crate::dimensions::RosterPlayer]>,
        removed: Arc<[Uuid]>,
    },
    StyledChat {
        content: mc_nbt::Tag,
    },
    ShieldCooldown {
        ticks: i32,
    },
    ItemCooldown {
        group: String,
        ticks: i32,
    },
    Attributes {
        attributes: Vec<PlayerAttribute>,
    },
    Joined {
        entity: EntityId,
        dimension: crate::dimensions::Dimension,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        game_mode: u8,
    },
    /// Server-authoritative position correction (teleport or rejected move).
    PositionCorrected {
        teleport_id: i32,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    },
    /// Accepted combat motion, in core units (blocks/second). Reliable:
    /// the victim must receive it even when intermediate snapshots skip.
    Motion {
        entity: EntityId,
        velocity: [f64; 3],
    },
    /// Reliable correction to this viewer, excluded from committed block writes.
    BlockResynced {
        position: BlockPos,
        block: u32,
    },
    /// Acknowledge block actions up to this client sequence.
    BlockActionsAcked {
        sequence: i32,
    },
    BookOpened {
        hand: Hand,
    },
    SignEditorOpened {
        position: BlockPos,
        front: bool,
        block: u32,
    },
    /// Vanilla block-entity update data, delivered reliably to a viewer.
    BlockEntityData {
        position: BlockPos,
        block_entity_type: u32,
        nbt: mc_nbt::Tag,
    },
    /// Native vanilla block action. The exact state precedes the event so the
    /// client can resolve state-owned note pitch/instrument before playback.
    BlockEvent {
        position: BlockPos,
        block: u32,
        state: u32,
        action: u8,
        parameter: u8,
    },
    /// Vanilla bell ring sound at the center of the bell block.
    BellRung {
        position: BlockPos,
    },
    /// Positional sound generated by an operator command.
    CommandSound {
        sound: mc_data::Identifier,
        source: i32,
        position: [i32; 3],
        volume: f32,
        pitch: f32,
    },
    /// Parameterless vanilla particle emitted by a command.
    CommandParticle {
        particle_id: i32,
        position: [f64; 3],
        delta: [f32; 3],
        speed: f32,
        count: i32,
        force: bool,
    },
    /// A menu opened for this player (window 0, the player inventory, is
    /// always open and never announced).
    MenuOpened {
        window: u8,
        /// Protocol menu type id.
        menu_type: u32,
        /// JSON text component, or a literal label (as for scoreboard text).
        title: String,
    },
    MountMenuOpened {
        window: u8,
        columns: u8,
        entity: EntityId,
    },
    /// Show this inline dialog (`Dialog.DIRECT_CODEC` NBT from [`crate::dialog::Dialog::into_tag`]).
    ShowDialog {
        dialog: mc_nbt::Tag,
    },
    /// Close whatever dialog the client is showing.
    ClearDialog,
    MenuClosed {
        window: u8,
    },
    /// The server selected a hotbar slot (pick item).
    HeldSlot {
        slot: u8,
    },
    MerchantOffers {
        window: u8,
        offers: Vec<crate::villager::VillagerTradeOffer>,
        level: u8,
        xp: i32,
        can_restock: bool,
    },
    /// Full authoritative contents of a menu, sent on open and on any client
    /// prediction mismatch.
    MenuContents {
        window: u8,
        state_id: i32,
        slots: Vec<Option<ItemStack>>,
        cursor: Option<ItemStack>,
    },
    MenuSlot {
        window: u8,
        state_id: i32,
        slot: i16,
        item: Option<ItemStack>,
    },
    /// The item carried on the cursor changed outside a full contents update.
    Cursor {
        item: Option<ItemStack>,
    },
    /// Menu property (furnace progress, enchantment seeds, ...).
    MenuProperty {
        window: u8,
        property: i16,
        value: i16,
    },
    /// Admitted plugin sound, encoded through the negotiated Loader channel.
    PluginClientSound {
        plugin_id: String,
        sound: mc_script::ScriptClientSound,
    },
    /// Encoded wire-3 Loader view message for this negotiated connection.
    PluginClientView {
        payload: Vec<u8>,
    },
    Experience {
        progress: f32,
        level: i32,
        total: i32,
    },
    Died {
        message: mc_nbt::Tag,
    },
    /// Game mode changed (game event "change game mode" plus abilities).
    GameModeChanged {
        game_mode: u8,
    },
    /// Weather projection (also supplied at login).
    WeatherChanged {
        raining: bool,
        rain_level: f32,
        thunder_level: f32,
    },
    /// World difficulty (0 peaceful .. 3 hard); the join burst says easy.
    DifficultyChanged {
        difficulty: u8,
    },
    /// `/title` overlay state (title, subtitle, action bar, times, clear/reset).
    Title(TitleChange),
    /// Server tick-rate manager state (`/tick rate|freeze|unfreeze`).
    TickingState {
        rate: f32,
        frozen: bool,
    },
    /// Frozen-tick budget (`/tick step`): remaining ticks to run.
    TickingStep {
        steps: i32,
    },
    /// Custom boss bar (`/bossbar`) update for this connection.
    BossBar {
        id: Uuid,
        change: BossBarChange,
    },
    Health {
        health: f32,
        food: u8,
        saturation: f32,
    },
    /// System message delivered to the reliable queue owner.
    Chat {
        message: String,
    },
    /// Vanilla statistics: `(stat type registry id, value registry id, count)`.
    Statistics {
        stats: Vec<(i32, i32, i32)>,
    },
    /// The core drops this connection (e.g. its reliable queue overflowed).
    /// Always the last change for the connection; the edge sends a
    /// disconnect with `reason` and closes.
    Disconnect {
        reason: String,
    },
    /// Preserve player identity/inventory while rebuilding the client level.
    DimensionChanged {
        teleport_id: i32,
        dimension: crate::dimensions::Dimension,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        game_mode: u8,
        portal_cooldown: u16,
    },
    /// A dead player respawned: rebuild the target client level, then sync
    /// position, without retaining the old local entity's data.
    Respawned {
        teleport_id: i32,
        dimension: crate::dimensions::Dimension,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        game_mode: u8,
    },
}
