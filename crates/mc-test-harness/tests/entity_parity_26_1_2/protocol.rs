use anyhow::{Context, Result, ensure};
use bytes::{Buf, Bytes};
use mc_protocol::codec::ReadMc;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundSetEntityData, ClientboundSetPassengers, EntityDataValue, EntityEvent,
    RemoveEntities, unpack_block_pos,
};

use super::model::{EntityAliases, EntityFact, EntityStatePacket, MetadataEntry};

// Packet IDs and layouts below come from the local 26.1.2
// `.analysis/protocol-dump.txt` GameProtocols clientbound registration table
// and local `javap`/wire-probe inspection, per ADR 0002. Registration indices:
// damage 25 (0x19), entity event 34 (0x22), removals 77 (0x4D), remove effect
// 78 (0x4E), entity data 99 (0x63), equipment 102 (0x66), passengers 107
// (0x6B), attributes 131 (0x83), and effect update 132 (0x84). The typed
// decoders establish full layouts for metadata, passengers, removals, and
// entity events; entity-scoped state packets begin with a VarInt runtime ID.
// Literal unit fixtures in this module prove codec boundaries only and are not
// vanilla oracle evidence.
pub(crate) const CLIENTBOUND_DAMAGE_EVENT_ID: i32 = 0x19;
pub(crate) const CLIENTBOUND_REMOVE_MOB_EFFECT_ID: i32 = 0x4E;
pub(crate) const CLIENTBOUND_SET_EQUIPMENT_ID: i32 = 0x66;
pub(crate) const CLIENTBOUND_UPDATE_ATTRIBUTES_ID: i32 = 0x83;
pub(crate) const CLIENTBOUND_UPDATE_MOB_EFFECT_ID: i32 = 0x84;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntityScopedPacketKind {
    Damage,
    EffectRemoved,
    Equipment,
    Attributes,
    EffectUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntityScopedPacket {
    pub(crate) runtime_entity_id: i32,
    pub(crate) kind: EntityScopedPacketKind,
    pub(crate) payload: Vec<u8>,
}

pub(crate) fn decode_entity_scoped_packet(
    packet_id: i32,
    mut body: Bytes,
) -> Result<Option<EntityScopedPacket>> {
    let kind = match packet_id {
        CLIENTBOUND_DAMAGE_EVENT_ID => EntityScopedPacketKind::Damage,
        CLIENTBOUND_REMOVE_MOB_EFFECT_ID => EntityScopedPacketKind::EffectRemoved,
        CLIENTBOUND_SET_EQUIPMENT_ID => EntityScopedPacketKind::Equipment,
        CLIENTBOUND_UPDATE_ATTRIBUTES_ID => EntityScopedPacketKind::Attributes,
        CLIENTBOUND_UPDATE_MOB_EFFECT_ID => EntityScopedPacketKind::EffectUpdated,
        _ => return Ok(None),
    };
    let runtime_entity_id = body.read_varint()?;
    Ok(Some(EntityScopedPacket {
        runtime_entity_id,
        kind,
        payload: body.to_vec(),
    }))
}

pub(crate) fn normalize_tracked_frame(
    packet_id: i32,
    body: Bytes,
    aliases: &EntityAliases,
    phase: &str,
) -> Result<Vec<EntityFact>> {
    if packet_id == CLIENTBOUND_DAMAGE_EVENT_ID {
        let mut body = body;
        let runtime_entity_id = body.read_varint()?;
        let Some(entity) = aliases.alias(runtime_entity_id) else {
            return Ok(Vec::new());
        };
        let source_type =
            u32::try_from(body.read_varint()?).context("negative damage source registry id")?;
        let cause = normalize_optional_entity_id(body.read_varint()?, aliases)?;
        let direct = normalize_optional_entity_id(body.read_varint()?, aliases)?;
        let source_position = if body.read_bool()? {
            Some(aliases.relative_position([body.read_f64()?, body.read_f64()?, body.read_f64()?]))
        } else {
            None
        };
        ensure!(!body.has_remaining(), "damage packet has trailing bytes");
        return Ok(vec![EntityFact::Damage {
            entity: entity.to_owned(),
            source_type,
            cause,
            direct,
            source_position,
        }]);
    }
    if packet_id == ClientboundSetEntityData::ID {
        if !packet_targets_alias(&body, aliases)? {
            return Ok(Vec::new());
        }
        let mut body = body;
        let packet = ClientboundSetEntityData::decode(&mut body)?;
        ensure!(
            !body.has_remaining(),
            "entity metadata packet has trailing bytes"
        );
        let Some(entity) = aliases.alias(packet.entity_id) else {
            return Ok(Vec::new());
        };
        let mut values = packet
            .values
            .into_iter()
            .map(|value| normalize_metadata_value(value, aliases))
            .collect::<Vec<_>>();
        // Entity data is an index-addressed map within one packet. Canonicalize
        // only that inherently unordered map; outer packet/fact order and
        // multiplicity remain untouched.
        values.sort();
        return Ok(vec![EntityFact::Metadata {
            phase: phase.to_owned(),
            entity: entity.to_owned(),
            values,
        }]);
    }
    if packet_id == ClientboundSetPassengers::ID {
        if !packet_targets_alias(&body, aliases)? {
            return Ok(Vec::new());
        }
        let mut body = body;
        let packet = ClientboundSetPassengers::decode(&mut body)?;
        ensure!(
            !body.has_remaining(),
            "passengers packet has trailing bytes"
        );
        let Some(vehicle) = aliases.alias(packet.vehicle_id) else {
            return Ok(Vec::new());
        };
        let passengers = packet
            .passenger_ids
            .into_iter()
            .map(|runtime_id| {
                aliases
                    .alias(runtime_id)
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("unbound passenger entity id {runtime_id}"))
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(vec![EntityFact::Passengers {
            vehicle: vehicle.to_owned(),
            passengers,
        }]);
    }
    if packet_id == RemoveEntities::ID {
        let mut body = body;
        let packet = RemoveEntities::decode(&mut body)?;
        ensure!(!body.has_remaining(), "removals packet has trailing bytes");
        return Ok(packet
            .entity_ids
            .into_iter()
            .filter_map(|runtime_id| aliases.alias(runtime_id))
            .map(|entity| EntityFact::Removed {
                entity: entity.to_owned(),
            })
            .collect());
    }
    if packet_id == EntityEvent::ID {
        let mut body = body;
        let packet = EntityEvent::decode(&mut body)?;
        ensure!(
            !body.has_remaining(),
            "entity event packet has trailing bytes"
        );
        let Some(entity) = aliases.alias(packet.entity_id) else {
            return Ok(Vec::new());
        };
        let fact = if phase == "passive-ai-schedule" && matches!(packet.event_id, 10 | 11) {
            EntityFact::ScheduleEvent {
                entity: entity.to_owned(),
                event_id: packet.event_id,
            }
        } else {
            EntityFact::StatusEvent {
                entity: entity.to_owned(),
                event_id: packet.event_id,
            }
        };
        return Ok(vec![fact]);
    }
    if let Some(packet) = decode_entity_scoped_packet(packet_id, body)? {
        let Some(entity) = aliases.alias(packet.runtime_entity_id) else {
            return Ok(Vec::new());
        };
        let kind = match packet.kind {
            EntityScopedPacketKind::Damage => EntityStatePacket::Damage,
            EntityScopedPacketKind::EffectRemoved => EntityStatePacket::EffectRemoved,
            EntityScopedPacketKind::Equipment => EntityStatePacket::Equipment,
            EntityScopedPacketKind::Attributes => EntityStatePacket::Attributes,
            EntityScopedPacketKind::EffectUpdated => EntityStatePacket::EffectUpdated,
        };
        return Ok(vec![EntityFact::PacketPayload {
            phase: phase.to_owned(),
            entity: entity.to_owned(),
            kind,
            payload: packet.payload,
        }]);
    }
    Ok(Vec::new())
}

fn normalize_metadata_value(value: EntityDataValue, aliases: &EntityAliases) -> MetadataEntry {
    match value {
        EntityDataValue::Byte { index, value } => MetadataEntry {
            index,
            value: format!("byte:{value}"),
        },
        EntityDataValue::Int { index, value } => MetadataEntry {
            index,
            value: format!("int:{value}"),
        },
        EntityDataValue::Long { index, value } => MetadataEntry {
            index,
            value: format!("long:{value}"),
        },
        EntityDataValue::Float { index, value } => MetadataEntry {
            index,
            value: format!("float-bits:{:08X}", value.to_bits()),
        },
        EntityDataValue::String { index, value } => MetadataEntry {
            index,
            value: format!("string:{value:?}"),
        },
        EntityDataValue::ItemStack { index, stack } => MetadataEntry {
            index,
            value: format!(
                "item:{}:{}:{:?}:{:?}",
                stack.item_id, stack.count, stack.damage, stack.enchantments
            ),
        },
        EntityDataValue::Boolean { index, value } => MetadataEntry {
            index,
            value: format!("bool:{value}"),
        },
        EntityDataValue::Rotations { index, value } => MetadataEntry {
            index,
            value: format!(
                "rotations-bits:{:08X}:{:08X}:{:08X}",
                value.x.to_bits(),
                value.y.to_bits(),
                value.z.to_bits()
            ),
        },
        EntityDataValue::BlockPosition { index, value } => MetadataEntry {
            index,
            value: normalized_block_position(value, aliases),
        },
        EntityDataValue::OptionalBlockPosition { index, value } => MetadataEntry {
            index,
            value: value.map_or_else(
                || "block-pos:none".into(),
                |position| normalized_block_position(position, aliases),
            ),
        },
        EntityDataValue::Direction { index, value } => MetadataEntry {
            index,
            value: format!("direction:{value:?}"),
        },
        EntityDataValue::OptionalLivingEntityReference { index, value } => MetadataEntry {
            index,
            value: if value.is_some() {
                "living-entity-ref:present".into()
            } else {
                "living-entity-ref:none".into()
            },
        },
        EntityDataValue::BlockState { index, value } => MetadataEntry {
            index,
            value: format!("block-state:{}", value.raw()),
        },
        EntityDataValue::OptionalBlockState { index, value } => MetadataEntry {
            index,
            value: value.map_or_else(
                || "block-state:none".into(),
                |state| format!("block-state:{}", state.raw()),
            ),
        },
        EntityDataValue::OptionalUnsignedInt { index, value } => MetadataEntry {
            index,
            value: value.map_or_else(
                || "unsigned-int:none".into(),
                |value| format!("unsigned-int:{value}"),
            ),
        },
        EntityDataValue::Pose { index, pose } => MetadataEntry {
            index,
            value: format!("pose:{pose:?}"),
        },
        EntityDataValue::HumanoidArm { index, value } => MetadataEntry {
            index,
            value: format!("humanoid-arm:{value:?}"),
        },
    }
}

fn normalized_block_position(value: i64, aliases: &EntityAliases) -> String {
    let (x, y, z) = unpack_block_pos(value);
    let position = aliases.relative_position([f64::from(x), f64::from(y), f64::from(z)]);
    format!("block-pos:{}:{}:{}", position.x, position.y, position.z)
}

fn normalize_optional_entity_id(
    encoded_id: i32,
    aliases: &EntityAliases,
) -> Result<Option<String>> {
    if encoded_id == 0 {
        return Ok(None);
    }
    let runtime_id = encoded_id
        .checked_sub(1)
        .context("damage source entity id underflow")?;
    aliases
        .alias(runtime_id)
        .map(str::to_owned)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("unbound damage source entity id {runtime_id}"))
}

fn packet_targets_alias(body: &Bytes, aliases: &EntityAliases) -> Result<bool> {
    let mut body = body.clone();
    Ok(aliases.alias(body.read_varint()?).is_some())
}

#[cfg(test)]
mod tests {
    use mc_protocol::packets::play::SHEEP_ENTITY_DATA_WOOL_INDEX;

    use super::*;

    fn aliases(bindings: &[(&str, i32)]) -> EntityAliases {
        let mut aliases = EntityAliases::new([0.0; 3]);
        for (alias, runtime_id) in bindings {
            aliases
                .bind_existing(alias, *runtime_id)
                .expect("bind fixture entity");
        }
        aliases
    }

    fn assert_trailing_bytes_rejected(
        packet_id: i32,
        body: &'static [u8],
        aliases: &EntityAliases,
        packet_name: &str,
    ) {
        let error =
            normalize_tracked_frame(packet_id, Bytes::from_static(body), aliases, "fixture")
                .expect_err("trailing garbage must reject the frame");
        assert!(
            error.to_string().contains("trailing bytes"),
            "{packet_name}: {error:#}"
        );
    }

    #[test]
    fn literal_entity_scoped_bodies_exclude_runtime_entity_id_from_payload() {
        let solaris = decode_entity_scoped_packet(
            CLIENTBOUND_UPDATE_ATTRIBUTES_ID,
            Bytes::from_static(&[0x11, 0x01, 0x02, 0x03]),
        )
        .expect("decode Solaris frame")
        .expect("recognized Solaris frame");
        let vanilla = decode_entity_scoped_packet(
            CLIENTBOUND_UPDATE_ATTRIBUTES_ID,
            Bytes::from_static(&[0xAC, 0x46, 0x01, 0x02, 0x03]),
        )
        .expect("decode vanilla frame")
        .expect("recognized vanilla frame");

        assert_ne!(solaris.runtime_entity_id, vanilla.runtime_entity_id);
        assert_eq!(solaris.kind, vanilla.kind);
        assert_eq!(solaris.payload, vanilla.payload);
        assert_eq!(solaris.payload, vec![1, 2, 3]);
    }

    #[test]
    fn local_literal_metadata_body_replaces_runtime_entity_id_with_alias() {
        // Local structural fixture: VarInt entity 41, byte serializer at wool index 18,
        // sheared bit set, then the metadata terminator. This is not oracle evidence.
        let body = Bytes::from_static(&[0x29, 0x12, 0x00, 0x10, 0xFF]);
        let aliases = aliases(&[("subject", 41)]);

        let facts = normalize_tracked_frame(ClientboundSetEntityData::ID, body, &aliases, "dirty")
            .expect("normalize metadata");

        assert_eq!(
            facts,
            vec![EntityFact::Metadata {
                phase: "dirty".into(),
                entity: "subject".into(),
                values: vec![MetadataEntry {
                    index: SHEEP_ENTITY_DATA_WOOL_INDEX,
                    value: "byte:16".into(),
                }],
            }]
        );
    }

    #[test]
    fn local_literal_damage_bodies_replace_embedded_attacker_entity_ids() {
        // Local structural fixtures, not oracle captures. Optional entity ids are
        // encoded as runtime id + 1 by the damage packet layout.
        let solaris_aliases = aliases(&[("subject", 17), ("player", 3)]);
        let vanilla_aliases = aliases(&[("subject", 9_004), ("player", 811)]);

        let solaris = normalize_tracked_frame(
            CLIENTBOUND_DAMAGE_EVENT_ID,
            Bytes::from_static(&[0x11, 0x05, 0x04, 0x04, 0x00]),
            &solaris_aliases,
            "attack",
        )
        .expect("normalize Solaris damage");
        let vanilla = normalize_tracked_frame(
            CLIENTBOUND_DAMAGE_EVENT_ID,
            Bytes::from_static(&[0xAC, 0x46, 0x05, 0xAC, 0x06, 0xAC, 0x06, 0x00]),
            &vanilla_aliases,
            "attack",
        )
        .expect("normalize vanilla damage");

        assert_eq!(solaris, vanilla);
        assert_eq!(
            solaris,
            vec![EntityFact::Damage {
                entity: "subject".into(),
                source_type: 5,
                cause: Some("player".into()),
                direct: Some("player".into()),
                source_position: None,
            }]
        );
    }

    #[test]
    fn unrelated_metadata_does_not_require_serializer_support() {
        let aliases = EntityAliases::new([0.0; 3]);

        let facts = normalize_tracked_frame(
            ClientboundSetEntityData::ID,
            Bytes::from_static(&[0x8F, 0x4E, 0x04, 0x7F, 0xFF]),
            &aliases,
            "default",
        )
        .expect("untracked metadata is ignored before value decoding");

        assert!(facts.is_empty());
    }

    #[test]
    fn local_literal_entity_event_uses_a_distinct_schedule_fact() {
        // EntityEvent uses a fixed-width big-endian i32 followed by one i8.
        let body = Bytes::from_static(&[0x00, 0x00, 0x00, 0x11, 0x0A]);
        let aliases = aliases(&[("subject", 17)]);

        let facts = normalize_tracked_frame(EntityEvent::ID, body, &aliases, "passive-ai-schedule")
            .expect("normalize schedule event");

        assert_eq!(
            facts,
            vec![EntityFact::ScheduleEvent {
                entity: "subject".into(),
                event_id: 10,
            }]
        );
    }

    #[test]
    fn metadata_rejects_trailing_garbage() {
        let aliases = aliases(&[("subject", 41)]);
        assert_trailing_bytes_rejected(
            ClientboundSetEntityData::ID,
            &[0x29, 0x12, 0x00, 0x10, 0xFF, 0x00],
            &aliases,
            "metadata",
        );
    }

    #[test]
    fn passengers_reject_trailing_garbage() {
        let aliases = aliases(&[("vehicle", 41), ("subject", 42)]);
        assert_trailing_bytes_rejected(
            ClientboundSetPassengers::ID,
            &[0x29, 0x01, 0x2A, 0x00],
            &aliases,
            "passengers",
        );
    }

    #[test]
    fn removals_reject_trailing_garbage() {
        let aliases = aliases(&[("subject", 41)]);
        assert_trailing_bytes_rejected(
            RemoveEntities::ID,
            &[0x01, 0x29, 0x00],
            &aliases,
            "removals",
        );
    }

    #[test]
    fn entity_events_reject_trailing_garbage() {
        let aliases = aliases(&[("subject", 41)]);
        assert_trailing_bytes_rejected(
            EntityEvent::ID,
            &[0x00, 0x00, 0x00, 0x29, 0x03, 0x00],
            &aliases,
            "entity event",
        );
    }
}
