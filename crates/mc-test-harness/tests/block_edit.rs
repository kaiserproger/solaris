//! M5.f — raw-TCP integration test for the break-block flow.
//!
//! Boots `mc_net::run` on an ephemeral port against an in-memory
//! generated world, walks the spawn burst, sends a
//! `ServerboundPlayerAction(START_DESTROY_BLOCK)` at the grass
//! cell directly under spawn `(0, -61, 0)`, and asserts that:
//!
//! - a `ClientboundBlockUpdate` for that position with the air
//!   state-id arrives,
//! - a `ClientboundBlockChangedAck` echoes our sequence,
//! - at least one `ClientboundLightUpdate` for chunk `(0, 0)`
//!   arrives with a well-shaped `LightData` payload (same M4.f
//!   mask invariants).
//!
//! Skipped silently when required vanilla data sidecars are missing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AGEABLE_ENTITY_DATA_BABY_INDEX, AddEntity, BlockChangedAck, BlockUpdate, ClientCommandAction,
    ClientboundBlockEntityData, ClientboundContainerSetContent, ClientboundContainerSetData,
    ClientboundContainerSetSlot, ClientboundCooldown, ClientboundExplode, ClientboundKeepAlive,
    ClientboundOpenScreen, ClientboundOpenSignEditor, ClientboundRespawn, ClientboundSetEntityData,
    ClientboundSetExperience, ClientboundSetHealth, ClientboundSetTime, ClientboundSystemChat,
    ClientboundTakeItemEntity, ConfirmTeleportation, ContainerInput, Direction, EntityAnimation,
    EntityAnimationAction, EntityDataValue, EntityEvent, EntityPose, EntityVec3, GameEvent,
    GameMode, HashedStack, HashedStackComponentHashes, ITEM_ENTITY_DATA_ITEM_INDEX,
    InteractionHand, LIVING_ENTITY_DATA_FLAGS_INDEX, LIVING_ENTITY_FLAG_USING_ITEM,
    LevelChunkWithLight, LevelEvent, LightUpdate, MoveEntityPosRot, MovePlayerFlags,
    PlayerActionKind, PlayerCommandAction, PlayerInput, RemoveEntities,
    SHEEP_ENTITY_DATA_WOOL_INDEX, SectionBlocksUpdate, ServerboundChatCommand,
    ServerboundClientCommand, ServerboundContainerButtonClick, ServerboundContainerClick,
    ServerboundContainerClose, ServerboundInteract, ServerboundKeepAlive,
    ServerboundMovePlayerPosRot, ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundPlayerCommand, ServerboundPlayerInput,
    ServerboundSetCarriedItem, ServerboundSignUpdate, ServerboundUseItem, ServerboundUseItemOn,
    SetCenterChunk, SetEntityMotion, SynchronizePlayerPosition, pack_block_pos, pack_section_pos,
    pack_section_relative_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;

#[path = "block_edit/support.rs"]
mod support;
use support::*;

const VIEW_DISTANCE: i32 = 2;

include!("block_edit/container_support.rs");
include!("block_edit/block_breaking.rs");
include!("block_edit/survival_inventory.rs");
include!("block_edit/item_lock_gate.rs");
include!("block_edit/survival_pickup_overflow.rs");
include!("block_edit/campfire.rs");
include!("block_edit/cauldron.rs");
include!("block_edit/crop_bonemeal.rs");
include!("block_edit/crop_harvest.rs");
include!("block_edit/fluid_scheduling.rs");
include!("block_edit/placement_rejection.rs");
include!("block_edit/plant_harvest.rs");
include!("block_edit/plant_lifecycle.rs");
include!("block_edit/inventory_crafting.rs");
include!("block_edit/embedded_playable.rs");
include!("block_edit/persistence.rs");
include!("block_edit/survival_crafting.rs");
include!("block_edit/crafting_table.rs");
include!("block_edit/furnaces.rs");
include!("block_edit/chests_and_hoppers.rs");
include!("block_edit/enchanting.rs");
include!("block_edit/inventory_clicks.rs");
include!("block_edit/stations_and_placement.rs");
include!("block_edit/multiplayer_sleep.rs");
include!("block_edit/pvp.rs");
include!("block_edit/sapling_growth.rs");
include!("block_edit/sign_edit.rs");
include!("block_edit/survival_lifecycle.rs");
include!("block_edit/toggle_blocks.rs");
include!("block_edit/vertical_plant_growth.rs");
include!("block_edit/wheat_harvest.rs");
include!("block_edit/wheat_seed_source.rs");
include!("block_edit/bucket_progression.rs");
include!("block_edit/animal_breeding.rs");
include!("block_edit/hostile_combat.rs");
include!("block_edit/explosions.rs");
