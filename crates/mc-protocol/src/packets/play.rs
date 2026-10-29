//! Play state — the bulk of the protocol surface.
//!
//! M1.g.2 scope: just enough packet types for the M1.g.3 handler to
//! send `Login (Play)` → `Synchronize Player Position` → `Game Event`
//! and run a KeepAlive loop.
//!
//! Packet IDs and field layouts have been verified against
//! `net.minecraft.network.protocol.game.GameProtocols` from the
//! unobfuscated vanilla 26.1.2 jar (per ADR 0002); each
//! `impl Packet for ... { const ID }` site cites the corresponding
//! vanilla `PacketType` constant name.

use std::mem::size_of;

use bytes::{Buf, BufMut};
use mc_nbt::Tag;
use uuid::Uuid;

use super::{ClientInformation, CustomPayload, MainHand, Packet, ResourcePackStatus};
use crate::codec::{
    DEFAULT_MAX_STRING_LEN, Identifier, ReadMc, WriteMc, bounded_vec, read_bounded_count,
    read_bounded_vec,
};
use crate::error::CodecError;
use crate::packets::login::GameProfileProperty;

mod advancements;
pub use advancements::{
    AdvancementDefinition, AdvancementDisplay, AdvancementFrame, AdvancementProgress,
    ClientboundSelectAdvancementsTab, ClientboundUpdateAdvancements, ServerboundSeenAdvancements,
};
mod boss_event;
mod combat_kill;
pub use combat_kill::ClientboundPlayerCombatKill;
#[cfg(test)]
mod component_security_tests;
mod copied_components;
mod crafter;
mod damage_event;
mod dialog;
pub use boss_event::{BossOperation, ClientboundBossEvent};
pub use crafter::ServerboundContainerSlotStateChanged;
pub use dialog::{ClientboundClearDialog, ClientboundShowDialog, ServerboundCustomClickAction};
mod entity_sync_26_1_2;
#[cfg(test)]
mod item_admission_tests;
mod item_stack_codec;
mod item_stack_template;
#[cfg(test)]
mod item_stack_template_tests;
mod map_data;
mod merchant;
pub use map_data::{ClientboundMapData, MapDecoration};
mod rename_item;
mod scoreboard;
mod set_beacon;
mod station_components;
mod ticking;
mod titles;
pub use rename_item::ServerboundRenameItem;
pub use scoreboard::{
    ClientboundResetScore, ClientboundSetDisplayObjective, ClientboundSetObjective,
    ClientboundSetPlayerTeam, ClientboundSetScore, TeamParameters,
};
pub use set_beacon::ServerboundSetBeacon;
pub use ticking::{ClientboundTickingState, ClientboundTickingStep};
pub use titles::{
    ClientboundClearTitles, ClientboundSetActionBarText, ClientboundSetSubtitleText,
    ClientboundSetTitleText, ClientboundSetTitlesAnimation,
};
mod vehicle_movement;
pub use vehicle_movement::{ClientboundMoveVehicle, ServerboundMoveVehicle};

pub use damage_event::ClientboundDamageEvent;
pub use entity_sync_26_1_2::{
    AttributeId, AttributeModifierOperation, ClientboundRemoveEntityEffect,
    ClientboundSetEntityEquipment, ClientboundSetEntityLeash, ClientboundUpdateEntityAttributes,
    ClientboundUpdateEntityEffect, EntityAttributeModifier, EntityAttributeSnapshot,
    EntityEffectFlags, EntityEquipment, EquipmentSlot, LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
    MobEffectId,
};
pub use merchant::{
    ClientboundMerchantOffers, MerchantItemCost, MerchantOffer, ServerboundSelectTrade,
};

mod chat_commands;
mod chunks;
mod entity;
mod env_state;
mod interaction;
mod inventory;
mod login_flow;
mod statistics;
pub use chat_commands::{
    ClientboundCommandSuggestions, ClientboundCommands, ClientboundOpenBook, ClientboundSystemChat,
    ClientboundTabList, CommandArgumentParser, CommandNode, CommandNodeKind, CommandStringKind,
    CommandSuggestionEntry, ServerboundChangeGameMode, ServerboundChat, ServerboundChatAck,
    ServerboundChatCommand, ServerboundChatCommandSigned, ServerboundChatSessionUpdate,
    ServerboundChunkBatchReceived, ServerboundClientTickEnd, ServerboundCommandSuggestion,
    ServerboundEditBook, ServerboundPlayerLoaded, ServerboundSignUpdate,
};
pub use chunks::{
    BlockChangedAck, BlockEntityInfo, BlockUpdate, ChunkHeightmap, ForgetLevelChunk,
    LevelChunkWithLight, LevelEvent, LightData, LightUpdate, SectionBlockChange,
    SectionBlocksUpdate, SetCenterChunk, SetChunkCacheRadius, pack_chunk_pos,
};
pub use entity::{
    AddEntity, BlockStateId, ClientboundSetEntityData, ClientboundSetPassengers,
    ClientboundSwingAnimation, ClientboundTakeItemEntity, EffectParticle, EntityAnimation,
    EntityAnimationAction, EntityDataValue, EntityDirection, EntityEvent, EntityPose,
    EntityPositionSync, EntityRotations, EntityVec3, MoveEntityPos, MoveEntityPosRot,
    MoveEntityRot, PlayerInfoActions, PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate,
    RemoveEntities, RotateHead, SetEntityMotion, SwingAnimationType,
};
pub use env_state::{
    ClientboundBlockDestruction, ClientboundBlockEntityData, ClientboundBlockEvent,
    ClientboundChangeDifficulty, ClientboundCommandParticle, ClientboundCooldown,
    ClientboundExplode, ClientboundInitializeBorder, ClientboundOpenSignEditor,
    ClientboundPlayerAbilities, ClientboundSetExperience, ClientboundSetHealth, ClientboundSetTime,
    ClientboundSound, ClientboundSoundEntity, ClientboundSweepParticle, ExplosionBlockParticle,
    GameEvent, OVERWORLD_WORLD_CLOCK_ID, THE_END_WORLD_CLOCK_ID, WorldClockUpdate,
};
pub use interaction::{
    ClientCommandAction, ConfirmTeleportation, MovePlayerFlags, PlayerActionKind,
    PlayerCommandAction, PlayerInput, ServerboundAttack, ServerboundClientCommand,
    ServerboundClientInformation, ServerboundCustomPayload, ServerboundInteract,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundMovePlayerPosRot,
    ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly, ServerboundPickItemFromBlock,
    ServerboundPickItemFromEntity, ServerboundPlayerAction, ServerboundPlayerCommand,
    ServerboundPlayerInput, ServerboundPunch, ServerboundResourcePack, ServerboundUseItem,
    ServerboundUseItemOn,
};
pub use inventory::{
    ClientboundContainerClose, ClientboundContainerSetContent, ClientboundContainerSetData,
    ClientboundContainerSetSlot, ClientboundMountScreenOpen, ClientboundOpenScreen,
    ClientboundRecipeBookAdd, ClientboundRecipeBookSettings, ClientboundSetCursorItem,
    ClientboundSetHeldSlot, ClientboundUpdateRecipes, ContainerInput, HashedStack,
    HashedStackComponentHashes, RecipeBookDisplay, RecipeBookEntry, RecipeBookIngredient,
    RecipeBookSlotDisplay, RecipeBookType, RecipeBookTypeSettings, RecipePropertySet,
    ServerboundContainerButtonClick, ServerboundContainerClick, ServerboundContainerClose,
    ServerboundPlaceRecipe, ServerboundRecipeBookChangeSettings, ServerboundRecipeBookSeenRecipe,
    ServerboundSetCarriedItem, ServerboundSetCreativeModeSlot, StonecutterRecipeEntry,
};
pub use login_flow::{
    ClientboundCustomPayload, ClientboundKeepAlive, ClientboundRespawn, ClientboundTransfer,
    LoginPlay, PlayDisconnect, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
pub use statistics::ClientboundAwardStats;

/// Vanilla's ceiling on the chunk-data payload (`TWO_MEGABYTES` in
/// `ClientboundLevelChunkPacketData`). Decoding rejects buffers larger
/// than this on the read side.
const MAX_CHUNK_DATA_LEN: usize = 2 * 1024 * 1024;

/// Sanity ceiling on length-prefixed `i64[]` fields: BitSet payloads
/// (one long per 64 sections, vanilla has 26) and heightmap data
/// (~37 longs for a 9-bits-per-entry 256-entry packing). 4 KiB worth
/// of longs is more than two orders of magnitude headroom.
const MAX_LONG_ARRAY_LEN: usize = 4096;

/// A section contains at most 16^3 block changes.
const MAX_SECTION_BLOCK_UPDATE_ENTRIES: usize = 4096;
/// Allocation safety ceiling for the packet-local weighted block-particle list.
/// TNT currently emits two entries; 4096 leaves section-scale headroom.
const MAX_EXPLOSION_BLOCK_PARTICLES: usize = 4096;
const MAX_PLAYER_INFO_ENTRIES: usize = 1024;
const MAX_GAME_PROFILE_PROPERTIES: usize = 16;
const MAX_GAME_PROFILE_PROPERTY_NAME_LEN: usize = 64;
const MAX_GAME_PROFILE_PROPERTY_VALUE_LEN: usize = 32_767;
const MAX_GAME_PROFILE_PROPERTY_SIGNATURE_LEN: usize = 1024;
const MAX_ENTITY_ID_LIST_LEN: usize = 1024;
const MAX_DIMENSION_NAMES: usize = 1024;
const MAX_CHUNK_HEIGHTMAPS: usize = 16;
const MAX_CHUNK_BLOCK_ENTITIES: usize = 1_000_000;
const MIN_BLOCK_ENTITY_BYTES: usize = 5;
const MAX_ENTITY_DATA_VALUES: usize = 64;
const MAX_CONTAINER_CLICK_CHANGED_SLOTS: usize = 128;
const MAX_HASHED_STACK_COMPONENT_HASHES: usize = 256;
/// Solaris packet-wide allocation fence for serverbound Container Click.
/// Vanilla bounds each collection independently; this additional aggregate
/// ceiling prevents one packet from allocating tens of thousands of hashes.
const MAX_CONTAINER_CLICK_COMPONENT_HASHES: usize = 4096;
const MAX_RECIPE_BOOK_ENTRIES: usize = 8192;
const MAX_RECIPE_BOOK_SLOTS: usize = 256;
const MAX_RECIPE_BOOK_REQUIREMENTS: usize = 256;
const MAX_RECIPE_BOOK_INGREDIENT_ITEMS: usize = 2048;
const MAX_RECIPE_BOOK_SLOT_DEPTH: usize = 16;
const MAX_COMMAND_LEN: usize = 32_767;
const MAX_CHAT_MESSAGE_LEN: usize = 256;
const MAX_COMMAND_NODE_COUNT: usize = 1024;
const MAX_COMMAND_CHILD_COUNT: usize = 1024;
const MAX_COMMAND_SUGGESTION_LEN: usize = 32_500;
const SIGN_LINE_COUNT: usize = 4;
const MAX_SIGN_LINE_LEN: usize = 384;
const LAST_SEEN_FIXED_BITSET_BYTES: usize = 3;
pub const ENTITY_DATA_ITEM_STACK_SERIALIZER_ID: i32 = 7;
pub const ENTITY_DATA_BYTE_SERIALIZER_ID: i32 = 0;
pub const ENTITY_DATA_BOOLEAN_SERIALIZER_ID: i32 = 8;
pub const ENTITY_DATA_POSE_SERIALIZER_ID: i32 = 20;
const ENTITY_DATA_INT_SERIALIZER_ID: i32 = 1;
const ENTITY_DATA_LONG_SERIALIZER_ID: i32 = 2;
const ENTITY_DATA_FLOAT_SERIALIZER_ID: i32 = 3;
const ENTITY_DATA_STRING_SERIALIZER_ID: i32 = 4;
const ENTITY_DATA_ROTATIONS_SERIALIZER_ID: i32 = 9;
const ENTITY_DATA_BLOCK_POSITION_SERIALIZER_ID: i32 = 10;
const ENTITY_DATA_OPTIONAL_BLOCK_POSITION_SERIALIZER_ID: i32 = 11;
const ENTITY_DATA_DIRECTION_SERIALIZER_ID: i32 = 12;
const ENTITY_DATA_OPTIONAL_LIVING_ENTITY_REFERENCE_SERIALIZER_ID: i32 = 13;
const ENTITY_DATA_BLOCK_STATE_SERIALIZER_ID: i32 = 14;
const ENTITY_DATA_OPTIONAL_BLOCK_STATE_SERIALIZER_ID: i32 = 15;
const ENTITY_DATA_VILLAGER_DATA_SERIALIZER_ID: i32 = 18;
const ENTITY_DATA_OPTIONAL_UNSIGNED_INT_SERIALIZER_ID: i32 = 19;
const ENTITY_DATA_HUMANOID_ARM_SERIALIZER_ID: i32 = 42;
const ENTITY_DATA_PARTICLE_SERIALIZER_ID: i32 = 16;
const ENTITY_DATA_PARTICLES_SERIALIZER_ID: i32 = 17;
/// Built-in effect count plus registry-growth headroom for particle metadata.
pub const MAX_ENTITY_EFFECT_PARTICLES: usize = 256;
/// `minecraft:particle_type` protocol id of `minecraft:entity_effect`
/// (26.3 `data/vanilla/reports/registries.json`); its stream codec is one `i32` ARGB.
const PARTICLE_TYPE_ENTITY_EFFECT: i32 = 28;
const PARTICLE_TYPE_RAID_OMEN: i32 = 123;
const PARTICLE_TYPE_TRIAL_OMEN: i32 = 124;
pub const ENTITY_DATA_SHARED_FLAGS_INDEX: u8 = 0;
pub const ENTITY_DATA_AIR_SUPPLY_INDEX: u8 = 1;
pub const ENTITY_DATA_POSE_INDEX: u8 = 6;
pub const LIVING_ENTITY_DATA_FLAGS_INDEX: u8 = 8;
pub const LIVING_ENTITY_DATA_EFFECT_PARTICLES_INDEX: u8 = 10;
pub const LIVING_ENTITY_DATA_EFFECT_AMBIENCE_INDEX: u8 = 11;
/// Player extends Avatar (main hand 15, skin flags 16), then absorption at 17.
pub const PLAYER_DATA_ABSORPTION_INDEX: u8 = 17;
/// `LivingEntity.SLEEPING_POS_ID` (OPTIONAL_BLOCK_POS) on the bundled vanilla
/// 26.1.2 server: after flags (8), health (9), effect particles (10),
/// effect ambience (11), arrow count (12) and stinger count (13).
pub const LIVING_ENTITY_DATA_SLEEPING_POS_INDEX: u8 = 14;
/// `AgeableMob.DATA_BABY_ID` on the bundled vanilla 26.1.2 server.
pub const AGEABLE_ENTITY_DATA_BABY_INDEX: u8 = 16;
/// `Sheep.DATA_WOOL_ID` on the bundled vanilla 26.1.2 server.
pub const SHEEP_ENTITY_DATA_WOOL_INDEX: u8 = 18;
/// `Villager.DATA_VILLAGER_DATA` after the 19 inherited accessors on the
/// bundled vanilla 26.1.2 server.
pub const VILLAGER_ENTITY_DATA_INDEX: u8 = 19;
pub const LIVING_ENTITY_FLAG_USING_ITEM: i8 = 0x01;
pub const LIVING_ENTITY_FLAG_OFF_HAND: i8 = 0x02;
pub const ITEM_ENTITY_DATA_ITEM_INDEX: u8 = 8;
/// `minecraft:data_component_type` wire ids (26.3 registry report).
pub const DATA_COMPONENT_CUSTOM_DATA_ID: i32 = mc_data::data_component_types_26_1_2::CUSTOM_DATA;
pub const DATA_COMPONENT_MAX_DAMAGE_ID: i32 = mc_data::data_component_types_26_1_2::MAX_DAMAGE;
pub const DATA_COMPONENT_DAMAGE_ID: i32 = mc_data::data_component_types_26_1_2::DAMAGE;
/// `DataComponents.CUSTOM_NAME` uses the `Component` stream codec, which is
/// network NBT for the supported literal text component shape.
pub const DATA_COMPONENT_CUSTOM_NAME_ID: i32 = mc_data::data_component_types_26_1_2::CUSTOM_NAME;
pub const DATA_COMPONENT_ITEM_MODEL_ID: i32 = mc_data::data_component_types_26_1_2::ITEM_MODEL;
pub const DATA_COMPONENT_ENCHANTMENTS_ID: i32 = mc_data::data_component_types_26_1_2::ENCHANTMENTS;

thread_local! {
    static VANILLA_ITEM_MODELS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Encode with `f` for a client that cannot resolve non-vanilla item models (no
/// Loader): item stacks then omit every `minecraft:item_model` outside the
/// `minecraft` namespace and the client shows the carrier item. A vanilla client
/// renders an unknown model id as the missing-model cube, never as the carrier.
/// `hide = false` encodes normally.
pub fn with_vanilla_item_models<R>(hide: bool, f: impl FnOnce() -> R) -> R {
    let previous = VANILLA_ITEM_MODELS.replace(hide);
    let result = f();
    VANILLA_ITEM_MODELS.set(previous);
    result
}

/// The item model an encoded stack carries under the active policy.
fn wire_item_model(stack: &ItemStack) -> Option<&mc_data::Identifier> {
    stack
        .item_model
        .as_deref()
        .filter(|model| !VANILLA_ITEM_MODELS.get() || model.namespace() == "minecraft")
}
pub const DATA_COMPONENT_SUSPICIOUS_STEW_EFFECTS_ID: i32 =
    mc_data::data_component_types_26_1_2::SUSPICIOUS_STEW_EFFECTS;

fn write_long_array<B: BufMut>(buf: &mut B, longs: &[i64]) -> Result<(), CodecError> {
    if longs.len() > MAX_LONG_ARRAY_LEN {
        return Err(CodecError::StringTooLong {
            len: longs.len(),
            max: MAX_LONG_ARRAY_LEN,
        });
    }
    let len = i32::try_from(longs.len()).map_err(|_| CodecError::StringTooLong {
        len: longs.len(),
        max: MAX_LONG_ARRAY_LEN,
    })?;
    buf.write_varint(len);
    for &v in longs {
        buf.write_i64(v);
    }
    Ok(())
}

fn read_long_array<B: Buf>(buf: &mut B) -> Result<Vec<i64>, CodecError> {
    read_bounded_vec(buf, MAX_LONG_ARRAY_LEN, size_of::<i64>(), ReadMc::read_i64)
}

fn read_count<B: Buf>(buf: &mut B, max: usize) -> Result<usize, CodecError> {
    read_bounded_count(buf, max, 1)
}

fn write_count<B: BufMut>(buf: &mut B, len: usize) -> Result<(), CodecError> {
    let len = i32::try_from(len).map_err(|_| CodecError::StringTooLong {
        len,
        max: i32::MAX as usize,
    })?;
    buf.write_varint(len);
    Ok(())
}

fn write_bounded_count<B: BufMut>(buf: &mut B, len: usize, max: usize) -> Result<(), CodecError> {
    if len > max {
        return Err(CodecError::StringTooLong { len, max });
    }
    write_count(buf, len)
}

fn pack_degrees(degrees: f32) -> u8 {
    (degrees.mul_add(256.0, 0.0) / 360.0).floor() as i32 as u8
}

fn unpack_degrees(packed: u8) -> f32 {
    (packed as i8 as i32 * 360) as f32 / 256.0
}

fn write_vec3<B: BufMut>(buf: &mut B, v: EntityVec3) {
    buf.write_f64(v.x);
    buf.write_f64(v.y);
    buf.write_f64(v.z);
}

fn read_vec3<B: Buf>(buf: &mut B) -> Result<EntityVec3, CodecError> {
    Ok(EntityVec3 {
        x: buf.read_f64()?,
        y: buf.read_f64()?,
        z: buf.read_f64()?,
    })
}

fn write_lp_vec3<B: BufMut>(buf: &mut B, v: EntityVec3) {
    let x = sanitize_lp_vec(v.x);
    let y = sanitize_lp_vec(v.y);
    let z = sanitize_lp_vec(v.z);
    let abs_max = x.abs().max(y.abs()).max(z.abs());
    if abs_max < 3.051944088384301E-5 {
        buf.write_u8(0);
        return;
    }

    let scale = abs_max.ceil() as u64;
    let has_continuation = (scale & 3) != scale;
    let scale_bits = if has_continuation {
        (scale & 3) | 4
    } else {
        scale
    };
    let packed = scale_bits
        | (pack_lp_component(x / scale as f64) << 3)
        | (pack_lp_component(y / scale as f64) << 18)
        | (pack_lp_component(z / scale as f64) << 33);
    buf.write_u8(packed as u8);
    buf.write_u8((packed >> 8) as u8);
    buf.write_u32((packed >> 16) as u32);
    if has_continuation {
        buf.write_varint((scale >> 2) as i32);
    }
}

fn read_lp_vec3<B: Buf>(buf: &mut B) -> Result<EntityVec3, CodecError> {
    let first = buf.read_u8()?;
    if first == 0 {
        return Ok(EntityVec3::ZERO);
    }
    let second = buf.read_u8()?;
    let rest = buf.read_u32()?;
    let packed = ((rest as u64) << 16) | ((second as u64) << 8) | first as u64;
    let mut scale = (first & 3) as u64;
    if first & 4 == 4 {
        scale |= ((buf.read_varint()? as u32 as u64) & 0xffff_ffff) << 2;
    }
    let scale = scale as f64;
    Ok(EntityVec3 {
        x: unpack_lp_component(packed >> 3) * scale,
        y: unpack_lp_component(packed >> 18) * scale,
        z: unpack_lp_component(packed >> 33) * scale,
    })
}

fn sanitize_lp_vec(v: f64) -> f64 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.7179869183E10, 1.7179869183E10)
    }
}

fn pack_lp_component(v: f64) -> u64 {
    ((v * 0.5 + 0.5) * 32766.0).round() as u64
}

fn unpack_lp_component(v: u64) -> f64 {
    (((v & 32767) as f64).min(32766.0) * 2.0 / 32766.0) - 1.0
}

// -----------------------------------------------------------------------
// Clientbound
// -----------------------------------------------------------------------

fn validate_simple_explosion_particle_id(particle_id: i32) -> Result<i32, CodecError> {
    // Vanilla 26.3 `minecraft:particle_type` IDs for the current TNT packet
    // shape: explosion_emitter=29, poof=69, smoke=72. These codecs have no payload.
    match particle_id {
        29 | 69 | 72 => Ok(particle_id),
        _ => Err(CodecError::NotSupported(
            "unsupported simple explosion particle id",
        )),
    }
}

fn write_explosion_sound_reference_holder<B: BufMut>(
    buf: &mut B,
    registry_id: i32,
) -> Result<(), CodecError> {
    let wire_id = registry_id
        .checked_add(1)
        .filter(|holder| *holder > 0)
        .ok_or(CodecError::NotSupported(
            "invalid explosion sound reference registry id",
        ))?;
    buf.write_varint(wire_id);
    Ok(())
}

fn read_explosion_sound_reference_holder<B: Buf>(buf: &mut B) -> Result<i32, CodecError> {
    let wire_id = buf.read_varint()?;
    if wire_id == 0 {
        return Err(CodecError::NotSupported("inline explosion sound holder"));
    }
    if wire_id < 0 {
        return Err(CodecError::NotSupported(
            "invalid explosion sound reference registry id",
        ));
    }
    Ok(wire_id - 1)
}

fn encode_clock_network_state<B: BufMut>(buf: &mut B, state: WorldClockUpdate) {
    buf.write_varlong(state.total_ticks);
    buf.write_f32(state.partial_tick);
    buf.write_f32(state.rate);
}

fn decode_clock_network_state<B: Buf>(buf: &mut B) -> Result<WorldClockUpdate, CodecError> {
    Ok(WorldClockUpdate {
        total_ticks: buf.read_varlong()?,
        partial_tick: buf.read_f32()?,
        rate: buf.read_f32()?,
    })
}

fn move_entity_delta_to_short(delta: f64) -> i16 {
    (delta * 4096.0)
        .round()
        .clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

fn java_float_equals(left: f32, right: f32) -> bool {
    fn canonical_bits(value: f32) -> u32 {
        if value.is_nan() {
            0x7FC0_0000
        } else {
            value.to_bits()
        }
    }

    canonical_bits(left) == canonical_bits(right)
}

// -----------------------------------------------------------------------

pub use mc_domain::GameMode;

// ---------------------------------------------------------------------

/// Pack vanilla's `SectionPos.asLong` representation:
/// `((x & 0x3FFFFF)<<42) | ((z & 0x3FFFFF)<<20) | (y & 0xFFFFF)`.
#[must_use]
pub fn pack_section_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3F_FFFF) << 42) | (((z as i64) & 0x3F_FFFF) << 20) | ((y as i64) & 0xF_FFFF)
}

/// Pack vanilla's section-relative block coordinate:
/// `(x&15)<<8 | (z&15)<<4 | (y&15)`.
#[must_use]
pub fn pack_section_relative_pos(x: i32, y: i32, z: i32) -> u16 {
    (((x as u16) & 15) << 8) | (((z as u16) & 15) << 4) | ((y as u16) & 15)
}

/// Pack one repeated SectionBlocksUpdate VarLong value.
#[must_use]
pub fn pack_section_block_update_value(state_id: i32, relative_pos: u16) -> i64 {
    ((state_id as i64) << 12) | i64::from(relative_pos & 0x0FFF)
}

// ---------------------------------------------------------------------

/// Direction of a `BlockPos` face. The gameplay value lives in `mc-domain`;
/// this module owns only its wire adaptation.
pub use mc_domain::Direction;

fn direction_from_wire(value: i32) -> Result<Direction, CodecError> {
    Direction::from_ordinal(value).ok_or(CodecError::StringTooLong {
        len: value as usize,
        max: 5,
    })
}

/// Which hand was used. The gameplay value lives in `mc-domain`; this
/// module owns only its VarInt wire adaptation.
pub use mc_domain::InteractionHand;

fn interaction_hand_from_wire(value: i32) -> Result<InteractionHand, CodecError> {
    InteractionHand::from_ordinal(value).ok_or(CodecError::StringTooLong {
        len: value as usize,
        max: 1,
    })
}

/// Pack `(x, y, z)` world coordinates into vanilla's `BlockPos`
/// `i64`. `x` and `z` are 26-bit signed; `y` is 12-bit signed.
/// Used by every packet in this module that carries a `BlockPos`.
#[must_use]
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    let x = (x as i64) & 0x3FF_FFFF;
    let z = (z as i64) & 0x3FF_FFFF;
    let y = (y as i64) & 0xFFF;
    (x << 38) | (z << 12) | y
}

/// Inverse of [`pack_block_pos`]. Sign-extends each field from its
/// declared bit width (26 bits for x/z, 12 bits for y).
#[must_use]
pub fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    let p = packed as u64;
    let x_raw = ((p >> 38) & 0x3FF_FFFF) as i32;
    let z_raw = ((p >> 12) & 0x3FF_FFFF) as i32;
    let y_raw = (p & 0xFFF) as i32;
    (
        sign_extend_26(x_raw),
        sign_extend_12(y_raw),
        sign_extend_26(z_raw),
    )
}

fn sign_extend_26(v: i32) -> i32 {
    if v & 0x200_0000 != 0 {
        v | !0x3FF_FFFF
    } else {
        v
    }
}

fn sign_extend_12(v: i32) -> i32 {
    if v & 0x800 != 0 { v | !0xFFF } else { v }
}

// ---------------------------------------------------------------------
// M6 — Inventory packets
// ---------------------------------------------------------------------

/// Canonical inventory state lives in `mc-data`; this module only owns its
/// Java Edition wire encoding/decoding.
pub use mc_data::item_stack::ItemStack;

/// Consume a canonical preserved component when the receiving protocol has no
/// corresponding component. Reuse the native codec, including nested-patch
/// validation and depth limits; unsupported payloads must still fail closed.
pub fn skip_native_item_component<B: Buf>(
    buf: &mut B,
    component_id: i32,
    depth: usize,
) -> Result<(), CodecError> {
    if depth > copied_components::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    copied_components::decode(buf, component_id, depth)?
        .map(|_| ())
        .ok_or(CodecError::NotSupported(
            "unsupported native item component",
        ))
}

pub(super) trait ItemStackWireCodec: Sized {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError>;
    fn encode_at_depth<B: BufMut>(&self, buf: &mut B, depth: usize) -> Result<(), CodecError>;
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError>;
    fn decode_at_depth<B: Buf>(buf: &mut B, depth: usize) -> Result<Self, CodecError>;
    fn encode_components<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError>;
    fn encode_components_at_depth<B: BufMut>(
        &self,
        buf: &mut B,
        depth: usize,
    ) -> Result<(), CodecError>;
    fn decode_components<B: Buf>(buf: &mut B, item_id: u32, count: i32)
    -> Result<Self, CodecError>;
    fn decode_components_at_depth<B: Buf>(
        buf: &mut B,
        item_id: u32,
        count: i32,
        depth: usize,
    ) -> Result<Self, CodecError>;
}

impl ItemStackWireCodec for ItemStack {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.encode_at_depth(buf, 0)
    }

    fn encode_at_depth<B: BufMut>(&self, buf: &mut B, depth: usize) -> Result<(), CodecError> {
        if depth > copied_components::MAX_ITEM_COMPONENT_DEPTH {
            return Err(CodecError::NotSupported("nested item component depth"));
        }
        if self.is_empty() {
            buf.write_varint(0);
            return Ok(());
        }
        buf.write_varint(self.count);
        buf.write_varint(self.item_id as i32);
        self.encode_components_at_depth(buf, depth)
    }

    fn encode_components<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.encode_components_at_depth(buf, 0)
    }

    fn encode_components_at_depth<B: BufMut>(
        &self,
        buf: &mut B,
        depth: usize,
    ) -> Result<(), CodecError> {
        if depth > copied_components::MAX_ITEM_COMPONENT_DEPTH {
            return Err(CodecError::NotSupported("nested item component depth"));
        }
        let copied = copied_components::prepare(self)?;
        let added = copied.iter().filter(|(id, _)| *id >= 0).count() as i32;
        let removed = copied.len() as i32 - added;
        let overridden = |id| copied_components::contains(&copied, id);
        let component_count = i32::from(self.damage.is_some() && !overridden(3))
            + i32::from(self.custom_name.is_some() && !overridden(6))
            + i32::from(wire_item_model(self).is_some() && !overridden(10))
            + i32::from(!self.enchantments.is_empty() && !overridden(13))
            + i32::from(
                !self.stew_effects.is_empty()
                    && !overridden(DATA_COMPONENT_SUSPICIOUS_STEW_EFFECTS_ID),
            )
            + i32::from(self.extended().max_damage.is_some() && !overridden(2))
            + i32::from(self.extended().custom_data.is_some() && !overridden(0))
            + station_components::count(self.extended(), &copied)
            + added;
        buf.write_varint(component_count);
        buf.write_varint(removed);
        if let Some(custom_data) = &self.extended().custom_data
            && !overridden(0)
        {
            let tag = mc_data::item_stack::decode_custom_data(custom_data)
                .map_err(CodecError::NotSupported)?;
            buf.write_varint(DATA_COMPONENT_CUSTOM_DATA_ID);
            mc_nbt::write_network(buf, &tag)?;
        }
        if let Some(max_damage) = self.extended().max_damage
            && !overridden(2)
        {
            mc_data::item_stack::validate_max_damage(max_damage)
                .map_err(CodecError::NotSupported)?;
            buf.write_varint(DATA_COMPONENT_MAX_DAMAGE_ID);
            buf.write_varint(max_damage as i32);
        }
        if let Some(damage) = self.damage
            && !overridden(3)
        {
            buf.write_varint(DATA_COMPONENT_DAMAGE_ID);
            buf.write_varint(damage);
        }
        if let Some(custom_name) = &self.custom_name
            && !overridden(6)
        {
            buf.write_varint(DATA_COMPONENT_CUSTOM_NAME_ID);
            write_literal_text_component(buf, custom_name)?;
        }
        if let Some(item_model) = wire_item_model(self)
            && !overridden(10)
        {
            buf.write_varint(DATA_COMPONENT_ITEM_MODEL_ID);
            buf.write_identifier(item_model)?;
        }
        if !self.enchantments.is_empty() && !overridden(13) {
            buf.write_varint(DATA_COMPONENT_ENCHANTMENTS_ID);
            write_count(buf, self.enchantments.len())?;
            for enchantment in &self.enchantments {
                let protocol_id =
                    mc_data::required_registry_entry_id("enchantment", &enchantment.id).ok_or(
                        CodecError::NotSupported("unknown enchantment registry entry"),
                    )?;
                if !(1..=255).contains(&enchantment.level) {
                    return Err(CodecError::NotSupported("invalid enchantment level"));
                }
                buf.write_varint(protocol_id as i32);
                buf.write_varint(enchantment.level);
            }
        }
        if !self.stew_effects.is_empty() && !overridden(DATA_COMPONENT_SUSPICIOUS_STEW_EFFECTS_ID) {
            buf.write_varint(DATA_COMPONENT_SUSPICIOUS_STEW_EFFECTS_ID);
            write_count(buf, self.stew_effects.len())?;
            for effect in &self.stew_effects {
                let id = mc_data::mob_effects_26_1_2::MobEffect::from_name(effect.id.as_str())
                    .ok_or(CodecError::NotSupported("unknown stew effect"))?;
                buf.write_varint(id as i32);
                buf.write_varint(effect.duration);
            }
        }
        station_components::encode(buf, self.extended(), &copied, depth)?;
        copied_components::encode(buf, copied, depth)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Self::decode_at_depth(buf, 0)
    }

    fn decode_at_depth<B: Buf>(buf: &mut B, depth: usize) -> Result<Self, CodecError> {
        item_stack_codec::decode_stack(buf, depth, false)
    }

    fn decode_components<B: Buf>(
        buf: &mut B,
        item_id: u32,
        count: i32,
    ) -> Result<Self, CodecError> {
        Self::decode_components_at_depth(buf, item_id, count, 0)
    }

    fn decode_components_at_depth<B: Buf>(
        buf: &mut B,
        item_id: u32,
        count: i32,
        depth: usize,
    ) -> Result<Self, CodecError> {
        item_stack_codec::decode_components(buf, item_id, count, depth, false)
    }
}
fn write_literal_text_component<B: BufMut>(buf: &mut B, text: &str) -> Result<(), CodecError> {
    mc_nbt::write_network_value(buf, &Tag::String(text.to_owned()))?;
    Ok(())
}

fn read_literal_text_component<B: Buf>(buf: &mut B) -> Result<String, CodecError> {
    let fields = match mc_nbt::read_network_value(buf)? {
        Tag::String(text) => return Ok(text),
        Tag::Compound(fields) => fields,
        _ => {
            return Err(CodecError::NotSupported(
                "ItemStack custom name must be a text component",
            ));
        }
    };
    let mut fields = fields.into_iter();
    let Some((name, Tag::String(text))) = fields.next() else {
        return Err(CodecError::NotSupported(
            "ItemStack custom name must be a literal text component",
        ));
    };
    if name != "text" || fields.next().is_some() {
        return Err(CodecError::NotSupported(
            "ItemStack custom name must be a literal text component",
        ));
    }
    Ok(text)
}

fn add_component_hashes_to_budget(
    component_hash_count: &mut usize,
    additional: usize,
) -> Result<(), CodecError> {
    let total = component_hash_count
        .checked_add(additional)
        .ok_or(CodecError::StringTooLong {
            len: usize::MAX,
            max: MAX_CONTAINER_CLICK_COMPONENT_HASHES,
        })?;
    if total > MAX_CONTAINER_CLICK_COMPONENT_HASHES {
        return Err(CodecError::StringTooLong {
            len: total,
            max: MAX_CONTAINER_CLICK_COMPONENT_HASHES,
        });
    }
    *component_hash_count = total;
    Ok(())
}

fn require_remaining<B: Buf>(buf: &B, minimum: usize) -> Result<(), CodecError> {
    let available = buf.remaining();
    if available < minimum {
        return Err(CodecError::Underflow {
            needed: minimum - available,
            available,
        });
    }
    Ok(())
}

fn recipe_book_shape_size(width: i32, height: i32) -> Result<usize, CodecError> {
    if width <= 0 || height <= 0 {
        return Err(CodecError::NotSupported(
            "non-positive recipe-book shaped dimensions",
        ));
    }
    let size = (width as usize)
        .checked_mul(height as usize)
        .ok_or(CodecError::NotSupported(
            "recipe-book shaped dimensions overflow",
        ))?;
    if size > MAX_RECIPE_BOOK_SLOTS {
        return Err(CodecError::StringTooLong {
            len: size,
            max: MAX_RECIPE_BOOK_SLOTS,
        });
    }
    Ok(size)
}

// -----------------------------------------------------------------------

#[cfg(test)]
#[path = "play/report_ids_tests.rs"]
mod report_ids_tests;
#[cfg(test)]
mod tests;
