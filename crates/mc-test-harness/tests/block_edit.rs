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
    AddEntity, BlockChangedAck, BlockUpdate, ClientCommandAction, ClientboundContainerSetContent,
    ClientboundContainerSetData, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ClientboundOpenScreen, ClientboundRespawn, ClientboundSetEntityData, ClientboundSetHealth,
    ClientboundTakeItemEntity, ConfirmTeleportation, ContainerInput, Direction, EntityAnimation,
    EntityAnimationAction, EntityDataValue, GameEvent, HashedStack, HashedStackComponentHashes,
    ITEM_ENTITY_DATA_ITEM_INDEX, InteractionHand, LevelChunkWithLight, LightUpdate,
    MoveEntityPosRot, MovePlayerFlags, PlayerActionKind, RemoveEntities, ServerboundChatCommand,
    ServerboundClientCommand, ServerboundContainerClick, ServerboundContainerClose,
    ServerboundKeepAlive, ServerboundMovePlayerPosRot, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundUseItem, ServerboundUseItemOn, SetCenterChunk,
    SetEntityMotion, SynchronizePlayerPosition, pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;

#[path = "block_edit/support.rs"]
mod support;
use support::*;

const VIEW_DISTANCE: i32 = 2;

include!("block_edit/breaks_and_crafting.rs");
include!("block_edit/campfire.rs");
include!("block_edit/crop_bonemeal.rs");
include!("block_edit/furnace_and_chests.rs");
include!("block_edit/survival_lifecycle.rs");
include!("block_edit/wheat_harvest.rs");
