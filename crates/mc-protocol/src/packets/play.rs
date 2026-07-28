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

use bytes::{Buf, BufMut};
use mc_nbt::Tag;
use uuid::Uuid;

use super::{ClientInformation, CustomPayload, MainHand, Packet, ResourcePackStatus};
use crate::codec::{DEFAULT_MAX_STRING_LEN, Identifier, ReadMc, WriteMc};
use crate::error::CodecError;
use crate::packets::login::GameProfileProperty;

mod entity_sync_26_1_2;
mod merchant;

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
pub const ENTITY_DATA_SHARED_FLAGS_INDEX: u8 = 0;
pub const ENTITY_DATA_AIR_SUPPLY_INDEX: u8 = 1;
pub const ENTITY_DATA_POSE_INDEX: u8 = 6;
pub const LIVING_ENTITY_DATA_FLAGS_INDEX: u8 = 8;
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
pub const DATA_COMPONENT_DAMAGE_ID: i32 = 3;
/// `minecraft:custom_name` in the bundled vanilla 26.1.2
/// `minecraft:data_component_type` registry. `DataComponents.CUSTOM_NAME`
/// uses the `Component` stream codec, which is network NBT for the supported
/// literal text component shape.
pub const DATA_COMPONENT_CUSTOM_NAME_ID: i32 = 6;
/// `minecraft:item_model` in the local vanilla 26.1.2
/// `minecraft:data_component_type` registry report.
pub const DATA_COMPONENT_ITEM_MODEL_ID: i32 = 10;
pub const DATA_COMPONENT_ENCHANTMENTS_ID: i32 = 13;

fn write_long_array<B: BufMut>(buf: &mut B, longs: &[i64]) -> Result<(), CodecError> {
    let len = i32::try_from(longs.len()).map_err(|_| CodecError::StringTooLong {
        len: longs.len(),
        max: i32::MAX as usize,
    })?;
    buf.write_varint(len);
    for &v in longs {
        buf.write_i64(v);
    }
    Ok(())
}

fn read_long_array<B: Buf>(buf: &mut B) -> Result<Vec<i64>, CodecError> {
    let len_signed = buf.read_varint()?;
    if len_signed < 0 {
        return Err(CodecError::NegativeLength(len_signed));
    }
    let len = len_signed as usize;
    if len > MAX_LONG_ARRAY_LEN {
        return Err(CodecError::StringTooLong {
            len,
            max: MAX_LONG_ARRAY_LEN,
        });
    }
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(buf.read_i64()?);
    }
    Ok(out)
}

fn read_count<B: Buf>(buf: &mut B, max: usize) -> Result<usize, CodecError> {
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    let count = count as usize;
    if count > max {
        return Err(CodecError::StringTooLong { len: count, max });
    }
    Ok(count)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ClientboundCustomPayload {
    // `.analysis/protocol-dump.txt`: game CLIENTBOUND_CUSTOM_PAYLOAD is
    // clientbound registration index 24, wire id 0x18. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x18;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode(buf)?,
        })
    }
}

/// `Login (Play)` (CB). The server's "welcome to the world" packet that
/// transitions the client into Play state proper.
///
/// The wire layout has been stable since 1.20.5; the leading packet ID
/// shifts by a small number between every patch release as new packets
/// are inserted. The value below is a best-effort placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPlay {
    pub entity_id: i32,
    pub is_hardcore: bool,
    pub dimension_names: Vec<Identifier>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub do_limited_crafting: bool,
    /// Numeric id into the `minecraft:dimension_type` registry — the
    /// position of the entry in our [`RegistryData`] packet, not a
    /// stable namespaced identifier.
    pub dimension_type_id: i32,
    pub dimension_name: Identifier,
    pub hashed_seed: i64,
    /// 0 = Survival, 1 = Creative, 2 = Adventure, 3 = Spectator.
    pub game_mode: u8,
    /// -1 = none, otherwise same encoding as `game_mode`.
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(Identifier, i64)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub enforces_secure_chat: bool,
}

impl Packet for LoginPlay {
    // Verified against vanilla 26.1.2 wire capture via `wire-probe`:
    // server-bound LoginPlay arrives as id 0x31, body 108 bytes,
    // matching our field layout exactly.
    const ID: i32 = 0x31;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.entity_id);
        buf.write_bool(self.is_hardcore);
        buf.write_varint(i32::try_from(self.dimension_names.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.dimension_names.len(),
                max: i32::MAX as usize,
            }
        })?);
        for name in &self.dimension_names {
            buf.write_identifier(name)?;
        }
        buf.write_varint(self.max_players);
        buf.write_varint(self.view_distance);
        buf.write_varint(self.simulation_distance);
        buf.write_bool(self.reduced_debug_info);
        buf.write_bool(self.enable_respawn_screen);
        buf.write_bool(self.do_limited_crafting);
        buf.write_varint(self.dimension_type_id);
        buf.write_identifier(&self.dimension_name)?;
        buf.write_i64(self.hashed_seed);
        buf.write_u8(self.game_mode);
        buf.write_i8(self.previous_game_mode);
        buf.write_bool(self.is_debug);
        buf.write_bool(self.is_flat);
        match &self.death_location {
            Some((dim, pos)) => {
                buf.write_bool(true);
                buf.write_identifier(dim)?;
                buf.write_i64(*pos);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.portal_cooldown);
        buf.write_varint(self.sea_level);
        buf.write_bool(self.enforces_secure_chat);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_i32()?;
        let is_hardcore = buf.read_bool()?;
        let count = buf.read_varint()?;
        if count < 0 {
            return Err(CodecError::NegativeLength(count));
        }
        let mut dimension_names = Vec::with_capacity(count as usize);
        for _ in 0..count {
            dimension_names.push(buf.read_identifier()?);
        }
        let max_players = buf.read_varint()?;
        let view_distance = buf.read_varint()?;
        let simulation_distance = buf.read_varint()?;
        let reduced_debug_info = buf.read_bool()?;
        let enable_respawn_screen = buf.read_bool()?;
        let do_limited_crafting = buf.read_bool()?;
        let dimension_type_id = buf.read_varint()?;
        let dimension_name = buf.read_identifier()?;
        let hashed_seed = buf.read_i64()?;
        let game_mode = buf.read_u8()?;
        let previous_game_mode = buf.read_i8()?;
        let is_debug = buf.read_bool()?;
        let is_flat = buf.read_bool()?;
        let death_location = if buf.read_bool()? {
            let dim = buf.read_identifier()?;
            let pos = buf.read_i64()?;
            Some((dim, pos))
        } else {
            None
        };
        let portal_cooldown = buf.read_varint()?;
        let sea_level = buf.read_varint()?;
        let enforces_secure_chat = buf.read_bool()?;
        Ok(Self {
            entity_id,
            is_hardcore,
            dimension_names,
            max_players,
            view_distance,
            simulation_distance,
            reduced_debug_info,
            enable_respawn_screen,
            do_limited_crafting,
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            enforces_secure_chat,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundRespawn {
    pub dimension_type_id: i32,
    pub dimension_name: Identifier,
    pub hashed_seed: i64,
    pub game_mode: u8,
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(Identifier, i64)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub data_to_keep: i8,
}

impl Packet for ClientboundRespawn {
    // CLIENTBOUND_RESPAWN is game-CB index 82 = wire id 0x52 in the
    // local 26.1.2 GameProtocols dump. Its body is CommonPlayerSpawnInfo
    // followed by one byte of keep flags.
    const ID: i32 = 0x52;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.dimension_type_id);
        buf.write_identifier(&self.dimension_name)?;
        buf.write_i64(self.hashed_seed);
        buf.write_u8(self.game_mode);
        buf.write_i8(self.previous_game_mode);
        buf.write_bool(self.is_debug);
        buf.write_bool(self.is_flat);
        match &self.death_location {
            Some((dim, pos)) => {
                buf.write_bool(true);
                buf.write_identifier(dim)?;
                buf.write_i64(*pos);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.portal_cooldown);
        buf.write_varint(self.sea_level);
        buf.write_i8(self.data_to_keep);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let dimension_type_id = buf.read_varint()?;
        let dimension_name = buf.read_identifier()?;
        let hashed_seed = buf.read_i64()?;
        let game_mode = buf.read_u8()?;
        let previous_game_mode = buf.read_i8()?;
        let is_debug = buf.read_bool()?;
        let is_flat = buf.read_bool()?;
        let death_location = if buf.read_bool()? {
            let dim = buf.read_identifier()?;
            let pos = buf.read_i64()?;
            Some((dim, pos))
        } else {
            None
        };
        let portal_cooldown = buf.read_varint()?;
        let sea_level = buf.read_varint()?;
        let data_to_keep = buf.read_i8()?;
        Ok(Self {
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            data_to_keep,
        })
    }
}

/// `Set Default Spawn Position` (CB). Tells the client where its compass
/// should point.
///
/// Per ADR 0002, verified against the vanilla 26.1.2 client jar via `javap`:
/// the payload is `LevelData.RespawnData` = `GlobalPos` plus yaw/pitch floats.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDefaultSpawnPosition {
    pub dimension: Identifier,
    /// Block position packed into an `i64` per the vanilla
    /// `BlockPos` format: `((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
    pub position: i64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for SetDefaultSpawnPosition {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_DEFAULT_SPAWN_POSITION sits at game-CB index 96
    // (skipping `CLIENTBOUND_BUNDLE` at index 0) = wire id 0x61.
    const ID: i32 = 0x61;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_identifier(&self.dimension)?;
        buf.write_i64(self.position);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            dimension: buf.read_identifier()?,
            position: buf.read_i64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
        })
    }
}

/// `Synchronize Player Position` (CB). Vanilla uses this both at first
/// spawn ("you are now at <pos>") and as a periodic snap-back when the
/// server's view of the player drifts from the client's.
///
/// Since 1.21.2 this packet has a richer "PositionMoveFlags" bitfield;
/// for M1.g we only need the initial-spawn form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Bitfield. Bit 0..4 = "X/Y/Z/yaw/pitch is relative". For an
    /// initial absolute teleport: 0.
    pub relative_flags: i32,
}

impl Packet for SynchronizePlayerPosition {
    // Verified against vanilla 26.1.2 wire capture: id 0x48, body 61
    // bytes — varint teleport_id + 6×f64 (xyz + delta) + 2×f32 (yaw,
    // pitch) + i32 relative_flags. Matches our field layout exactly.
    const ID: i32 = 0x48;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.teleport_id);
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f64(self.dx);
        buf.write_f64(self.dy);
        buf.write_f64(self.dz);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_i32(self.relative_flags);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            teleport_id: buf.read_varint()?,
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            dx: buf.read_f64()?,
            dy: buf.read_f64()?,
            dz: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            relative_flags: buf.read_i32()?,
        })
    }
}

/// Clientbound `KeepAlive (Play)`. The server pings every ~15s with a
/// fresh `i64`; the client must echo it within ~30s or be disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundKeepAlive {
    pub id: i64,
}

impl Packet for ClientboundKeepAlive {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_KEEP_ALIVE at game-CB index 44 = wire id 0x2C.
    const ID: i32 = 0x2C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_i64()?,
        })
    }
}

/// `Disconnect (Play)` (CB). The polite "kick" packet; legal only inside
/// the Play state.
///
/// Vanilla has moved its disconnect-reason payload from a JSON string
/// to a binary NBT `Component` (since 1.20.4). For M1.g we serialise a
/// minimal NBT representation directly inline; full text-component
/// support is later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayDisconnect {
    /// Pre-serialised NBT bytes for the reason `Component`.
    pub reason_nbt: Vec<u8>,
}

impl Packet for PlayDisconnect {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_DISCONNECT at game-CB index 32 = wire id 0x20.
    const ID: i32 = 0x20;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.put_slice(&self.reason_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        // We can only round-trip what we wrote; without an NBT parser
        // inline we just slurp the rest of the body.
        let remaining = buf.remaining();
        let mut bytes = vec![0u8; remaining];
        buf.copy_to_slice(&mut bytes);
        Ok(Self { reason_nbt: bytes })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosionBlockParticle {
    pub particle_id: i32,
    pub scaling: f32,
    pub speed: f32,
    pub weight: i32,
}

/// Packet-local outbound TNT subset of vanilla's explode packet.
///
/// Particle options are restricted to verified payload-free IDs, and the
/// sound holder is reference-only. Decode support validates this same subset;
/// inline sound holders are intentionally unsupported.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundExplode {
    pub center: EntityVec3,
    pub radius: f32,
    pub block_count: i32,
    pub knockback: Option<EntityVec3>,
    pub explosion_particle_id: i32,
    /// Semantic registry ID for the reference-only sound-holder subset.
    /// The wire value is this ID plus one; inline holders are unsupported here.
    pub sound_reference_id: i32,
    pub block_particles: Vec<ExplosionBlockParticle>,
}

fn validate_simple_explosion_particle_id(particle_id: i32) -> Result<i32, CodecError> {
    // Vanilla 26.1.2 IDs verified for the current TNT packet shape:
    // explosion_emitter=22, poof=59, smoke=62. These codecs have no payload.
    match particle_id {
        22 | 59 | 62 => Ok(particle_id),
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

impl Packet for ClientboundExplode {
    // Vanilla 26.1.2 `CLIENTBOUND_EXPLODE` at game-CB index 36.
    const ID: i32 = 0x24;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_vec3(buf, self.center);
        buf.write_f32(self.radius);
        buf.write_i32(self.block_count);
        match self.knockback {
            Some(knockback) => {
                buf.write_bool(true);
                write_vec3(buf, knockback);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(validate_simple_explosion_particle_id(
            self.explosion_particle_id,
        )?);
        write_explosion_sound_reference_holder(buf, self.sound_reference_id)?;
        write_bounded_count(
            buf,
            self.block_particles.len(),
            MAX_EXPLOSION_BLOCK_PARTICLES,
        )?;
        for particle in &self.block_particles {
            buf.write_varint(validate_simple_explosion_particle_id(particle.particle_id)?);
            buf.write_f32(particle.scaling);
            buf.write_f32(particle.speed);
            if particle.weight < 0 {
                return Err(CodecError::NotSupported(
                    "negative explosion block particle weight",
                ));
            }
            buf.write_varint(particle.weight);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let center = read_vec3(buf)?;
        let radius = buf.read_f32()?;
        let block_count = buf.read_i32()?;
        let knockback = if buf.read_bool()? {
            Some(read_vec3(buf)?)
        } else {
            None
        };
        let explosion_particle_id = validate_simple_explosion_particle_id(buf.read_varint()?)?;
        let sound_reference_id = read_explosion_sound_reference_holder(buf)?;
        let block_particle_count = read_count(buf, MAX_EXPLOSION_BLOCK_PARTICLES)?;
        let mut block_particles = Vec::with_capacity(block_particle_count);
        for _ in 0..block_particle_count {
            let particle_id = validate_simple_explosion_particle_id(buf.read_varint()?)?;
            let scaling = buf.read_f32()?;
            let speed = buf.read_f32()?;
            let weight = buf.read_varint()?;
            if weight < 0 {
                return Err(CodecError::NotSupported(
                    "negative explosion block particle weight",
                ));
            }
            block_particles.push(ExplosionBlockParticle {
                particle_id,
                scaling,
                speed,
                weight,
            });
        }
        Ok(Self {
            center,
            radius,
            block_count,
            knockback,
            explosion_particle_id,
            sound_reference_id,
            block_particles,
        })
    }
}

/// `Clientbound Forget Level Chunk` (CB). Unloads one chunk from the
/// client's currently loaded view.
///
/// Per ADR 0002, verified against vanilla 26.1.2: `GameProtocols`
/// registers `CLIENTBOUND_FORGET_LEVEL_CHUNK` at game-CB index 37
/// (including `CLIENTBOUND_BUNDLE`), and
/// `ClientboundForgetLevelChunkPacket.write` calls
/// `FriendlyByteBuf.writeChunkPos`. `writeChunkPos` writes one raw
/// big-endian `i64` from `ChunkPos.pack(x, z)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetLevelChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for ForgetLevelChunk {
    const ID: i32 = 0x25;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(pack_chunk_pos(self.chunk_x, self.chunk_z));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let packed = buf.read_i64()?;
        Ok(Self {
            chunk_x: packed as i32,
            chunk_z: (packed >> 32) as i32,
        })
    }
}

/// Pack vanilla's `ChunkPos.pack(x, z)` representation:
/// `(x & 0xffffffff) | ((z & 0xffffffff) << 32)`.
#[must_use]
pub fn pack_chunk_pos(x: i32, z: i32) -> i64 {
    ((x as i64) & 0xFFFF_FFFF) | (((z as i64) & 0xFFFF_FFFF) << 32)
}

/// `Game Event` (CB). Used here for one specific game event:
/// `start_waiting_for_level_chunks`, sent right after Login (Play) to
/// tell the client "you can stop showing the loading screen".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    pub event: u8,
    pub value: f32,
}

impl GameEvent {
    pub const EVENT_CHANGE_GAME_MODE: u8 = 3;
    pub const EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
}

impl Packet for GameEvent {
    // Verified against vanilla 26.1.2 wire capture: id 0x26, body
    // `0d 00 00 00 00` = event 13 (start_waiting_for_chunks), value 0.0.
    const ID: i32 = 0x26;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.event);
        buf.write_f32(self.value);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            event: buf.read_u8()?,
            value: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundInitializeBorder {
    pub center_x: f64,
    pub center_z: f64,
    pub old_size: f64,
    pub new_size: f64,
    pub lerp_time: i64,
    pub absolute_max_size: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
}

impl Packet for ClientboundInitializeBorder {
    // `.analysis/protocol-dump.txt` / GameProtocols: game-CB index 43,
    // wire id 0x2B. `ClientboundInitializeBorderPacket` writes doubles for
    // center/size, then VarLong lerp time and three VarInts.
    const ID: i32 = 0x2B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.center_x);
        buf.write_f64(self.center_z);
        buf.write_f64(self.old_size);
        buf.write_f64(self.new_size);
        buf.write_varlong(self.lerp_time);
        buf.write_varint(self.absolute_max_size);
        buf.write_varint(self.warning_blocks);
        buf.write_varint(self.warning_time);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            center_x: buf.read_f64()?,
            center_z: buf.read_f64()?,
            old_size: buf.read_f64()?,
            new_size: buf.read_f64()?,
            lerp_time: buf.read_varlong()?,
            absolute_max_size: buf.read_varint()?,
            warning_blocks: buf.read_varint()?,
            warning_time: buf.read_varint()?,
        })
    }
}

/// Clientbound `ChangeDifficulty`. Vanilla sends this during Play entry
/// right after `LoginPlay` to inform the client of the current difficulty
/// setting and whether it is locked.
///
/// Wire format: VarInt ordinal (0=PEACEFUL, 1=EASY, 2=NORMAL, 3=HARD),
/// followed by a bool (`locked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundChangeDifficulty {
    /// Difficulty ordinal. 0 = PEACEFUL, 1 = EASY, 2 = NORMAL, 3 = HARD.
    pub difficulty: u8,
    /// Whether the difficulty is locked (cannot be changed in-game).
    pub locked: bool,
}

impl Packet for ClientboundChangeDifficulty {
    // `.analysis/protocol-dump.txt` / `GameProtocols`:
    // CLIENTBOUND_CHANGE_DIFFICULTY is clientbound registration index 10,
    // wire id 0x0A.
    const ID: i32 = 0x0A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(i32::from(self.difficulty));
        buf.write_bool(self.locked);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let difficulty = buf.read_varint()?;
        if !(0..=3).contains(&difficulty) {
            return Err(CodecError::NotSupported("difficulty ordinal out of range"));
        }
        Ok(Self {
            difficulty: difficulty as u8,
            locked: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCooldown {
    pub cooldown_group: Identifier,
    pub duration: i32,
}

impl Packet for ClientboundCooldown {
    // `.analysis/protocol-dump.txt` / `GameProtocols`:
    // CLIENTBOUND_COOLDOWN is clientbound registration index 22,
    // wire id 0x16. `ClientboundCooldownPacket.STREAM_CODEC` is
    // `Identifier.STREAM_CODEC` followed by `VAR_INT` duration.
    const ID: i32 = 0x16;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_identifier(&self.cooldown_group)?;
        buf.write_varint(self.duration);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            cooldown_group: buf.read_identifier()?,
            duration: buf.read_varint()?,
        })
    }
}

/// One 26.1.2 world-clock state carried by `ClientboundSetTime`.
///
/// `total_ticks` is a VarLong on the wire. The client advances it by
/// `game_time_delta * rate`, carrying the fractional remainder in
/// `partial_tick` until the next authoritative update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldClockUpdate {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
}

/// The embedded 26.1.2 `world_clock` registry is ordered as overworld then End.
/// `WorldClock.STREAM_CODEC` therefore writes these holder ids as VarInts.
pub const OVERWORLD_WORLD_CLOCK_ID: i32 = 0;
pub const THE_END_WORLD_CLOCK_ID: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundSetTime {
    pub game_time: i64,
    pub overworld_clock: Option<WorldClockUpdate>,
    pub the_end_clock: Option<WorldClockUpdate>,
}

impl ClientboundSetTime {
    #[must_use]
    pub const fn overworld(game_time: i64, total_ticks: i64, rate: f32) -> Self {
        Self {
            game_time,
            overworld_clock: Some(WorldClockUpdate {
                total_ticks,
                partial_tick: 0.0,
                rate,
            }),
            the_end_clock: None,
        }
    }
}

impl Packet for ClientboundSetTime {
    // `.analysis/protocol-dump.txt` / `GameProtocols`: game
    // CLIENTBOUND_SET_TIME is clientbound registration index 113, wire id 0x71.
    // Local 26.1.2 `javap` evidence:
    // - `long gameTime`;
    // - VarInt-sized map;
    // - `WorldClock.STREAM_CODEC`, a holder-registry VarInt id;
    // - vanilla `ClockNetworkState`: VarLong totalTicks, float partialTick, float rate.
    const ID: i32 = 0x71;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.game_time);
        let update_count =
            i32::from(self.overworld_clock.is_some()) + i32::from(self.the_end_clock.is_some());
        buf.write_varint(update_count);
        if let Some(state) = self.overworld_clock {
            buf.write_varint(OVERWORLD_WORLD_CLOCK_ID);
            encode_clock_network_state(buf, state);
        }
        if let Some(state) = self.the_end_clock {
            buf.write_varint(THE_END_WORLD_CLOCK_ID);
            encode_clock_network_state(buf, state);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let game_time = buf.read_i64()?;
        let update_count = buf.read_varint()?;
        if update_count < 0 {
            return Err(CodecError::NegativeLength(update_count));
        }
        if update_count > 2 {
            return Err(CodecError::NotSupported(
                "more than two 26.1.2 world-clock updates",
            ));
        }
        let mut overworld_clock = None;
        let mut the_end_clock = None;
        for _ in 0..update_count {
            let clock_id = buf.read_varint()?;
            let state = decode_clock_network_state(buf)?;
            match clock_id {
                OVERWORLD_WORLD_CLOCK_ID if overworld_clock.is_none() => {
                    overworld_clock = Some(state);
                }
                THE_END_WORLD_CLOCK_ID if the_end_clock.is_none() => {
                    the_end_clock = Some(state);
                }
                OVERWORLD_WORLD_CLOCK_ID | THE_END_WORLD_CLOCK_ID => {
                    return Err(CodecError::NotSupported(
                        "duplicate 26.1.2 world-clock update",
                    ));
                }
                _ => {
                    return Err(CodecError::NotSupported(
                        "unknown 26.1.2 world-clock registry id",
                    ));
                }
            }
        }
        Ok(Self {
            game_time,
            overworld_clock,
            the_end_clock,
        })
    }
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundPlayerAbilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub can_fly: bool,
    pub instabuild: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundSetHealth {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundSetExperience {
    pub experience_progress: f32,
    pub total_experience: i32,
    pub experience_level: i32,
}

impl Packet for ClientboundSetHealth {
    // Verified from `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_HEALTH is
    // game-CB index 104 = wire id 0x68. `javap -p -c
    // ClientboundSetHealthPacket` shows f32 health, VarInt food, f32 saturation.
    const ID: i32 = 0x68;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.health);
        buf.write_varint(self.food);
        buf.write_f32(self.saturation);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            health: buf.read_f32()?,
            food: buf.read_varint()?,
            saturation: buf.read_f32()?,
        })
    }
}

impl Packet for ClientboundSetExperience {
    // Verified from `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_EXPERIENCE
    // is game-CB index 103 = wire id 0x67. Vanilla 26.1.2 `javap -p -c`
    // shows f32 progress, VarInt level, VarInt total experience.
    const ID: i32 = 0x67;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.experience_progress);
        buf.write_varint(self.experience_level);
        buf.write_varint(self.total_experience);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let experience_progress = buf.read_f32()?;
        let experience_level = buf.read_varint()?;
        let total_experience = buf.read_varint()?;
        Ok(Self {
            experience_progress,
            total_experience,
            experience_level,
        })
    }
}

impl Packet for ClientboundPlayerAbilities {
    // Verified from `.analysis/protocol-dump.txt`: CLIENTBOUND_PLAYER_ABILITIES
    // is game-CB index 64 = wire id 0x40. `javap -p -c
    // ClientboundPlayerAbilitiesPacket` shows one flags byte
    // (1=invulnerable, 2=flying, 4=canFly, 8=instabuild), then f32
    // flying speed and f32 walking speed.
    const ID: i32 = 0x40;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let mut flags = 0_u8;
        if self.invulnerable {
            flags |= 0x01;
        }
        if self.flying {
            flags |= 0x02;
        }
        if self.can_fly {
            flags |= 0x04;
        }
        if self.instabuild {
            flags |= 0x08;
        }
        buf.write_u8(flags);
        buf.write_f32(self.flying_speed);
        buf.write_f32(self.walking_speed);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let flags = buf.read_u8()?;
        Ok(Self {
            invulnerable: flags & 0x01 != 0,
            flying: flags & 0x02 != 0,
            can_fly: flags & 0x04 != 0,
            instabuild: flags & 0x08 != 0,
            flying_speed: buf.read_f32()?,
            walking_speed: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundBlockEntityData {
    pub position: i64,
    pub block_entity_type: i32,
    pub nbt: Tag,
}

impl Packet for ClientboundBlockEntityData {
    // Verified from `.analysis/protocol-dump.txt`: CLIENTBOUND_BLOCK_ENTITY_DATA
    // is game-CB index 6 = wire id 0x06. Declared field order is BlockPos,
    // BlockEntityType registry id, then network-NBT compound tag.
    const ID: i32 = 0x06;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_varint(self.block_entity_type);
        mc_nbt::write_network(buf, &self.nbt)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            block_entity_type: buf.read_varint()?,
            nbt: mc_nbt::read_network(buf)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundOpenSignEditor {
    pub position: i64,
    pub is_front_text: bool,
}

impl Packet for ClientboundOpenSignEditor {
    // Verified from `.analysis/protocol-dump.txt`: CLIENTBOUND_OPEN_SIGN_EDITOR
    // is game-CB index 60 = wire id 0x3C. Declared field order is BlockPos,
    // then front/back text boolean.
    const ID: i32 = 0x3C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_bool(self.is_front_text);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            is_front_text: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl EntityVec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionMoveRotation {
    pub position: EntityVec3,
    pub delta_movement: EntityVec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl PositionMoveRotation {
    fn encode<B: BufMut>(&self, buf: &mut B) {
        write_vec3(buf, self.position);
        write_vec3(buf, self.delta_movement);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: read_vec3(buf)?,
            delta_movement: read_vec3(buf)?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerInfoActions(u8);

impl PlayerInfoActions {
    pub const ADD_PLAYER: Self = Self(1 << 0);
    pub const INITIALIZE_CHAT: Self = Self(1 << 1);
    pub const UPDATE_GAME_MODE: Self = Self(1 << 2);
    pub const UPDATE_LISTED: Self = Self(1 << 3);
    pub const UPDATE_LATENCY: Self = Self(1 << 4);
    pub const UPDATE_DISPLAY_NAME: Self = Self(1 << 5);
    pub const UPDATE_LIST_ORDER: Self = Self(1 << 6);
    pub const UPDATE_HAT: Self = Self(1 << 7);

    pub const fn minimal_add_player() -> Self {
        Self(
            Self::ADD_PLAYER.0
                | Self::UPDATE_GAME_MODE.0
                | Self::UPDATE_LISTED.0
                | Self::UPDATE_LATENCY.0
                | Self::UPDATE_DISPLAY_NAME.0
                | Self::UPDATE_LIST_ORDER.0
                | Self::UPDATE_HAT.0,
        )
    }

    fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoEntry {
    pub profile_id: Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
    pub listed: bool,
    pub latency: i32,
    pub game_mode: i32,
    pub list_order: i32,
    pub show_hat: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoUpdate {
    pub actions: PlayerInfoActions,
    pub entries: Vec<PlayerInfoEntry>,
}

impl Packet for PlayerInfoUpdate {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_PLAYER_INFO_UPDATE
    // sits at game-CB index 70 = wire id 0x46. `javap` shows an 8-bit fixed
    // EnumSet, then a VarInt-counted entry list keyed by profile UUID.
    const ID: i32 = 0x46;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.actions.0);
        write_count(buf, self.entries.len())?;
        for entry in &self.entries {
            buf.write_uuid(entry.profile_id);
            if self.actions.contains(PlayerInfoActions::ADD_PLAYER) {
                buf.write_string(&entry.name, 16)?;
                if entry.properties.len() > MAX_GAME_PROFILE_PROPERTIES {
                    return Err(CodecError::StringTooLong {
                        len: entry.properties.len(),
                        max: MAX_GAME_PROFILE_PROPERTIES,
                    });
                }
                write_count(buf, entry.properties.len())?;
                for property in &entry.properties {
                    buf.write_string(&property.name, MAX_GAME_PROFILE_PROPERTY_NAME_LEN)?;
                    buf.write_string(&property.value, MAX_GAME_PROFILE_PROPERTY_VALUE_LEN)?;
                    match &property.signature {
                        Some(signature) => {
                            buf.write_bool(true);
                            buf.write_string(signature, MAX_GAME_PROFILE_PROPERTY_SIGNATURE_LEN)?;
                        }
                        None => buf.write_bool(false),
                    }
                }
            }
            if self.actions.contains(PlayerInfoActions::INITIALIZE_CHAT) {
                return Err(CodecError::NotSupported("player chat session data"));
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_GAME_MODE) {
                buf.write_varint(entry.game_mode);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LISTED) {
                buf.write_bool(entry.listed);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LATENCY) {
                buf.write_varint(entry.latency);
            }
            if self
                .actions
                .contains(PlayerInfoActions::UPDATE_DISPLAY_NAME)
            {
                buf.write_bool(false);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LIST_ORDER) {
                buf.write_varint(entry.list_order);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_HAT) {
                buf.write_bool(entry.show_hat);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let actions = PlayerInfoActions(buf.read_u8()?);
        let count = read_count(buf, MAX_PLAYER_INFO_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let profile_id = buf.read_uuid()?;
            let mut name = String::new();
            let mut properties = Vec::new();
            let mut listed = false;
            let mut latency = 0;
            let mut game_mode = 0;
            let mut list_order = 0;
            let mut show_hat = false;
            if actions.contains(PlayerInfoActions::ADD_PLAYER) {
                name = buf.read_string(16)?;
                let property_count = read_count(buf, MAX_GAME_PROFILE_PROPERTIES)?;
                properties.reserve(property_count);
                for _ in 0..property_count {
                    let property_name = buf.read_string(MAX_GAME_PROFILE_PROPERTY_NAME_LEN)?;
                    let value = buf.read_string(MAX_GAME_PROFILE_PROPERTY_VALUE_LEN)?;
                    let signature = if buf.read_bool()? {
                        Some(buf.read_string(MAX_GAME_PROFILE_PROPERTY_SIGNATURE_LEN)?)
                    } else {
                        None
                    };
                    properties.push(GameProfileProperty {
                        name: property_name,
                        value,
                        signature,
                    });
                }
            }
            if actions.contains(PlayerInfoActions::INITIALIZE_CHAT) {
                return Err(CodecError::NotSupported("player chat session data"));
            }
            if actions.contains(PlayerInfoActions::UPDATE_GAME_MODE) {
                game_mode = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_LISTED) {
                listed = buf.read_bool()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_LATENCY) {
                latency = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_DISPLAY_NAME) {
                let has_display_name = buf.read_bool()?;
                if has_display_name {
                    return Err(CodecError::NotSupported("player display name component"));
                }
            }
            if actions.contains(PlayerInfoActions::UPDATE_LIST_ORDER) {
                list_order = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_HAT) {
                show_hat = buf.read_bool()?;
            }
            entries.push(PlayerInfoEntry {
                profile_id,
                name,
                properties,
                listed,
                latency,
                game_mode,
                list_order,
                show_hat,
            });
        }
        Ok(Self { actions, entries })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoRemove {
    pub profile_ids: Vec<Uuid>,
}

impl Packet for PlayerInfoRemove {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_PLAYER_INFO_REMOVE
    // sits at game-CB index 69 = wire id 0x45. Body is a VarInt-counted UUID list.
    const ID: i32 = 0x45;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.profile_ids.len())?;
        for id in &self.profile_ids {
            buf.write_uuid(*id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = read_count(buf, MAX_PLAYER_INFO_ENTRIES)?;
        let mut profile_ids = Vec::with_capacity(count);
        for _ in 0..count {
            profile_ids.push(buf.read_uuid()?);
        }
        Ok(Self { profile_ids })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddEntity {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub entity_type_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub movement: EntityVec3,
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub data: i32,
}

impl Packet for AddEntity {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ADD_ENTITY is
    // game-CB index 1 = wire id 0x01. `javap` shows VarInt id, UUID,
    // registry entity type id, xyz doubles, `Vec3.LP_STREAM_CODEC`, packed
    // x/y/head rotations, then VarInt data.
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_uuid(self.uuid);
        buf.write_varint(self.entity_type_id);
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        write_lp_vec3(buf, self.movement);
        buf.write_u8(pack_degrees(self.pitch));
        buf.write_u8(pack_degrees(self.yaw));
        buf.write_u8(pack_degrees(self.head_yaw));
        buf.write_varint(self.data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            uuid: buf.read_uuid()?,
            entity_type_id: buf.read_varint()?,
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            movement: read_lp_vec3(buf)?,
            pitch: unpack_degrees(buf.read_u8()?),
            yaw: unpack_degrees(buf.read_u8()?),
            head_yaw: unpack_degrees(buf.read_u8()?),
            data: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityAnimationAction {
    SwingMainHand,
    WakeUp,
    SwingOffHand,
    CriticalHit,
    MagicCriticalHit,
    Raw(u8),
}

impl EntityAnimationAction {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::SwingMainHand => 0,
            Self::WakeUp => 2,
            Self::SwingOffHand => 3,
            Self::CriticalHit => 4,
            Self::MagicCriticalHit => 5,
            Self::Raw(value) => value,
        }
    }

    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::SwingMainHand,
            2 => Self::WakeUp,
            3 => Self::SwingOffHand,
            4 => Self::CriticalHit,
            5 => Self::MagicCriticalHit,
            other => Self::Raw(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityAnimation {
    pub entity_id: i32,
    pub action: EntityAnimationAction,
}

impl Packet for EntityAnimation {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_ANIMATE is game-CB index 2 = wire id 0x02.
    // `javap -p -c ClientboundAnimatePacket` shows VarInt entity id
    // followed by one unsigned byte action.
    const ID: i32 = 0x02;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_u8(self.action.as_u8());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            action: EntityAnimationAction::from_u8(buf.read_u8()?),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityEvent {
    pub entity_id: i32,
    pub event_id: i8,
}

impl Packet for EntityEvent {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_ENTITY_EVENT is game-CB index 34 = wire id 0x22.
    // `javap -p -c ClientboundEntityEventPacket` shows i32 entity id
    // followed by one signed byte event id.
    const ID: i32 = 0x22;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.entity_id);
        buf.write_i8(self.event_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_i32()?,
            event_id: buf.read_i8()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEntityPos {
    pub entity_id: i32,
    pub delta_x: i16,
    pub delta_y: i16,
    pub delta_z: i16,
    pub on_ground: bool,
}

impl MoveEntityPos {
    #[must_use]
    pub fn delta_to_short(delta: f64) -> i16 {
        move_entity_delta_to_short(delta)
    }
}

impl Packet for MoveEntityPos {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_MOVE_ENTITY_POS
    // immediately precedes CLIENTBOUND_MOVE_ENTITY_POS_ROT, so its game-CB
    // index is 53 = wire id 0x35. `ClientboundMoveEntityPacket$Pos` carries
    // VarInt id, three i16 relative deltas, then onGround bool.
    const ID: i32 = 0x35;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_i16(self.delta_x);
        buf.write_i16(self.delta_y);
        buf.write_i16(self.delta_z);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            delta_x: buf.read_i16()?,
            delta_y: buf.read_i16()?,
            delta_z: buf.read_i16()?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEntityPosRot {
    pub entity_id: i32,
    pub delta_x: i16,
    pub delta_y: i16,
    pub delta_z: i16,
    pub yaw: u8,
    pub pitch: u8,
    pub on_ground: bool,
}

impl MoveEntityPosRot {
    #[must_use]
    pub fn delta_to_short(delta: f64) -> i16 {
        move_entity_delta_to_short(delta)
    }

    #[must_use]
    pub fn pack_degrees(degrees: f32) -> u8 {
        pack_degrees(degrees)
    }
}

fn move_entity_delta_to_short(delta: f64) -> i16 {
    (delta * 4096.0)
        .round()
        .clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

impl Packet for MoveEntityPosRot {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_MOVE_ENTITY_POS_ROT is game-CB index 54 = wire id 0x36.
    // `javap -p -c ClientboundMoveEntityPacket$PosRot` shows VarInt id,
    // three i16 relative deltas, packed yRot byte, packed xRot byte,
    // then onGround bool.
    const ID: i32 = 0x36;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_i16(self.delta_x);
        buf.write_i16(self.delta_y);
        buf.write_i16(self.delta_z);
        buf.write_u8(self.yaw);
        buf.write_u8(self.pitch);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            delta_x: buf.read_i16()?,
            delta_y: buf.read_i16()?,
            delta_z: buf.read_i16()?,
            yaw: buf.read_u8()?,
            pitch: buf.read_u8()?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityPositionSync {
    pub entity_id: i32,
    pub values: PositionMoveRotation,
    pub on_ground: bool,
}

impl Packet for EntityPositionSync {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ENTITY_POSITION_SYNC
    // is game-CB index 35 = wire id 0x23. Body is VarInt id,
    // `PositionMoveRotation.STREAM_CODEC`, then onGround bool.
    const ID: i32 = 0x23;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        self.values.encode(buf);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            values: PositionMoveRotation::decode(buf)?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetEntityMotion {
    pub entity_id: i32,
    pub movement: EntityVec3,
}

impl Packet for SetEntityMotion {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_SET_ENTITY_MOTION is game-CB index 101 = wire id 0x65.
    // `javap -p -c ClientboundSetEntityMotionPacket` shows VarInt id plus
    // `Vec3.LP_STREAM_CODEC` movement.
    const ID: i32 = 0x65;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        write_lp_vec3(buf, self.movement);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            movement: read_lp_vec3(buf)?,
        })
    }
}

/// A non-negative `Block.BLOCK_STATE_REGISTRY` wire id.
///
/// This type distinguishes block-state ids from other registry ids. It does
/// not claim that the id exists in a particular runtime registry; callers
/// must resolve that before constructing packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStateId(i32);

impl BlockStateId {
    pub fn new(raw: u32) -> Result<Self, CodecError> {
        let raw = i32::try_from(raw).map_err(|_| {
            CodecError::NotSupported("block-state registry id exceeds VarInt range")
        })?;
        Ok(Self(raw))
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0 as u32
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let raw = buf.read_varint()?;
        if raw < 0 {
            return Err(CodecError::NotSupported("negative block-state registry id"));
        }
        Ok(Self(raw))
    }
}

/// Three big-endian floats used by entity-data serializer id 9.
#[derive(Debug, Clone, Copy)]
pub struct EntityRotations {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl PartialEq for EntityRotations {
    fn eq(&self, other: &Self) -> bool {
        java_float_equals(self.x, other.x)
            && java_float_equals(self.y, other.y)
            && java_float_equals(self.z, other.z)
    }
}

impl Eq for EntityRotations {}

/// The six direction ids established by `Direction.STREAM_CODEC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityDirection {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl EntityDirection {
    const fn wire_id(self) -> i32 {
        match self {
            Self::Down => 0,
            Self::Up => 1,
            Self::North => 2,
            Self::South => 3,
            Self::West => 4,
            Self::East => 5,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Down,
            1 => Self::Up,
            2 => Self::North,
            3 => Self::South,
            4 => Self::West,
            5 => Self::East,
            _ => return Err(CodecError::NotSupported("unknown entity direction")),
        })
    }
}

/// Entity-data serializers implemented for 26.1.2.
///
/// Supported serializer ids are 0-4, 7-15, 18-20, and 42. Component,
/// particle, variant, global-position, vector, quaternion, and
/// resolvable-profile serializers are rejected explicitly because this crate
/// does not yet have faithful local value types for their payloads.
#[derive(Debug, Clone)]
pub enum EntityDataValue {
    Byte {
        index: u8,
        value: i8,
    },
    Int {
        index: u8,
        value: i32,
    },
    Long {
        index: u8,
        value: i64,
    },
    Float {
        index: u8,
        value: f32,
    },
    String {
        index: u8,
        value: String,
    },
    ItemStack {
        index: u8,
        stack: ItemStack,
    },
    Boolean {
        index: u8,
        value: bool,
    },
    Rotations {
        index: u8,
        value: EntityRotations,
    },
    /// Packed `BlockPos::asLong()` value.
    BlockPosition {
        index: u8,
        value: i64,
    },
    OptionalBlockPosition {
        index: u8,
        value: Option<i64>,
    },
    Direction {
        index: u8,
        value: EntityDirection,
    },
    OptionalLivingEntityReference {
        index: u8,
        value: Option<Uuid>,
    },
    BlockState {
        index: u8,
        value: BlockStateId,
    },
    OptionalBlockState {
        index: u8,
        value: Option<BlockStateId>,
    },
    VillagerData {
        index: u8,
        villager_type: i32,
        profession: i32,
        level: i32,
    },
    OptionalUnsignedInt {
        index: u8,
        value: Option<u32>,
    },
    Pose {
        index: u8,
        pose: EntityPose,
    },
    HumanoidArm {
        index: u8,
        value: MainHand,
    },
}

impl PartialEq for EntityDataValue {
    fn eq(&self, other: &Self) -> bool {
        if self.index() != other.index() || self.serializer_id() != other.serializer_id() {
            return false;
        }

        match (self, other) {
            (Self::Byte { value: left, .. }, Self::Byte { value: right, .. }) => left == right,
            (Self::Int { value: left, .. }, Self::Int { value: right, .. }) => left == right,
            (Self::Long { value: left, .. }, Self::Long { value: right, .. }) => left == right,
            (Self::Float { value: left, .. }, Self::Float { value: right, .. }) => {
                java_float_equals(*left, *right)
            }
            (Self::String { value: left, .. }, Self::String { value: right, .. }) => left == right,
            (Self::ItemStack { stack: left, .. }, Self::ItemStack { stack: right, .. }) => {
                left == right
            }
            (Self::Boolean { value: left, .. }, Self::Boolean { value: right, .. }) => {
                left == right
            }
            (Self::Rotations { value: left, .. }, Self::Rotations { value: right, .. }) => {
                left == right
            }
            (Self::BlockPosition { value: left, .. }, Self::BlockPosition { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalBlockPosition { value: left, .. },
                Self::OptionalBlockPosition { value: right, .. },
            ) => left == right,
            (Self::Direction { value: left, .. }, Self::Direction { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalLivingEntityReference { value: left, .. },
                Self::OptionalLivingEntityReference { value: right, .. },
            ) => left == right,
            (Self::BlockState { value: left, .. }, Self::BlockState { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalBlockState { value: left, .. },
                Self::OptionalBlockState { value: right, .. },
            ) => left == right,
            (
                Self::VillagerData {
                    villager_type: left_type,
                    profession: left_profession,
                    level: left_level,
                    ..
                },
                Self::VillagerData {
                    villager_type: right_type,
                    profession: right_profession,
                    level: right_level,
                    ..
                },
            ) => {
                left_type == right_type
                    && left_profession == right_profession
                    && left_level == right_level
            }
            (
                Self::OptionalUnsignedInt { value: left, .. },
                Self::OptionalUnsignedInt { value: right, .. },
            ) => left == right,
            (Self::Pose { pose: left, .. }, Self::Pose { pose: right, .. }) => left == right,
            (Self::HumanoidArm { value: left, .. }, Self::HumanoidArm { value: right, .. }) => {
                left == right
            }
            _ => false,
        }
    }
}

impl Eq for EntityDataValue {}

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

impl EntityDataValue {
    #[must_use]
    pub const fn index(&self) -> u8 {
        match self {
            Self::Byte { index, .. }
            | Self::Int { index, .. }
            | Self::Long { index, .. }
            | Self::Float { index, .. }
            | Self::String { index, .. }
            | Self::ItemStack { index, .. }
            | Self::Boolean { index, .. }
            | Self::Rotations { index, .. }
            | Self::BlockPosition { index, .. }
            | Self::OptionalBlockPosition { index, .. }
            | Self::Direction { index, .. }
            | Self::OptionalLivingEntityReference { index, .. }
            | Self::BlockState { index, .. }
            | Self::OptionalBlockState { index, .. }
            | Self::VillagerData { index, .. }
            | Self::OptionalUnsignedInt { index, .. }
            | Self::Pose { index, .. }
            | Self::HumanoidArm { index, .. } => *index,
        }
    }

    #[must_use]
    pub const fn serializer_id(&self) -> i32 {
        match self {
            Self::Byte { .. } => ENTITY_DATA_BYTE_SERIALIZER_ID,
            Self::Int { .. } => ENTITY_DATA_INT_SERIALIZER_ID,
            Self::Long { .. } => ENTITY_DATA_LONG_SERIALIZER_ID,
            Self::Float { .. } => ENTITY_DATA_FLOAT_SERIALIZER_ID,
            Self::String { .. } => ENTITY_DATA_STRING_SERIALIZER_ID,
            Self::ItemStack { .. } => ENTITY_DATA_ITEM_STACK_SERIALIZER_ID,
            Self::Boolean { .. } => ENTITY_DATA_BOOLEAN_SERIALIZER_ID,
            Self::Rotations { .. } => ENTITY_DATA_ROTATIONS_SERIALIZER_ID,
            Self::BlockPosition { .. } => ENTITY_DATA_BLOCK_POSITION_SERIALIZER_ID,
            Self::OptionalBlockPosition { .. } => ENTITY_DATA_OPTIONAL_BLOCK_POSITION_SERIALIZER_ID,
            Self::Direction { .. } => ENTITY_DATA_DIRECTION_SERIALIZER_ID,
            Self::OptionalLivingEntityReference { .. } => {
                ENTITY_DATA_OPTIONAL_LIVING_ENTITY_REFERENCE_SERIALIZER_ID
            }
            Self::BlockState { .. } => ENTITY_DATA_BLOCK_STATE_SERIALIZER_ID,
            Self::OptionalBlockState { .. } => ENTITY_DATA_OPTIONAL_BLOCK_STATE_SERIALIZER_ID,
            Self::VillagerData { .. } => ENTITY_DATA_VILLAGER_DATA_SERIALIZER_ID,
            Self::OptionalUnsignedInt { .. } => ENTITY_DATA_OPTIONAL_UNSIGNED_INT_SERIALIZER_ID,
            Self::Pose { .. } => ENTITY_DATA_POSE_SERIALIZER_ID,
            Self::HumanoidArm { .. } => ENTITY_DATA_HUMANOID_ARM_SERIALIZER_ID,
        }
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.index());
        buf.write_varint(self.serializer_id());
        match self {
            Self::Byte { value, .. } => buf.write_i8(*value),
            Self::Int { value, .. } => buf.write_varint(*value),
            Self::Long { value, .. } => buf.write_varlong(*value),
            Self::Float { value, .. } => buf.write_f32(*value),
            Self::String { value, .. } => {
                buf.write_string(value, DEFAULT_MAX_STRING_LEN)?;
            }
            Self::ItemStack { stack, .. } => stack.encode(buf)?,
            Self::Boolean { value, .. } => buf.write_bool(*value),
            Self::Rotations { value, .. } => {
                buf.write_f32(value.x);
                buf.write_f32(value.y);
                buf.write_f32(value.z);
            }
            Self::BlockPosition { value, .. } => buf.write_i64(*value),
            Self::OptionalBlockPosition { value, .. } => {
                buf.write_bool(value.is_some());
                if let Some(value) = value {
                    buf.write_i64(*value);
                }
            }
            Self::Direction { value, .. } => buf.write_varint(value.wire_id()),
            Self::OptionalLivingEntityReference { value, .. } => {
                buf.write_bool(value.is_some());
                if let Some(value) = value {
                    buf.write_uuid(*value);
                }
            }
            Self::BlockState { value, .. } => buf.write_varint(value.0),
            Self::OptionalBlockState { value, .. } => match value {
                None => buf.write_varint(0),
                Some(value) if value.0 == 0 => {
                    return Err(CodecError::NotSupported(
                        "optional block-state id zero means absent",
                    ));
                }
                Some(value) => buf.write_varint(value.0),
            },
            Self::VillagerData {
                villager_type,
                profession,
                level,
                ..
            } => {
                buf.write_varint(*villager_type);
                buf.write_varint(*profession);
                buf.write_varint(*level);
            }
            Self::OptionalUnsignedInt { value, .. } => match value {
                None => buf.write_varint(0),
                Some(value) => {
                    let wire = value
                        .checked_add(1)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or(CodecError::NotSupported(
                            "optional unsigned integer exceeds VarInt range",
                        ))?;
                    buf.write_varint(wire);
                }
            },
            Self::Pose { pose, .. } => buf.write_varint(*pose as i32),
            Self::HumanoidArm { value, .. } => buf.write_varint(match value {
                MainHand::Left => 0,
                MainHand::Right => 1,
            }),
        }
        Ok(())
    }

    fn decode<B: Buf>(index: u8, serializer_id: i32, buf: &mut B) -> Result<Self, CodecError> {
        Ok(match serializer_id {
            ENTITY_DATA_BYTE_SERIALIZER_ID => Self::Byte {
                index,
                value: buf.read_i8()?,
            },
            ENTITY_DATA_INT_SERIALIZER_ID => Self::Int {
                index,
                value: buf.read_varint()?,
            },
            ENTITY_DATA_LONG_SERIALIZER_ID => Self::Long {
                index,
                value: buf.read_varlong()?,
            },
            ENTITY_DATA_FLOAT_SERIALIZER_ID => Self::Float {
                index,
                value: buf.read_f32()?,
            },
            ENTITY_DATA_STRING_SERIALIZER_ID => Self::String {
                index,
                value: buf.read_string(DEFAULT_MAX_STRING_LEN)?,
            },
            ENTITY_DATA_ITEM_STACK_SERIALIZER_ID => Self::ItemStack {
                index,
                stack: ItemStack::decode(buf)?,
            },
            ENTITY_DATA_BOOLEAN_SERIALIZER_ID => Self::Boolean {
                index,
                value: buf.read_bool()?,
            },
            ENTITY_DATA_ROTATIONS_SERIALIZER_ID => Self::Rotations {
                index,
                value: EntityRotations {
                    x: buf.read_f32()?,
                    y: buf.read_f32()?,
                    z: buf.read_f32()?,
                },
            },
            ENTITY_DATA_BLOCK_POSITION_SERIALIZER_ID => Self::BlockPosition {
                index,
                value: buf.read_i64()?,
            },
            ENTITY_DATA_OPTIONAL_BLOCK_POSITION_SERIALIZER_ID => Self::OptionalBlockPosition {
                index,
                value: if buf.read_bool()? {
                    Some(buf.read_i64()?)
                } else {
                    None
                },
            },
            ENTITY_DATA_DIRECTION_SERIALIZER_ID => Self::Direction {
                index,
                value: EntityDirection::from_wire(buf.read_varint()?)?,
            },
            ENTITY_DATA_OPTIONAL_LIVING_ENTITY_REFERENCE_SERIALIZER_ID => {
                Self::OptionalLivingEntityReference {
                    index,
                    value: if buf.read_bool()? {
                        Some(buf.read_uuid()?)
                    } else {
                        None
                    },
                }
            }
            ENTITY_DATA_BLOCK_STATE_SERIALIZER_ID => Self::BlockState {
                index,
                value: BlockStateId::decode(buf)?,
            },
            ENTITY_DATA_OPTIONAL_BLOCK_STATE_SERIALIZER_ID => {
                let raw = buf.read_varint()?;
                if raw < 0 {
                    return Err(CodecError::NotSupported("negative block-state registry id"));
                }
                Self::OptionalBlockState {
                    index,
                    value: (raw != 0).then_some(BlockStateId(raw)),
                }
            }
            ENTITY_DATA_VILLAGER_DATA_SERIALIZER_ID => Self::VillagerData {
                index,
                villager_type: buf.read_varint()?,
                profession: buf.read_varint()?,
                level: buf.read_varint()?,
            },
            ENTITY_DATA_OPTIONAL_UNSIGNED_INT_SERIALIZER_ID => {
                let raw = buf.read_varint()?;
                if raw < 0 {
                    return Err(CodecError::NotSupported(
                        "negative optional unsigned integer",
                    ));
                }
                Self::OptionalUnsignedInt {
                    index,
                    value: (raw != 0).then_some((raw - 1) as u32),
                }
            }
            ENTITY_DATA_POSE_SERIALIZER_ID => Self::Pose {
                index,
                pose: EntityPose::from_wire(buf.read_varint()?)?,
            },
            ENTITY_DATA_HUMANOID_ARM_SERIALIZER_ID => Self::HumanoidArm {
                index,
                value: match buf.read_varint()? {
                    0 => MainHand::Left,
                    1 => MainHand::Right,
                    _ => return Err(CodecError::NotSupported("unknown humanoid arm")),
                },
            },
            _ => {
                return Err(CodecError::NotSupported(
                    "entity data serializer is not implemented",
                ));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityPose {
    Standing = 0,
    FallFlying = 1,
    Sleeping = 2,
    Swimming = 3,
    SpinAttack = 4,
    Crouching = 5,
    LongJumping = 6,
    Dying = 7,
    Croaking = 8,
    UsingTongue = 9,
    Sitting = 10,
    Roaring = 11,
    Sniffing = 12,
    Emerging = 13,
    Digging = 14,
    Sliding = 15,
    Shooting = 16,
    Inhaling = 17,
}

impl EntityPose {
    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Standing,
            1 => Self::FallFlying,
            2 => Self::Sleeping,
            3 => Self::Swimming,
            4 => Self::SpinAttack,
            5 => Self::Crouching,
            6 => Self::LongJumping,
            7 => Self::Dying,
            8 => Self::Croaking,
            9 => Self::UsingTongue,
            10 => Self::Sitting,
            11 => Self::Roaring,
            12 => Self::Sniffing,
            13 => Self::Emerging,
            14 => Self::Digging,
            15 => Self::Sliding,
            16 => Self::Shooting,
            17 => Self::Inhaling,
            _ => return Err(CodecError::NotSupported("unknown entity pose")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetEntityData {
    pub entity_id: i32,
    pub values: Vec<EntityDataValue>,
}

impl Packet for ClientboundSetEntityData {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_ENTITY_DATA is
    // game-CB index 99 = wire id 0x63. `javap -p -c` shows VarInt entity id,
    // repeated DataValue entries, then EOF marker byte 255. DataValue writes one
    // unsigned-byte metadata index, VarInt serializer id, then serializer payload.
    const ID: i32 = 0x63;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.values.len() > MAX_ENTITY_DATA_VALUES {
            return Err(CodecError::StringTooLong {
                len: self.values.len(),
                max: MAX_ENTITY_DATA_VALUES,
            });
        }
        let mut seen_indices = [false; 0xFF];
        for value in &self.values {
            let index = value.index();
            if index == 0xFF {
                return Err(CodecError::NotSupported(
                    "entity data index 255 is reserved",
                ));
            }
            if seen_indices[index as usize] {
                return Err(CodecError::NotSupported("duplicate entity data index"));
            }
            seen_indices[index as usize] = true;
        }

        buf.write_varint(self.entity_id);
        for value in &self.values {
            value.encode(buf)?;
        }
        buf.write_u8(0xFF);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_varint()?;
        let mut values = Vec::new();
        let mut seen_indices = [false; 0xFF];
        loop {
            let index = buf.read_u8()?;
            if index == 0xFF {
                break;
            }
            if values.len() >= MAX_ENTITY_DATA_VALUES {
                return Err(CodecError::StringTooLong {
                    len: values.len() + 1,
                    max: MAX_ENTITY_DATA_VALUES,
                });
            }
            if seen_indices[index as usize] {
                return Err(CodecError::NotSupported("duplicate entity data index"));
            }
            seen_indices[index as usize] = true;
            let serializer_id = buf.read_varint()?;
            values.push(EntityDataValue::decode(index, serializer_id, buf)?);
        }
        Ok(Self { entity_id, values })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundTakeItemEntity {
    pub item_entity_id: i32,
    pub player_entity_id: i32,
    pub amount: i32,
}

impl Packet for ClientboundTakeItemEntity {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_TAKE_ITEM_ENTITY is
    // game-CB index 124 = wire id 0x7C. `javap -p -c` shows three VarInts:
    // item entity id, player entity id, and picked-up amount.
    const ID: i32 = 0x7C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.item_entity_id);
        buf.write_varint(self.player_entity_id);
        buf.write_varint(self.amount);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            item_entity_id: buf.read_varint()?,
            player_entity_id: buf.read_varint()?,
            amount: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveEntities {
    pub entity_ids: Vec<i32>,
}

impl Packet for RemoveEntities {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_REMOVE_ENTITIES
    // is game-CB index 77 = wire id 0x4D. Body is FriendlyByteBuf.writeIntIdList.
    const ID: i32 = 0x4D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.entity_ids.len())?;
        for id in &self.entity_ids {
            buf.write_varint(*id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = read_count(buf, MAX_ENTITY_ID_LIST_LEN)?;
        let mut entity_ids = Vec::with_capacity(count);
        for _ in 0..count {
            entity_ids.push(buf.read_varint()?);
        }
        Ok(Self { entity_ids })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetPassengers {
    pub vehicle_id: i32,
    pub passenger_ids: Vec<i32>,
}

impl Packet for ClientboundSetPassengers {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_PASSENGERS
    // is game-CB index 107 = wire id 0x6B. Body is a vehicle VarInt followed
    // by a VarInt-length-prefixed array of passenger entity VarInts.
    const ID: i32 = 0x6B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.vehicle_id);
        write_bounded_count(buf, self.passenger_ids.len(), MAX_ENTITY_ID_LIST_LEN)?;
        for passenger_id in &self.passenger_ids {
            buf.write_varint(*passenger_id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let vehicle_id = buf.read_varint()?;
        let count = read_count(buf, MAX_ENTITY_ID_LIST_LEN)?;
        let mut passenger_ids = Vec::with_capacity(count);
        for _ in 0..count {
            passenger_ids.push(buf.read_varint()?);
        }
        Ok(Self {
            vehicle_id,
            passenger_ids,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotateHead {
    pub entity_id: i32,
    pub head_yaw: f32,
}

impl Packet for RotateHead {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ROTATE_HEAD is
    // game-CB index 83 = wire id 0x53. Body is VarInt entity id and packed yaw.
    const ID: i32 = 0x53;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_u8(pack_degrees(self.head_yaw));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            head_yaw: unpack_degrees(buf.read_u8()?),
        })
    }
}

/// A single heightmap entry inside [`LevelChunkWithLight`].
///
/// In 26.1 the heightmaps field on the wire is *no longer* an NBT
/// compound (it was, pre-1.20). It is a `Map<Heightmap$Types, long[]>`
/// encoded via `ByteBufCodecs.map(EnumMap, Heightmap$Types.STREAM_CODEC,
/// LONG_ARRAY)` — count, then `VarInt typeId` + `long[]` per entry.
///
/// `type_id` is the ordinal of the entry in `Heightmap$Types`
/// (verified by `javap -p -c` of the unobfuscated jar, per ADR 0002).
/// Only the three entries with `Usage.CLIENT` are sent:
/// `WORLD_SURFACE = 1`, `MOTION_BLOCKING = 4`,
/// `MOTION_BLOCKING_NO_LEAVES = 5`. The packed `long[]` is the same
/// 9-bits-per-entry, non-crossing-i64 packing the on-disk heightmap
/// uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHeightmap {
    pub type_id: i32,
    pub data: Vec<i64>,
}

impl ChunkHeightmap {
    pub const WORLD_SURFACE: i32 = 1;
    pub const MOTION_BLOCKING: i32 = 4;
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// One block-entity sidecar entry carried inside [`LevelChunkWithLight`].
///
/// Verified via `javap -p -c` of
/// `ClientboundLevelChunkPacketData$BlockEntityInfo.write` (ADR 0002):
/// `u8 packedXZ`, `i16 y`, `VarInt typeId` (id into
/// `minecraft:block_entity_type` registry), then network-NBT
/// (unnamed-root) compound tag.
///
/// `packed_xz` is `(x << 4) | z` with `x`, `z` both 4-bit
/// section-relative; vanilla never emits chunk-relative coords here.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityInfo {
    pub packed_xz: u8,
    pub y: i16,
    pub type_id: i32,
    pub nbt: Tag,
}

impl BlockEntityInfo {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.packed_xz);
        buf.write_i16(self.y);
        buf.write_varint(self.type_id);
        mc_nbt::write_network(buf, &self.nbt)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packed_xz: buf.read_u8()?,
            y: buf.read_i16()?,
            type_id: buf.read_varint()?,
            nbt: mc_nbt::read_network(buf)?,
        })
    }
}

/// Sky and block light payload inlined into [`LevelChunkWithLight`].
///
/// Verified via `javap -p -c` of
/// `ClientboundLightUpdatePacketData.write` (ADR 0002): four BitSets
/// (`writeBitSet` = `writeLongArray(bs.toLongArray())`) followed by
/// two `Collection<byte[]>` lists, each entry a 2048-byte light layer
/// (4 bits per block × 16³ blocks = 2048 bytes).
///
/// Bit `i` set in `sky_y_mask` (resp. `block_y_mask`) means section
/// `i` has explicit sky-light (resp. block-light) data in
/// `sky_updates` (resp. `block_updates`). Bit `i` set in
/// `empty_sky_y_mask` (resp. `empty_block_y_mask`) means section `i`
/// has no light data — the client uses its own default. Section
/// indexing extends by one entry on each side of the world Y range,
/// so for a 24-section column there are 26 indexable sections
/// (`-1..=24`); vanilla packs this in a single i64 per bitset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightData {
    pub sky_y_mask: Vec<i64>,
    pub block_y_mask: Vec<i64>,
    pub empty_sky_y_mask: Vec<i64>,
    pub empty_block_y_mask: Vec<i64>,
    pub sky_updates: Vec<Vec<u8>>,
    pub block_updates: Vec<Vec<u8>>,
}

impl LightData {
    /// Length of one light-data layer in bytes (`16³ / 2`, since vanilla
    /// packs 4 bits per block).
    pub const LIGHT_LAYER_BYTES: usize = 2048;

    /// "No lighting data" payload: all masks empty, both update lists
    /// empty. Legal on the wire; the client uses its own defaults.
    /// Six VarInt(0) bytes total.
    pub fn empty() -> Self {
        Self::default()
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_long_array(buf, &self.sky_y_mask)?;
        write_long_array(buf, &self.block_y_mask)?;
        write_long_array(buf, &self.empty_sky_y_mask)?;
        write_long_array(buf, &self.empty_block_y_mask)?;
        Self::encode_light_list(buf, &self.sky_updates)?;
        Self::encode_light_list(buf, &self.block_updates)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let sky_y_mask = read_long_array(buf)?;
        let block_y_mask = read_long_array(buf)?;
        let empty_sky_y_mask = read_long_array(buf)?;
        let empty_block_y_mask = read_long_array(buf)?;
        let sky_updates = Self::decode_light_list(buf)?;
        let block_updates = Self::decode_light_list(buf)?;
        Ok(Self {
            sky_y_mask,
            block_y_mask,
            empty_sky_y_mask,
            empty_block_y_mask,
            sky_updates,
            block_updates,
        })
    }

    fn encode_light_list<B: BufMut>(buf: &mut B, layers: &[Vec<u8>]) -> Result<(), CodecError> {
        let len = i32::try_from(layers.len()).map_err(|_| CodecError::StringTooLong {
            len: layers.len(),
            max: i32::MAX as usize,
        })?;
        buf.write_varint(len);
        for layer in layers {
            buf.write_byte_array(layer);
        }
        Ok(())
    }

    fn decode_light_list<B: Buf>(buf: &mut B) -> Result<Vec<Vec<u8>>, CodecError> {
        let len_signed = buf.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        let len = len_signed as usize;
        // Each layer is at most LIGHT_LAYER_BYTES; cap collection length
        // at one entry per Y section in a 1024-block-tall column (huge
        // overshoot for vanilla's 26).
        if len > 1024 {
            return Err(CodecError::StringTooLong { len, max: 1024 });
        }
        let mut layers = Vec::with_capacity(len);
        for _ in 0..len {
            layers.push(buf.read_byte_array(Self::LIGHT_LAYER_BYTES)?);
        }
        Ok(layers)
    }
}

/// `Level Chunk With Light` (CB). The packet that finally puts blocks
/// in front of the client. Wraps the bodies of vanilla's
/// `ClientboundLevelChunkPacketData` and `ClientboundLightUpdatePacketData`
/// behind the chunk coordinates.
///
/// `data` is the paletted-container chunk body: section-by-section
/// (`i16 block_count`, paletted block-state container, paletted biome
/// container) concatenated. M3.a defines the outer packet only —
/// constructing `data` from a `mc_world::Chunk` is M3.b's job, so the
/// field is exposed as raw bytes here and a `Vec<u8>` round-trips
/// through `encode` / `decode` unchanged.
///
/// The leading `i32 chunk_x` / `i32 chunk_z` are raw `int`s on the
/// wire (not VarInts), per
/// `ClientboundLevelChunkWithLightPacket.STREAM_CODEC` in the
/// unobfuscated jar.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub heightmaps: Vec<ChunkHeightmap>,
    pub data: Vec<u8>,
    pub block_entities: Vec<BlockEntityInfo>,
    pub light: LightData,
}

impl Packet for LevelChunkWithLight {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT at game-CB index 45 = 0x2D.
    const ID: i32 = 0x2D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.chunk_x);
        buf.write_i32(self.chunk_z);

        let hm_count =
            i32::try_from(self.heightmaps.len()).map_err(|_| CodecError::StringTooLong {
                len: self.heightmaps.len(),
                max: i32::MAX as usize,
            })?;
        buf.write_varint(hm_count);
        for hm in &self.heightmaps {
            buf.write_varint(hm.type_id);
            write_long_array(buf, &hm.data)?;
        }

        if self.data.len() > MAX_CHUNK_DATA_LEN {
            return Err(CodecError::StringTooLong {
                len: self.data.len(),
                max: MAX_CHUNK_DATA_LEN,
            });
        }
        buf.write_byte_array(&self.data);

        let be_count =
            i32::try_from(self.block_entities.len()).map_err(|_| CodecError::StringTooLong {
                len: self.block_entities.len(),
                max: i32::MAX as usize,
            })?;
        buf.write_varint(be_count);
        for be in &self.block_entities {
            be.encode(buf)?;
        }

        self.light.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let chunk_x = buf.read_i32()?;
        let chunk_z = buf.read_i32()?;

        let hm_count_signed = buf.read_varint()?;
        if hm_count_signed < 0 {
            return Err(CodecError::NegativeLength(hm_count_signed));
        }
        let hm_count = hm_count_signed as usize;
        // Six entries in Heightmap$Types; cap at 16 to leave room.
        if hm_count > 16 {
            return Err(CodecError::StringTooLong {
                len: hm_count,
                max: 16,
            });
        }
        let mut heightmaps = Vec::with_capacity(hm_count);
        for _ in 0..hm_count {
            heightmaps.push(ChunkHeightmap {
                type_id: buf.read_varint()?,
                data: read_long_array(buf)?,
            });
        }

        let data = buf.read_byte_array(MAX_CHUNK_DATA_LEN)?;

        let be_count_signed = buf.read_varint()?;
        if be_count_signed < 0 {
            return Err(CodecError::NegativeLength(be_count_signed));
        }
        let be_count = be_count_signed as usize;
        // One block entity per block in a 16³ section × 24 sections =
        // 98 304; cap one order of magnitude higher than that.
        if be_count > 1_000_000 {
            return Err(CodecError::StringTooLong {
                len: be_count,
                max: 1_000_000,
            });
        }
        let mut block_entities = Vec::with_capacity(be_count);
        for _ in 0..be_count {
            block_entities.push(BlockEntityInfo::decode(buf)?);
        }

        let light = LightData::decode(buf)?;

        Ok(Self {
            chunk_x,
            chunk_z,
            heightmaps,
            data,
            block_entities,
            light,
        })
    }
}

/// `ClientboundLevelEventPacket` (CB). Vanilla uses this compact event
/// channel for client-rendered world effects such as block destroy particles
/// and their matching sound.
///
/// Verified against the local 26.1.2 source: raw `i32` event id, packed
/// `BlockPos`, raw `i32` event data, then one global-event boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelEvent {
    pub event_id: i32,
    pub position: i64,
    pub data: i32,
    pub global: bool,
}

impl Packet for LevelEvent {
    // CLIENTBOUND_LEVEL_EVENT at game-CB index 46 = wire id 0x2E.
    const ID: i32 = 0x2E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.event_id);
        buf.write_i64(self.position);
        buf.write_i32(self.data);
        buf.write_bool(self.global);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            event_id: buf.read_i32()?,
            position: buf.read_i64()?,
            data: buf.read_i32()?,
            global: buf.read_bool()?,
        })
    }
}

/// `Set Center Chunk` (CB). Tells the client which chunk is the center
/// of its view-distance window — the chunk it is "looking from". Every
/// `LevelChunkWithLight` packet that follows is rendered relative to
/// this anchor; if the center is wrong the client will silently
/// discard chunks that fall outside its (anchored, but still
/// vd-bounded) loaded ring.
///
/// Wire layout per `javap -p -c` of
/// `ClientboundSetChunkCacheCenterPacket.write` (ADR 0002): two
/// VarInts, `x` then `z`, in chunk coordinates (= `floor(world / 16)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCenterChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for SetCenterChunk {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_CHUNK_CACHE_CENTER at game-CB index 94 = 0x5E.
    const ID: i32 = 0x5E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.chunk_x);
        buf.write_varint(self.chunk_z);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            chunk_x: buf.read_varint()?,
            chunk_z: buf.read_varint()?,
        })
    }
}

// -----------------------------------------------------------------------
// Serverbound
// -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }

    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => Self::Survival,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundChangeGameMode {
    pub mode: GameMode,
}

impl Packet for ServerboundChangeGameMode {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHANGE_GAME_MODE
    // is game-SB index 5 = wire id 0x05. `javap -p -c
    // ServerboundChangeGameModePacket` shows the body is GameType.STREAM_CODEC;
    // `javap GameType` shows ids survival=0, creative=1, adventure=2,
    // spectator=3 via ByteBufCodecs.idMapper.
    const ID: i32 = 0x05;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.mode.id());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            mode: GameMode::from_id(buf.read_varint()?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChatCommand {
    pub command: String,
}

impl Packet for ServerboundChatCommand {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHAT_COMMAND is
    // game-SB index 7 = wire id 0x07. `javap -p -c
    // ServerboundChatCommandPacket` shows a single FriendlyByteBuf UTF string.
    const ID: i32 = 0x07;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.command, MAX_COMMAND_LEN)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            command: buf.read_string(MAX_COMMAND_LEN)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChat {
    pub message: String,
    pub timestamp_millis: i64,
    pub salt: i64,
    pub signature: Option<[u8; 256]>,
    pub last_seen_offset: i32,
    pub last_seen_acknowledged: [u8; LAST_SEEN_FIXED_BITSET_BYTES],
    pub last_seen_checksum: i8,
}

impl Packet for ServerboundChat {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHAT is
    // game-SB index 9 = wire id 0x09. Local decompiled
    // `ServerboundChatPacket` reads UTF(256), Instant as epoch millis, salt,
    // nullable MessageSignature (256 bytes), then LastSeenMessages.Update:
    // VarInt offset, fixed 20-bit BitSet, and checksum byte.
    const ID: i32 = 0x09;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.message, MAX_CHAT_MESSAGE_LEN)?;
        buf.write_i64(self.timestamp_millis);
        buf.write_i64(self.salt);
        match self.signature {
            Some(signature) => {
                buf.write_bool(true);
                buf.put_slice(&signature);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.last_seen_offset);
        buf.put_slice(&self.last_seen_acknowledged);
        buf.write_i8(self.last_seen_checksum);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let message = buf.read_string(MAX_CHAT_MESSAGE_LEN)?;
        let timestamp_millis = buf.read_i64()?;
        let salt = buf.read_i64()?;
        let signature = if buf.read_bool()? {
            let mut signature = [0_u8; 256];
            if buf.remaining() < signature.len() {
                return Err(CodecError::Underflow {
                    needed: signature.len() - buf.remaining(),
                    available: buf.remaining(),
                });
            }
            buf.copy_to_slice(&mut signature);
            Some(signature)
        } else {
            None
        };
        let last_seen_offset = buf.read_varint()?;
        let mut last_seen_acknowledged = [0_u8; LAST_SEEN_FIXED_BITSET_BYTES];
        if buf.remaining() < last_seen_acknowledged.len() {
            return Err(CodecError::Underflow {
                needed: last_seen_acknowledged.len() - buf.remaining(),
                available: buf.remaining(),
            });
        }
        buf.copy_to_slice(&mut last_seen_acknowledged);
        Ok(Self {
            message,
            timestamp_millis,
            salt,
            signature,
            last_seen_offset,
            last_seen_acknowledged,
            last_seen_checksum: buf.read_i8()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundCommandSuggestion {
    pub id: i32,
    pub command: String,
}

impl Packet for ServerboundCommandSuggestion {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_COMMAND_SUGGESTION is
    // serverbound registration index 15, wire id 0x0F. Local decompiled source
    // reads VarInt id then `readUtf(32500)`.
    const ID: i32 = 0x0F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let command_len = self.command.encode_utf16().count();
        if command_len > MAX_COMMAND_SUGGESTION_LEN {
            return Err(CodecError::StringTooLong {
                len: command_len,
                max: MAX_COMMAND_SUGGESTION_LEN,
            });
        }
        buf.write_varint(self.id);
        buf.write_string(&self.command, MAX_COMMAND_SUGGESTION_LEN)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_varint()?,
            command: buf.read_string(MAX_COMMAND_SUGGESTION_LEN)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundSignUpdate {
    pub position: i64,
    pub lines: Vec<String>,
    pub is_front_text: bool,
}

impl Packet for ServerboundSignUpdate {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_SIGN_UPDATE is
    // game-SB index 61 = wire id 0x3D. The vanilla 26.1.2 stream codec in
    // `.analysis/decompiled/.../ServerboundSignUpdatePacket.java` writes
    // BlockPos, front/back text boolean, then four UTF(384) lines.
    const ID: i32 = 0x3D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.lines.len() != SIGN_LINE_COUNT {
            return Err(CodecError::NotSupported(
                "sign update must contain four lines",
            ));
        }
        for line in &self.lines {
            let line_len = line.encode_utf16().count();
            if line_len > MAX_SIGN_LINE_LEN {
                return Err(CodecError::StringTooLong {
                    len: line_len,
                    max: MAX_SIGN_LINE_LEN,
                });
            }
        }
        buf.write_i64(self.position);
        buf.write_bool(self.is_front_text);
        for line in &self.lines {
            buf.write_string(line, MAX_SIGN_LINE_LEN)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let position = buf.read_i64()?;
        let is_front_text = buf.read_bool()?;
        let mut lines = Vec::with_capacity(SIGN_LINE_COUNT);
        for _ in 0..SIGN_LINE_COUNT {
            lines.push(buf.read_string(MAX_SIGN_LINE_LEN)?);
        }
        Ok(Self {
            position,
            lines,
            is_front_text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCommandSuggestions {
    pub id: i32,
    pub start: i32,
    pub length: i32,
    pub suggestions: Vec<CommandSuggestionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCommands {
    pub nodes: Vec<CommandNode>,
    pub root_index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNode {
    pub kind: CommandNodeKind,
    pub children: Vec<i32>,
    pub executable: bool,
    pub restricted: bool,
    pub redirect: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandNodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        parser: CommandArgumentParser,
        suggestions: Option<Identifier>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandArgumentParser {
    String(CommandStringKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStringKind {
    SingleWord,
    QuotablePhrase,
    GreedyPhrase,
}

impl CommandNode {
    pub fn root(children: Vec<i32>) -> Self {
        Self {
            kind: CommandNodeKind::Root,
            children,
            executable: false,
            restricted: false,
            redirect: None,
        }
    }

    pub fn literal(name: impl Into<String>, children: Vec<i32>, executable: bool) -> Self {
        Self {
            kind: CommandNodeKind::Literal(name.into()),
            children,
            executable,
            restricted: false,
            redirect: None,
        }
    }

    pub fn argument(
        name: impl Into<String>,
        parser: CommandArgumentParser,
        children: Vec<i32>,
        executable: bool,
    ) -> Self {
        Self {
            kind: CommandNodeKind::Argument {
                name: name.into(),
                parser,
                suggestions: None,
            },
            children,
            executable,
            restricted: false,
            redirect: None,
        }
    }

    pub fn restricted(mut self, restricted: bool) -> Self {
        self.restricted = restricted;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSuggestionEntry {
    pub text: String,
    pub tooltip_nbt: Option<Vec<u8>>,
}

impl Packet for ClientboundCommandSuggestions {
    // `.analysis/protocol-dump.txt`: game CLIENTBOUND_COMMAND_SUGGESTIONS is
    // clientbound registration index 15, wire id 0x0F. Local decompiled source
    // writes VarInt id/start/length and a list of entries. Solaris currently
    // emits empty lists, so optional component tooltips are not produced.
    const ID: i32 = 0x0F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.id);
        buf.write_varint(self.start);
        buf.write_varint(self.length);
        write_count(buf, self.suggestions.len())?;
        for entry in &self.suggestions {
            buf.write_string(&entry.text, MAX_COMMAND_LEN)?;
            match &entry.tooltip_nbt {
                Some(tooltip) => {
                    buf.write_bool(true);
                    buf.put_slice(tooltip);
                }
                None => buf.write_bool(false),
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let id = buf.read_varint()?;
        let start = buf.read_varint()?;
        let length = buf.read_varint()?;
        let count = read_count(buf, 256)?;
        let mut suggestions = Vec::with_capacity(count);
        for _ in 0..count {
            let text = buf.read_string(MAX_COMMAND_LEN)?;
            let tooltip_nbt = if buf.read_bool()? {
                let mut tooltip = vec![0; buf.remaining()];
                buf.copy_to_slice(&mut tooltip);
                Some(tooltip)
            } else {
                None
            };
            suggestions.push(CommandSuggestionEntry { text, tooltip_nbt });
        }
        Ok(Self {
            id,
            start,
            length,
            suggestions,
        })
    }
}

impl Packet for ClientboundCommands {
    // `.analysis/protocol-dump.txt` / `GameProtocols`: game
    // CLIENTBOUND_COMMANDS is clientbound registration index 16, wire id 0x10.
    // `ClientboundCommandsPacket` writes a list of nodes followed by rootIndex;
    // each node is flags, VarInt child array, optional redirect, then the
    // literal or argument payload selected by flags & 3.
    const ID: i32 = 0x10;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.nodes.len())?;
        for node in &self.nodes {
            node.encode(buf)?;
        }
        buf.write_varint(self.root_index);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = read_count(buf, MAX_COMMAND_NODE_COUNT)?;
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            nodes.push(CommandNode::decode(buf)?);
        }
        Ok(Self {
            nodes,
            root_index: buf.read_varint()?,
        })
    }
}

impl CommandNode {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let mut flags = match self.kind {
            CommandNodeKind::Root => 0,
            CommandNodeKind::Literal(_) => 1,
            CommandNodeKind::Argument { .. } => 2,
        };
        if self.executable {
            flags |= 4;
        }
        if self.redirect.is_some() {
            flags |= 8;
        }
        if matches!(
            self.kind,
            CommandNodeKind::Argument {
                suggestions: Some(_),
                ..
            }
        ) {
            flags |= 16;
        }
        if self.restricted {
            flags |= 32;
        }

        buf.put_u8(flags);
        write_count(buf, self.children.len())?;
        for &child in &self.children {
            buf.write_varint(child);
        }
        if let Some(redirect) = self.redirect {
            buf.write_varint(redirect);
        }
        match &self.kind {
            CommandNodeKind::Root => {}
            CommandNodeKind::Literal(name) => buf.write_string(name, MAX_COMMAND_LEN)?,
            CommandNodeKind::Argument {
                name,
                parser,
                suggestions,
            } => {
                buf.write_string(name, MAX_COMMAND_LEN)?;
                parser.encode(buf);
                if let Some(suggestions) = suggestions {
                    buf.write_identifier(suggestions)?;
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let flags = buf.read_u8()?;
        let child_count = read_count(buf, MAX_COMMAND_CHILD_COUNT)?;
        let mut children = Vec::with_capacity(child_count);
        for _ in 0..child_count {
            children.push(buf.read_varint()?);
        }
        let redirect = if flags & 8 != 0 {
            Some(buf.read_varint()?)
        } else {
            None
        };
        let suggestions = flags & 16 != 0;
        let kind = match flags & 3 {
            0 => CommandNodeKind::Root,
            1 => CommandNodeKind::Literal(buf.read_string(MAX_COMMAND_LEN)?),
            2 => {
                let name = buf.read_string(MAX_COMMAND_LEN)?;
                let parser = CommandArgumentParser::decode(buf)?;
                let suggestions = suggestions.then(|| buf.read_identifier()).transpose()?;
                CommandNodeKind::Argument {
                    name,
                    parser,
                    suggestions,
                }
            }
            _ => return Err(CodecError::NotSupported("unknown command node type")),
        };
        Ok(Self {
            kind,
            children,
            executable: flags & 4 != 0,
            restricted: flags & 32 != 0,
            redirect,
        })
    }
}

impl CommandArgumentParser {
    const BRIGADIER_STRING_ID: i32 = 5;

    fn encode<B: BufMut>(self, buf: &mut B) {
        match self {
            Self::String(kind) => {
                buf.write_varint(Self::BRIGADIER_STRING_ID);
                buf.write_varint(kind as i32);
            }
        }
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            Self::BRIGADIER_STRING_ID => Ok(Self::String(CommandStringKind::decode(buf)?)),
            _ => Err(CodecError::NotSupported(
                "unsupported command argument parser",
            )),
        }
    }
}

impl CommandStringKind {
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => Ok(Self::SingleWord),
            1 => Ok(Self::QuotablePhrase),
            2 => Ok(Self::GreedyPhrase),
            _ => Err(CodecError::NotSupported(
                "unsupported brigadier string kind",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSystemChat {
    pub content_nbt: Vec<u8>,
    pub overlay: bool,
}

impl Packet for ClientboundSystemChat {
    // `.analysis/protocol-dump.txt` / `GameProtocols`: game
    // CLIENTBOUND_SYSTEM_CHAT is clientbound registration index 121, wire id
    // 0x79. `ClientboundSystemChatPacket` writes trusted Component NBT then a
    // bool overlay flag.
    const ID: i32 = 0x79;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.put_slice(&self.content_nbt);
        buf.write_bool(self.overlay);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        if buf.remaining() < 2 {
            return Err(CodecError::Underflow {
                needed: 2 - buf.remaining(),
                available: buf.remaining(),
            });
        }
        let content_len = buf.remaining() - 1;
        let mut content_nbt = vec![0; content_len];
        buf.copy_to_slice(&mut content_nbt);
        Ok(Self {
            content_nbt,
            overlay: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundChatAck {
    pub offset: i32,
}

impl Packet for ServerboundChatAck {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CHAT_ACK is serverbound
    // registration index 6, wire id 0x06. Local decompiled source reads one
    // VarInt offset.
    const ID: i32 = 0x06;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.offset);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            offset: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundChunkBatchReceived {
    pub desired_chunks_per_tick: f32,
}

impl Packet for ServerboundChunkBatchReceived {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CHUNK_BATCH_RECEIVED is
    // serverbound registration index 11, wire id 0x0B. Local decompiled source
    // reads one f32 desiredChunksPerTick.
    const ID: i32 = 0x0B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.desired_chunks_per_tick);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            desired_chunks_per_tick: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundClientTickEnd;

impl Packet for ServerboundClientTickEnd {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CLIENT_TICK_END is
    // serverbound registration index 13, wire id 0x0D. Local decompiled source
    // uses `StreamCodec.unit`, so the body is empty.
    const ID: i32 = 0x0D;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundPlayerLoaded;

impl Packet for ServerboundPlayerLoaded {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_PLAYER_LOADED is
    // serverbound registration index 44, wire id 0x2C. Local decompiled source
    // uses `StreamCodec.unit`, so the body is empty.
    const ID: i32 = 0x2C;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundResourcePack {
    pub status: ResourcePackStatus,
}

impl Packet for ServerboundResourcePack {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_RESOURCE_PACK is
    // serverbound registration index 49, wire id 0x31. Body is local decompiled
    // `ServerboundResourcePackPacket(UUID id, Action action)`.
    const ID: i32 = 0x31;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.status.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            status: ResourcePackStatus::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundClientInformation {
    pub information: ClientInformation,
}

impl Packet for ServerboundClientInformation {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CLIENT_INFORMATION is
    // serverbound registration index 14, wire id 0x0E. Body delegates to local
    // decompiled `ServerboundClientInformationPacket(ClientInformation)`.
    const ID: i32 = 0x0E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.information.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            information: ClientInformation::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ServerboundCustomPayload {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CUSTOM_PAYLOAD is
    // serverbound registration index 22, wire id 0x16. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x16;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode_serverbound(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode_serverbound(buf)?,
        })
    }
}

/// `Confirm Teleportation` (SB). Client echoes our
/// `SynchronizePlayerPosition.teleport_id` back to confirm it accepted
/// the snap. If we don't see this we may need to resend the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmTeleportation {
    pub teleport_id: i32,
}

impl Packet for ConfirmTeleportation {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_ACCEPT_TELEPORTATION at game-SB index 0 = wire id 0x00.
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.teleport_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            teleport_id: buf.read_varint()?,
        })
    }
}

/// Serverbound `KeepAlive (Play)`. Echo of the value we sent in
/// [`ClientboundKeepAlive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundKeepAlive {
    pub id: i64,
}

impl Packet for ServerboundKeepAlive {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_KEEP_ALIVE at game-SB index 28 = wire id 0x1C.
    const ID: i32 = 0x1C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_i64()?,
        })
    }
}

/// `Serverbound Attack` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundAttackPacket(int entityId)` uses `ByteBufCodecs.VAR_INT` and is
/// registered immediately after `SERVERBOUND_ACCEPT_TELEPORTATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundAttack {
    pub entity_id: i32,
}

impl Packet for ServerboundAttack {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
        })
    }
}

/// `Serverbound Interact` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundInteractPacket(int entityId, InteractionHand hand, Vec3 location,
/// boolean usingSecondaryAction)` uses `Vec3.LP_STREAM_CODEC`. Entity attacks are
/// carried by `ServerboundAttackPacket`, not by this packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundInteract {
    pub entity_id: i32,
    pub hand: InteractionHand,
    pub location: EntityVec3,
    pub using_secondary_action: bool,
}

impl Packet for ServerboundInteract {
    const ID: i32 = 0x1A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.hand as i32);
        write_lp_vec3(buf, self.location);
        buf.write_bool(self.using_secondary_action);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            hand: InteractionHand::from_wire(buf.read_varint()?)?,
            location: read_lp_vec3(buf)?,
            using_secondary_action: buf.read_bool()?,
        })
    }
}

/// Flags shared by vanilla's four `ServerboundMovePlayerPacket` variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovePlayerFlags {
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerFlags {
    #[must_use]
    pub fn new(on_ground: bool, horizontal_collision: bool) -> Self {
        Self {
            on_ground,
            horizontal_collision,
        }
    }

    fn from_byte(byte: u8) -> Self {
        Self {
            on_ground: byte & 0x01 != 0,
            horizontal_collision: byte & 0x02 != 0,
        }
    }

    fn to_byte(self) -> u8 {
        u8::from(self.on_ground) | (u8::from(self.horizontal_collision) << 1)
    }
}

/// `ServerboundMovePlayerPacket$Pos` (SB). Carries position plus
/// movement flags. Per ADR 0002, verified against javap of vanilla
/// 26.1.2: `SERVERBOUND_MOVE_PLAYER_POS`, stream order
/// `double x, double y, double z, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerPos {
    // GameProtocols serverbound index 30 = wire id 0x1E.
    const ID: i32 = 0x1E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$PosRot` (SB). Carries position,
/// rotation, and movement flags. Per ADR 0002, verified against javap
/// of vanilla 26.1.2: `SERVERBOUND_MOVE_PLAYER_POS_ROT`, stream order
/// `double x, double y, double z, float yRot, float xRot, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerPosRot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerPosRot {
    // GameProtocols serverbound index 31 = wire id 0x1F.
    const ID: i32 = 0x1F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$Rot` (SB). Carries rotation plus
/// movement flags. Per ADR 0002, verified against javap of vanilla
/// 26.1.2: `SERVERBOUND_MOVE_PLAYER_ROT`, stream order
/// `float yRot, float xRot, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerRot {
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerRot {
    // GameProtocols serverbound index 32 = wire id 0x20.
    const ID: i32 = 0x20;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$StatusOnly` (SB). Carries movement
/// flags without position or rotation. Per ADR 0002, verified against
/// javap of vanilla 26.1.2: `SERVERBOUND_MOVE_PLAYER_STATUS_ONLY`,
/// stream order `unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundMovePlayerStatusOnly {
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerStatusOnly {
    // GameProtocols serverbound index 33 = wire id 0x21.
    const ID: i32 = 0x21;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

// ---------------------------------------------------------------------
// M5.b — clientbound edit / ack / relight packets
// ---------------------------------------------------------------------

/// `Clientbound Block Update` (CB). Single-block delta the server
/// sends in response to any state change at a `BlockPos`. Per
/// ADR 0002, verified against `javap -p` of the unobfuscated jar:
/// `ClientboundBlockUpdatePacket(BlockPos pos, BlockState state)`,
/// the `BlockState` wire form being its global state-id VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpdate {
    /// Block position packed into vanilla's standard `BlockPos`
    /// `i64`. Use [`pack_block_pos`] / [`unpack_block_pos`].
    pub position: i64,
    /// Global block-state id — the same id space `blocks.json` /
    /// `mc_world::BlockStateId` use.
    pub state_id: i32,
}

impl Packet for BlockUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_BLOCK_UPDATE at game-CB index 8 = wire id 0x08.
    const ID: i32 = 0x08;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_varint(self.state_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            state_id: buf.read_varint()?,
        })
    }
}

/// One entry inside [`SectionBlocksUpdate`]. `relative_pos` is the
/// section-local 12-bit coordinate value produced by
/// [`pack_section_relative_pos`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionBlockChange {
    pub relative_pos: u16,
    pub state_id: i32,
}

/// `Clientbound Section Blocks Update` (CB). Multi-block delta for one
/// chunk section. Per ADR 0002, verified against `javap -p`:
/// `ClientboundSectionBlocksUpdatePacket(SectionPos sectionPos,
/// short[] positions, BlockState[] states)`. Vanilla's stream codec writes
/// the raw `SectionPos.asLong`, then a VarInt count, then each entry as a
/// VarLong `(state_id << 12) | (section_relative_pos & 0xFFF)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionBlocksUpdate {
    /// Raw vanilla `SectionPos.asLong`. Use [`pack_section_pos`].
    pub section_pos: i64,
    pub changes: Vec<SectionBlockChange>,
}

impl Packet for SectionBlocksUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SECTION_BLOCKS_UPDATE at game-CB index 84 = wire id 0x54.
    const ID: i32 = 0x54;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.section_pos);
        if self.changes.len() > MAX_SECTION_BLOCK_UPDATE_ENTRIES {
            return Err(CodecError::StringTooLong {
                len: self.changes.len(),
                max: MAX_SECTION_BLOCK_UPDATE_ENTRIES,
            });
        }
        let count = i32::try_from(self.changes.len()).map_err(|_| CodecError::StringTooLong {
            len: self.changes.len(),
            max: i32::MAX as usize,
        })?;
        buf.write_varint(count);
        for change in &self.changes {
            buf.write_varlong(pack_section_block_update_value(
                change.state_id,
                change.relative_pos,
            ));
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let section_pos = buf.read_i64()?;
        let count_signed = buf.read_varint()?;
        if count_signed < 0 {
            return Err(CodecError::NegativeLength(count_signed));
        }
        let count = count_signed as usize;
        if count > MAX_SECTION_BLOCK_UPDATE_ENTRIES {
            return Err(CodecError::StringTooLong {
                len: count,
                max: MAX_SECTION_BLOCK_UPDATE_ENTRIES,
            });
        }
        let mut changes = Vec::with_capacity(count);
        for _ in 0..count {
            let value = buf.read_varlong()?;
            changes.push(SectionBlockChange {
                relative_pos: (value & 0xFFF) as u16,
                state_id: (value >> 12) as i32,
            });
        }
        Ok(Self {
            section_pos,
            changes,
        })
    }
}

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

/// `Clientbound Block Changed Ack` (CB). One-VarInt packet that
/// echoes the `sequence` from a `ServerboundPlayerAction` or
/// `ServerboundUseItemOn`; without it the vanilla client rolls
/// back its predicted edit. Per ADR 0002, verified against
/// `javap -p`: `ClientboundBlockChangedAckPacket(int sequence)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChangedAck {
    pub sequence: i32,
}

impl Packet for BlockChangedAck {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_BLOCK_CHANGED_ACK at game-CB index 4 = wire id 0x04.
    const ID: i32 = 0x04;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            sequence: buf.read_varint()?,
        })
    }
}

/// `Clientbound Light Update` (CB). Per-chunk light delta — sent
/// when block changes alter sky / block light without re-streaming
/// the full chunk. Per ADR 0002, verified against `javap -p`:
/// `ClientboundLightUpdatePacket(int x, int z,
/// ClientboundLightUpdatePacketData lightData)`. The wire body for
/// the chunk coordinates is a pair of VarInts (matching vanilla's
/// `STREAM_CODEC`), not raw `i32`s — distinct from
/// `LevelChunkWithLight` which writes the coordinates as `i32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightUpdate {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub light: LightData,
}

impl Packet for LightUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_LIGHT_UPDATE at game-CB index 48 = wire id 0x30.
    const ID: i32 = 0x30;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.chunk_x);
        buf.write_varint(self.chunk_z);
        self.light.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            chunk_x: buf.read_varint()?,
            chunk_z: buf.read_varint()?,
            light: LightData::decode(buf)?,
        })
    }
}

// ---------------------------------------------------------------------
// M5.a — serverbound interaction packets
// ---------------------------------------------------------------------

/// One of the eight actions vanilla packs into
/// `ServerboundPlayerAction`. Enum discriminants match the wire
/// VarInt: the order is the vanilla source's declaration order
/// (`ServerboundPlayerActionPacket$Action`), verified against
/// `javap -p` on the unobfuscated jar.
///
/// M5's break-block path only acts on `StartDestroyBlock` and
/// `StopDestroyBlock`. The other variants are decoded so the
/// handler can log-and-ignore them without breaking the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerActionKind {
    StartDestroyBlock = 0,
    AbortDestroyBlock = 1,
    StopDestroyBlock = 2,
    DropAllItems = 3,
    DropItem = 4,
    ReleaseUseItem = 5,
    SwapItemWithOffhand = 6,
    Stab = 7,
}

impl PlayerActionKind {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::StartDestroyBlock,
            1 => Self::AbortDestroyBlock,
            2 => Self::StopDestroyBlock,
            3 => Self::DropAllItems,
            4 => Self::DropItem,
            5 => Self::ReleaseUseItem,
            6 => Self::SwapItemWithOffhand,
            7 => Self::Stab,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 7,
                });
            }
        })
    }
}

/// Direction of a `BlockPos` face — six values matching
/// `net.minecraft.core.Direction` ordinals (`DOWN, UP, NORTH,
/// SOUTH, WEST, EAST`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Direction {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::Down,
            1 => Self::Up,
            2 => Self::North,
            3 => Self::South,
            4 => Self::West,
            5 => Self::East,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 5,
                });
            }
        })
    }

    /// `(dx, dy, dz)` unit normal of the face. Used by the
    /// place-block handler to compute the target cell from the
    /// clicked face.
    #[must_use]
    pub fn normal(self) -> (i32, i32, i32) {
        match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }
}

/// `Serverbound Player Action` (SB). Carries break-start /
/// break-abort / break-stop, plus item-drop and a few miscellaneous
/// actions. Per ADR 0002, verified against `javap -p` of the
/// unobfuscated jar: `ServerboundPlayerActionPacket(action, pos,
/// direction, sequence)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlayerAction {
    pub action: PlayerActionKind,
    /// Block position packed into an `i64` per the vanilla
    /// `BlockPos.toLong` format used elsewhere in this module
    /// (`SetDefaultSpawnPosition`, `BlockUpdate`).
    pub position: i64,
    pub direction: Direction,
    pub sequence: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCommandAction {
    PressShiftKey = 0,
    ReleaseShiftKey = 1,
    StopSleeping = 2,
    StartSprinting = 3,
    StopSprinting = 4,
    StartRidingJump = 5,
    StopRidingJump = 6,
    OpenInventory = 7,
    StartFallFlying = 8,
}

impl PlayerCommandAction {
    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::PressShiftKey,
            1 => Self::ReleaseShiftKey,
            2 => Self::StopSleeping,
            3 => Self::StartSprinting,
            4 => Self::StopSprinting,
            5 => Self::StartRidingJump,
            6 => Self::StopRidingJump,
            7 => Self::OpenInventory,
            8 => Self::StartFallFlying,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 8,
                });
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlayerCommand {
    pub entity_id: i32,
    pub action: PlayerCommandAction,
    pub data: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

impl PlayerInput {
    #[must_use]
    pub const fn from_flags(flags: u8) -> Self {
        Self {
            forward: flags & 0x01 != 0,
            backward: flags & 0x02 != 0,
            left: flags & 0x04 != 0,
            right: flags & 0x08 != 0,
            jump: flags & 0x10 != 0,
            shift: flags & 0x20 != 0,
            sprint: flags & 0x40 != 0,
        }
    }

    #[must_use]
    pub const fn flags(self) -> u8 {
        (self.forward as u8)
            | ((self.backward as u8) << 1)
            | ((self.left as u8) << 2)
            | ((self.right as u8) << 3)
            | ((self.jump as u8) << 4)
            | ((self.shift as u8) << 5)
            | ((self.sprint as u8) << 6)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundPlayerInput {
    pub input: PlayerInput,
}

impl Packet for ServerboundPlayerInput {
    // `.analysis/protocol-dump.txt`: SERVERBOUND_PLAYER_INPUT follows
    // SERVERBOUND_PLAYER_COMMAND at game-SB index 43, wire id 0x2B. `javap -p -c`
    // of `ServerboundPlayerInputPacket` and `Input$1` shows one signed byte bitset:
    // forward, backward, left, right, jump, shift, sprint.
    const ID: i32 = 0x2B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.input.flags());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            input: PlayerInput::from_flags(buf.read_u8()?),
        })
    }
}

impl Packet for ServerboundPlayerCommand {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_PLAYER_COMMAND is
    // serverbound registration index 42, wire id 0x2A. The packet record fields
    // are `int id`, `Action action`, then `int data`; vanilla writes all three as
    // VarInts.
    const ID: i32 = 0x2A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.action as i32);
        buf.write_varint(self.data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            action: PlayerCommandAction::from_wire(buf.read_varint()?)?,
            data: buf.read_varint()?,
        })
    }
}

impl Packet for ServerboundPlayerAction {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_PLAYER_ACTION sits at game-SB index 41 = wire id 0x29.
    const ID: i32 = 0x29;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.action as i32);
        buf.write_i64(self.position);
        buf.write_varint(self.direction as i32);
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let action = PlayerActionKind::from_wire(buf.read_varint()?)?;
        let position = buf.read_i64()?;
        let direction = Direction::from_wire(buf.read_varint()?)?;
        let sequence = buf.read_varint()?;
        Ok(Self {
            action,
            position,
            direction,
            sequence,
        })
    }
}

/// Which hand was used. Vanilla VarInt: 0 = main, 1 = off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionHand {
    MainHand = 0,
    OffHand = 1,
}

impl InteractionHand {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::MainHand,
            1 => Self::OffHand,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 1,
                });
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundSwing {
    pub hand: InteractionHand,
}

impl Packet for ServerboundSwing {
    // `.analysis/protocol-dump.txt`: SERVERBOUND_SWING follows
    // SERVERBOUND_SPECTATE_ENTITY at game-SB index 63, wire id 0x3F.
    // `javap -p` shows a single InteractionHand field.
    const ID: i32 = 0x3F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            hand: InteractionHand::from_wire(buf.read_varint()?)?,
        })
    }
}

/// `Serverbound Use Item On` (SB) — the place-block / right-click
/// packet. Per ADR 0002, verified against `javap -p`:
/// `ServerboundUseItemOnPacket(BlockHitResult blockHit,
/// InteractionHand hand, int sequence)`.
///
/// The wire order, per the unobfuscated jar's stream codec, is
/// `hand` (VarInt) first, then the `BlockHitResult` body, then
/// `sequence`. `BlockHitResult` on the wire is *not* the in-memory
/// shape — it ships as
/// `(BlockPos pos, Direction direction, Vec3 cursor, bool inside,
/// bool world_border_hit)` (`pos` packed as the standard `i64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundUseItemOn {
    pub hand: InteractionHand,
    pub position: i64,
    pub direction: Direction,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_z: f32,
    pub inside: bool,
    pub world_border_hit: bool,
    pub sequence: i32,
}

impl Packet for ServerboundUseItemOn {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_USE_ITEM_ON sits at game-SB index 66 = wire id 0x42.
    const ID: i32 = 0x42;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        buf.write_i64(self.position);
        buf.write_varint(self.direction as i32);
        buf.write_f32(self.cursor_x);
        buf.write_f32(self.cursor_y);
        buf.write_f32(self.cursor_z);
        buf.write_bool(self.inside);
        buf.write_bool(self.world_border_hit);
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let hand = InteractionHand::from_wire(buf.read_varint()?)?;
        let position = buf.read_i64()?;
        let direction = Direction::from_wire(buf.read_varint()?)?;
        let cursor_x = buf.read_f32()?;
        let cursor_y = buf.read_f32()?;
        let cursor_z = buf.read_f32()?;
        let inside = buf.read_bool()?;
        let world_border_hit = buf.read_bool()?;
        let sequence = buf.read_varint()?;
        Ok(Self {
            hand,
            position,
            direction,
            cursor_x,
            cursor_y,
            cursor_z,
            inside,
            world_border_hit,
            sequence,
        })
    }
}

/// `Serverbound Use Item` (SB) — right-click air / consume held item.
/// Verified via `.analysis/protocol-dump.txt` and `javap -p`:
/// `ServerboundUseItemPacket(InteractionHand hand, int sequence,
/// float yRot, float xRot)`; game-SB index 67 = wire id 0x43.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundUseItem {
    pub hand: InteractionHand,
    pub sequence: i32,
    pub y_rot: f32,
    pub x_rot: f32,
}

impl Packet for ServerboundUseItem {
    const ID: i32 = 0x43;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        buf.write_varint(self.sequence);
        buf.write_f32(self.y_rot);
        buf.write_f32(self.x_rot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            hand: InteractionHand::from_wire(buf.read_varint()?)?,
            sequence: buf.read_varint()?,
            y_rot: buf.read_f32()?,
            x_rot: buf.read_f32()?,
        })
    }
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

/// One slot of a vanilla container. The modern wire format encodes an
/// empty stack as a single zero-byte `count` VarInt; a non-empty stack
/// is `(count, item_id, components_to_add, components_to_remove,
/// [DataComponentPatch entries...])`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemStack {
    /// `0` ⇒ empty slot; `count > 0` ⇒ `count` copies of `item_id`.
    pub count: i32,
    /// Item-registry id (the protocol_id from
    /// `data/vanilla/reports/registries.json:minecraft:item`).
    pub item_id: u32,
    /// Narrow M23 component support: `minecraft:damage`, encoded as
    /// DataComponents registration id 3 with a VarInt integer payload.
    pub damage: Option<i32>,
    /// `minecraft:enchantments`, encoded as a registry-id to level map.
    pub enchantments: Vec<mc_data::ItemEnchantment>,
    /// `minecraft:custom_name`, limited to the literal text-component shape
    /// used by Solaris script menus.
    pub custom_name: Option<String>,
    /// `minecraft:item_model`, encoded as one namespaced resource identifier.
    pub item_model: Option<std::sync::Arc<Identifier>>,
}

impl ItemStack {
    /// An empty slot.
    pub const EMPTY: ItemStack = ItemStack {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }

    #[must_use]
    pub fn new(item_id: u32, count: i32) -> Self {
        Self {
            count,
            item_id,
            damage: None,
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
        }
    }

    #[must_use]
    pub fn with_damage(mut self, damage: i32) -> Self {
        self.damage = Some(damage.max(0));
        self
    }

    #[must_use]
    pub fn with_enchantment(mut self, id: Identifier, level: i32) -> Self {
        self.enchantments.retain(|enchantment| enchantment.id != id);
        self.enchantments
            .push(mc_data::ItemEnchantment { id, level });
        self.enchantments
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
        self
    }

    #[must_use]
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(name.into());
        self
    }

    #[must_use]
    pub fn with_item_model(mut self, model: Identifier) -> Self {
        self.item_model = Some(std::sync::Arc::new(model));
        self
    }

    pub fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.is_empty() {
            buf.write_varint(0);
            return Ok(());
        }
        buf.write_varint(self.count);
        buf.write_varint(self.item_id as i32);
        let component_count = i32::from(self.damage.is_some())
            + i32::from(self.custom_name.is_some())
            + i32::from(self.item_model.is_some())
            + i32::from(!self.enchantments.is_empty());
        buf.write_varint(component_count);
        buf.write_varint(0);
        if let Some(damage) = self.damage {
            buf.write_varint(DATA_COMPONENT_DAMAGE_ID);
            buf.write_varint(damage);
        }
        if let Some(custom_name) = &self.custom_name {
            buf.write_varint(DATA_COMPONENT_CUSTOM_NAME_ID);
            write_literal_text_component(buf, custom_name)?;
        }
        if let Some(item_model) = &self.item_model {
            buf.write_varint(DATA_COMPONENT_ITEM_MODEL_ID);
            buf.write_identifier(item_model)?;
        }
        if !self.enchantments.is_empty() {
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
        Ok(())
    }

    pub fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = buf.read_varint()?;
        if count <= 0 {
            return Ok(Self::EMPTY);
        }
        let item_id = buf.read_varint()? as u32;
        let n_add = buf.read_varint()?;
        let n_remove = buf.read_varint()?;
        if n_add < 0 || n_remove < 0 {
            return Err(CodecError::NegativeLength(n_add.min(n_remove)));
        }
        if n_add > 4 || n_remove != 0 {
            return Err(CodecError::NotSupported(
                "ItemStack with unsupported DataComponentPatch shape",
            ));
        }
        let mut damage = None;
        let mut custom_name = None;
        let mut item_model = None;
        let mut enchantments = Vec::new();
        for _ in 0..n_add {
            let component_id = buf.read_varint()?;
            match component_id {
                DATA_COMPONENT_DAMAGE_ID if damage.is_none() => {
                    damage = Some(buf.read_varint()?.max(0));
                }
                DATA_COMPONENT_CUSTOM_NAME_ID if custom_name.is_none() => {
                    custom_name = Some(read_literal_text_component(buf)?);
                }
                DATA_COMPONENT_ITEM_MODEL_ID if item_model.is_none() => {
                    item_model = Some(std::sync::Arc::new(buf.read_identifier()?));
                }
                DATA_COMPONENT_ENCHANTMENTS_ID if enchantments.is_empty() => {
                    let count = read_count(buf, 256)?;
                    enchantments.reserve(count);
                    for _ in 0..count {
                        let protocol_id = buf.read_varint()?;
                        let protocol_id = u32::try_from(protocol_id).map_err(|_| {
                            CodecError::NotSupported("negative enchantment registry id")
                        })?;
                        let id = mc_data::required_registry_entry("enchantment", protocol_id)
                            .ok_or(CodecError::NotSupported(
                                "unknown enchantment registry entry",
                            ))?;
                        let level = buf.read_varint()?;
                        if !(1..=255).contains(&level)
                            || enchantments
                                .iter()
                                .any(|entry: &mc_data::ItemEnchantment| entry.id == id)
                        {
                            return Err(CodecError::NotSupported(
                                "invalid ItemStack enchantments component",
                            ));
                        }
                        enchantments.push(mc_data::ItemEnchantment { id, level });
                    }
                    enchantments.sort_unstable_by(|left, right| left.id.cmp(&right.id));
                }
                _ => {
                    return Err(CodecError::NotSupported(
                        "ItemStack with unsupported DataComponentPatch component",
                    ));
                }
            }
        }
        Ok(Self {
            count,
            item_id,
            damage,
            enchantments,
            custom_name,
            item_model,
        })
    }
}

fn write_literal_text_component<B: BufMut>(buf: &mut B, text: &str) -> Result<(), CodecError> {
    mc_nbt::write_network(
        buf,
        &mc_nbt::Tag::Compound(vec![(
            "text".to_owned(),
            mc_nbt::Tag::String(text.to_owned()),
        )]),
    )?;
    Ok(())
}

fn read_literal_text_component<B: Buf>(buf: &mut B) -> Result<String, CodecError> {
    let mc_nbt::Tag::Compound(fields) = mc_nbt::read_network(buf)? else {
        return Err(CodecError::NotSupported(
            "ItemStack custom name must be a text component",
        ));
    };
    let mut fields = fields.into_iter();
    let Some((name, mc_nbt::Tag::String(text))) = fields.next() else {
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

/// `Clientbound Set Held Slot` (CB). Tells the client which hotbar
/// slot the server believes is selected. Server emits this on login
/// to seed the cursor. Per ADR 0002, verified against `javap -p`:
/// `ClientboundSetHeldSlotPacket(int slot)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundSetHeldSlot {
    pub slot: i32,
}

impl Packet for ClientboundSetHeldSlot {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_HELD_SLOT at game-CB index 105 = wire id 0x69.
    const ID: i32 = 0x69;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.slot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_varint()?,
        })
    }
}

/// `Clientbound Container Set Content` (CB). Replaces every slot of a
/// container in one packet. M6 uses it on login to seed window 0
/// (player inventory) with the starter kit. Per ADR 0002, verified
/// against `javap -p`:
/// `ClientboundContainerSetContentPacket(int containerId, int stateId,
/// List<ItemStack> items, ItemStack carriedItem)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundContainerSetContent {
    pub container_id: i32,
    pub state_id: i32,
    pub items: Vec<ItemStack>,
    pub carried_item: ItemStack,
}

impl Packet for ClientboundContainerSetContent {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_CONTAINER_SET_CONTENT at game-CB index 18 = wire id 0x12.
    const ID: i32 = 0x12;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        let len = i32::try_from(self.items.len()).map_err(|_| CodecError::StringTooLong {
            len: self.items.len(),
            max: i32::MAX as usize,
        })?;
        buf.write_varint(len);
        for item in &self.items {
            item.encode(buf)?;
        }
        self.carried_item.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let state_id = buf.read_varint()?;
        let len_signed = buf.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        // A vanilla player inventory is 46 slots; allow up to 256 as
        // a soft cap so future container types still fit.
        let len = len_signed as usize;
        if len > 256 {
            return Err(CodecError::StringTooLong { len, max: 256 });
        }
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(ItemStack::decode(buf)?);
        }
        let carried_item = ItemStack::decode(buf)?;
        Ok(Self {
            container_id,
            state_id,
            items,
            carried_item,
        })
    }
}

/// `Clientbound Container Close` (CB). Verified against the local vanilla
/// 26.1.2 class: `ClientboundContainerClosePacket(int containerId)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundContainerClose {
    pub container_id: i32,
}

impl Packet for ClientboundContainerClose {
    const ID: i32 = 0x11;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
        })
    }
}

/// `Clientbound Container Set Data` (CB). Furnace/progress bars use this packet.
/// Verified against the local vanilla 26.1.2 class:
/// `ClientboundContainerSetDataPacket(int containerId, int id, int value)`;
/// `id` and `value` are encoded as shorts on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundContainerSetData {
    pub container_id: i32,
    pub id: i16,
    pub value: i16,
}

impl Packet for ClientboundContainerSetData {
    const ID: i32 = 0x13;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_i16(self.id);
        buf.write_i16(self.value);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            id: buf.read_i16()?,
            value: buf.read_i16()?,
        })
    }
}

/// `Clientbound Container Set Slot` (CB). Single-slot update inside a
/// container. M6 emits one of these after a place mutation to
/// decrement the held stack's count. Per ADR 0002, verified against
/// `javap -p`:
/// `ClientboundContainerSetSlotPacket(int containerId, int stateId,
/// int slot, ItemStack itemStack)`. The Java field type is `int` but
/// the wire codec encodes `slot` as a `short` — same convention as
/// the symmetric serverbound `SetCarriedItem`. M6 missed this and
/// shipped the slot as varint, which slid the downstream `ItemStack`
/// payload and made vanilla 26.1.2 clients fail decode on every
/// post-edit `ContainerSetSlot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundContainerSetSlot {
    pub container_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub item_stack: ItemStack,
}

impl Packet for ClientboundContainerSetSlot {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_CONTAINER_SET_SLOT at game-CB index 20 = wire id 0x14.
    const ID: i32 = 0x14;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        buf.write_i16(self.slot);
        self.item_stack.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            state_id: buf.read_varint()?,
            slot: buf.read_i16()?,
            item_stack: ItemStack::decode(buf)?,
        })
    }
}

/// `Clientbound Open Screen` (CB). Verified against the local vanilla 26.1.2
/// class: `ClientboundOpenScreenPacket(int containerId, MenuType<?> type,
/// Component title)`. The title is an opaque binary text `Component` payload;
/// decode treats it as the final field and consumes the rest of the packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundOpenScreen {
    pub container_id: i32,
    pub menu_type: i32,
    pub title_nbt: Vec<u8>,
}

impl Packet for ClientboundOpenScreen {
    const ID: i32 = 0x3B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.menu_type);
        buf.put_slice(&self.title_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let menu_type = buf.read_varint()?;
        let remaining = buf.remaining();
        let mut title_nbt = vec![0; remaining];
        buf.copy_to_slice(&mut title_nbt);
        Ok(Self {
            container_id,
            menu_type,
            title_nbt,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedStackComponentHashes {
    pub added: Vec<(i32, i32)>,
    pub removed: Vec<i32>,
}

impl HashedStackComponentHashes {
    pub fn empty() -> Self {
        Self {
            added: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn component_hash_count(&self) -> Result<usize, CodecError> {
        for len in [self.added.len(), self.removed.len()] {
            if len > MAX_HASHED_STACK_COMPONENT_HASHES {
                return Err(CodecError::StringTooLong {
                    len,
                    max: MAX_HASHED_STACK_COMPONENT_HASHES,
                });
            }
        }
        Ok(self.added.len() + self.removed.len())
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.component_hash_count()?;

        write_count(buf, self.added.len())?;
        for (component_id, hash) in &self.added {
            buf.write_varint(*component_id);
            buf.write_i32(*hash);
        }

        write_count(buf, self.removed.len())?;
        for component_id in &self.removed {
            buf.write_varint(*component_id);
        }
        Ok(())
    }

    fn decode_with_budget<B: Buf>(
        buf: &mut B,
        component_hash_count: &mut usize,
    ) -> Result<Self, CodecError> {
        let added_len = read_count(buf, MAX_HASHED_STACK_COMPONENT_HASHES)?;
        add_component_hashes_to_budget(component_hash_count, added_len)?;
        require_remaining(buf, added_len * 5 + 1)?;
        let mut added = Vec::with_capacity(added_len);
        for _ in 0..added_len {
            added.push((buf.read_varint()?, buf.read_i32()?));
        }

        let removed_len = read_count(buf, MAX_HASHED_STACK_COMPONENT_HASHES)?;
        add_component_hashes_to_budget(component_hash_count, removed_len)?;
        require_remaining(buf, removed_len)?;
        let mut removed = Vec::with_capacity(removed_len);
        for _ in 0..removed_len {
            removed.push(buf.read_varint()?);
        }
        Ok(Self { added, removed })
    }

    #[cfg(test)]
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let mut component_hash_count = 0;
        Self::decode_with_budget(buf, &mut component_hash_count)
    }
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

/// Client-side hash view of an item stack, used only by serverbound
/// container reconciliation. The server never trusts these values for
/// inventory mutation; they are decoded so the stream stays aligned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashedStack {
    Empty,
    Actual {
        item_id: u32,
        count: i32,
        components: HashedStackComponentHashes,
    },
}

impl HashedStack {
    pub fn empty() -> Self {
        Self::Empty
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.component_hash_count()?;
        match self {
            Self::Empty => buf.write_bool(false),
            Self::Actual {
                item_id,
                count,
                components,
            } => {
                buf.write_bool(true);
                buf.write_varint(*item_id as i32);
                buf.write_varint(*count);
                components.encode(buf)?;
            }
        }
        Ok(())
    }

    fn component_hash_count(&self) -> Result<usize, CodecError> {
        match self {
            Self::Empty => Ok(0),
            Self::Actual {
                item_id,
                count,
                components,
            } => {
                if *item_id > i32::MAX as u32 {
                    return Err(CodecError::NotSupported(
                        "hashed stack item id exceeds VarInt range",
                    ));
                }
                if *count <= 0 {
                    return Err(CodecError::NotSupported(
                        "HashedStack actual item with non-positive count",
                    ));
                }
                components.component_hash_count()
            }
        }
    }

    fn decode<B: Buf>(buf: &mut B, component_hash_count: &mut usize) -> Result<Self, CodecError> {
        if !buf.read_bool()? {
            return Ok(Self::Empty);
        }
        let item_id = buf.read_varint()?;
        if item_id < 0 {
            return Err(CodecError::NegativeLength(item_id));
        }
        let count = buf.read_varint()?;
        if count <= 0 {
            return Err(CodecError::NotSupported(
                "HashedStack actual item with non-positive count",
            ));
        }
        Ok(Self::Actual {
            item_id: item_id as u32,
            count,
            components: HashedStackComponentHashes::decode_with_budget(buf, component_hash_count)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerInput {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

impl ContainerInput {
    const fn as_wire(self) -> i32 {
        match self {
            Self::Pickup => 0,
            Self::QuickMove => 1,
            Self::Swap => 2,
            Self::Clone => 3,
            Self::Throw => 4,
            Self::QuickCraft => 5,
            Self::PickupAll => 6,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Pickup,
            1 => Self::QuickMove,
            2 => Self::Swap,
            3 => Self::Clone,
            4 => Self::Throw,
            5 => Self::QuickCraft,
            6 => Self::PickupAll,
            _ => return Err(CodecError::NotSupported("unknown ContainerInput id")),
        })
    }
}

/// `Serverbound Container Button Click` (SB). Vanilla 26.1.2 encodes the
/// container id with `ContainerId.STREAM_CODEC`, followed by a VarInt button id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundContainerButtonClick {
    pub container_id: i32,
    pub button_id: i32,
}

impl Packet for ServerboundContainerButtonClick {
    const ID: i32 = 0x11;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.button_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            button_id: buf.read_varint()?,
        })
    }
}

/// `Serverbound Container Click` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundContainerClickPacket(int containerId, int stateId, short slotNum,
/// byte buttonNum, ContainerInput containerInput, Int2ObjectMap<HashedStack>
/// changedSlots, HashedStack carriedItem)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundContainerClick {
    pub container_id: i32,
    pub state_id: i32,
    pub slot_num: i16,
    pub button_num: i8,
    pub container_input: ContainerInput,
    pub changed_slots: Vec<(i16, HashedStack)>,
    pub carried_item: HashedStack,
}

impl Packet for ServerboundContainerClick {
    const ID: i32 = 0x12;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.changed_slots.len() > MAX_CONTAINER_CLICK_CHANGED_SLOTS {
            return Err(CodecError::StringTooLong {
                len: self.changed_slots.len(),
                max: MAX_CONTAINER_CLICK_CHANGED_SLOTS,
            });
        }
        let mut component_hash_count = 0;
        for (_, stack) in &self.changed_slots {
            add_component_hashes_to_budget(
                &mut component_hash_count,
                stack.component_hash_count()?,
            )?;
        }
        add_component_hashes_to_budget(
            &mut component_hash_count,
            self.carried_item.component_hash_count()?,
        )?;

        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        buf.write_i16(self.slot_num);
        buf.write_i8(self.button_num);
        buf.write_varint(self.container_input.as_wire());
        write_count(buf, self.changed_slots.len())?;
        for (slot, stack) in &self.changed_slots {
            buf.write_i16(*slot);
            stack.encode(buf)?;
        }
        self.carried_item.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let state_id = buf.read_varint()?;
        let slot_num = buf.read_i16()?;
        let button_num = buf.read_i8()?;
        let container_input = ContainerInput::from_wire(buf.read_varint()?)?;
        let changed_len = read_count(buf, MAX_CONTAINER_CLICK_CHANGED_SLOTS)?;
        require_remaining(buf, changed_len * 3 + 1)?;
        let mut changed_slots = Vec::with_capacity(changed_len);
        let mut component_hash_count = 0;
        for _ in 0..changed_len {
            changed_slots.push((
                buf.read_i16()?,
                HashedStack::decode(buf, &mut component_hash_count)?,
            ));
        }
        let carried_item = HashedStack::decode(buf, &mut component_hash_count)?;
        Ok(Self {
            container_id,
            state_id,
            slot_num,
            button_num,
            container_input,
            changed_slots,
            carried_item,
        })
    }
}

/// `Serverbound Set Carried Item` (SB). Sent when the client scrolls
/// the hotbar — `slot` ∈ `0..=8`. Per ADR 0002, verified against
/// `javap -p`: `ServerboundSetCarriedItemPacket(short slot)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundSetCarriedItem {
    pub slot: i16,
}

impl Packet for ServerboundSetCarriedItem {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_SET_CARRIED_ITEM at game-SB index 53 = wire id 0x35.
    const ID: i32 = 0x35;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i16(self.slot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_i16()?,
        })
    }
}

/// `Serverbound Place Recipe` (SB). Per local 26.1.2 `javap`:
/// `ServerboundPlaceRecipePacket(int containerId, RecipeDisplayId recipe,
/// boolean useMaxItems)`, encoded as container-id VarInt, recipe display
/// index VarInt, then bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlaceRecipe {
    pub container_id: i32,
    pub recipe_display_id: i32,
    pub use_max_items: bool,
}

impl Packet for ServerboundPlaceRecipe {
    // GameProtocols registers SERVERBOUND_PLACE_RECIPE immediately before
    // PLAYER_ABILITIES / PLAYER_ACTION; with SERVERBOUND_PLAYER_ACTION pinned
    // at index 41, this packet is game-SB index 39 = wire id 0x27.
    const ID: i32 = 0x27;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.recipe_display_id);
        buf.write_bool(self.use_max_items);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            recipe_display_id: buf.read_varint()?,
            use_max_items: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecipeBookTypeSettings {
    pub is_open: bool,
    pub is_filtering: bool,
}

/// Clientbound recipe-book UI settings. The local vanilla 26.1.2 codec writes
/// one open/filtering boolean pair for each of the four recipe-book types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientboundRecipeBookSettings {
    pub crafting: RecipeBookTypeSettings,
    pub furnace: RecipeBookTypeSettings,
    pub blast_furnace: RecipeBookTypeSettings,
    pub smoker: RecipeBookTypeSettings,
}

impl Packet for ClientboundRecipeBookSettings {
    const ID: i32 = 0x4C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        for settings in [self.crafting, self.furnace, self.blast_furnace, self.smoker] {
            buf.write_bool(settings.is_open);
            buf.write_bool(settings.is_filtering);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let mut read_settings = || -> Result<RecipeBookTypeSettings, CodecError> {
            Ok(RecipeBookTypeSettings {
                is_open: buf.read_bool()?,
                is_filtering: buf.read_bool()?,
            })
        };
        Ok(Self {
            crafting: read_settings()?,
            furnace: read_settings()?,
            blast_furnace: read_settings()?,
            smoker: read_settings()?,
        })
    }
}

/// The slot-display variants Solaris emits in initial recipe-book entries.
/// Registry ids are pinned to the bundled 26.1.2 `minecraft:slot_display`
/// registry report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeBookSlotDisplay {
    Empty,
    AnyFuel,
    Item { item_id: i32 },
    ItemStack { item_id: i32, count: i32 },
    Tag(Identifier),
    Composite(Vec<RecipeBookSlotDisplay>),
}

impl RecipeBookSlotDisplay {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.encode_with_depth(buf, 0)
    }

    fn encode_with_depth<B: BufMut>(&self, buf: &mut B, depth: usize) -> Result<(), CodecError> {
        if depth >= MAX_RECIPE_BOOK_SLOT_DEPTH {
            return Err(CodecError::NotSupported(
                "recipe-book slot display nesting is too deep",
            ));
        }
        match self {
            Self::Empty => buf.write_varint(0),
            Self::AnyFuel => buf.write_varint(1),
            Self::Item { item_id } => {
                if *item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book item registry id",
                    ));
                }
                buf.write_varint(4);
                buf.write_varint(*item_id);
            }
            Self::ItemStack { item_id, count } => {
                if *item_id < 0 || *count <= 0 {
                    return Err(CodecError::NotSupported("invalid recipe-book item stack"));
                }
                buf.write_varint(5);
                buf.write_varint(*item_id);
                buf.write_varint(*count);
                // Recipe displays only need the default item component patch.
                buf.write_varint(0);
                buf.write_varint(0);
            }
            Self::Tag(tag) => {
                buf.write_varint(6);
                buf.write_identifier(tag)?;
            }
            Self::Composite(displays) => {
                buf.write_varint(10);
                write_bounded_count(buf, displays.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for display in displays {
                    display.encode_with_depth(buf, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Self::decode_with_depth(buf, 0)
    }

    fn decode_with_depth<B: Buf>(buf: &mut B, depth: usize) -> Result<Self, CodecError> {
        if depth >= MAX_RECIPE_BOOK_SLOT_DEPTH {
            return Err(CodecError::NotSupported(
                "recipe-book slot display nesting is too deep",
            ));
        }
        match buf.read_varint()? {
            0 => Ok(Self::Empty),
            1 => Ok(Self::AnyFuel),
            4 => {
                let item_id = buf.read_varint()?;
                if item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book item registry id",
                    ));
                }
                Ok(Self::Item { item_id })
            }
            5 => {
                let item_id = buf.read_varint()?;
                let count = buf.read_varint()?;
                if item_id < 0 || count <= 0 {
                    return Err(CodecError::NotSupported("invalid recipe-book item stack"));
                }
                let components_to_add = buf.read_varint()?;
                let components_to_remove = buf.read_varint()?;
                if components_to_add != 0 || components_to_remove != 0 {
                    return Err(CodecError::NotSupported(
                        "recipe-book item stack with component patch",
                    ));
                }
                Ok(Self::ItemStack { item_id, count })
            }
            6 => Ok(Self::Tag(buf.read_identifier()?)),
            10 => {
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                let mut displays = Vec::with_capacity(count);
                for _ in 0..count {
                    displays.push(Self::decode_with_depth(buf, depth + 1)?);
                }
                Ok(Self::Composite(displays))
            }
            _ => Err(CodecError::NotSupported(
                "unsupported recipe-book slot display type",
            )),
        }
    }
}

/// Vanilla's holder-set representation used by crafting requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeBookIngredient {
    Tag(Identifier),
    Items(Vec<i32>),
}

/// One entry in the recipe-property-set map carried by
/// `ClientboundUpdateRecipesPacket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipePropertySet {
    pub key: Identifier,
    pub item_ids: Vec<i32>,
}

/// One input/output offer in vanilla's stonecutter recipe set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecutterRecipeEntry {
    pub input: RecipeBookIngredient,
    pub result: RecipeBookSlotDisplay,
}

/// `ClientboundUpdateRecipesPacket` from the local vanilla 26.1.2 jar.
///
/// The first field is a map of recipe-property-set keys to item registry-id
/// lists. The second is the stonecutter's single-input offer list. Solaris
/// currently sends an empty property-set map and item-stack stonecutter
/// displays, but the codec remains typed for both fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundUpdateRecipes {
    pub item_sets: Vec<RecipePropertySet>,
    pub stonecutter_recipes: Vec<StonecutterRecipeEntry>,
}

impl Packet for ClientboundUpdateRecipes {
    // GameProtocols' bundle packet is wire id 0. UPDATE_RECIPES is the
    // 133rd following clientbound registration, giving wire id 0x85.
    const ID: i32 = 0x85;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_bounded_count(buf, self.item_sets.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for item_set in &self.item_sets {
            buf.write_identifier(&item_set.key)?;
            write_bounded_count(
                buf,
                item_set.item_ids.len(),
                MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
            )?;
            for item_id in &item_set.item_ids {
                if *item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe property-set item registry id",
                    ));
                }
                buf.write_varint(*item_id);
            }
        }
        write_bounded_count(buf, self.stonecutter_recipes.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for recipe in &self.stonecutter_recipes {
            recipe.input.encode(buf)?;
            recipe.result.encode(buf)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let item_set_count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut item_sets = Vec::with_capacity(item_set_count);
        for _ in 0..item_set_count {
            let key = buf.read_identifier()?;
            let item_count = read_count(buf, MAX_RECIPE_BOOK_INGREDIENT_ITEMS)?;
            let mut item_ids = Vec::with_capacity(item_count);
            for _ in 0..item_count {
                let item_id = buf.read_varint()?;
                if item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe property-set item registry id",
                    ));
                }
                item_ids.push(item_id);
            }
            item_sets.push(RecipePropertySet { key, item_ids });
        }

        let recipe_count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut stonecutter_recipes = Vec::with_capacity(recipe_count);
        for _ in 0..recipe_count {
            stonecutter_recipes.push(StonecutterRecipeEntry {
                input: RecipeBookIngredient::decode(buf)?,
                result: RecipeBookSlotDisplay::decode(buf)?,
            });
        }
        Ok(Self {
            item_sets,
            stonecutter_recipes,
        })
    }
}

impl RecipeBookIngredient {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Tag(tag) => {
                buf.write_varint(0);
                buf.write_identifier(tag)?;
            }
            Self::Items(items) => {
                if items.is_empty() {
                    return Err(CodecError::NotSupported(
                        "empty direct recipe-book ingredient",
                    ));
                }
                if items.len() > MAX_RECIPE_BOOK_INGREDIENT_ITEMS {
                    return Err(CodecError::StringTooLong {
                        len: items.len(),
                        max: MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
                    });
                }
                let marker =
                    i32::try_from(items.len() + 1).map_err(|_| CodecError::StringTooLong {
                        len: items.len(),
                        max: i32::MAX as usize - 1,
                    })?;
                buf.write_varint(marker);
                for item_id in items {
                    if *item_id < 0 {
                        return Err(CodecError::NotSupported(
                            "negative recipe-book item registry id",
                        ));
                    }
                    buf.write_varint(*item_id);
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let marker = buf.read_varint()?;
        if marker < 0 {
            return Err(CodecError::NegativeLength(marker));
        }
        if marker == 0 {
            return Ok(Self::Tag(buf.read_identifier()?));
        }
        let count = marker as usize - 1;
        if count == 0 {
            return Err(CodecError::NotSupported(
                "empty direct recipe-book ingredient",
            ));
        }
        if count > MAX_RECIPE_BOOK_INGREDIENT_ITEMS {
            return Err(CodecError::StringTooLong {
                len: count,
                max: MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
            });
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            let item_id = buf.read_varint()?;
            if item_id < 0 {
                return Err(CodecError::NotSupported(
                    "negative recipe-book item registry id",
                ));
            }
            items.push(item_id);
        }
        Ok(Self::Items(items))
    }
}

/// Recipe display variants currently backed by Solaris' recipe executor.
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeBookDisplay {
    Shapeless {
        ingredients: Vec<RecipeBookSlotDisplay>,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
    },
    Shaped {
        width: i32,
        height: i32,
        ingredients: Vec<RecipeBookSlotDisplay>,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
    },
    Furnace {
        ingredient: RecipeBookSlotDisplay,
        fuel: RecipeBookSlotDisplay,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
        duration: i32,
        experience: f32,
    },
}

impl RecipeBookDisplay {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Shapeless {
                ingredients,
                result,
                crafting_station,
            } => {
                buf.write_varint(0);
                write_bounded_count(buf, ingredients.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for ingredient in ingredients {
                    ingredient.encode(buf)?;
                }
                result.encode(buf)?;
                crafting_station.encode(buf)?;
            }
            Self::Shaped {
                width,
                height,
                ingredients,
                result,
                crafting_station,
            } => {
                let expected = recipe_book_shape_size(*width, *height)?;
                if ingredients.len() != expected {
                    return Err(CodecError::NotSupported(
                        "recipe-book shaped ingredient count does not match dimensions",
                    ));
                }
                buf.write_varint(1);
                buf.write_varint(*width);
                buf.write_varint(*height);
                write_bounded_count(buf, ingredients.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for ingredient in ingredients {
                    ingredient.encode(buf)?;
                }
                result.encode(buf)?;
                crafting_station.encode(buf)?;
            }
            Self::Furnace {
                ingredient,
                fuel,
                result,
                crafting_station,
                duration,
                experience,
            } => {
                if *duration < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book cooking duration",
                    ));
                }
                buf.write_varint(2);
                ingredient.encode(buf)?;
                fuel.encode(buf)?;
                result.encode(buf)?;
                crafting_station.encode(buf)?;
                buf.write_varint(*duration);
                buf.write_f32(*experience);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => {
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                let mut ingredients = Vec::with_capacity(count);
                for _ in 0..count {
                    ingredients.push(RecipeBookSlotDisplay::decode(buf)?);
                }
                Ok(Self::Shapeless {
                    ingredients,
                    result: RecipeBookSlotDisplay::decode(buf)?,
                    crafting_station: RecipeBookSlotDisplay::decode(buf)?,
                })
            }
            1 => {
                let width = buf.read_varint()?;
                let height = buf.read_varint()?;
                let expected = recipe_book_shape_size(width, height)?;
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                if count != expected {
                    return Err(CodecError::NotSupported(
                        "recipe-book shaped ingredient count does not match dimensions",
                    ));
                }
                let mut ingredients = Vec::with_capacity(count);
                for _ in 0..count {
                    ingredients.push(RecipeBookSlotDisplay::decode(buf)?);
                }
                Ok(Self::Shaped {
                    width,
                    height,
                    ingredients,
                    result: RecipeBookSlotDisplay::decode(buf)?,
                    crafting_station: RecipeBookSlotDisplay::decode(buf)?,
                })
            }
            2 => {
                let ingredient = RecipeBookSlotDisplay::decode(buf)?;
                let fuel = RecipeBookSlotDisplay::decode(buf)?;
                let result = RecipeBookSlotDisplay::decode(buf)?;
                let crafting_station = RecipeBookSlotDisplay::decode(buf)?;
                let duration = buf.read_varint()?;
                if duration < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book cooking duration",
                    ));
                }
                Ok(Self::Furnace {
                    ingredient,
                    fuel,
                    result,
                    crafting_station,
                    duration,
                    experience: buf.read_f32()?,
                })
            }
            _ => Err(CodecError::NotSupported(
                "unsupported recipe-book display type",
            )),
        }
    }
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

#[derive(Debug, Clone, PartialEq)]
pub struct RecipeBookEntry {
    pub display_id: i32,
    pub display: RecipeBookDisplay,
    pub group: Option<i32>,
    pub category_id: i32,
    pub crafting_requirements: Option<Vec<RecipeBookIngredient>>,
    pub flags: u8,
}

impl RecipeBookEntry {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.display_id < 0 || self.category_id < 0 {
            return Err(CodecError::NotSupported(
                "negative recipe-book display or category id",
            ));
        }
        buf.write_varint(self.display_id);
        self.display.encode(buf)?;
        match self.group {
            None => buf.write_varint(0),
            Some(group) if (0..i32::MAX).contains(&group) => buf.write_varint(group + 1),
            Some(_) => {
                return Err(CodecError::NotSupported("invalid recipe-book group id"));
            }
        }
        buf.write_varint(self.category_id);
        match &self.crafting_requirements {
            None => buf.write_bool(false),
            Some(requirements) => {
                buf.write_bool(true);
                write_bounded_count(buf, requirements.len(), MAX_RECIPE_BOOK_REQUIREMENTS)?;
                for requirement in requirements {
                    requirement.encode(buf)?;
                }
            }
        }
        buf.write_u8(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let display_id = buf.read_varint()?;
        if display_id < 0 {
            return Err(CodecError::NotSupported("negative recipe-book display id"));
        }
        let display = RecipeBookDisplay::decode(buf)?;
        let group_marker = buf.read_varint()?;
        if group_marker < 0 {
            return Err(CodecError::NegativeLength(group_marker));
        }
        let group = (group_marker != 0).then_some(group_marker - 1);
        let category_id = buf.read_varint()?;
        if category_id < 0 {
            return Err(CodecError::NotSupported("negative recipe-book category id"));
        }
        let crafting_requirements = if buf.read_bool()? {
            let count = read_count(buf, MAX_RECIPE_BOOK_REQUIREMENTS)?;
            let mut requirements = Vec::with_capacity(count);
            for _ in 0..count {
                requirements.push(RecipeBookIngredient::decode(buf)?);
            }
            Some(requirements)
        } else {
            None
        };
        Ok(Self {
            display_id,
            display,
            group,
            category_id,
            crafting_requirements,
            flags: buf.read_u8()?,
        })
    }
}

/// `ClientboundRecipeBookAddPacket` from the local vanilla 26.1.2 jar.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundRecipeBookAdd {
    pub entries: Vec<RecipeBookEntry>,
    pub replace: bool,
}

impl Packet for ClientboundRecipeBookAdd {
    const ID: i32 = 0x4A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_bounded_count(buf, self.entries.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for entry in &self.entries {
            entry.encode(buf)?;
        }
        buf.write_bool(self.replace);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(RecipeBookEntry::decode(buf)?);
        }
        Ok(Self {
            entries,
            replace: buf.read_bool()?,
        })
    }
}

/// `Serverbound Container Close` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundContainerClosePacket` carries one `readContainerId` VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundContainerClose {
    pub container_id: i32,
}

impl Packet for ServerboundContainerClose {
    const ID: i32 = 0x13;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeBookType {
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

impl RecipeBookType {
    const fn as_wire(self) -> i32 {
        match self {
            Self::Crafting => 0,
            Self::Furnace => 1,
            Self::BlastFurnace => 2,
            Self::Smoker => 3,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Crafting,
            1 => Self::Furnace,
            2 => Self::BlastFurnace,
            3 => Self::Smoker,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 3,
                });
            }
        })
    }
}

/// `Serverbound Recipe Book Change Settings` (SB). Verified against the local
/// vanilla 26.1.2 class: `RecipeBookType` enum ordinal, then two booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundRecipeBookChangeSettings {
    pub book_type: RecipeBookType,
    pub is_open: bool,
    pub is_filtering: bool,
}

impl Packet for ServerboundRecipeBookChangeSettings {
    const ID: i32 = 0x2E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.book_type.as_wire());
        buf.write_bool(self.is_open);
        buf.write_bool(self.is_filtering);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            book_type: RecipeBookType::from_wire(buf.read_varint()?)?,
            is_open: buf.read_bool()?,
            is_filtering: buf.read_bool()?,
        })
    }
}

/// `Serverbound Recipe Book Seen Recipe` (SB). Verified against the local
/// vanilla 26.1.2 class: one `RecipeDisplayId` VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundRecipeBookSeenRecipe {
    pub recipe_display_id: i32,
}

impl Packet for ServerboundRecipeBookSeenRecipe {
    const ID: i32 = 0x2F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.recipe_display_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            recipe_display_id: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCommandAction {
    PerformRespawn,
    RequestStats,
    RequestGameruleValues,
}

impl ClientCommandAction {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::PerformRespawn,
            1 => Self::RequestStats,
            2 => Self::RequestGameruleValues,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 2,
                });
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundClientCommand {
    pub action: ClientCommandAction,
}

impl Packet for ServerboundClientCommand {
    // SERVERBOUND_CLIENT_COMMAND is game-SB index 12 = wire id 0x0C;
    // Action enum ordinals are PERFORM_RESPAWN=0, REQUEST_STATS=1,
    // REQUEST_GAMERULE_VALUES=2 per local javap.
    const ID: i32 = 0x0C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.action as i32);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            action: ClientCommandAction::from_wire(buf.read_varint()?)?,
        })
    }
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests;
