//! Entity synchronization packet codecs unique to Java Edition 26.1.2.
//!
//! `ClientboundSetEntityData` and `ClientboundSetPassengers` remain in the
//! parent module as their canonical APIs. This module only owns the five
//! additional packet families whose ids and layouts were checked against the
//! local 26.1.2 protocol dump and `javap` output per ADR 0002.

use bytes::{Buf, BufMut};

use super::{ItemStack, ItemStackWireCodec};
use crate::CodecError;
use crate::codec::{Identifier, ReadMc, WriteMc, read_bounded_vec};
use crate::packets::Packet;

/// `LivingEntity.DATA_HEALTH_ID` in the local vanilla 26.1.2 decompile.
///
/// `Entity` defines indices 0 through 7, then `LivingEntity` defines its flags
/// at index 8 and health with the FLOAT serializer at index 9.
pub const LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2: u8 = 9;

// Vanilla 26.1.2 exposes 35 built-in attributes. Sixty-four leaves registry
// growth headroom while bounding the nested packet allocation to 4096 entries.
const MAX_ENTITY_ATTRIBUTES: usize = 64;
const MAX_ATTRIBUTE_MODIFIERS: usize = 64;
const MAX_EQUIPMENT_ENTRIES: usize = 8;

fn write_bounded_count<B: BufMut>(buf: &mut B, len: usize, max: usize) -> Result<(), CodecError> {
    if len > max {
        return Err(CodecError::StringTooLong { len, max });
    }
    let count = i32::try_from(len).map_err(|_| CodecError::StringTooLong { len, max })?;
    buf.write_varint(count);
    Ok(())
}

/// A non-negative `minecraft:attribute` registry wire id.
///
/// The type distinguishes attribute ids from other registries. It does not
/// assert membership in a runtime registry; callers must resolve that before
/// constructing packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeId(i32);

impl AttributeId {
    pub fn new(raw: u32) -> Result<Self, CodecError> {
        let raw = i32::try_from(raw)
            .map_err(|_| CodecError::NotSupported("attribute registry id exceeds VarInt range"))?;
        Ok(Self(raw))
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0 as u32
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let raw = buf.read_varint()?;
        if raw < 0 {
            return Err(CodecError::NotSupported("negative attribute registry id"));
        }
        Ok(Self(raw))
    }
}

/// A non-negative `minecraft:mob_effect` registry wire id.
///
/// The type distinguishes effect ids from other registries. It does not
/// assert membership in a runtime registry; callers must resolve that before
/// constructing packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobEffectId(i32);

impl MobEffectId {
    pub fn new(raw: u32) -> Result<Self, CodecError> {
        let raw = i32::try_from(raw)
            .map_err(|_| CodecError::NotSupported("mob-effect registry id exceeds VarInt range"))?;
        Ok(Self(raw))
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0 as u32
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let raw = buf.read_varint()?;
        if raw < 0 {
            return Err(CodecError::NotSupported("negative mob-effect registry id"));
        }
        Ok(Self(raw))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeModifierOperation {
    AddValue,
    AddMultipliedBase,
    AddMultipliedTotal,
}

impl AttributeModifierOperation {
    const fn wire_id(self) -> i32 {
        match self {
            Self::AddValue => 0,
            Self::AddMultipliedBase => 1,
            Self::AddMultipliedTotal => 2,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::AddValue,
            1 => Self::AddMultipliedBase,
            2 => Self::AddMultipliedTotal,
            _ => {
                return Err(CodecError::NotSupported(
                    "unknown attribute modifier operation",
                ));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityAttributeModifier {
    pub id: Identifier,
    pub amount: f64,
    pub operation: AttributeModifierOperation,
}

impl EntityAttributeModifier {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_identifier(&self.id)?;
        buf.write_f64(self.amount);
        buf.write_varint(self.operation.wire_id());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_identifier()?,
            amount: buf.read_f64()?,
            operation: AttributeModifierOperation::from_wire(buf.read_varint()?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityAttributeSnapshot {
    pub attribute_id: AttributeId,
    pub base: f64,
    pub modifiers: Vec<EntityAttributeModifier>,
}

impl EntityAttributeSnapshot {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.attribute_id.0);
        buf.write_f64(self.base);
        write_bounded_count(buf, self.modifiers.len(), MAX_ATTRIBUTE_MODIFIERS)?;
        for modifier in &self.modifiers {
            modifier.encode(buf)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let attribute_id = AttributeId::decode(buf)?;
        let base = buf.read_f64()?;
        let modifiers = read_bounded_vec(
            buf,
            MAX_ATTRIBUTE_MODIFIERS,
            10,
            EntityAttributeModifier::decode,
        )?;
        Ok(Self {
            attribute_id,
            base,
            modifiers,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundUpdateEntityAttributes {
    pub entity_id: i32,
    pub attributes: Vec<EntityAttributeSnapshot>,
}

impl Packet for ClientboundUpdateEntityAttributes {
    // `.analysis/protocol-dump.txt`: CLIENTBOUND_UPDATE_ATTRIBUTES is game-CB
    // index 131 = wire id 0x83. Both collections use VarInt counts.
    const ID: i32 = 0x83;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.attributes.len() > MAX_ENTITY_ATTRIBUTES {
            return Err(CodecError::StringTooLong {
                len: self.attributes.len(),
                max: MAX_ENTITY_ATTRIBUTES,
            });
        }
        for attribute in &self.attributes {
            if attribute.modifiers.len() > MAX_ATTRIBUTE_MODIFIERS {
                return Err(CodecError::StringTooLong {
                    len: attribute.modifiers.len(),
                    max: MAX_ATTRIBUTE_MODIFIERS,
                });
            }
        }

        buf.write_varint(self.entity_id);
        write_bounded_count(buf, self.attributes.len(), MAX_ENTITY_ATTRIBUTES)?;
        for attribute in &self.attributes {
            attribute.encode(buf)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_varint()?;
        let attributes = read_bounded_vec(
            buf,
            MAX_ENTITY_ATTRIBUTES,
            1,
            EntityAttributeSnapshot::decode,
        )?;
        Ok(Self {
            entity_id,
            attributes,
        })
    }
}

/// The ordinal order exposed by `EquipmentSlot.VALUES` in 26.1.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSlot {
    MainHand,
    OffHand,
    Feet,
    Legs,
    Chest,
    Head,
    Body,
    Saddle,
}

impl EquipmentSlot {
    const fn wire_id(self) -> u8 {
        match self {
            Self::MainHand => 0,
            Self::OffHand => 1,
            Self::Feet => 2,
            Self::Legs => 3,
            Self::Chest => 4,
            Self::Head => 5,
            Self::Body => 6,
            Self::Saddle => 7,
        }
    }

    fn from_wire(value: u8) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::MainHand,
            1 => Self::OffHand,
            2 => Self::Feet,
            3 => Self::Legs,
            4 => Self::Chest,
            5 => Self::Head,
            6 => Self::Body,
            7 => Self::Saddle,
            _ => return Err(CodecError::NotSupported("unknown equipment slot")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityEquipment {
    pub slot: EquipmentSlot,
    pub item: ItemStack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetEntityEquipment {
    pub entity_id: i32,
    pub equipment: Vec<EntityEquipment>,
}

impl Packet for ClientboundSetEntityEquipment {
    // `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_EQUIPMENT is game-CB
    // index 102 = wire id 0x66. Bit 7 means another entry follows.
    const ID: i32 = 0x66;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.equipment.is_empty() {
            return Err(CodecError::NotSupported(
                "entity equipment list cannot be empty",
            ));
        }
        if self.equipment.len() > MAX_EQUIPMENT_ENTRIES {
            return Err(CodecError::StringTooLong {
                len: self.equipment.len(),
                max: MAX_EQUIPMENT_ENTRIES,
            });
        }

        buf.write_varint(self.entity_id);
        for (index, equipment) in self.equipment.iter().enumerate() {
            let continuation = u8::from(index + 1 < self.equipment.len()) << 7;
            buf.write_u8(equipment.slot.wire_id() | continuation);
            equipment.item.encode(buf)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_varint()?;
        let mut equipment = Vec::new();
        loop {
            if equipment.len() >= MAX_EQUIPMENT_ENTRIES {
                return Err(CodecError::StringTooLong {
                    len: equipment.len() + 1,
                    max: MAX_EQUIPMENT_ENTRIES,
                });
            }
            let encoded_slot = buf.read_u8()?;
            let slot = EquipmentSlot::from_wire(encoded_slot & 0x7F)?;
            let item = ItemStack::decode(buf)?;
            equipment.push(EntityEquipment { slot, item });
            if encoded_slot & 0x80 == 0 {
                break;
            }
        }
        Ok(Self {
            entity_id,
            equipment,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityEffectFlags {
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
    pub blend: bool,
}

impl EntityEffectFlags {
    fn wire_bits(self) -> u8 {
        u8::from(self.ambient)
            | (u8::from(self.visible) << 1)
            | (u8::from(self.show_icon) << 2)
            | (u8::from(self.blend) << 3)
    }

    fn from_wire(value: u8) -> Result<Self, CodecError> {
        if value & !0x0F != 0 {
            return Err(CodecError::NotSupported("unknown entity effect flag bits"));
        }
        Ok(Self {
            ambient: value & 0x01 != 0,
            visible: value & 0x02 != 0,
            show_icon: value & 0x04 != 0,
            blend: value & 0x08 != 0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundUpdateEntityEffect {
    pub entity_id: i32,
    pub effect_id: MobEffectId,
    pub amplifier: i32,
    pub duration_ticks: i32,
    pub flags: EntityEffectFlags,
}

impl Packet for ClientboundUpdateEntityEffect {
    // `.analysis/protocol-dump.txt`: CLIENTBOUND_UPDATE_MOB_EFFECT is game-CB
    // index 132 = wire id 0x84.
    const ID: i32 = 0x84;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.effect_id.0);
        buf.write_varint(self.amplifier);
        buf.write_varint(self.duration_ticks);
        buf.write_u8(self.flags.wire_bits());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            effect_id: MobEffectId::decode(buf)?,
            amplifier: buf.read_varint()?,
            duration_ticks: buf.read_varint()?,
            flags: EntityEffectFlags::from_wire(buf.read_u8()?)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundRemoveEntityEffect {
    pub entity_id: i32,
    pub effect_id: MobEffectId,
}

impl Packet for ClientboundRemoveEntityEffect {
    // `.analysis/protocol-dump.txt`: CLIENTBOUND_REMOVE_MOB_EFFECT is game-CB
    // index 78 = wire id 0x4E.
    const ID: i32 = 0x4E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.effect_id.0);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            effect_id: MobEffectId::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundSetEntityLeash {
    pub source_entity_id: i32,
    /// `None` is the wire destination id zero used to detach the leash.
    pub holder_entity_id: Option<i32>,
}

impl Packet for ClientboundSetEntityLeash {
    // `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_ENTITY_LINK is game-CB
    // index 100 = wire id 0x64. Both ids are fixed-width big-endian ints.
    const ID: i32 = 0x64;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.holder_entity_id == Some(0) {
            return Err(CodecError::NotSupported(
                "entity leash holder id zero means detached",
            ));
        }
        buf.write_i32(self.source_entity_id);
        buf.write_i32(self.holder_entity_id.unwrap_or(0));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let source_entity_id = buf.read_i32()?;
        let holder_entity_id = match buf.read_i32()? {
            0 => None,
            value => Some(value),
        };
        Ok(Self {
            source_entity_id,
            holder_entity_id,
        })
    }
}

#[cfg(test)]
#[path = "entity_sync_26_1_2_tests.rs"]
mod tests;
