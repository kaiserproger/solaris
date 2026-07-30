use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const PLUGIN_API_CRATES: &[&str] = &["mc-extension", "mc-script"];
const FORBIDDEN_API_TYPES: &[&str] = &[
    "WorldHandle",
    "SessionRegistry",
    "WorldStorage",
    "ShutdownHandle",
    "SaveHandle",
    "OutboundPressureHandle",
    "RuntimeControlPlane",
    "EntityStore",
    "ChunkScheduler",
];
const FORBIDDEN_API_TRANSPORTS: &[&str] = &[
    "TryRecvError",
    "std::sync::mpsc",
    "tokio::sync",
    "mpsc::Sender",
    "mpsc::Receiver",
    "SyncSender",
    "Receiver<",
    "Sender<",
    "Arc<Mutex",
    "Arc<RwLock",
    "parking_lot",
    "DashMap",
    "JoinHandle",
];
const FORBIDDEN_API_DEPENDENCIES: &[&str] = &[
    "mc-net",
    "mc-world",
    "mc-server",
    "mc-entity",
    "mc-physics",
    "mc-data",
    "mc-protocol",
    "mc-nbt",
    "mc-worldgen",
];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    path: PathBuf,
    line: usize,
    message: String,
}

struct OwnershipRule {
    name: &'static str,
    module_file: &'static str,
    parent_file: &'static str,
    mod_declaration: &'static str,
    definition_file: &'static str,
    definition_anchor: &'static str,
}

const MC_NET_OWNERSHIP: &[OwnershipRule] = &[
    OwnershipRule {
        name: "combat",
        module_file: "crates/mc-net/src/play/combat/mod.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod combat;",
        definition_file: "crates/mc-net/src/play/combat/player_damage.rs",
        definition_anchor: "pub(in crate::play) struct PlayerDamageRequest",
    },
    OwnershipRule {
        name: "player action state",
        module_file: "crates/mc-net/src/play/combat/player_actions.rs",
        parent_file: "crates/mc-net/src/play/combat/mod.rs",
        mod_declaration: "mod player_actions;",
        definition_file: "crates/mc-net/src/play/combat/player_actions.rs",
        definition_anchor: "pub(in crate::play) struct ShieldUseState",
    },
    OwnershipRule {
        name: "player attack recharge rules",
        module_file: "crates/mc-net/src/play/combat/player_actions.rs",
        parent_file: "crates/mc-net/src/play/combat/mod.rs",
        mod_declaration: "mod player_actions;",
        definition_file: "crates/mc-net/src/play/combat/player_actions.rs",
        definition_anchor: "pub(in crate::play) fn begin_player_attack_attempt",
    },
    OwnershipRule {
        name: "held weapon attack durability",
        module_file: "crates/mc-net/src/play/combat/player_actions.rs",
        parent_file: "crates/mc-net/src/play/combat/mod.rs",
        mod_declaration: "mod player_actions;",
        definition_file: "crates/mc-net/src/play/combat/player_actions.rs",
        definition_anchor: "pub(in crate::play) fn damage_held_weapon_stack",
    },
    OwnershipRule {
        name: "shield damage rules",
        module_file: "crates/mc-net/src/play/combat/player_actions.rs",
        parent_file: "crates/mc-net/src/play/combat/mod.rs",
        mod_declaration: "mod player_actions;",
        definition_file: "crates/mc-net/src/play/combat/player_actions.rs",
        definition_anchor: "pub(in crate::play) fn shield_blocks_damage",
    },
    OwnershipRule {
        name: "shield inventory durability",
        module_file: "crates/mc-net/src/play/combat/player_actions.rs",
        parent_file: "crates/mc-net/src/play/combat/mod.rs",
        mod_declaration: "mod player_actions;",
        definition_file: "crates/mc-net/src/play/combat/player_actions.rs",
        definition_anchor: "pub(in crate::play) fn damage_active_shield_slots",
    },
    OwnershipRule {
        name: "player combat",
        module_file: "crates/mc-net/src/play/session/player_combat.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_combat;",
        definition_file: "crates/mc-net/src/play/session/player_combat.rs",
        definition_anchor: "pub(in crate::play) struct PlayerEntityAttack",
    },
    OwnershipRule {
        name: "player state",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(in crate::play) fn commit_player_survival",
    },
    OwnershipRule {
        name: "player persistence registration",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(in crate::play) fn register_player_persistence",
    },
    OwnershipRule {
        name: "player inventory authority",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(in crate::play) fn commit_player_inventory",
    },
    OwnershipRule {
        name: "player persistence snapshots",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(crate) fn persisted_player_states",
    },
    OwnershipRule {
        name: "player persistence acknowledgement",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(crate) fn acknowledge_saved_player_states",
    },
    OwnershipRule {
        name: "player save notification wait",
        module_file: "crates/mc-net/src/play/session/player_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state;",
        definition_file: "crates/mc-net/src/play/session/player_state.rs",
        definition_anchor: "pub(crate) async fn wait_for_player_save_request",
    },
    OwnershipRule {
        name: "outbound",
        module_file: "crates/mc-net/src/play/session/outbound.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound;",
        definition_file: "crates/mc-net/src/play/session/outbound.rs",
        definition_anchor: "pub(in crate::play) enum OutboundCommand",
    },
    OwnershipRule {
        name: "furnace",
        module_file: "crates/mc-net/src/play/containers/furnace.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod furnace;",
        definition_file: "crates/mc-net/src/play/containers/furnace.rs",
        definition_anchor: "pub(in crate::play) enum FurnaceKind",
    },
    OwnershipRule {
        name: "chest",
        module_file: "crates/mc-net/src/play/containers/chest.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod chest;",
        definition_file: "crates/mc-net/src/play/containers/chest.rs",
        definition_anchor: "pub(in crate::play) struct ChestClickInput",
    },
    OwnershipRule {
        name: "crafting",
        module_file: "crates/mc-net/src/play/containers/crafting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod crafting;",
        definition_file: "crates/mc-net/src/play/containers/crafting.rs",
        definition_anchor: "pub(in crate::play) struct CraftingTableWindow",
    },
    OwnershipRule {
        name: "crafting rules",
        module_file: "crates/mc-net/src/play/containers/crafting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod crafting;",
        definition_file: "crates/mc-net/src/play/containers/crafting.rs",
        definition_anchor: "pub(in crate::play) fn crafting_result_from_input",
    },
    OwnershipRule {
        name: "crafting projection",
        module_file: "crates/mc-net/src/play/containers/crafting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod crafting;",
        definition_file: "crates/mc-net/src/play/containers/crafting.rs",
        definition_anchor: "pub(in crate::play) fn crafting_table_input_projection",
    },
    OwnershipRule {
        name: "crafting click authority",
        module_file: "crates/mc-net/src/play/containers/crafting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod crafting;",
        definition_file: "crates/mc-net/src/play/containers/crafting.rs",
        definition_anchor: "pub(in crate::play) fn apply_pickup_click",
    },
    OwnershipRule {
        name: "inventory crafting quick move",
        module_file: "crates/mc-net/src/play/containers/crafting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod crafting;",
        definition_file: "crates/mc-net/src/play/containers/crafting.rs",
        definition_anchor: "pub(in crate::play) fn apply_crafting_quick_move_click",
    },
    OwnershipRule {
        name: "shared inventory click primitive",
        module_file: "crates/mc-net/src/play/inventory.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod inventory;",
        definition_file: "crates/mc-net/src/play/inventory.rs",
        definition_anchor: "pub(crate) fn apply_regular_pickup_slot",
    },
    OwnershipRule {
        name: "player inventory slot placement",
        module_file: "crates/mc-net/src/play/inventory.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod inventory;",
        definition_file: "crates/mc-net/src/play/inventory.rs",
        definition_anchor: "pub(crate) fn can_place_in_player_slot",
    },
    OwnershipRule {
        name: "enchanting",
        module_file: "crates/mc-net/src/play/containers/enchanting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod enchanting;",
        definition_file: "crates/mc-net/src/play/containers/enchanting.rs",
        definition_anchor: "pub(in crate::play) struct EnchantingTableWindow",
    },
    OwnershipRule {
        name: "enchanting rules",
        module_file: "crates/mc-net/src/play/containers/enchanting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod enchanting;",
        definition_file: "crates/mc-net/src/play/containers/enchanting.rs",
        definition_anchor: "pub(in crate::play) fn enchant_item_candidate",
    },
    OwnershipRule {
        name: "enchanting projection",
        module_file: "crates/mc-net/src/play/containers/enchanting.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod enchanting;",
        definition_file: "crates/mc-net/src/play/containers/enchanting.rs",
        definition_anchor: "pub(in crate::play) fn enchanting_table_input_projection",
    },
    OwnershipRule {
        name: "stonecutter",
        module_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod stonecutter;",
        definition_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        definition_anchor: "pub(in crate::play) struct StonecutterWindow",
    },
    OwnershipRule {
        name: "stonecutter click rules",
        module_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod stonecutter;",
        definition_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        definition_anchor: "pub(in crate::play) struct StonecutterClickInput",
    },
    OwnershipRule {
        name: "stonecutter click planner",
        module_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod stonecutter;",
        definition_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        definition_anchor: "pub(in crate::play) fn plan_click",
    },
    OwnershipRule {
        name: "stonecutter recipe selection",
        module_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod stonecutter;",
        definition_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        definition_anchor: "pub(in crate::play) fn select_stonecutter_recipe",
    },
    OwnershipRule {
        name: "stonecutter projection",
        module_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        parent_file: "crates/mc-net/src/play/containers.rs",
        mod_declaration: "mod stonecutter;",
        definition_file: "crates/mc-net/src/play/containers/stonecutter.rs",
        definition_anchor: "pub(in crate::play) fn stonecutter_input_projection",
    },
    OwnershipRule {
        name: "campfire",
        module_file: "crates/mc-net/src/play/campfire.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod campfire;",
        definition_file: "crates/mc-net/src/play/campfire.rs",
        definition_anchor: "pub(in crate::play) struct CampfireCookingState",
    },
    OwnershipRule {
        name: "campfire use adapter",
        module_file: "crates/mc-net/src/play/campfire_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod campfire_adapter;",
        definition_file: "crates/mc-net/src/play/campfire_adapter.rs",
        definition_anchor: "pub(in crate::play) async fn handle_campfire_use_on",
    },
    OwnershipRule {
        name: "campfire cooking tick adapter",
        module_file: "crates/mc-net/src/play/campfire_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod campfire_adapter;",
        definition_file: "crates/mc-net/src/play/campfire_adapter.rs",
        definition_anchor: "pub(in crate::play) async fn run_campfire_cooking_ticks_owned",
    },
    OwnershipRule {
        name: "campfire hydration adapter",
        module_file: "crates/mc-net/src/play/campfire_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod campfire_adapter;",
        definition_file: "crates/mc-net/src/play/campfire_adapter.rs",
        definition_anchor: "pub(crate) async fn hydrate_persisted_campfire_cooking_strict",
    },
    OwnershipRule {
        name: "campfire recovery adapter",
        module_file: "crates/mc-net/src/play/campfire_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod campfire_adapter;",
        definition_file: "crates/mc-net/src/play/campfire_adapter.rs",
        definition_anchor: "pub(crate) async fn recover_pending_campfire_outputs",
    },
    OwnershipRule {
        name: "use item on protocol adapter",
        module_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod use_item_on_adapter;",
        definition_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        definition_anchor: "pub(super) async fn handle_use_item_on",
    },
    OwnershipRule {
        name: "use item on preflight",
        module_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod use_item_on_adapter;",
        definition_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        definition_anchor: "pub(super) fn classify_use_item_on_preflight",
    },
    OwnershipRule {
        name: "use item on rejection projection",
        module_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod use_item_on_adapter;",
        definition_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        definition_anchor: "pub(super) async fn reject_use_item_on_with_resync",
    },
    OwnershipRule {
        name: "sign update adapter",
        module_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod use_item_on_adapter;",
        definition_file: "crates/mc-net/src/play/use_item_on_adapter.rs",
        definition_anchor: "pub(super) async fn handle_sign_update",
    },
    OwnershipRule {
        name: "fluid tick planning",
        module_file: "crates/mc-net/src/play/fluids.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod fluids;",
        definition_file: "crates/mc-net/src/play/fluids.rs",
        definition_anchor: "pub(super) fn plan_scheduled_fluid_tick_edits",
    },
    OwnershipRule {
        name: "fluid flow rules",
        module_file: "crates/mc-net/src/play/fluids.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod fluids;",
        definition_file: "crates/mc-net/src/play/fluids.rs",
        definition_anchor: "pub(super) fn fluid_tick_edits",
    },
    OwnershipRule {
        name: "fluid state construction",
        module_file: "crates/mc-net/src/play/fluids.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod fluids;",
        definition_file: "crates/mc-net/src/play/fluids.rs",
        definition_anchor: "pub(super) fn fluid_state_with_level",
    },
    OwnershipRule {
        name: "toggle plans",
        module_file: "crates/mc-net/src/play/toggles.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod toggles;",
        definition_file: "crates/mc-net/src/play/toggles.rs",
        definition_anchor: "pub(super) struct ToggleBlockPlan",
    },
    OwnershipRule {
        name: "toggle interaction rules",
        module_file: "crates/mc-net/src/play/toggles.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod toggles;",
        definition_file: "crates/mc-net/src/play/toggles.rs",
        definition_anchor: "pub(super) fn plan_toggle_block_interaction",
    },
    OwnershipRule {
        name: "toggle power propagation",
        module_file: "crates/mc-net/src/play/toggles.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod toggles;",
        definition_file: "crates/mc-net/src/play/toggles.rs",
        definition_anchor: "pub(super) fn extend_adjacent_power_target_edits",
    },
    OwnershipRule {
        name: "random tick rules",
        module_file: "crates/mc-net/src/play/random_ticks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod random_ticks;",
        definition_file: "crates/mc-net/src/play/random_ticks.rs",
        definition_anchor: "pub(super) fn random_tick_edit_seeded",
    },
    OwnershipRule {
        name: "natural leaf decay drops",
        module_file: "crates/mc-net/src/play/random_ticks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod random_ticks;",
        definition_file: "crates/mc-net/src/play/random_ticks.rs",
        definition_anchor: "pub(super) fn natural_leaf_decay_drops",
    },
    OwnershipRule {
        name: "random tick sampling",
        module_file: "crates/mc-net/src/play/random_ticks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod random_ticks;",
        definition_file: "crates/mc-net/src/play/random_ticks.rs",
        definition_anchor: "pub(super) fn sample_random_tick_positions",
    },
    OwnershipRule {
        name: "scheduled block planning",
        module_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod scheduled_blocks;",
        definition_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        definition_anchor: "pub(super) fn plan_scheduled_block_tick_edits",
    },
    OwnershipRule {
        name: "resident hopper planning",
        module_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod scheduled_blocks;",
        definition_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        definition_anchor: "pub(super) fn plan_resident_hopper_transfer",
    },
    OwnershipRule {
        name: "hopper transfer authority",
        module_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod scheduled_blocks;",
        definition_file: "crates/mc-net/src/play/scheduled_blocks.rs",
        definition_anchor: "pub(super) fn scheduled_hopper_transfer",
    },
    OwnershipRule {
        name: "incremental light sources",
        module_file: "crates/mc-net/src/play/lighting.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod lighting;",
        definition_file: "crates/mc-net/src/play/lighting.rs",
        definition_anchor: "pub(super) struct IncrementalLightSources",
    },
    OwnershipRule {
        name: "incremental light computation",
        module_file: "crates/mc-net/src/play/lighting.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod lighting;",
        definition_file: "crates/mc-net/src/play/lighting.rs",
        definition_anchor: "pub(super) fn compute_incremental_light_updates",
    },
    OwnershipRule {
        name: "baked light persistence",
        module_file: "crates/mc-net/src/play/lighting.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod lighting;",
        definition_file: "crates/mc-net/src/play/lighting.rs",
        definition_anchor: "pub(super) fn persist_baked_light_updates",
    },
    OwnershipRule {
        name: "applied edit incremental relight",
        module_file: "crates/mc-net/src/play/lighting.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod lighting;",
        definition_file: "crates/mc-net/src/play/lighting.rs",
        definition_anchor: "pub(super) fn collect_incremental_light_updates_for_applied_edits",
    },
    OwnershipRule {
        name: "block placement plan",
        module_file: "crates/mc-net/src/play/block_placement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_placement;",
        definition_file: "crates/mc-net/src/play/block_placement.rs",
        definition_anchor: "pub(super) fn plan_block_placement",
    },
    OwnershipRule {
        name: "block placement result",
        module_file: "crates/mc-net/src/play/block_placement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_placement;",
        definition_file: "crates/mc-net/src/play/block_placement.rs",
        definition_anchor: "pub(super) struct PlannedBlockPlacement",
    },
    OwnershipRule {
        name: "sign placement rules",
        module_file: "crates/mc-net/src/play/block_placement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_placement;",
        definition_file: "crates/mc-net/src/play/block_placement.rs",
        definition_anchor: "pub(super) fn sign_placement_state",
    },
    OwnershipRule {
        name: "accepted player movement",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) struct AcceptedAbsoluteMovement",
    },
    OwnershipRule {
        name: "player collision rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn player_pose_collides_with_solid_in_snapshot",
    },
    OwnershipRule {
        name: "pending teleport state",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn confirm_pending_teleport",
    },
    OwnershipRule {
        name: "player water contact rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn player_water_overlap_in_snapshot",
    },
    OwnershipRule {
        name: "player campfire contact rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn player_touches_lit_campfire_in_snapshot",
    },
    OwnershipRule {
        name: "farmland landing rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn farmland_trample_pos",
    },
    OwnershipRule {
        name: "player fall damage rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn fall_damage_amount",
    },
    OwnershipRule {
        name: "player movement exhaustion rules",
        module_file: "crates/mc-net/src/play/movement.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod movement;",
        definition_file: "crates/mc-net/src/play/movement.rs",
        definition_anchor: "pub(super) fn movement_exhaustion",
    },
    OwnershipRule {
        name: "chunk view replacement",
        module_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod chunk_view_authority;",
        definition_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        definition_anchor: "pub(in crate::play) fn replace_view",
    },
    OwnershipRule {
        name: "prepared chunk view commit",
        module_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod chunk_view_authority;",
        definition_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        definition_anchor: "pub(in crate::play) fn mark_loaded_if_prepared_revision_current",
    },
    OwnershipRule {
        name: "ordered chunk recipients",
        module_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod chunk_view_authority;",
        definition_file: "crates/mc-net/src/play/session/chunk_view_authority.rs",
        definition_anchor: "pub(in crate::play) fn ordered_loaded_recipients_for_chunks",
    },
    OwnershipRule {
        name: "survival break authority",
        module_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod survival_action_authority;",
        definition_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_survival_break",
    },
    OwnershipRule {
        name: "survival placement transaction",
        module_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod survival_action_authority;",
        definition_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn prepare_survival_placement_transaction",
    },
    OwnershipRule {
        name: "bucket use authority",
        module_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod survival_action_authority;",
        definition_file: "crates/mc-net/src/play/session/survival_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_bucket_use",
    },
    OwnershipRule {
        name: "generic entity spawn lifecycle",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(super) fn spawn_command_entity_locked",
    },
    OwnershipRule {
        name: "falling block spawn lifecycle adapter",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(in crate::play) fn spawn_falling_block",
    },
    OwnershipRule {
        name: "command entity spawn lifecycle adapter",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(in crate::play) fn spawn_command_entity",
    },
    OwnershipRule {
        name: "dying entity lifecycle",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(super) fn finish_dying_entities_locked",
    },
    OwnershipRule {
        name: "dying entity lifecycle adapter",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(in crate::play) fn tick_dying_entities",
    },
    OwnershipRule {
        name: "generic entity removal lifecycle",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(super) fn remove_server_entity_locked",
    },
    OwnershipRule {
        name: "entity chunk index lifecycle",
        module_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/entity_lifecycle.rs",
        definition_anchor: "pub(super) fn track_entity_chunk_locked",
    },
    OwnershipRule {
        name: "session admission lifecycle",
        module_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod session_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        definition_anchor: "pub(in crate::play) fn try_register",
    },
    OwnershipRule {
        name: "empty session wake lifecycle",
        module_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod session_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        definition_anchor: "pub(crate) async fn wait_for_session_empty",
    },
    OwnershipRule {
        name: "session teardown lifecycle",
        module_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod session_lifecycle;",
        definition_file: "crates/mc-net/src/play/session/session_lifecycle.rs",
        definition_anchor: "pub(in crate::play) fn unregister_preserving_player_state",
    },
    OwnershipRule {
        name: "accepted player pose authority",
        module_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_authority;",
        definition_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        definition_anchor: "pub(super) fn accept_player_pose_locked",
    },
    OwnershipRule {
        name: "player pose commit adapter",
        module_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        definition_anchor: "pub(in crate::play) fn commit_player_pose",
    },
    OwnershipRule {
        name: "accepted player pose publication adapter",
        module_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        definition_anchor: "fn publish_accepted_player_pose",
    },
    OwnershipRule {
        name: "player body push adapter",
        module_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_pose_adapter.rs",
        definition_anchor: "fn push_entities_from_player",
    },
    OwnershipRule {
        name: "bed occupancy planning",
        module_file: "crates/mc-net/src/play/beds.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod beds;",
        definition_file: "crates/mc-net/src/play/beds.rs",
        definition_anchor: "pub(super) fn plan_bed_occupied_edits",
    },
    OwnershipRule {
        name: "loaded bed interaction planning",
        module_file: "crates/mc-net/src/play/beds.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod beds;",
        definition_file: "crates/mc-net/src/play/beds.rs",
        definition_anchor: "pub(super) fn plan_loaded_bed_interaction",
    },
    OwnershipRule {
        name: "bed wake planning",
        module_file: "crates/mc-net/src/play/beds.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod beds;",
        definition_file: "crates/mc-net/src/play/beds.rs",
        definition_anchor: "pub(super) fn safe_bed_wake_pose",
    },
    OwnershipRule {
        name: "sleep morning calculation",
        module_file: "crates/mc-net/src/play/beds.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod beds;",
        definition_file: "crates/mc-net/src/play/beds.rs",
        definition_anchor: "pub(in crate::play) fn next_morning_time",
    },
    OwnershipRule {
        name: "falling block start chunks",
        module_file: "crates/mc-net/src/play/falling_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod falling_blocks;",
        definition_file: "crates/mc-net/src/play/falling_blocks.rs",
        definition_anchor: "pub(super) fn falling_block_start_chunks",
    },
    OwnershipRule {
        name: "falling block start planning",
        module_file: "crates/mc-net/src/play/falling_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod falling_blocks;",
        definition_file: "crates/mc-net/src/play/falling_blocks.rs",
        definition_anchor: "pub(super) fn plan_falling_block_starts",
    },
    OwnershipRule {
        name: "falling block landing chunks",
        module_file: "crates/mc-net/src/play/falling_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod falling_blocks;",
        definition_file: "crates/mc-net/src/play/falling_blocks.rs",
        definition_anchor: "pub(super) fn falling_block_landing_chunks",
    },
    OwnershipRule {
        name: "falling block landing planning",
        module_file: "crates/mc-net/src/play/falling_blocks.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod falling_blocks;",
        definition_file: "crates/mc-net/src/play/falling_blocks.rs",
        definition_anchor: "pub(super) fn plan_falling_block_landings",
    },
    OwnershipRule {
        name: "player command execution adapter",
        module_file: "crates/mc-net/src/play/command_execution.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod command_execution;",
        definition_file: "crates/mc-net/src/play/command_execution.rs",
        definition_anchor: "pub(super) async fn execute_player_command",
    },
    OwnershipRule {
        name: "player game mode command adapter",
        module_file: "crates/mc-net/src/play/command_execution.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod command_execution;",
        definition_file: "crates/mc-net/src/play/command_execution.rs",
        definition_anchor: "pub(super) async fn apply_game_mode",
    },
    OwnershipRule {
        name: "client command execution adapter",
        module_file: "crates/mc-net/src/play/command_execution.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod command_execution;",
        definition_file: "crates/mc-net/src/play/command_execution.rs",
        definition_anchor: "pub(super) async fn handle_client_command",
    },
    OwnershipRule {
        name: "conditional block storage commit adapter",
        module_file: "crates/mc-net/src/play/block_edit_commit.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_edit_commit;",
        definition_file: "crates/mc-net/src/play/block_edit_commit.rs",
        definition_anchor: "pub(super) fn apply_block_edit_batch_to_storage_conditionally",
    },
    OwnershipRule {
        name: "visible block edit finalization adapter",
        module_file: "crates/mc-net/src/play/block_edit_commit.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_edit_commit;",
        definition_file: "crates/mc-net/src/play/block_edit_commit.rs",
        definition_anchor: "pub(super) async fn finalize_visible_block_edit_outcome",
    },
    OwnershipRule {
        name: "opaque block entity conditional commit adapter",
        module_file: "crates/mc-net/src/play/block_edit_commit.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_edit_commit;",
        definition_file: "crates/mc-net/src/play/block_edit_commit.rs",
        definition_anchor: "pub(super) fn apply_opaque_block_entity_to_storage_conditionally",
    },
    OwnershipRule {
        name: "player block edit acknowledgement adapter",
        module_file: "crates/mc-net/src/play/block_edit_commit.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod block_edit_commit;",
        definition_file: "crates/mc-net/src/play/block_edit_commit.rs",
        definition_anchor: "pub(super) async fn apply_player_block_edit_batch_conditionally",
    },
    OwnershipRule {
        name: "player state event adapter",
        module_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        definition_anchor: "pub(in crate::play) fn commit_player_state_event(",
    },
    OwnershipRule {
        name: "player animation publication adapter",
        module_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        definition_anchor: "pub(in crate::play) fn broadcast_player_animation(",
    },
    OwnershipRule {
        name: "player entity data publication adapter",
        module_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        definition_anchor: "pub(in crate::play) fn broadcast_player_entity_data(",
    },
    OwnershipRule {
        name: "player including-self entity data publication adapter",
        module_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_state_adapter;",
        definition_file: "crates/mc-net/src/play/session/player_state_adapter.rs",
        definition_anchor: "pub(in crate::play) fn broadcast_player_entity_data_including_self(",
    },
    OwnershipRule {
        name: "bucket interaction adapter",
        module_file: "crates/mc-net/src/play/bucket_interactions.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod bucket_interactions;",
        definition_file: "crates/mc-net/src/play/bucket_interactions.rs",
        definition_anchor: "pub(super) async fn handle_bucket_use_on",
    },
    OwnershipRule {
        name: "cauldron bucket interaction adapter",
        module_file: "crates/mc-net/src/play/bucket_interactions.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod bucket_interactions;",
        definition_file: "crates/mc-net/src/play/bucket_interactions.rs",
        definition_anchor: "pub(super) async fn handle_cauldron_bucket_use_on",
    },
    OwnershipRule {
        name: "bucket simulation response adapter",
        module_file: "crates/mc-net/src/play/bucket_interactions.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod bucket_interactions;",
        definition_file: "crates/mc-net/src/play/bucket_interactions.rs",
        definition_anchor: "async fn commit_bucket_use_and_respond",
    },
    OwnershipRule {
        name: "bucket inventory replacement",
        module_file: "crates/mc-net/src/play/bucket_interactions.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod bucket_interactions;",
        definition_file: "crates/mc-net/src/play/bucket_interactions.rs",
        definition_anchor: "pub(in crate::play) fn plan_bucket_replacement",
    },
    OwnershipRule {
        name: "external disconnect publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(crate) fn disconnect_player(",
    },
    OwnershipRule {
        name: "external custom payload publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(crate) fn send_custom_payload(",
    },
    OwnershipRule {
        name: "system chat publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(in crate::play) fn broadcast_system_chat(",
    },
    OwnershipRule {
        name: "direct script chat publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(crate) fn send_script_system_chat(",
    },
    OwnershipRule {
        name: "broadcast script chat publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(crate) fn broadcast_script_system_chat(",
    },
    OwnershipRule {
        name: "outbound pressure debug publication adapter",
        module_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod outbound_publication;",
        definition_file: "crates/mc-net/src/play/session/outbound_publication.rs",
        definition_anchor: "pub(in crate::play) fn debug_outbound_pressure_dispatches(",
    },
    OwnershipRule {
        name: "fall damage adapter",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) async fn apply_fall_damage",
    },
    OwnershipRule {
        name: "contact block damage adapter",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) async fn apply_contact_block_damage",
    },
    OwnershipRule {
        name: "player damage publication adapter",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) fn apply_player_damage_publication",
    },
    OwnershipRule {
        name: "applied player damage publication DTO",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) struct AppliedPlayerDamagePublication",
    },
    OwnershipRule {
        name: "player damage application DTO",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) struct PlayerDamageApplication",
    },
    OwnershipRule {
        name: "general player damage adapter",
        module_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        parent_file: "crates/mc-net/src/play.rs",
        mod_declaration: "mod player_damage_adapter;",
        definition_file: "crates/mc-net/src/play/player_damage_adapter.rs",
        definition_anchor: "pub(super) async fn apply_player_damage",
    },
    OwnershipRule {
        name: "entity interaction geometry",
        module_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod interaction_geometry;",
        definition_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        definition_anchor: "pub(super) fn entity_geometry",
    },
    OwnershipRule {
        name: "block reach geometry",
        module_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod interaction_geometry;",
        definition_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        definition_anchor: "pub(in crate::play) fn within_block_reach",
    },
    OwnershipRule {
        name: "entity reach geometry",
        module_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod interaction_geometry;",
        definition_file: "crates/mc-net/src/play/session/interaction_geometry.rs",
        definition_anchor: "pub(in crate::play) fn within_entity_reach",
    },
    OwnershipRule {
        name: "player body push authority",
        module_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_authority;",
        definition_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        definition_anchor: "pub(super) fn plan_entities_from_player_candidate_geometry_locked",
    },
    OwnershipRule {
        name: "player body push stale fence",
        module_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_authority;",
        definition_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        definition_anchor: "pub(super) fn filter_current_expected_entity_snapshots",
    },
    OwnershipRule {
        name: "player body push publication",
        module_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_pose_authority;",
        definition_file: "crates/mc-net/src/play/session/player_pose_authority.rs",
        definition_anchor: "pub(super) fn publish_player_body_pushes_locked",
    },
    OwnershipRule {
        name: "TNT ignition item authority",
        module_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_item_action_authority;",
        definition_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_tnt_ignition",
    },
    OwnershipRule {
        name: "food use item authority",
        module_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_item_action_authority;",
        definition_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_food_use",
    },
    OwnershipRule {
        name: "bow release item authority",
        module_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_item_action_authority;",
        definition_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_bow_release",
    },
    OwnershipRule {
        name: "selected item drop authority",
        module_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod player_item_action_authority;",
        definition_file: "crates/mc-net/src/play/session/player_item_action_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_selected_item_drop",
    },
    OwnershipRule {
        name: "passive mobs",
        module_file: "crates/mc-net/src/play/session/passive_mobs.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod passive_mobs;",
        definition_file: "crates/mc-net/src/play/session/passive_mobs.rs",
        definition_anchor: "pub(super) fn plan_breeding",
    },
    OwnershipRule {
        name: "passive mob authority",
        module_file: "crates/mc-net/src/play/session/passive_mobs/authority.rs",
        parent_file: "crates/mc-net/src/play/session/passive_mobs.rs",
        mod_declaration: "mod authority;",
        definition_file: "crates/mc-net/src/play/session/passive_mobs/authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_animal_feed",
    },
    OwnershipRule {
        name: "persistence projection",
        module_file: "crates/mc-net/src/play/session/entity_simulation/persistence_projection.rs",
        parent_file: "crates/mc-net/src/play/session/entity_simulation.rs",
        mod_declaration: "mod persistence_projection;",
        definition_file: "crates/mc-net/src/play/session/entity_simulation/persistence_projection.rs",
        definition_anchor: "pub(super) fn project_owner_save",
    },
    OwnershipRule {
        name: "entity combat",
        module_file: "crates/mc-net/src/play/session/entity_combat.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_combat;",
        definition_file: "crates/mc-net/src/play/session/entity_combat.rs",
        definition_anchor: "pub(in crate::play) struct ServerEntityPlayerAttack",
    },
    OwnershipRule {
        name: "pickup authority",
        module_file: "crates/mc-net/src/play/session/pickups.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod pickups;",
        definition_file: "crates/mc-net/src/play/session/pickups.rs",
        definition_anchor: "pub(in crate::play) struct CreditedItemPickup",
    },
    OwnershipRule {
        name: "visibility publication",
        module_file: "crates/mc-net/src/play/session/visibility.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod visibility;",
        definition_file: "crates/mc-net/src/play/session/visibility.rs",
        definition_anchor: "pub(in crate::play) fn server_entity_snapshot_from",
    },
    OwnershipRule {
        name: "visibility movement",
        module_file: "crates/mc-net/src/play/session/visibility.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod visibility;",
        definition_file: "crates/mc-net/src/play/session/visibility.rs",
        definition_anchor: "pub(super) fn publish_entity_movement_locked",
    },
    OwnershipRule {
        name: "visibility mirror",
        module_file: "crates/mc-net/src/play/session/visibility.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod visibility;",
        definition_file: "crates/mc-net/src/play/session/visibility.rs",
        definition_anchor: "pub(super) fn refresh_visibility_locked",
    },
    OwnershipRule {
        name: "projectile spawn",
        module_file: "crates/mc-net/src/play/session/projectiles.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod projectiles;",
        definition_file: "crates/mc-net/src/play/session/projectiles.rs",
        definition_anchor: "pub(super) fn spawn_arrow_locked",
    },
    OwnershipRule {
        name: "projectile hit authority",
        module_file: "crates/mc-net/src/play/session/projectiles.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod projectiles;",
        definition_file: "crates/mc-net/src/play/session/projectiles.rs",
        definition_anchor: "pub(super) fn resolve_arrow_entity_hits_locked",
    },
    OwnershipRule {
        name: "projectile geometry",
        module_file: "crates/mc-net/src/play/session/projectiles.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod projectiles;",
        definition_file: "crates/mc-net/src/play/session/projectiles.rs",
        definition_anchor: "pub(super) fn segment_aabb_intersection_t",
    },
    OwnershipRule {
        name: "container registry shards",
        module_file: "crates/mc-net/src/play/session/container_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod container_state;",
        definition_file: "crates/mc-net/src/play/session/container_state.rs",
        definition_anchor: "pub(super) struct ContainerRegistryShards",
    },
    OwnershipRule {
        name: "container registry guard",
        module_file: "crates/mc-net/src/play/session/container_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod container_state;",
        definition_file: "crates/mc-net/src/play/session/container_state.rs",
        definition_anchor: "pub(super) struct ContainerRegistryGuard<'a>",
    },
    OwnershipRule {
        name: "container recipient planning",
        module_file: "crates/mc-net/src/play/session/container_state.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod container_state;",
        definition_file: "crates/mc-net/src/play/session/container_state.rs",
        definition_anchor: "pub(super) fn chest_recipients",
    },
    OwnershipRule {
        name: "campfire use authority",
        module_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod campfire_authority;",
        definition_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        definition_anchor: "pub(in crate::play) fn commit_campfire_use",
    },
    OwnershipRule {
        name: "campfire cooking authority",
        module_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod campfire_authority;",
        definition_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        definition_anchor: "pub(in crate::play) fn tick_campfire_cooking_conditionally",
    },
    OwnershipRule {
        name: "campfire regional transaction",
        module_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod campfire_authority;",
        definition_file: "crates/mc-net/src/play/session/campfire_authority.rs",
        definition_anchor: "pub(in crate::play) fn prepare_campfire_use_transaction",
    },
    OwnershipRule {
        name: "expired TNT authority",
        module_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod explosion_authority;",
        definition_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        definition_anchor: "pub(in crate::play) struct ExpiredPrimedTnt",
    },
    OwnershipRule {
        name: "primed TNT claim",
        module_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod explosion_authority;",
        definition_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        definition_anchor: "pub(in crate::play) fn claim_due_primed_tnt",
    },
    OwnershipRule {
        name: "explosion entity impact",
        module_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod explosion_authority;",
        definition_file: "crates/mc-net/src/play/session/explosion_authority.rs",
        definition_anchor: "pub(in crate::play) fn apply_explosion_entity_impacts",
    },
    OwnershipRule {
        name: "hostile attack authority",
        module_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod hostile_authority;",
        definition_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        definition_anchor: "pub(in crate::play) fn tick_hostile_attacks",
    },
    OwnershipRule {
        name: "hostile target refresh",
        module_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod hostile_authority;",
        definition_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        definition_anchor: "pub(super) fn update_hostile_targets",
    },
    OwnershipRule {
        name: "hostile bed exclusion",
        module_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod hostile_authority;",
        definition_file: "crates/mc-net/src/play/session/hostile_authority.rs",
        definition_anchor: "pub(in crate::play) fn has_rest_preventing_hostile_near_bed",
    },
    OwnershipRule {
        name: "herd spawn outcome",
        module_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod herd_spawn_authority;",
        definition_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        definition_anchor: "pub(in crate::play) struct HerdSpawnOutcome",
    },
    OwnershipRule {
        name: "pending hostile activation",
        module_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod herd_spawn_authority;",
        definition_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        definition_anchor: "pub(in crate::play) fn activate_pending_hostiles_owned",
    },
    OwnershipRule {
        name: "natural spawn domain",
        module_file: "crates/mc-entity/src/natural_spawn_26_1_2.rs",
        parent_file: "crates/mc-entity/src/lib.rs",
        mod_declaration: "pub mod natural_spawn_26_1_2;",
        definition_file: "crates/mc-entity/src/natural_spawn_26_1_2.rs",
        definition_anchor: "pub struct HerdSpawn",
    },
    OwnershipRule {
        name: "herd candidate planning",
        module_file: "crates/mc-entity/src/natural_spawn_26_1_2/planning.rs",
        parent_file: "crates/mc-entity/src/natural_spawn_26_1_2.rs",
        mod_declaration: "mod planning;",
        definition_file: "crates/mc-entity/src/natural_spawn_26_1_2/planning.rs",
        definition_anchor: "pub fn build_herd_spawn_candidates",
    },
    OwnershipRule {
        name: "periodic natural spawn scheduler",
        module_file: "crates/mc-entity/src/natural_spawn_26_1_2/scheduler.rs",
        parent_file: "crates/mc-entity/src/natural_spawn_26_1_2.rs",
        mod_declaration: "mod scheduler;",
        definition_file: "crates/mc-entity/src/natural_spawn_26_1_2/scheduler.rs",
        definition_anchor: "pub struct NaturalSpawnScheduler",
    },
    OwnershipRule {
        name: "periodic natural spawn planning",
        module_file: "crates/mc-entity/src/natural_spawn_26_1_2/planning.rs",
        parent_file: "crates/mc-entity/src/natural_spawn_26_1_2.rs",
        mod_declaration: "mod planning;",
        definition_file: "crates/mc-entity/src/natural_spawn_26_1_2/planning.rs",
        definition_anchor: "pub fn plan_periodic_category",
    },
    OwnershipRule {
        name: "periodic natural spawn authority",
        module_file: "crates/mc-net/src/play/session/herd_spawn_authority/periodic.rs",
        parent_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        mod_declaration: "mod periodic;",
        definition_file: "crates/mc-net/src/play/session/herd_spawn_authority/periodic.rs",
        definition_anchor: "pub(in crate::play) fn tick_periodic_natural_spawning",
    },
    OwnershipRule {
        name: "natural spawn commit publication",
        module_file: "crates/mc-net/src/play/session/herd_spawn_authority/commit.rs",
        parent_file: "crates/mc-net/src/play/session/herd_spawn_authority.rs",
        mod_declaration: "mod commit;",
        definition_file: "crates/mc-net/src/play/session/herd_spawn_authority/commit.rs",
        definition_anchor: "pub(in crate::play::session) fn install_committed_herd_spawns_locked",
    },
    OwnershipRule {
        name: "natural spawn ticker",
        module_file: "crates/mc-net/src/server/natural_spawn_ticker.rs",
        parent_file: "crates/mc-net/src/server.rs",
        mod_declaration: "mod natural_spawn_ticker;",
        definition_file: "crates/mc-net/src/server/natural_spawn_ticker.rs",
        definition_anchor: "pub(super) struct NaturalSpawnTicker",
    },
    OwnershipRule {
        name: "entity spawn facts",
        module_file: "crates/mc-net/src/play/session/entity_spawn_facts.rs",
        parent_file: "crates/mc-net/src/play/session.rs",
        mod_declaration: "mod entity_spawn_facts;",
        definition_file: "crates/mc-net/src/play/session/entity_spawn_facts.rs",
        definition_anchor: "pub(in crate::play::session) fn apply_entity_facts",
    },
    OwnershipRule {
        name: "simulation queue snapshot",
        module_file: "crates/mc-net/src/play/simulation/queue.rs",
        parent_file: "crates/mc-net/src/play/simulation.rs",
        mod_declaration: "mod queue;",
        definition_file: "crates/mc-net/src/play/simulation/queue.rs",
        definition_anchor: "pub(crate) struct SimulationQueueSnapshot",
    },
    OwnershipRule {
        name: "simulation queue shutdown",
        module_file: "crates/mc-net/src/play/simulation/queue.rs",
        parent_file: "crates/mc-net/src/play/simulation.rs",
        mod_declaration: "mod queue;",
        definition_file: "crates/mc-net/src/play/simulation/queue.rs",
        definition_anchor: "pub(crate) fn shutdown(&mut self)",
    },
    OwnershipRule {
        name: "regional block edit job",
        module_file: "crates/mc-net/src/play/simulation/regional_mutation.rs",
        parent_file: "crates/mc-net/src/play/simulation.rs",
        mod_declaration: "mod regional_mutation;",
        definition_file: "crates/mc-net/src/play/simulation/regional_mutation.rs",
        definition_anchor: "struct RegionalBlockEditJob",
    },
    OwnershipRule {
        name: "regional block edit execution",
        module_file: "crates/mc-net/src/play/simulation/regional_mutation.rs",
        parent_file: "crates/mc-net/src/play/simulation.rs",
        mod_declaration: "mod regional_mutation;",
        definition_file: "crates/mc-net/src/play/simulation/regional_mutation.rs",
        definition_anchor: "async fn process_regional_block_edit_run",
    },
    OwnershipRule {
        name: "connection driver",
        module_file: "crates/mc-net/src/connection_driver.rs",
        parent_file: "crates/mc-net/src/lib.rs",
        mod_declaration: "mod connection_driver;",
        definition_file: "crates/mc-net/src/connection_driver.rs",
        definition_anchor: "pub(crate) struct ConnectionServices",
    },
];

const LEGACY_MC_NET_PARENTS: &[&str] = &[
    "crates/mc-net/src/play.rs",
    "crates/mc-net/src/play/session.rs",
    "crates/mc-net/src/server.rs",
];

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("code-health") => {
            if let Some(arg) = args.next() {
                eprintln!("code-health: unknown option {arg:?}");
                std::process::exit(2);
            }
            if let Err(code) = run_code_health() {
                std::process::exit(code);
            }
        }
        _ => {
            eprintln!("usage: cargo run -p xtask -- code-health");
            std::process::exit(2);
        }
    }
}

fn run_code_health() -> Result<(), i32> {
    let root = workspace_root().map_err(|err| {
        eprintln!("code-health: {err}");
        2
    })?;
    let mut findings = Vec::new();
    scan_mc_net_ownership(&root, MC_NET_OWNERSHIP, &mut findings);
    scan_rust_sources(&root.join("crates"), &mut findings);
    scan_api_manifests(&root, &mut findings);

    println!("Solaris code-health report");
    println!();

    if findings.is_empty() {
        println!("summary: 0 fail");
        println!("verdict: KEEP");
        return Ok(());
    }

    for finding in &findings {
        println!(
            "FAIL: {}:{}: {}",
            display_path(&root, &finding.path),
            finding.line,
            finding.message
        );
    }

    println!();
    println!("summary: {} fail", findings.len());
    println!("verdict: CLEANUP_REQUIRED");
    Err(1)
}

fn workspace_root() -> Result<PathBuf, String> {
    let mut dir = env::current_dir().map_err(|err| format!("current_dir failed: {err}"))?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("could not find workspace root with Cargo.toml and crates/".into());
        }
    }
}

fn scan_rust_sources(dir: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            scan_rust_sources(&path, findings);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            scan_rust_file(&path, findings);
        }
    }
}

fn scan_mc_net_ownership(root: &Path, rules: &[OwnershipRule], findings: &mut Vec<Finding>) {
    for rule in rules {
        let module_path = root.join(rule.module_file);
        let module_source = fs::read_to_string(&module_path).ok();
        if module_source.is_none() {
            findings.push(Finding {
                path: module_path.clone(),
                line: 1,
                message: format!("{} owner module file is missing", rule.name),
            });
        }

        let parent_path = root.join(rule.parent_file);
        if let Ok(parent_source) = fs::read_to_string(&parent_path) {
            if !parent_source
                .lines()
                .any(|line| line.trim() == rule.mod_declaration)
            {
                findings.push(Finding {
                    path: parent_path.clone(),
                    line: 1,
                    message: format!(
                        "{} parent module is missing `{}`",
                        rule.name, rule.mod_declaration
                    ),
                });
            }
            if let Some(line) = source_line(&parent_source, rule.definition_anchor) {
                findings.push(Finding {
                    path: parent_path.clone(),
                    line,
                    message: format!("{} definition returned to a parent module", rule.name),
                });
            }
        } else {
            findings.push(Finding {
                path: parent_path.clone(),
                line: 1,
                message: format!("{} parent module file is missing", rule.name),
            });
        }

        let definition_path = root.join(rule.definition_file);
        let separate_definition_source = if definition_path == module_path {
            None
        } else {
            match fs::read_to_string(&definition_path) {
                Ok(source) => Some(source),
                Err(_) => {
                    findings.push(Finding {
                        path: definition_path.clone(),
                        line: 1,
                        message: format!("{} definition file is missing", rule.name),
                    });
                    None
                }
            }
        };
        let definition_source = if definition_path == module_path {
            module_source.as_deref()
        } else {
            separate_definition_source.as_deref()
        };
        if definition_source.is_some_and(|source| !source.contains(rule.definition_anchor)) {
            findings.push(Finding {
                path: definition_path,
                line: 1,
                message: format!(
                    "{} owner module is missing definition anchor `{}`",
                    rule.name, rule.definition_anchor
                ),
            });
        }

        for legacy_parent in LEGACY_MC_NET_PARENTS {
            if *legacy_parent == rule.parent_file {
                continue;
            }
            let legacy_path = root.join(legacy_parent);
            let Ok(legacy_source) = fs::read_to_string(&legacy_path) else {
                continue;
            };
            if let Some(line) = source_line(&legacy_source, rule.definition_anchor) {
                findings.push(Finding {
                    path: legacy_path,
                    line,
                    message: format!("{} definition returned to a parent module", rule.name),
                });
            }
        }
    }
}

fn source_line(source: &str, anchor: &str) -> Option<usize> {
    source.find(anchor).map(|offset| {
        source[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1
    })
}

fn scan_rust_file(path: &Path, findings: &mut Vec<Finding>) {
    let Ok(source) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = source.lines().collect();
    scan_generic_modules(path, &lines, findings);
    scan_combat_boundary(path, &lines, findings);
    scan_player_combat_adapter(path, &lines, findings);
    scan_player_combat_shared_shield(path, &lines, findings);
    scan_command_execution_adapter(path, &lines, findings);
    scan_block_edit_commit_adapter(path, &lines, findings);
    scan_bucket_interaction_adapter(path, &lines, findings);
    scan_campfire_adapter(path, &lines, findings);
    scan_use_item_on_adapter(path, &lines, findings);
    scan_player_damage_adapter(path, &lines, findings);
    scan_interaction_geometry(path, &lines, findings);
    scan_container_facade(path, &lines, findings);
    scan_explicit_play_boundaries(path, &lines, findings);
    scan_api_leaks(path, &lines, findings);
}

fn scan_container_facade(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/containers.rs") {
        return;
    }
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if normalized.contains("usesuper::*") || normalized.contains("usecrate::play::*") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "container facade bypasses its explicit module contract".into(),
            });
        }
    }
}

fn cfg_test_item_lines(lines: &[&str]) -> Vec<bool> {
    let mut result = vec![false; lines.len()];
    let mut brace_depth = 0usize;
    let mut test_scope_depths = Vec::new();
    let mut pending_test_item = false;

    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if normalized.starts_with("#[cfg(test)]") {
            pending_test_item = true;
        }
        result[index] = pending_test_item || !test_scope_depths.is_empty();

        let opens = line.bytes().filter(|byte| *byte == b'{').count();
        let closes = line.bytes().filter(|byte| *byte == b'}').count();
        let next_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
        if pending_test_item && opens > 0 {
            if opens > closes {
                test_scope_depths.push(next_depth);
            }
            pending_test_item = false;
        } else if pending_test_item && normalized.ends_with(';') && !normalized.starts_with("#[") {
            pending_test_item = false;
        }
        brace_depth = next_depth;
        test_scope_depths.retain(|depth| brace_depth >= *depth);
    }

    result
}

fn glob_use_item_starts(lines: &[&str], scope: &str) -> Vec<bool> {
    let mut result = vec![false; lines.len()];
    let mut start = None;
    let mut item = String::new();

    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if start.is_none() {
            if !normalized.starts_with("use") {
                continue;
            }
            start = Some(index);
            item.clear();
        }
        item.push_str(&normalized);
        if item.contains(';') {
            if item.contains(scope)
                && item.contains('*')
                && let Some(start) = start
            {
                result[start] = true;
            }
            start = None;
            item.clear();
        }
    }

    result
}

fn scan_explicit_play_boundaries(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let path_text = path.to_string_lossy();
    let herd_spawn_facade = path.ends_with("mc-net/src/play/session/herd_spawn_authority.rs");
    let herd_spawn_authority_submodule =
        path_text.contains("mc-net/src/play/session/herd_spawn_authority/");
    let natural_spawn_ticker = path.ends_with("mc-net/src/server/natural_spawn_ticker.rs");
    let natural_spawn_planning = path.ends_with("mc-entity/src/natural_spawn_26_1_2/planning.rs");
    let natural_spawn_scheduler = path.ends_with("mc-entity/src/natural_spawn_26_1_2/scheduler.rs");
    let natural_spawn_domain = path.ends_with("mc-entity/src/natural_spawn_26_1_2.rs")
        || path_text.contains("mc-entity/src/natural_spawn_26_1_2/");
    let entity_spawn_facts = path.ends_with("mc-net/src/play/session/entity_spawn_facts.rs");
    let forbid_toggle_runtime = path.ends_with("mc-net/src/play/toggles.rs");
    let forbid_random_runtime = path.ends_with("mc-net/src/play/random_ticks.rs");
    let forbid_authority_production_async = path
        .ends_with("mc-net/src/play/session/hostile_authority.rs")
        || path.ends_with("mc-net/src/play/session/herd_spawn_authority.rs")
        || herd_spawn_authority_submodule;
    let forbid_scheduled_block_runtime = path.ends_with("mc-net/src/play/scheduled_blocks.rs");
    let forbid_lighting_runtime = path.ends_with("mc-net/src/play/lighting.rs");
    let forbid_block_placement_runtime = path.ends_with("mc-net/src/play/block_placement.rs");
    let forbid_movement_runtime = path.ends_with("mc-net/src/play/movement.rs");
    let forbid_player_action_runtime = path.ends_with("mc-net/src/play/combat/player_actions.rs");
    let forbid_survival_action_runtime =
        path.ends_with("mc-net/src/play/session/survival_action_authority.rs");
    let forbid_entity_lifecycle_runtime =
        path.ends_with("mc-net/src/play/session/entity_lifecycle.rs");
    let forbid_session_lifecycle_sleep =
        path.ends_with("mc-net/src/play/session/session_lifecycle.rs");
    let forbid_player_pose_runtime =
        path.ends_with("mc-net/src/play/session/player_pose_authority.rs");
    let forbid_bed_runtime = path.ends_with("mc-net/src/play/beds.rs");
    let forbid_player_item_action_runtime =
        path.ends_with("mc-net/src/play/session/player_item_action_authority.rs");
    let forbid_falling_block_runtime = path.ends_with("mc-net/src/play/falling_blocks.rs");
    let forbid_player_state_runtime = path.ends_with("mc-net/src/play/session/player_state.rs");
    let forbid_player_pose_adapter_runtime =
        path.ends_with("mc-net/src/play/session/player_pose_adapter.rs");
    let forbid_player_state_adapter_runtime =
        path.ends_with("mc-net/src/play/session/player_state_adapter.rs");
    let forbid_outbound_publication_runtime =
        path.ends_with("mc-net/src/play/session/outbound_publication.rs");
    let forbid_chunk_view_production_async =
        path.ends_with("mc-net/src/play/session/chunk_view_authority.rs");
    let (boundary, forbid_outbound, forbid_survival, forbid_direct_send) =
        if path.ends_with("mc-net/src/play/containers/furnace.rs") {
            ("furnace domain", true, false, false)
        } else if path.ends_with("mc-net/src/play/containers/chest.rs") {
            ("chest domain", true, false, false)
        } else if path.ends_with("mc-net/src/play/containers/crafting.rs") {
            ("crafting domain", true, false, true)
        } else if path.ends_with("mc-net/src/play/containers/enchanting.rs") {
            ("enchanting domain", true, false, false)
        } else if path.ends_with("mc-net/src/play/containers/stonecutter.rs") {
            ("stonecutter domain", true, false, false)
        } else if path.ends_with("mc-net/src/play/campfire.rs") {
            ("campfire domain", true, true, false)
        } else if path.ends_with("mc-net/src/play/fluids.rs") {
            ("fluid rules", true, false, false)
        } else if path.ends_with("mc-net/src/play/toggles.rs") {
            ("toggle rules", true, false, false)
        } else if path.ends_with("mc-net/src/play/random_ticks.rs") {
            ("random tick rules", true, false, true)
        } else if path.ends_with("mc-net/src/play/scheduled_blocks.rs") {
            ("scheduled block domain", true, false, true)
        } else if path.ends_with("mc-net/src/play/lighting.rs") {
            ("incremental lighting", true, false, true)
        } else if path.ends_with("mc-net/src/play/block_placement.rs") {
            ("block placement rules", true, false, true)
        } else if path.ends_with("mc-net/src/play/movement.rs") {
            ("player movement rules", true, false, true)
        } else if path.ends_with("mc-net/src/play/combat/player_actions.rs") {
            ("player action rules", true, true, true)
        } else if path.ends_with("mc-net/src/play/session/player_state.rs") {
            ("player state adapter", true, false, false)
        } else if path.ends_with("mc-net/src/play/session/pickups.rs") {
            ("pickup authority", false, false, false)
        } else if path.ends_with("mc-net/src/play/session/visibility.rs") {
            ("visibility publication", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/projectiles.rs") {
            ("projectile authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/container_state.rs") {
            ("container state", false, false, false)
        } else if path.ends_with("mc-net/src/play/session/container_views.rs") {
            ("container views", false, false, false)
        } else if path.ends_with("mc-net/src/play/session/transactions.rs") {
            ("session transactions", false, false, false)
        } else if path.ends_with("mc-net/src/play/session/campfire_authority.rs") {
            ("campfire session authority", true, false, true)
        } else if path.ends_with("mc-net/src/play/session/explosion_authority.rs") {
            ("explosion session authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/hostile_authority.rs") {
            ("hostile session authority", false, false, true)
        } else if natural_spawn_domain {
            ("natural spawn entity domain", true, false, true)
        } else if path.ends_with("mc-net/src/play/session/herd_spawn_authority.rs") {
            ("herd spawn authority", false, false, true)
        } else if herd_spawn_authority_submodule {
            ("herd spawn authority submodule", false, false, true)
        } else if natural_spawn_ticker {
            ("natural spawn ticker", true, false, true)
        } else if entity_spawn_facts {
            ("entity spawn facts", true, false, true)
        } else if path.ends_with("mc-net/src/play/session/chunk_view_authority.rs") {
            ("chunk view authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/survival_action_authority.rs") {
            ("survival action authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/entity_lifecycle.rs") {
            ("entity lifecycle authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/session_lifecycle.rs") {
            ("session registration lifecycle", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/player_pose_authority.rs") {
            ("player pose authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/beds.rs") {
            ("bed rules", true, false, true)
        } else if path.ends_with("mc-net/src/play/session/player_item_action_authority.rs") {
            ("player item action authority", false, false, true)
        } else if path.ends_with("mc-net/src/play/falling_blocks.rs") {
            ("falling block rules", true, false, true)
        } else if path.ends_with("mc-net/src/play/session/player_pose_adapter.rs") {
            ("player pose adapter", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/player_state_adapter.rs") {
            ("player state adapter", false, false, true)
        } else if path.ends_with("mc-net/src/play/session/outbound_publication.rs") {
            ("outbound publication adapter", false, false, true)
        } else {
            return;
        };

    let test_only = cfg_test_item_lines(lines);
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        let parent_glob = parent_globs[index];
        let play_glob = play_globs[index];
        if parent_glob
            || play_glob
            || normalized.contains("InteractionState")
            || normalized.contains("ServerConfig")
            || (forbid_outbound && normalized.contains("OutboundCommand"))
            || (forbid_survival && normalized.contains("survival::"))
            || (forbid_direct_send
                && !test_only[index]
                && (normalized.contains(".send(") || normalized.contains("try_send(")))
            || (forbid_toggle_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "WorldWriter",
                    "WorldWrite",
                    "SessionRegistry",
                    "session::",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mc_protocol::packets",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_random_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "WorldWriter",
                    "WorldWrite",
                    "SessionRegistry",
                    "session::",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mpsc::",
                    "Sender<",
                    "Receiver<",
                    "mc_protocol::packets",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_authority_production_async
                && !test_only[index]
                && (normalized.contains("asyncfn") || normalized.contains(".await")))
            || (herd_spawn_facade
                && !test_only[index]
                && [
                    "WorldReadView",
                    "BlockMaterialIds",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (natural_spawn_ticker
                && [
                    "crate::play::session",
                    "crate::play::simulation",
                    "mc_entity",
                    "WorldStorage",
                    "WorldMutationView",
                    "ChunkLight",
                    "BlockPos",
                    "HerdSpawn",
                    "SpawnEntity",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "lock_inner",
                    "lock_entities",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || ((natural_spawn_planning || natural_spawn_scheduler || entity_spawn_facts)
                && [
                    "SessionRegistry",
                    "SessionRegistryInner",
                    "OutboundCommand",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "install_committed_entity_publications",
                    "WorldStorage",
                    "WorldMutationView",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mpsc",
                    "Sender<",
                    "Receiver<",
                    "mc_protocol::",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (natural_spawn_domain
                && [
                    "mc_net",
                    "crate::play",
                    "SessionRegistry",
                    "SessionRegistryInner",
                    "OutboundCommand",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "WorldStorage",
                    "WorldMutationView",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mpsc",
                    "Sender<",
                    "Receiver<",
                    "mc_protocol::",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (natural_spawn_scheduler
                && ["mc_net", "crate::play", "mc_physics", "mc_world"]
                    .iter()
                    .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_scheduled_block_runtime
                && [
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mc_protocol::packets",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_lighting_runtime
                && [
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_block_placement_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "SessionRegistry",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_movement_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "SessionRegistry",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "VisibilityDispatch",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_player_action_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "SessionRegistry",
                    "PlayerPersistence",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "OutboundCommand",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_survival_action_runtime
                && ["asyncfn", ".await", "tokio::", "sleep("]
                    .iter()
                    .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_entity_lifecycle_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "mpsc",
                    "Sender<",
                    "Receiver<",
                    "std::thread::spawn",
                    "spawn_blocking",
                    "sleep(",
                    "mc_protocol::",
                    "dispatch_visibility_commands",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_session_lifecycle_sleep
                && (normalized.contains("sleep(")
                    || normalized.contains("dispatch_visibility_command")))
            || (forbid_player_pose_runtime
                && [
                    "PlayerPersistence",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_bed_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "SessionRegistry",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_player_item_action_runtime
                && [
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_falling_block_runtime
                && [
                    "WorldStorage",
                    "WorldMutationView",
                    "WorldHandle",
                    "SessionRegistry",
                    "Mutex",
                    "RwLock",
                    "parking_lot",
                    ".lock(",
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "VisibilityDispatch",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_player_state_runtime
                && [
                    "sleep(",
                    "tokio::time",
                    "yield_now(",
                    ".send(",
                    "try_send(",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_player_pose_adapter_runtime
                && [
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "yield_now(",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_player_state_adapter_runtime
                && [
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "yield_now(",
                    "dispatch_visibility_commands",
                    "write_packet",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_outbound_publication_runtime
                && [
                    "asyncfn",
                    ".await",
                    "tokio::",
                    "sleep(",
                    "yield_now(",
                    "write_packet",
                    "mpsc",
                    "Sender",
                    "Receiver",
                    "Clientbound",
                    "Serverbound",
                    "WorldStorage",
                    "WorldMutationView",
                    "SimulationAuthority",
                    "PlayerPersistedState",
                    "lock_entities",
                    "lock_session_entities",
                    "Mutex",
                    "RwLock",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden)))
            || (forbid_chunk_view_production_async
                && !test_only[index]
                && (normalized.contains("asyncfn") || normalized.contains(".await")))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: format!("{boundary} bypasses its explicit module contract"),
            });
        }
    }
}

fn scan_player_combat_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/session/player_combat.rs") {
        return;
    }
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if normalized.contains("usesuper::*")
            || normalized.contains("usecrate::play::*")
            || normalized.contains("InteractionState")
            || normalized.contains("ServerConfig")
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "player combat adapter bypasses its explicit module contract".into(),
            });
        }
    }
}

fn scan_command_execution_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/command_execution.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "WorldStorage",
                "WorldMutationView",
                "Mutex",
                "RwLock",
                "parking_lot",
                ".lock(",
                "sleep(",
                "yield_now(",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "command execution adapter bypasses owner APIs".into(),
            });
        }
    }
}

fn scan_block_edit_commit_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/block_edit_commit.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "ServerConfig",
                "WorldMutationView",
                "Mutex",
                "RwLock",
                "parking_lot",
                "sleep(",
                "yield_now(",
                "interval(",
                "mpsc",
                "oneshot",
                "tokio::spawn",
                "spawn_blocking",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "block edit commit adapter bypasses its explicit owner boundary".into(),
            });
        }
    }

    let source = lines
        .iter()
        .map(|line| line.split_whitespace().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "#[cfg(test)]\nasyncfnapply_block_edit_batch_to_world_conditionally",
        "#[cfg(not(test))]\n{\nmatchstate\n.simulation\n.apply_block_edits_with_scheduled_ticks(",
        "#[cfg(not(test))]\nletbroadcast_peer_blocks=false;",
    ] {
        if !source.contains(required) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "block edit commit adapter lost a required production ownership fence"
                    .into(),
            });
        }
    }

    let player_edit_source = source
        .split_once("pub(super)asyncfnapply_player_block_edit_batch_conditionally")
        .map(|(_, source)| source)
        .unwrap_or_default();
    let resync =
        player_edit_source.find("send_loaded_block_edit_resyncs(state,writer,edits).await?;");
    let ack = player_edit_source.find("write_packet(writer,&BlockChangedAck{sequence}");
    if resync.is_none()
        || ack.is_none()
        || resync >= ack
        || player_edit_source
            .matches("write_packet(writer,&BlockChangedAck{sequence}")
            .count()
            != 1
    {
        findings.push(Finding {
            path: path.to_path_buf(),
            line: 1,
            message: "player block edit must resync before exactly one acknowledgement".into(),
        });
    }
}

fn scan_bucket_interaction_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/bucket_interactions.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "ServerConfig",
                "WorldStorage",
                "WorldMutationView",
                "Mutex",
                "RwLock",
                "parking_lot",
                ".lock(",
                "sleep(",
                "yield_now(",
                "interval(",
                "mpsc",
                "oneshot",
                "tokio::spawn",
                "spawn_blocking",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "bucket interaction adapter bypasses owner APIs".into(),
            });
        }
    }

    let source = lines
        .iter()
        .map(|line| line.split_whitespace().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let commit = source
        .split_once("asyncfncommit_bucket_use_and_respond")
        .and_then(|(_, source)| source.split_once("fnplan_cauldron_bucket_use"))
        .map(|(source, _)| source)
        .unwrap_or_default();
    let owner = commit.find("state.simulation.commit_bucket_use(plan).await");
    let resync = commit.find("send_loaded_block_edit_resyncs(state,writer,&[edit]).await?");
    let finalize = commit
        .find("finalize_visible_block_edit_outcome(state,writer,committed.block,false).await?");
    let animation = commit.find(
        "dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id))",
    );
    let acknowledgements = commit
        .match_indices("write_block_ack(writer,state.compression,sequence).await?")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let inventory_updates = commit
        .match_indices("write_inventory_slot_updates(")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let ordered = owner.zip(resync).zip(finalize).zip(animation).is_some_and(
        |(((owner, resync), finalize), animation)| {
            acknowledgements.len() == 2
                && inventory_updates.len() == 2
                && owner < resync
                && resync < inventory_updates[0]
                && inventory_updates[0] < acknowledgements[0]
                && acknowledgements[0] < finalize
                && finalize < acknowledgements[1]
                && acknowledgements[1] < inventory_updates[1]
                && inventory_updates[1] < animation
        },
    );
    if !ordered {
        findings.push(Finding {
            path: path.to_path_buf(),
            line: 1,
            message: "bucket response adapter lost owner or response ordering".into(),
        });
    }
}

fn scan_campfire_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/campfire_adapter.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "Mutex",
                "RwLock",
                "parking_lot",
                "sleep(",
                "yield_now(",
                "interval(",
                "mpsc",
                "oneshot",
                "tokio::spawn",
                "spawn_blocking",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "campfire adapter bypasses its explicit owner boundary".into(),
            });
        }
    }
}

fn scan_use_item_on_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/use_item_on_adapter.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "Mutex",
                "RwLock",
                "parking_lot",
                "sleep(",
                "yield_now(",
                "interval(",
                "mpsc",
                "oneshot",
                "tokio::spawn",
                "spawn_blocking",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "use-item-on adapter bypasses its explicit owner boundary".into(),
            });
        }
    }
}

fn scan_player_damage_adapter(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/player_damage_adapter.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "ServerConfig",
                "WorldStorage",
                "WorldMutationView",
                "SessionRegistry",
                "Mutex",
                "RwLock",
                "parking_lot",
                ".lock(",
                "sleep(",
                "yield_now(",
                "interval(",
                "mpsc",
                "oneshot",
                "tokio::spawn",
                "spawn_blocking",
                ".send(",
                "try_send(",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "player damage adapter bypasses owner APIs".into(),
            });
        }
    }

    let source = lines
        .iter()
        .map(|line| line.split_whitespace().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let publication = source
        .split_once("pub(super)fnapply_player_damage_publication")
        .and_then(|(_, source)| source.split_once("pub(super)structPlayerDamageApplication"))
        .map(|(source, _)| source)
        .unwrap_or_default();
    for required in [
        "lethealth_accepted=survival_state.health==publication.expected_health;",
        "ifhealth_accepted&&publication.died",
        "died:health_accepted&&publication.died",
        "fresh_hurt:health_accepted&&publication.fresh_hurt",
        "knockback:health_accepted.then_some(publication.knockback).flatten()",
    ] {
        if !publication.contains(required) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "player damage publication lost its accepted-health CAS fence".into(),
            });
            break;
        }
    }

    let damage = source
        .split_once("pub(super)asyncfnapply_player_damage")
        .map(|(_, source)| source)
        .unwrap_or_default();
    let shield_commit = damage.find("commit_player_survival_update_with_shield(");
    let retry_fence = damage.find("ifshield_commit_attempts>=2");
    let regular_commit = damage.find("commit_player_survival_update(");
    let fallback_packet =
        damage.find("write_packet(writer,&survival_state.as_packet(),compression).await?");
    let owner_flow_is_intact = shield_commit
        .zip(retry_fence)
        .zip(regular_commit)
        .zip(fallback_packet)
        .is_some_and(
            |(((shield_commit, retry_fence), regular_commit), fallback_packet)| {
                shield_commit < retry_fence
                    && retry_fence < regular_commit
                    && regular_commit < fallback_packet
                    && damage
                        .matches("commit_player_survival_update_with_shield(")
                        .count()
                        == 1
            },
        );
    if !owner_flow_is_intact {
        findings.push(Finding {
            path: path.to_path_buf(),
            line: 1,
            message: "player damage adapter lost its bounded owner commit flow".into(),
        });
    }
}

fn scan_interaction_geometry(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/session/interaction_geometry.rs") {
        return;
    }
    let parent_globs = glob_use_item_starts(lines, "super::");
    let play_globs = glob_use_item_starts(lines, "play::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index]
            || play_globs[index]
            || [
                "InteractionState",
                "SessionRegistry",
                "WorldStorage",
                "WorldHandle",
                "Mutex",
                "RwLock",
                "parking_lot",
                ".lock(",
                "asyncfn",
                ".await",
                "tokio::",
                "sleep(",
                "yield_now(",
                "mpsc",
                "Sender",
                "Receiver",
                "OutboundCommand",
                "VisibilityDispatch",
                "write_packet",
                "Clientbound",
                "Serverbound",
            ]
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "interaction geometry is not a pure dependency boundary".into(),
            });
        }
    }
}

fn scan_player_combat_shared_shield(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-net/src/play/session/player_combat.rs") {
        return;
    }
    let test_only = cfg_test_item_lines(lines);
    let production_source = lines
        .iter()
        .zip(&test_only)
        .filter_map(|(line, test_only)| (!test_only).then_some(*line))
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "shield_use_matches_slot(",
        "shield_blocks_damage_since(",
        "damage_active_shield_slot(",
    ] {
        if !production_source.contains(required) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: format!("player combat bypasses shared shield owner `{required}`"),
            });
        }
    }
    for (index, line) in lines.iter().enumerate() {
        if test_only[index] {
            continue;
        }
        let normalized = line.split_whitespace().collect::<String>();
        if [
            "fnshield_use_matches_slot(",
            "fnshield_blocks_damage_since(",
            "fndamage_active_shield_slot(",
            "SHIELD_ACTIVATION_DELAY_TICKS",
            "SHIELD_FRONT_ARC_DOT_MIN",
            "SHIELD_FALLBACK_MAX_DAMAGE",
            "minecraft:shield",
            "saturating_sub(",
            ".hypot(",
            ".floor(",
        ]
        .iter()
        .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "player combat duplicates shared shield authority".into(),
            });
        }
    }
}

fn scan_generic_modules(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if matches!(trimmed, "mod utils;" | "mod common;" | "mod shared;")
            || matches!(
                trimmed,
                "pub mod utils;" | "pub mod common;" | "pub mod shared;"
            )
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "generic utils/common/shared module; use a domain module".into(),
            });
        }
    }
}

fn scan_combat_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    let is_combat_module = components.windows(4).any(|window| {
        window[0] == "mc-net" && window[1] == "src" && window[2] == "play" && window[3] == "combat"
    });
    if !is_combat_module {
        return;
    }

    const FORBIDDEN: &[&str] = &[
        "super::super::",
        "crate::play::",
        "crate::{play::",
        "InteractionState",
        "OutboundCommand",
        "ServerConfig",
    ];
    let parent_globs = glob_use_item_starts(lines, "super::");
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if parent_globs[index] || FORBIDDEN.iter().any(|token| normalized.contains(token)) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "combat domain depends on play/session internals".into(),
            });
        }
    }
}

fn scan_api_leaks(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let is_extension_api = PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name));
    if !is_extension_api {
        return;
    }

    scan_api_evolution_guards(path, lines, findings);

    let aliases = forbidden_api_aliases(lines);
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let publicish = trimmed.starts_with("pub ") || trimmed.starts_with("pub(");
        if !publicish {
            continue;
        }

        let mut public_item = String::new();
        for item_line in &lines[index..] {
            public_item.push_str(item_line);
            public_item.push('\n');
            if public_item_end(item_line) {
                break;
            }
        }

        if contains_forbidden_api_surface(&public_item, &aliases) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API exposes internal runtime or transport type".into(),
            });
        }
    }
}

fn scan_api_evolution_guards(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub enum ") && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public enum missing #[non_exhaustive]".into(),
            });
        }
        if public_struct_exposes_fields(lines, index) && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public field struct missing #[non_exhaustive]".into(),
            });
        }
    }
}

fn has_non_exhaustive_attr(lines: &[&str], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        if trimmed.starts_with("#[") {
            if trimmed == "#[non_exhaustive]" {
                return true;
            }
            continue;
        }
        break;
    }
    false
}

fn public_struct_exposes_fields(lines: &[&str], index: usize) -> bool {
    let trimmed = lines[index].trim_start();
    if !trimmed.starts_with("pub struct ") {
        return false;
    }

    if let Some((_, tuple_tail)) = trimmed.split_once('(') {
        if tuple_tail.trim_start().starts_with("pub ") {
            return true;
        }
        if tuple_tail.contains(");") || tuple_tail.trim_end().ends_with(';') {
            return false;
        }
        for line in lines.iter().skip(index + 1) {
            let trimmed = line.trim_start();
            if trimmed.starts_with("pub ") || trimmed.starts_with("pub(") {
                return true;
            }
            if trimmed.contains(");") || trimmed.ends_with(';') {
                break;
            }
        }
        return false;
    }

    let mut saw_brace = false;
    for (offset, line) in lines[index..].iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.contains('{') {
            saw_brace = true;
            if trimmed.contains("{ pub ") || trimmed.contains("{pub ") {
                return true;
            }
        }
        if saw_brace && offset > 0 && (trimmed.starts_with("pub ") || trimmed.starts_with("pub(")) {
            return true;
        }
        if saw_brace && trimmed.contains('}') {
            break;
        }
        if !saw_brace && trimmed.ends_with(';') {
            break;
        }
    }
    false
}

fn scan_api_manifests(root: &Path, findings: &mut Vec<Finding>) {
    for crate_name in PLUGIN_API_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let Ok(source) = fs::read_to_string(&manifest) else {
            continue;
        };
        scan_api_manifest(&manifest, &source, findings);
    }
}

fn scan_api_manifest(path: &Path, source: &str, findings: &mut Vec<Finding>) {
    if !PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name))
    {
        return;
    }

    for (index, line) in source.lines().enumerate() {
        let Some(dependency) = manifest_dependency_name(line.trim()) else {
            continue;
        };
        if FORBIDDEN_API_DEPENDENCIES.contains(&dependency) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: format!("plugin/script API depends on internal crate `{dependency}`"),
            });
        }
    }
}

fn manifest_dependency_name(line: &str) -> Option<&str> {
    if let Some(section) = line
        .strip_prefix("[dependencies.")
        .or_else(|| line.strip_prefix("[dev-dependencies."))
        .or_else(|| line.strip_prefix("[build-dependencies."))
    {
        return section.strip_suffix(']');
    }

    if line.starts_with('[') {
        return None;
    }

    let (name, _) = line.split_once('=')?;
    let name = name.trim().trim_matches('"');
    (!name.is_empty()).then_some(name)
}

fn forbidden_api_aliases(lines: &[&str]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::new();
    for line in lines {
        let trimmed = line.trim();
        if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
            continue;
        }
        if !contains_forbidden_api_surface(trimmed, &BTreeSet::new()) {
            continue;
        }
        if let Some((_, alias)) = trimmed.rsplit_once(" as ") {
            let alias = alias.trim_end_matches(';').trim();
            if !alias.is_empty() {
                aliases.insert(alias.to_owned());
            }
        }
    }
    aliases
}

fn contains_forbidden_api_surface(source: &str, aliases: &BTreeSet<String>) -> bool {
    FORBIDDEN_API_TYPES
        .iter()
        .chain(FORBIDDEN_API_TRANSPORTS.iter())
        .any(|token| source.contains(token))
        || aliases.iter().any(|alias| source.contains(alias))
}

fn public_item_end(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.ends_with(';')
        || trimmed.ends_with('{')
        || trimmed.ends_with('}')
        || trimmed.ends_with(',')
}

fn path_has_component(path: &Path, needle: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == needle)
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
