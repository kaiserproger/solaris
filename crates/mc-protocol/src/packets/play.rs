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

use super::Packet;
use crate::codec::{Identifier, ReadMc, WriteMc};
use crate::error::CodecError;

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
const MAX_PLAYER_INFO_ENTRIES: usize = 1024;
const MAX_ENTITY_ID_LIST_LEN: usize = 1024;
const MAX_COMMAND_LEN: usize = 32_767;

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
            buf.write_identifier(name);
        }
        buf.write_varint(self.max_players);
        buf.write_varint(self.view_distance);
        buf.write_varint(self.simulation_distance);
        buf.write_bool(self.reduced_debug_info);
        buf.write_bool(self.enable_respawn_screen);
        buf.write_bool(self.do_limited_crafting);
        buf.write_varint(self.dimension_type_id);
        buf.write_identifier(&self.dimension_name);
        buf.write_i64(self.hashed_seed);
        buf.write_u8(self.game_mode);
        buf.write_i8(self.previous_game_mode);
        buf.write_bool(self.is_debug);
        buf.write_bool(self.is_flat);
        match &self.death_location {
            Some((dim, pos)) => {
                buf.write_bool(true);
                buf.write_identifier(dim);
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

/// `Set Default Spawn Position` (CB). Tells the client where its compass
/// should point.
///
/// In 26.1.2 the body is vanilla's `LevelData$RespawnData` record:
/// a `GlobalPos` (= dimension identifier + packed `BlockPos`) plus a
/// yaw and pitch, mapping onto the structured fields below.
/// Per ADR 0002, verified against `javap -p` of the unobfuscated jar:
/// `CLIENTBOUND_SET_DEFAULT_SPAWN_POSITION`.
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
        buf.write_identifier(&self.dimension);
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
                buf.write_varint(0);
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
            let mut listed = false;
            let mut latency = 0;
            let mut game_mode = 0;
            let mut list_order = 0;
            let mut show_hat = false;
            if actions.contains(PlayerInfoActions::ADD_PLAYER) {
                name = buf.read_string(16)?;
                let property_count = read_count(buf, 16)?;
                if property_count != 0 {
                    return Err(CodecError::NotSupported("game profile properties"));
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
        (delta * 4096.0)
            .round()
            .clamp(i16::MIN as f64, i16::MAX as f64) as i16
    }

    #[must_use]
    pub fn pack_degrees(degrees: f32) -> u8 {
        pack_degrees(degrees)
    }
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
/// [DataComponentPatch entries…])`. M6 only emits stacks with zero
/// component patches, so the encoder writes two trailing zero VarInts;
/// the decoder reads them back and refuses non-zero patch counts —
/// full DataComponentPatch handling is M7+.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemStack {
    /// `0` ⇒ empty slot; `count > 0` ⇒ `count` copies of `item_id`.
    pub count: i32,
    /// Item-registry id (the protocol_id from
    /// `data/vanilla/reports/registries.json:minecraft:item`).
    pub item_id: u32,
}

impl ItemStack {
    /// An empty slot.
    pub const EMPTY: ItemStack = ItemStack {
        count: 0,
        item_id: 0,
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }

    #[must_use]
    pub fn new(item_id: u32, count: i32) -> Self {
        Self { count, item_id }
    }

    pub fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.is_empty() {
            buf.write_varint(0);
            return Ok(());
        }
        buf.write_varint(self.count);
        buf.write_varint(self.item_id as i32);
        // No component patches in M6.
        buf.write_varint(0);
        buf.write_varint(0);
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
        if n_add != 0 || n_remove != 0 {
            return Err(CodecError::NotSupported(
                "ItemStack with DataComponentPatch (M7+)",
            ));
        }
        Ok(Self { count, item_id })
    }
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

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<P: Packet + PartialEq + std::fmt::Debug>(p: P) {
        let mut buf = Vec::new();
        p.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded: P = P::decode(&mut cursor).unwrap();
        assert_eq!(decoded, p);
        assert!(cursor.is_empty(), "all bytes consumed");
    }

    fn sample_identifier(s: &str) -> Identifier {
        Identifier::parse(s).unwrap()
    }

    #[test]
    fn login_play_round_trip_minimum() {
        round_trip(LoginPlay {
            entity_id: 1,
            is_hardcore: false,
            dimension_names: vec![sample_identifier("minecraft:overworld")],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            do_limited_crafting: false,
            dimension_type_id: 0,
            dimension_name: sample_identifier("minecraft:overworld"),
            hashed_seed: 0,
            game_mode: 0,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: false,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            enforces_secure_chat: false,
        });
    }

    #[test]
    fn login_play_round_trip_with_death_location() {
        round_trip(LoginPlay {
            entity_id: 42,
            is_hardcore: true,
            dimension_names: vec![
                sample_identifier("minecraft:overworld"),
                sample_identifier("minecraft:the_nether"),
                sample_identifier("minecraft:the_end"),
            ],
            max_players: 100,
            view_distance: 16,
            simulation_distance: 12,
            reduced_debug_info: true,
            enable_respawn_screen: false,
            do_limited_crafting: true,
            dimension_type_id: 2,
            dimension_name: sample_identifier("minecraft:the_nether"),
            hashed_seed: i64::MIN,
            game_mode: 3,
            previous_game_mode: 0,
            is_debug: true,
            is_flat: true,
            death_location: Some((sample_identifier("minecraft:overworld"), 1_234_567_890)),
            portal_cooldown: 100,
            sea_level: 0,
            enforces_secure_chat: true,
        });
    }

    #[test]
    fn set_default_spawn_round_trip() {
        round_trip(SetDefaultSpawnPosition {
            dimension: sample_identifier("minecraft:overworld"),
            position: 0x0000_0FFF_FFFF_FFFF,
            yaw: 1.5,
            pitch: -0.25,
        });
    }

    #[test]
    fn synchronize_player_position_round_trip() {
        round_trip(SynchronizePlayerPosition {
            teleport_id: 1,
            x: 0.5,
            y: 64.0,
            z: -0.5,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            relative_flags: 0,
        });
    }

    #[test]
    fn keepalive_round_trip_both_directions() {
        round_trip(ClientboundKeepAlive {
            id: 0x0123_4567_89AB_CDEF,
        });
        round_trip(ServerboundKeepAlive { id: i64::MIN });
        round_trip(ServerboundKeepAlive { id: 0 });
    }

    #[test]
    fn move_player_ids_match_javap() {
        assert_eq!(ServerboundMovePlayerPos::ID, 0x1E);
        assert_eq!(ServerboundMovePlayerPosRot::ID, 0x1F);
        assert_eq!(ServerboundMovePlayerRot::ID, 0x20);
        assert_eq!(ServerboundMovePlayerStatusOnly::ID, 0x21);
    }

    #[test]
    fn move_player_packets_round_trip() {
        let flags = MovePlayerFlags::new(true, true);
        round_trip(ServerboundMovePlayerPos {
            x: 16.5,
            y: -58.0,
            z: -0.25,
            flags,
        });
        round_trip(ServerboundMovePlayerPosRot {
            x: -16.5,
            y: 70.0,
            z: 32.25,
            yaw: 180.0,
            pitch: -20.0,
            flags,
        });
        round_trip(ServerboundMovePlayerRot {
            yaw: 45.0,
            pitch: 10.0,
            flags,
        });
        round_trip(ServerboundMovePlayerStatusOnly { flags });
    }

    #[test]
    fn move_player_flags_use_low_two_bits() {
        let packet = ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, true),
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0x03]);

        let mut cursor: &[u8] = &[0x02];
        let decoded = ServerboundMovePlayerStatusOnly::decode(&mut cursor).unwrap();
        assert_eq!(decoded.flags, MovePlayerFlags::new(false, true));
    }

    #[test]
    fn confirm_teleportation_round_trip() {
        round_trip(ConfirmTeleportation { teleport_id: 1 });
    }

    #[test]
    fn play_disconnect_carries_opaque_nbt_bytes() {
        // We are not parsing NBT inside the packet, just shuttling it.
        round_trip(PlayDisconnect {
            reason_nbt: vec![0x0A, 0x00, 0x08, b'r', b'e', b'a', b's', b'o', b'n', 0x00],
        });
    }

    #[test]
    fn forget_level_chunk_id_matches_javap() {
        assert_eq!(ForgetLevelChunk::ID, 0x25);
    }

    #[test]
    fn forget_level_chunk_round_trips() {
        for (x, z) in [(0, 0), (1, -1), (-100_000, 100_000), (i32::MIN, i32::MAX)] {
            round_trip(ForgetLevelChunk {
                chunk_x: x,
                chunk_z: z,
            });
        }
    }

    #[test]
    fn forget_level_chunk_wire_layout_uses_chunk_pos_long() {
        let packet = ForgetLevelChunk {
            chunk_x: -1,
            chunk_z: 2,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0, 0, 0, 2, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn game_event_round_trip() {
        round_trip(GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        });
    }

    #[test]
    fn player_visible_packet_ids_match_javap() {
        assert_eq!(AddEntity::ID, 0x01);
        assert_eq!(EntityPositionSync::ID, 0x23);
        assert_eq!(PlayerInfoRemove::ID, 0x45);
        assert_eq!(PlayerInfoUpdate::ID, 0x46);
        assert_eq!(RemoveEntities::ID, 0x4D);
        assert_eq!(RotateHead::ID, 0x53);
    }

    #[test]
    fn player_info_update_minimal_add_player_wire_layout() {
        let uuid = Uuid::from_u128(0x00112233445566778899aabbccddeeff);
        let packet = PlayerInfoUpdate {
            actions: PlayerInfoActions::minimal_add_player(),
            entries: vec![PlayerInfoEntry {
                profile_id: uuid,
                name: "Steve".to_string(),
                listed: true,
                latency: 0,
                game_mode: 0,
                list_order: 0,
                show_hat: true,
            }],
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf[0], 0xFD);
        assert_eq!(buf[1], 0x01);
        assert_eq!(&buf[2..18], uuid.as_bytes());
        assert_eq!(&buf[18..24], &[5, b'S', b't', b'e', b'v', b'e']);
        assert_eq!(&buf[24..], &[0, 0, 1, 0, 0, 0, 1]);

        let mut cursor: &[u8] = &buf;
        let decoded = PlayerInfoUpdate::decode(&mut cursor).unwrap();
        assert_eq!(decoded, packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn add_player_entity_zero_motion_round_trips() {
        round_trip(AddEntity {
            entity_id: 42,
            uuid: Uuid::from_u128(0x00112233445566778899aabbccddeeff),
            entity_type_id: 155,
            x: 0.5,
            y: -59.0,
            z: 0.5,
            movement: EntityVec3::ZERO,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        });
    }

    #[test]
    fn entity_position_sync_round_trips() {
        round_trip(EntityPositionSync {
            entity_id: 42,
            values: PositionMoveRotation {
                position: EntityVec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                delta_movement: EntityVec3::ZERO,
                yaw: 90.0,
                pitch: -15.0,
            },
            on_ground: true,
        });
    }

    #[test]
    fn entity_animation_id_and_wire_layout_match_server_javap() {
        assert_eq!(EntityAnimation::ID, 0x02);
        let packet = EntityAnimation {
            entity_id: 300,
            action: EntityAnimationAction::SwingMainHand,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0xAC, 0x02, 0x00]);

        let mut cursor: &[u8] = &buf;
        assert_eq!(EntityAnimation::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn entity_event_id_and_wire_layout_match_server_javap() {
        assert_eq!(EntityEvent::ID, 0x22);
        let packet = EntityEvent {
            entity_id: 0x0102_0304,
            event_id: -1,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0x02, 0x03, 0x04, 0xFF]);

        let mut cursor: &[u8] = &buf;
        assert_eq!(EntityEvent::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn move_entity_pos_rot_id_and_wire_layout_match_server_javap() {
        assert_eq!(MoveEntityPosRot::ID, 0x36);
        let packet = MoveEntityPosRot {
            entity_id: 300,
            delta_x: 4,
            delta_y: -8,
            delta_z: 12,
            yaw: 64,
            pitch: 250,
            on_ground: true,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0xAC, 0x02, 0x00, 0x04, 0xFF, 0xF8, 0x00, 0x0C, 0x40, 0xFA, 0x01
            ]
        );

        let mut cursor: &[u8] = &buf;
        assert_eq!(MoveEntityPosRot::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn move_entity_delta_scales_to_vanilla_short_units() {
        assert_eq!(MoveEntityPosRot::delta_to_short(1.0 / 4096.0), 1);
        assert_eq!(MoveEntityPosRot::delta_to_short(-0.5), -2048);
        assert_eq!(MoveEntityPosRot::pack_degrees(90.0), 64);
    }

    #[test]
    fn set_entity_motion_id_and_zero_motion_layout_match_server_javap() {
        assert_eq!(SetEntityMotion::ID, 0x65);
        let packet = SetEntityMotion {
            entity_id: 5,
            movement: EntityVec3::ZERO,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0x05, 0x00]);

        let mut cursor: &[u8] = &buf;
        assert_eq!(SetEntityMotion::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn set_entity_motion_round_trips_non_zero_lp_vec3() {
        let packet = SetEntityMotion {
            entity_id: 42,
            movement: EntityVec3 {
                x: 0.1,
                y: -0.2,
                z: 0.3,
            },
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded = SetEntityMotion::decode(&mut cursor).unwrap();
        assert_eq!(decoded.entity_id, packet.entity_id);
        assert!((decoded.movement.x - packet.movement.x).abs() < 0.000_1);
        assert!((decoded.movement.y - packet.movement.y).abs() < 0.000_1);
        assert!((decoded.movement.z - packet.movement.z).abs() < 0.000_1);
        assert!(cursor.is_empty());
    }

    #[test]
    fn serverbound_change_game_mode_id_and_layout_match_javap() {
        assert_eq!(ServerboundChangeGameMode::ID, 0x05);
        let packet = ServerboundChangeGameMode {
            mode: GameMode::Creative,
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01]);

        let mut cursor: &[u8] = &buf;
        assert_eq!(
            ServerboundChangeGameMode::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn serverbound_chat_command_id_and_layout_match_javap() {
        assert_eq!(ServerboundChatCommand::ID, 0x07);
        let packet = ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(buf[0], 17);

        let mut cursor: &[u8] = &buf;
        assert_eq!(ServerboundChatCommand::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn remove_player_packets_round_trip() {
        let uuid = Uuid::from_u128(0x00112233445566778899aabbccddeeff);
        round_trip(RemoveEntities {
            entity_ids: vec![42, 43],
        });
        round_trip(PlayerInfoRemove {
            profile_ids: vec![uuid],
        });
        let mut buf = Vec::new();
        RotateHead {
            entity_id: 42,
            head_yaw: 90.0,
        }
        .encode(&mut buf)
        .unwrap();
        assert_eq!(buf, vec![42, 64]);
    }

    // ---- SetCenterChunk ----

    #[test]
    fn set_center_chunk_id_matches_javap() {
        assert_eq!(SetCenterChunk::ID, 0x5E);
    }

    #[test]
    fn set_center_chunk_round_trips() {
        for (x, z) in [(0, 0), (1, -1), (-100_000, 100_000), (i32::MIN, i32::MAX)] {
            round_trip(SetCenterChunk {
                chunk_x: x,
                chunk_z: z,
            });
        }
    }

    // ---- LevelChunkWithLight ----

    fn minimal_chunk_packet() -> LevelChunkWithLight {
        LevelChunkWithLight {
            chunk_x: 0,
            chunk_z: 0,
            heightmaps: Vec::new(),
            data: Vec::new(),
            block_entities: Vec::new(),
            light: LightData::empty(),
        }
    }

    #[test]
    fn level_chunk_with_light_id_matches_javap() {
        // 0x2D = game-CB index 45 (CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT).
        assert_eq!(LevelChunkWithLight::ID, 0x2D);
    }

    #[test]
    fn level_chunk_with_light_empty_byte_layout() {
        // The all-empty form: i32 x, i32 z, six VarInt(0)s for
        // (heightmap-count, data-len, block-entity-count, four BitSets),
        // two VarInt(0)s for the sky/block update lists.
        // Total: 4 + 4 + 9*1 = 17 bytes.
        let mut buf = Vec::new();
        minimal_chunk_packet().encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0, 0, 0, 0, // chunk_x = 0
                0, 0, 0, 0, // chunk_z = 0
                0, // heightmap count = 0
                0, // chunk data length = 0
                0, // block entity count = 0
                0, // sky_y_mask longs = 0
                0, // block_y_mask longs = 0
                0, // empty_sky_y_mask longs = 0
                0, // empty_block_y_mask longs = 0
                0, // sky_updates count = 0
                0, // block_updates count = 0
            ]
        );
    }

    #[test]
    fn level_chunk_with_light_round_trips_empty() {
        round_trip(minimal_chunk_packet());
    }

    #[test]
    fn level_chunk_with_light_round_trips_with_heightmaps_and_data() {
        round_trip(LevelChunkWithLight {
            chunk_x: -3,
            chunk_z: 7,
            heightmaps: vec![
                ChunkHeightmap {
                    type_id: ChunkHeightmap::MOTION_BLOCKING,
                    data: vec![0x0123_4567_89AB_CDEF, 0],
                },
                ChunkHeightmap {
                    type_id: ChunkHeightmap::WORLD_SURFACE,
                    data: vec![-1; 4],
                },
            ],
            data: (0..512).map(|i| (i & 0xFF) as u8).collect(),
            block_entities: Vec::new(),
            light: LightData::empty(),
        });
    }

    #[test]
    fn level_chunk_with_light_round_trips_with_non_empty_light_masks() {
        round_trip(LevelChunkWithLight {
            chunk_x: 0,
            chunk_z: 0,
            heightmaps: Vec::new(),
            data: Vec::new(),
            block_entities: Vec::new(),
            light: LightData {
                // All 26 indexable Y sections marked "has data".
                sky_y_mask: vec![(1i64 << 26) - 1],
                block_y_mask: vec![(1i64 << 26) - 1],
                empty_sky_y_mask: Vec::new(),
                empty_block_y_mask: Vec::new(),
                sky_updates: vec![vec![0xFFu8; LightData::LIGHT_LAYER_BYTES]; 26],
                block_updates: vec![vec![0u8; LightData::LIGHT_LAYER_BYTES]; 26],
            },
        });
    }

    #[test]
    fn level_chunk_with_light_round_trips_with_block_entities() {
        // Network-NBT compound with one byte tag inside, modelling a
        // (toy) block entity payload.
        let nbt = mc_nbt::Tag::Compound(vec![("k".to_string(), mc_nbt::Tag::Byte(42))]);
        round_trip(LevelChunkWithLight {
            chunk_x: 4,
            chunk_z: -8,
            heightmaps: Vec::new(),
            data: Vec::new(),
            block_entities: vec![BlockEntityInfo {
                packed_xz: (3 << 4) | 9,
                y: 64,
                type_id: 7,
                nbt,
            }],
            light: LightData::empty(),
        });
    }

    #[test]
    fn level_chunk_with_light_rejects_oversized_chunk_data_on_decode() {
        // Hand-encode an i32(0), i32(0), heightmap-count VarInt(0), then
        // a VarInt declaring (MAX_CHUNK_DATA_LEN + 1) bytes of chunk
        // data. Decode should reject before allocating.
        let mut buf = Vec::new();
        buf.extend_from_slice(&0i32.to_be_bytes());
        buf.extend_from_slice(&0i32.to_be_bytes());
        buf.write_varint(0);
        buf.write_varint((MAX_CHUNK_DATA_LEN + 1) as i32);
        let mut cursor: &[u8] = &buf;
        let err = LevelChunkWithLight::decode(&mut cursor).unwrap_err();
        assert!(matches!(err, CodecError::StringTooLong { .. }));
    }

    // ---- M5.a: serverbound interaction packets ----

    #[test]
    fn block_pos_pack_round_trips_positive_and_negative_coords() {
        for &(x, y, z) in &[
            (0, 0, 0),
            (1, 2, 3),
            (-1, -1, -1),
            (100, 64, -100),
            (i32::MAX >> 6, 2047, i32::MIN >> 6), // edges of 26-bit signed
            (0, -2048, 0),                        // edge of 12-bit signed
        ] {
            let packed = pack_block_pos(x, y, z);
            let (rx, ry, rz) = unpack_block_pos(packed);
            assert_eq!(
                (rx, ry, rz),
                (x, y, z),
                "round trip failed for ({x}, {y}, {z})"
            );
        }
    }

    #[test]
    fn serverbound_player_action_round_trip() {
        round_trip(ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(10, -60, -7),
            direction: Direction::Up,
            sequence: 1,
        });
        round_trip(ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: pack_block_pos(0, 200, 0),
            direction: Direction::North,
            sequence: 12345,
        });
    }

    #[test]
    fn serverbound_player_action_rejects_out_of_range_action() {
        // VarInt(8) — one past the last variant.
        let mut buf = Vec::new();
        buf.write_varint(8);
        buf.write_i64(0);
        buf.write_varint(0);
        buf.write_varint(0);
        let mut cursor: &[u8] = &buf;
        let err = ServerboundPlayerAction::decode(&mut cursor).unwrap_err();
        assert!(matches!(err, CodecError::StringTooLong { .. }));
    }

    #[test]
    fn serverbound_use_item_on_round_trip() {
        round_trip(ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(3, -60, 4),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.25,
            inside: false,
            world_border_hit: false,
            sequence: 7,
        });
        round_trip(ServerboundUseItemOn {
            hand: InteractionHand::OffHand,
            position: pack_block_pos(-1, 70, -1),
            direction: Direction::East,
            cursor_x: 0.0,
            cursor_y: 0.0,
            cursor_z: 0.0,
            inside: true,
            world_border_hit: true,
            sequence: 99,
        });
    }

    #[test]
    fn direction_normal_matches_vanilla_axes() {
        assert_eq!(Direction::Down.normal(), (0, -1, 0));
        assert_eq!(Direction::Up.normal(), (0, 1, 0));
        assert_eq!(Direction::North.normal(), (0, 0, -1));
        assert_eq!(Direction::South.normal(), (0, 0, 1));
        assert_eq!(Direction::West.normal(), (-1, 0, 0));
        assert_eq!(Direction::East.normal(), (1, 0, 0));
    }

    // ---- M5.b: clientbound edit / ack / relight packets ----

    #[test]
    fn block_update_round_trip() {
        round_trip(BlockUpdate {
            position: pack_block_pos(0, -60, 0),
            state_id: 1, // stone in our test registry
        });
        round_trip(BlockUpdate {
            position: pack_block_pos(-7, 200, 3),
            state_id: 29_872,
        });
    }

    #[test]
    fn section_blocks_update_id_matches_javap() {
        assert_eq!(SectionBlocksUpdate::ID, 0x54);
    }

    #[test]
    fn section_pos_packs_negative_coords_like_vanilla() {
        let packed = pack_section_pos(-1, -2, 3);
        assert_eq!(packed, -4_398_042_316_802);
        assert_eq!(
            packed.to_be_bytes(),
            [0xFF, 0xFF, 0xFC, 0, 0, 0x3F, 0xFF, 0xFE]
        );
    }

    #[test]
    fn section_relative_pos_uses_xzy_nibbles() {
        assert_eq!(pack_section_relative_pos(0, 0, 0), 0);
        assert_eq!(pack_section_relative_pos(1, 2, 3), 0x0132);
        assert_eq!(pack_section_relative_pos(-1, -2, -3), 0x0FDE);
    }

    #[test]
    fn section_blocks_update_wire_layout() {
        let packet = SectionBlocksUpdate {
            section_pos: pack_section_pos(-1, -2, 3),
            changes: vec![SectionBlockChange {
                relative_pos: pack_section_relative_pos(1, 2, 3),
                state_id: 1,
            }],
        };

        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0xFF, 0xFF, 0xFC, 0, 0, 0x3F, 0xFF, 0xFE, // SectionPos.asLong
                0x01, // count
                0xB2, 0x22, // VarLong((1 << 12) | 0x132)
            ]
        );

        let mut cursor: &[u8] = &buf;
        assert_eq!(SectionBlocksUpdate::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn section_blocks_update_round_trip_multiple_entries() {
        round_trip(SectionBlocksUpdate {
            section_pos: pack_section_pos(12, -4, -9),
            changes: vec![
                SectionBlockChange {
                    relative_pos: pack_section_relative_pos(0, 0, 0),
                    state_id: 0,
                },
                SectionBlockChange {
                    relative_pos: pack_section_relative_pos(15, 15, 15),
                    state_id: 29_872,
                },
            ],
        });
    }

    #[test]
    fn block_changed_ack_round_trip() {
        round_trip(BlockChangedAck { sequence: 0 });
        round_trip(BlockChangedAck { sequence: 1 });
        round_trip(BlockChangedAck { sequence: i32::MAX });
    }

    #[test]
    fn light_update_round_trip_empty() {
        round_trip(LightUpdate {
            chunk_x: 0,
            chunk_z: 0,
            light: LightData::empty(),
        });
    }

    #[test]
    fn light_update_round_trip_with_layers() {
        // Use the same shape as the existing LightData non-empty
        // round-trip test in this module: one full-bright layer per
        // section + an empty-mask-clearing zero across all 26 slots.
        let sky_layer = vec![0xFFu8; LightData::LIGHT_LAYER_BYTES];
        let block_layer = vec![0u8; LightData::LIGHT_LAYER_BYTES];
        let light = LightData {
            sky_y_mask: vec![(1 << 26) - 1],
            block_y_mask: vec![(1 << 26) - 1],
            empty_sky_y_mask: vec![0],
            empty_block_y_mask: vec![0],
            sky_updates: vec![sky_layer; 26],
            block_updates: vec![block_layer; 26],
        };
        round_trip(LightUpdate {
            chunk_x: -3,
            chunk_z: 7,
            light,
        });
    }

    #[test]
    fn item_stack_empty_round_trips_as_single_zero_byte() {
        let mut buf = Vec::new();
        ItemStack::EMPTY.encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0u8]);
        let mut cur: &[u8] = &buf;
        assert_eq!(ItemStack::decode(&mut cur).unwrap(), ItemStack::EMPTY);
        assert!(cur.is_empty());
    }

    #[test]
    fn item_stack_non_empty_round_trips() {
        let stone = ItemStack::new(1, 64);
        let mut buf = Vec::new();
        stone.encode(&mut buf).unwrap();
        // count=64 (0x40), item_id=1, n_add=0, n_remove=0.
        assert_eq!(buf, vec![0x40, 0x01, 0x00, 0x00]);
        let mut cur: &[u8] = &buf;
        assert_eq!(ItemStack::decode(&mut cur).unwrap(), stone);
    }

    #[test]
    fn item_stack_decoder_refuses_component_patches() {
        // count=1, item_id=1, n_add=1 (unsupported), …
        let bytes: Vec<u8> = vec![0x01, 0x01, 0x01, 0x00];
        let mut cur: &[u8] = &bytes;
        let err = ItemStack::decode(&mut cur).unwrap_err();
        assert!(matches!(err, CodecError::NotSupported(_)));
    }

    #[test]
    fn set_held_slot_round_trip() {
        round_trip(ClientboundSetHeldSlot { slot: 0 });
        round_trip(ClientboundSetHeldSlot { slot: 3 });
    }

    #[test]
    fn container_set_content_round_trip_starter_kit() {
        let mut items = vec![ItemStack::EMPTY; 46];
        // Slot 36 = hotbar slot 0.
        items[36] = ItemStack::new(1, 64); // stone
        items[37] = ItemStack::new(28, 64); // dirt
        items[38] = ItemStack::new(36, 64); // oak_planks
        items[39] = ItemStack::new(323, 64); // torch
        round_trip(ClientboundContainerSetContent {
            container_id: 0,
            state_id: 1,
            items,
            carried_item: ItemStack::EMPTY,
        });
    }

    #[test]
    fn container_set_slot_round_trip() {
        round_trip(ClientboundContainerSetSlot {
            container_id: 0,
            state_id: 5,
            slot: 36,
            item_stack: ItemStack::new(28, 63),
        });
        round_trip(ClientboundContainerSetSlot {
            container_id: 0,
            state_id: 6,
            slot: 36,
            item_stack: ItemStack::EMPTY,
        });
    }

    #[test]
    fn set_carried_item_round_trip() {
        round_trip(ServerboundSetCarriedItem { slot: 0 });
        round_trip(ServerboundSetCarriedItem { slot: 8 });
    }
}
