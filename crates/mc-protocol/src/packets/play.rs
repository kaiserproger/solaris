//! Play state — the bulk of the protocol surface.
//!
//! M1.g.2 scope: just enough packet types for the M1.g.3 handler to
//! send `Login (Play)` → `Set Default Spawn Position` →
//! `Synchronize Player Position`, then run a KeepAlive loop.
//!
//! **Packet IDs are placeholders.** They are based on the modern
//! protocol's general layout and are tagged `TODO(M1.g.4)` until they
//! can be validated against a wire capture from the bundled vanilla
//! 26.1.2 server. The on-wire field layouts are believed to be correct
//! since protocol 770; only the leading discriminator may shift.

use bytes::{Buf, BufMut};

use super::Packet;
use crate::codec::{Identifier, ReadMc, WriteMc};
use crate::error::CodecError;

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetDefaultSpawnPosition {
    /// Block position packed into an `i64` per the vanilla
    /// `BlockPos` format: `((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
    /// We carry the packed form rather than a struct so the codec is
    /// trivially round-trippable; helper accessors live alongside in
    /// `mc-world` when that lands.
    pub position: i64,
    pub angle: f32,
}

impl Packet for SetDefaultSpawnPosition {
    // TODO(M1.g.4): verify against wire capture.
    const ID: i32 = 0x5C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_f32(self.angle);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            angle: buf.read_f32()?,
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
    // TODO(M1.g.4): verify against wire capture.
    const ID: i32 = 0x27;

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
    // TODO(M1.g.4): verify against wire capture.
    const ID: i32 = 0x1D;

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
    // TODO(M1.g.4): verify against wire capture.
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
    // TODO(M1.g.4): verify against wire capture.
    const ID: i32 = 0x1A;

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
            position: 0x0000_0FFF_FFFF_FFFF,
            angle: 1.5,
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
}
