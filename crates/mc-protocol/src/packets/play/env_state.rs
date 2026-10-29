//! Play environment-state packets: world clock, difficulty, borders, abilities, explosions.

use super::*;

/// Entity-attached sound with a direct holder (no sound registry-id coupling).
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSoundEntity {
    pub sound: Identifier,
    pub fixed_range: Option<f32>,
    pub source: i32,
    pub entity_id: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

impl Packet for ClientboundSoundEntity {
    // GameProtocols.CLIENTBOUND_SOUND_ENTITY, registration index 116.
    // Direct SoundEvent holder: 0, identifier, optional fixed range.
    const ID: i32 = 0x76;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(0);
        buf.write_identifier(&self.sound)?;
        buf.write_bool(self.fixed_range.is_some());
        if let Some(range) = self.fixed_range {
            buf.write_f32(range);
        }
        buf.write_varint(self.source);
        buf.write_varint(self.entity_id);
        buf.write_f32(self.volume);
        buf.write_f32(self.pitch);
        buf.write_i64(self.seed);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        if buf.read_varint()? != 0 {
            return Err(CodecError::NotSupported("registry sound holder"));
        }
        let sound = buf.read_identifier()?;
        let fixed_range = if buf.read_bool()? {
            Some(buf.read_f32()?)
        } else {
            None
        };
        Ok(Self {
            sound,
            fixed_range,
            source: buf.read_varint()?,
            entity_id: buf.read_varint()?,
            volume: buf.read_f32()?,
            pitch: buf.read_f32()?,
            seed: buf.read_i64()?,
        })
    }
}

/// Positional sound with a direct holder. Coordinates are fixed-point eighths.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSound {
    pub sound: Identifier,
    pub fixed_range: Option<f32>,
    pub source: i32,
    pub position: [i32; 3],
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

impl Packet for ClientboundSound {
    // 26.1.2 GameProtocols: CLIENTBOUND_SOUND follows SOUND_ENTITY (116).
    // ClientboundSoundPacket.write: holder, source, xyz ints, volume, pitch, seed.
    const ID: i32 = 0x77;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(0);
        buf.write_identifier(&self.sound)?;
        buf.write_bool(self.fixed_range.is_some());
        if let Some(range) = self.fixed_range {
            buf.write_f32(range);
        }
        buf.write_varint(self.source);
        for coordinate in self.position {
            buf.write_i32(coordinate);
        }
        buf.write_f32(self.volume);
        buf.write_f32(self.pitch);
        buf.write_i64(self.seed);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        if buf.read_varint()? != 0 {
            return Err(CodecError::NotSupported("registry sound holder"));
        }
        let sound = buf.read_identifier()?;
        let fixed_range = if buf.read_bool()? {
            Some(buf.read_f32()?)
        } else {
            None
        };
        Ok(Self {
            sound,
            fixed_range,
            source: buf.read_varint()?,
            position: [buf.read_i32()?, buf.read_i32()?, buf.read_i32()?],
            volume: buf.read_f32()?,
            pitch: buf.read_f32()?,
            seed: buf.read_i64()?,
        })
    }
}
/// Parameterless particle with command-specified distribution (26.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundCommandParticle {
    pub particle_id: i32,
    pub position: [f64; 3],
    pub delta: [f32; 3],
    pub speed: f32,
    pub count: i32,
    pub force: bool,
}

impl Packet for ClientboundCommandParticle {
    const ID: i32 = 0x30;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.particle_id);
        buf.write_bool(self.force);
        buf.write_bool(self.force);
        for coordinate in self.position {
            buf.write_f64(coordinate);
        }
        for coordinate in self.delta {
            buf.write_f32(coordinate);
        }
        for _ in 0..3 {
            buf.write_f32(self.speed);
        }
        buf.write_varint(self.count);
        buf.write_varint(0);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let particle_id = buf.read_varint()?;
        let force = buf.read_bool()?;
        let _always_show = buf.read_bool()?;
        let position = [buf.read_f64()?, buf.read_f64()?, buf.read_f64()?];
        let delta = [buf.read_f32()?, buf.read_f32()?, buf.read_f32()?];
        let speed = buf.read_f32()?;
        if buf.read_f32()? != speed || buf.read_f32()? != speed {
            return Err(CodecError::NotSupported("asymmetric particle speed"));
        }
        let count = buf.read_varint()?;
        if buf.read_varint()? != 0 {
            return Err(CodecError::NotSupported("particle randomization"));
        }
        Ok(Self {
            particle_id,
            position,
            delta,
            speed,
            count,
            force,
        })
    }
}

/// Vanilla parameterless sweep particle (26.3 `minecraft:sweep_attack` = 77).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundSweepParticle {
    pub position: [f64; 3],
    pub direction: [f32; 3],
}

/// `minecraft:particle_type` id of `minecraft:sweep_attack` in 26.3.
const PARTICLE_TYPE_SWEEP_ATTACK: i32 = 77;

impl Packet for ClientboundSweepParticle {
    // 26.3 `ClientboundLevelParticlesPacket.STREAM_CODEC`: particle first,
    // overrideLimiter, alwaysShow, x/y/z, x/y/z dist, x/y/z max speed,
    // VarInt count, VarInt randomization type (0 = default).
    const ID: i32 = 0x30;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(PARTICLE_TYPE_SWEEP_ATTACK);
        buf.write_bool(false);
        buf.write_bool(false);
        for coordinate in self.position {
            buf.write_f64(coordinate);
        }
        for direction in self.direction {
            buf.write_f32(direction);
        }
        for _ in 0..3 {
            buf.write_f32(0.0);
        }
        buf.write_varint(0);
        buf.write_varint(0);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        if buf.read_varint()? != PARTICLE_TYPE_SWEEP_ATTACK {
            return Err(CodecError::NotSupported("non-sweep particle"));
        }
        if buf.read_bool()? || buf.read_bool()? {
            return Err(CodecError::NotSupported("forced sweep particle"));
        }
        let position = [buf.read_f64()?, buf.read_f64()?, buf.read_f64()?];
        let direction = [buf.read_f32()?, buf.read_f32()?, buf.read_f32()?];
        let speeds = [buf.read_f32()?, buf.read_f32()?, buf.read_f32()?];
        if speeds != [0.0; 3] || buf.read_varint()? != 0 || buf.read_varint()? != 0 {
            return Err(CodecError::NotSupported("non-sweep particle"));
        }
        Ok(Self {
            position,
            direction,
        })
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
    /// 26.3 `ClientboundExplodePacket.playSound` (trailing bool).
    pub play_sound: bool,
}

impl Packet for ClientboundExplode {
    // 26.3 `CLIENTBOUND_EXPLODE` (`packets.json` play clientbound 36).
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
        buf.write_bool(self.play_sound);
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
        let block_particles = read_bounded_vec(buf, MAX_EXPLOSION_BLOCK_PARTICLES, 10, |buf| {
            let particle_id = validate_simple_explosion_particle_id(buf.read_varint()?)?;
            let scaling = buf.read_f32()?;
            let speed = buf.read_f32()?;
            let weight = buf.read_varint()?;
            if weight < 0 {
                return Err(CodecError::NotSupported(
                    "negative explosion block particle weight",
                ));
            }
            Ok(ExplosionBlockParticle {
                particle_id,
                scaling,
                speed,
                weight,
            })
        })?;
        let play_sound = buf.read_bool()?;
        Ok(Self {
            center,
            radius,
            block_count,
            knockback,
            explosion_particle_id,
            sound_reference_id,
            block_particles,
            play_sound,
        })
    }
}

/// `Game Event` (CB). Vanilla uses this packet for client-global state transitions
/// including weather, game-mode changes, and the post-login level-chunk loading fence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    pub event: u8,
    pub value: f32,
}

impl GameEvent {
    pub const EVENT_START_RAINING: u8 = 1;
    pub const EVENT_STOP_RAINING: u8 = 2;
    pub const EVENT_CHANGE_GAME_MODE: u8 = 3;
    pub const EVENT_RAIN_LEVEL_CHANGE: u8 = 7;
    pub const EVENT_THUNDER_LEVEL_CHANGE: u8 = 8;
    pub const EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
}

impl Packet for GameEvent {
    // Verified against vanilla 26.1.2 wire capture: id 0x26, body
    // `0d 00 00 00 00` = event 13 (start_waiting_for_chunks), value 0.0.
    const ID: i32 = 0x27;

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
    const ID: i32 = 0x2C;

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
    const ID: i32 = 0x73;

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
    const ID: i32 = 0x6A;

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
    const ID: i32 = 0x69;

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
    const ID: i32 = 0x41;

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
        let mut migrated;
        let nbt = if self.block_entity_type
            == mc_data::block_entity_kinds::BlockEntityKind::DecoratedPot.protocol_id() as i32
            && let Tag::Compound(fields) = &self.nbt
            && matches!(
                mc_data::item_nbt::field(fields, "sherds"),
                Some(Tag::List(_))
            ) {
            migrated = self.nbt.clone();
            let Tag::Compound(fields) = &mut migrated else {
                unreachable!()
            };
            mc_data::item_nbt::upgrade_pot_decorations(
                &mut fields
                    .iter_mut()
                    .find(|(key, _)| key == "sherds")
                    .unwrap()
                    .1,
            )
            .map_err(|_| CodecError::NotSupported("invalid decorated pot sherds"))?;
            &migrated
        } else {
            &self.nbt
        };
        mc_nbt::write_network(buf, nbt)?;
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

/// `ClientboundBlockEventPacket` (CB), used by containers to synchronize lid
/// animation state without changing the block state itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundBlockEvent {
    pub position: i64,
    pub action: u8,
    pub parameter: u8,
    pub block_type: i32,
}

impl Packet for ClientboundBlockEvent {
    // The cached 26.1.2 packet report assigns minecraft:block_event game-CB
    // id 0x07. Local javap of ClientboundBlockEventPacket.write confirms:
    // BlockPos, byte b0, byte b1, then BLOCK registry VarInt.
    const ID: i32 = 0x07;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_u8(self.action);
        buf.write_u8(self.parameter);
        buf.write_varint(self.block_type);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            action: buf.read_u8()?,
            parameter: buf.read_u8()?,
            block_type: buf.read_varint()?,
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
    const ID: i32 = 0x3D;

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

/// `ClientboundBlockDestructionPacket` (CB): per-breaker block-break crack
/// animation stage (MOBS-06 zombie door banging: stage 0-9 while banging,
/// clearing when the chain breaks). `progress` 0-10 on the wire where 10+
/// clears; Solaris only sends 0-9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundBlockDestruction {
    pub breaker_entity_id: i32,
    pub position: i64,
    pub progress: u8,
}

impl Packet for ClientboundBlockDestruction {
    // `data/vanilla/reports/packets.json` assigns minecraft:block_destruction
    // game-CB id 0x05. Local javap of ClientboundBlockDestructionPacket shows
    // VarInt breaker id, BlockPos, then one unsigned-byte progress.
    const ID: i32 = 0x05;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.breaker_entity_id);
        buf.write_i64(self.position);
        buf.write_u8(self.progress);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            breaker_entity_id: buf.read_varint()?,
            position: buf.read_i64()?,
            progress: buf.read_u8()?,
        })
    }
}

#[cfg(test)]
mod shield_sound_tests {
    use super::*;

    #[test]
    fn direct_sound_entity_holder_matches_vanilla_sound_event_stream_codec() {
        for fixed_range in [None, Some(16.0)] {
            let packet = ClientboundSoundEntity {
                sound: Identifier::parse("minecraft:item.shield.block").unwrap(),
                fixed_range,
                source: 7,
                entity_id: 41,
                volume: 1.0,
                pitch: 0.9,
                seed: 123,
            };
            let mut bytes = Vec::new();
            packet.encode(&mut bytes).unwrap();
            let mut wire = &bytes[..];
            assert_eq!(
                wire.read_varint().unwrap(),
                0,
                "direct holder discriminator"
            );
            assert_eq!(wire.read_identifier().unwrap(), packet.sound);
            assert_eq!(wire.read_bool().unwrap(), fixed_range.is_some());
            if let Some(range) = fixed_range {
                assert_eq!(wire.read_f32().unwrap(), range);
            }
            assert_eq!(wire.read_varint().unwrap(), 7);
            assert_eq!(wire.read_varint().unwrap(), 41);
            assert_eq!(wire.read_f32().unwrap(), 1.0);
            assert_eq!(wire.read_f32().unwrap(), 0.9);
            assert_eq!(wire.read_i64().unwrap(), 123);
            assert!(wire.is_empty());
            assert_eq!(
                ClientboundSoundEntity::decode(&mut &bytes[..]).unwrap(),
                packet
            );
        }
    }
}

#[cfg(test)]
mod positional_sound_tests {
    use super::*;

    #[test]
    fn bell_sound_matches_verified_positional_wire_layout() {
        for fixed_range in [None, Some(32.0)] {
            let packet = ClientboundSound {
                sound: Identifier::parse("minecraft:block.bell.use").unwrap(),
                fixed_range,
                source: 4,
                position: [-60, 1612, 68],
                volume: 2.0,
                pitch: 1.0,
                seed: 42,
            };
            let mut bytes = Vec::new();
            packet.encode(&mut bytes).unwrap();
            let mut expected = vec![0, 24];
            expected.extend_from_slice(b"minecraft:block.bell.use");
            // Identifier is 24 bytes, then optional range and SoundSource.BLOCKS.
            expected.push(u8::from(fixed_range.is_some()));
            if let Some(range) = fixed_range {
                expected.extend_from_slice(&range.to_be_bytes());
            }
            expected.push(4);
            for coordinate in packet.position {
                expected.extend_from_slice(&coordinate.to_be_bytes());
            }
            expected.extend_from_slice(&2.0_f32.to_be_bytes());
            expected.extend_from_slice(&1.0_f32.to_be_bytes());
            expected.extend_from_slice(&42_i64.to_be_bytes());
            assert_eq!(bytes, expected);
            assert_eq!(ClientboundSound::ID, 0x77);
            assert_eq!(ClientboundSound::decode(&mut &bytes[..]).unwrap(), packet);
        }
    }
}
