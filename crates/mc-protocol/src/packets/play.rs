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
    fn game_event_round_trip() {
        round_trip(GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        });
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
}
