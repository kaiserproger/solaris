use super::super::{
    BlockStateId, ClientboundSetEntityData, ClientboundSetPassengers, EntityDataValue,
    EntityDirection, EntityPose, EntityRotations, ItemStack, MAX_ENTITY_DATA_VALUES,
};
use super::{
    AttributeId, AttributeModifierOperation, ClientboundRemoveEntityEffect,
    ClientboundSetEntityEquipment, ClientboundSetEntityLeash, ClientboundUpdateEntityAttributes,
    ClientboundUpdateEntityEffect, EntityAttributeModifier, EntityAttributeSnapshot,
    EntityEffectFlags, EntityEquipment, EquipmentSlot, LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
    MAX_ATTRIBUTE_MODIFIERS, MAX_ENTITY_ATTRIBUTES, MAX_EQUIPMENT_ENTRIES, MobEffectId,
};
use crate::CodecError;
use crate::codec::{Identifier, WriteMc};
use crate::packets::{MainHand, Packet};

fn encode<P: Packet>(packet: &P) -> Vec<u8> {
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    bytes
}

fn decode_exact<P: Packet>(bytes: &[u8]) -> P {
    let mut cursor = bytes;
    let packet = P::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty(), "decoder left trailing bytes");
    packet
}

fn assert_every_truncated_prefix_fails<P: Packet>(bytes: &[u8]) {
    for end in 0..bytes.len() {
        let mut cursor = &bytes[..end];
        assert!(
            P::decode(&mut cursor).is_err(),
            "{} accepted truncated prefix of {end}/{} bytes",
            std::any::type_name::<P>(),
            bytes.len()
        );
    }
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

#[test]
fn entity_sync_packet_ids_match_the_26_1_2_protocol_dump() {
    assert_eq!(ClientboundSetEntityData::ID, 0x63);
    assert_eq!(ClientboundSetEntityLeash::ID, 0x64);
    assert_eq!(ClientboundSetEntityEquipment::ID, 0x66);
    assert_eq!(ClientboundSetPassengers::ID, 0x6B);
    assert_eq!(ClientboundRemoveEntityEffect::ID, 0x4E);
    assert_eq!(ClientboundUpdateEntityAttributes::ID, 0x83);
    assert_eq!(ClientboundUpdateEntityEffect::ID, 0x84);
}

#[test]
fn living_health_metadata_matches_the_local_26_1_2_decompile() {
    let packet = ClientboundSetEntityData {
        entity_id: 42,
        values: vec![EntityDataValue::Float {
            index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
            value: 6.5,
        }],
    };

    assert_eq!(
        encode(&packet),
        [0x2a, 0x09, 0x03, 0x40, 0xd0, 0x00, 0x00, 0xff]
    );
    assert_eq!(
        decode_exact::<ClientboundSetEntityData>(&encode(&packet)),
        packet
    );
}

#[test]
fn entity_data_supported_serializers_match_javap_layouts_and_round_trip() {
    let entity_reference = uuid::Uuid::from_bytes([
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ]);
    let packet = ClientboundSetEntityData {
        entity_id: 300,
        values: vec![
            EntityDataValue::Byte {
                index: 0,
                value: -2,
            },
            EntityDataValue::Int {
                index: 1,
                value: 300,
            },
            EntityDataValue::Long {
                index: 2,
                value: 300,
            },
            EntityDataValue::Float {
                index: 3,
                value: 1.5,
            },
            EntityDataValue::String {
                index: 4,
                value: "hi".into(),
            },
            EntityDataValue::ItemStack {
                index: 7,
                stack: ItemStack::EMPTY,
            },
            EntityDataValue::Boolean {
                index: 8,
                value: true,
            },
            EntityDataValue::Rotations {
                index: 9,
                value: EntityRotations {
                    x: 1.0,
                    y: -2.0,
                    z: 0.5,
                },
            },
            EntityDataValue::BlockPosition {
                index: 10,
                value: 0x0102_0304_0506_0708,
            },
            EntityDataValue::OptionalBlockPosition {
                index: 11,
                value: None,
            },
            EntityDataValue::OptionalBlockPosition {
                index: 12,
                value: Some(0x1112_1314_1516_1718),
            },
            EntityDataValue::Direction {
                index: 13,
                value: EntityDirection::East,
            },
            EntityDataValue::OptionalLivingEntityReference {
                index: 14,
                value: None,
            },
            EntityDataValue::OptionalLivingEntityReference {
                index: 15,
                value: Some(entity_reference),
            },
            EntityDataValue::BlockState {
                index: 16,
                value: BlockStateId::new(128).unwrap(),
            },
            EntityDataValue::OptionalBlockState {
                index: 17,
                value: None,
            },
            EntityDataValue::OptionalBlockState {
                index: 18,
                value: Some(BlockStateId::new(5).unwrap()),
            },
            EntityDataValue::OptionalUnsignedInt {
                index: 19,
                value: None,
            },
            EntityDataValue::OptionalUnsignedInt {
                index: 20,
                value: Some(6),
            },
            EntityDataValue::Pose {
                index: 21,
                pose: EntityPose::Crouching,
            },
            EntityDataValue::HumanoidArm {
                index: 22,
                value: MainHand::Right,
            },
        ],
    };

    let mut expected = vec![
        0xAC, 0x02, // entity id 300
        0x00, 0x00, 0xFE, // byte serializer 0
        0x01, 0x01, 0xAC, 0x02, // int serializer 1
        0x02, 0x02, 0xAC, 0x02, // long serializer 2
        0x03, 0x03,
    ];
    expected.extend_from_slice(&1.5_f32.to_be_bytes());
    expected.extend_from_slice(&[
        0x04, 0x04, 0x02, b'h', b'i', // string serializer 4
        0x07, 0x07, 0x00, // empty item stack serializer 7
        0x08, 0x08, 0x01, // boolean serializer 8
        0x09, 0x09,
    ]);
    expected.extend_from_slice(&1.0_f32.to_be_bytes());
    expected.extend_from_slice(&(-2.0_f32).to_be_bytes());
    expected.extend_from_slice(&0.5_f32.to_be_bytes());
    expected.extend_from_slice(&[0x0A, 0x0A]);
    expected.extend_from_slice(&0x0102_0304_0506_0708_i64.to_be_bytes());
    expected.extend_from_slice(&[0x0B, 0x0B, 0x00, 0x0C, 0x0B, 0x01]);
    expected.extend_from_slice(&0x1112_1314_1516_1718_i64.to_be_bytes());
    expected.extend_from_slice(&[
        0x0D, 0x0C, 0x05, // east direction id
        0x0E, 0x0D, 0x00, // optional living entity reference: absent
        0x0F, 0x0D, 0x01, // optional living entity reference: present
    ]);
    expected.extend_from_slice(entity_reference.as_bytes());
    expected.extend_from_slice(&[
        0x10, 0x0E, 0x80, 0x01, // block-state id 128
        0x11, 0x0F, 0x00, // optional block state: absent
        0x12, 0x0F, 0x05, // optional block state: present
        0x13, 0x13, 0x00, // optional unsigned int: absent
        0x14, 0x13, 0x07, // optional unsigned int stores value + 1
        0x15, 0x14, 0x05, // crouching pose
        0x16, 0x2A, 0x01, // right humanoid arm
        0xFF,
    ]);

    let encoded = encode(&packet);
    assert_eq!(encoded, expected);
    assert_eq!(decode_exact::<ClientboundSetEntityData>(&encoded), packet);
    assert_every_truncated_prefix_fails::<ClientboundSetEntityData>(&encoded);
}

#[test]
fn entity_data_float_equality_matches_java_float_equals() {
    let positive_zero = EntityDataValue::Float {
        index: 1,
        value: 0.0,
    };
    let negative_zero = EntityDataValue::Float {
        index: 1,
        value: -0.0,
    };
    assert_ne!(positive_zero, negative_zero);

    let first_nan = EntityDataValue::Float {
        index: 1,
        value: f32::from_bits(0x7FC0_0001),
    };
    let second_nan = EntityDataValue::Float {
        index: 1,
        value: f32::from_bits(0xFFC0_1234),
    };
    assert_eq!(first_nan, second_nan);

    let first_rotations = EntityRotations {
        x: f32::from_bits(0x7FC0_0001),
        y: 0.0,
        z: -0.0,
    };
    let second_rotations = EntityRotations {
        x: f32::from_bits(0xFFC0_1234),
        y: 0.0,
        z: -0.0,
    };
    assert_eq!(first_rotations, second_rotations);
    assert_ne!(
        first_rotations,
        EntityRotations {
            z: 0.0,
            ..second_rotations
        }
    );
}

#[test]
fn entity_data_rejects_duplicate_indices_on_encode_and_decode() {
    let packet = ClientboundSetEntityData {
        entity_id: 1,
        values: vec![
            EntityDataValue::Byte { index: 3, value: 1 },
            EntityDataValue::Int { index: 3, value: 2 },
        ],
    };
    assert_eq!(
        packet.encode(&mut Vec::new()).unwrap_err(),
        CodecError::NotSupported("duplicate entity data index")
    );

    let mut encoded: &[u8] = &[1, 3, 0, 1, 3, 1, 2, 0xFF];
    assert_eq!(
        ClientboundSetEntityData::decode(&mut encoded).unwrap_err(),
        CodecError::NotSupported("duplicate entity data index")
    );
}

#[test]
fn entity_data_rejects_reserved_indices_and_over_cap_collections() {
    let reserved = ClientboundSetEntityData {
        entity_id: 1,
        values: vec![EntityDataValue::Byte {
            index: 0xFF,
            value: 0,
        }],
    };
    assert_eq!(
        reserved.encode(&mut Vec::new()).unwrap_err(),
        CodecError::NotSupported("entity data index 255 is reserved")
    );

    let oversized = ClientboundSetEntityData {
        entity_id: 1,
        values: (0..=MAX_ENTITY_DATA_VALUES as u8)
            .map(|index| EntityDataValue::Byte { index, value: 0 })
            .collect(),
    };
    assert_eq!(
        oversized.encode(&mut Vec::new()).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_DATA_VALUES + 1,
            max: MAX_ENTITY_DATA_VALUES,
        }
    );

    let mut encoded = vec![1];
    for index in 0..=MAX_ENTITY_DATA_VALUES as u8 {
        encoded.extend_from_slice(&[index, 0, 0]);
    }
    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_DATA_VALUES + 1,
            max: MAX_ENTITY_DATA_VALUES,
        }
    );
}

#[test]
fn entity_data_explicitly_rejects_unsupported_serializers() {
    let unsupported = [5, 6, 16, 17, 18, 21, 34, 39, 40, 41, 43];
    for serializer_id in unsupported {
        let mut encoded = vec![1, 0];
        encoded.write_varint(serializer_id);
        let mut cursor: &[u8] = &encoded;
        assert_eq!(
            ClientboundSetEntityData::decode(&mut cursor).unwrap_err(),
            CodecError::NotSupported("entity data serializer is not implemented")
        );
    }
}

#[test]
fn entity_data_rejects_invalid_enum_and_optional_payloads() {
    let cases: &[(&[u8], &'static str)] = &[
        (&[1, 0, 12, 6], "unknown entity direction"),
        (&[1, 0, 20, 18], "unknown entity pose"),
        (
            &[1, 0, 14, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F],
            "negative block-state registry id",
        ),
        (
            &[1, 0, 15, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F],
            "negative block-state registry id",
        ),
        (
            &[1, 0, 19, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F],
            "negative optional unsigned integer",
        ),
        (&[1, 0, 42, 2], "unknown humanoid arm"),
    ];

    for (encoded, message) in cases {
        let mut cursor = *encoded;
        assert_eq!(
            ClientboundSetEntityData::decode(&mut cursor).unwrap_err(),
            CodecError::NotSupported(message)
        );
    }
}

#[test]
fn entity_data_encode_rejects_ambiguous_or_overflowing_optional_values() {
    let ambiguous_block_state = ClientboundSetEntityData {
        entity_id: 1,
        values: vec![EntityDataValue::OptionalBlockState {
            index: 0,
            value: Some(BlockStateId::new(0).unwrap()),
        }],
    };
    assert_eq!(
        ambiguous_block_state.encode(&mut Vec::new()).unwrap_err(),
        CodecError::NotSupported("optional block-state id zero means absent")
    );

    let overflowing_unsigned_int = ClientboundSetEntityData {
        entity_id: 1,
        values: vec![EntityDataValue::OptionalUnsignedInt {
            index: 0,
            value: Some(i32::MAX as u32),
        }],
    };
    assert_eq!(
        overflowing_unsigned_int
            .encode(&mut Vec::new())
            .unwrap_err(),
        CodecError::NotSupported("optional unsigned integer exceeds VarInt range")
    );
}

#[test]
fn registry_id_types_reject_values_outside_nonnegative_varint_range() {
    let too_large = i32::MAX as u32 + 1;
    assert_eq!(
        BlockStateId::new(too_large).unwrap_err(),
        CodecError::NotSupported("block-state registry id exceeds VarInt range")
    );
    assert_eq!(
        AttributeId::new(too_large).unwrap_err(),
        CodecError::NotSupported("attribute registry id exceeds VarInt range")
    );
    assert_eq!(
        MobEffectId::new(too_large).unwrap_err(),
        CodecError::NotSupported("mob-effect registry id exceeds VarInt range")
    );
}

#[test]
fn update_attributes_matches_registry_holder_and_modifier_stream_codecs() {
    let packet = ClientboundUpdateEntityAttributes {
        entity_id: 300,
        attributes: vec![EntityAttributeSnapshot {
            attribute_id: AttributeId::new(5).unwrap(),
            base: 1.5,
            modifiers: vec![
                EntityAttributeModifier {
                    id: identifier("minecraft:a"),
                    amount: -2.0,
                    operation: AttributeModifierOperation::AddValue,
                },
                EntityAttributeModifier {
                    id: identifier("x:y"),
                    amount: 0.25,
                    operation: AttributeModifierOperation::AddMultipliedTotal,
                },
            ],
        }],
    };

    let mut expected = vec![0xAC, 0x02, 0x01, 0x05];
    expected.extend_from_slice(&1.5_f64.to_be_bytes());
    expected.extend_from_slice(&[0x02, 0x0B]);
    expected.extend_from_slice(b"minecraft:a");
    expected.extend_from_slice(&(-2.0_f64).to_be_bytes());
    expected.push(0x00);
    expected.extend_from_slice(&[0x03, b'x', b':', b'y']);
    expected.extend_from_slice(&0.25_f64.to_be_bytes());
    expected.push(0x02);

    let encoded = encode(&packet);
    assert_eq!(encoded, expected);
    assert_eq!(
        decode_exact::<ClientboundUpdateEntityAttributes>(&encoded),
        packet
    );
    assert_every_truncated_prefix_fails::<ClientboundUpdateEntityAttributes>(&encoded);
}

#[test]
fn update_attributes_rejects_negative_and_over_cap_counts() {
    let mut negative_attributes: &[u8] = &[1, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut negative_attributes).unwrap_err(),
        CodecError::NegativeLength(-1)
    );

    let mut too_many_attributes = vec![1];
    too_many_attributes.write_varint((MAX_ENTITY_ATTRIBUTES + 1) as i32);
    let mut cursor: &[u8] = &too_many_attributes;
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut cursor).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_ATTRIBUTES + 1,
            max: MAX_ENTITY_ATTRIBUTES,
        }
    );

    let mut negative_modifiers = vec![1, 1, 0];
    negative_modifiers.extend_from_slice(&0.0_f64.to_be_bytes());
    negative_modifiers.write_varint(-1);
    let mut cursor: &[u8] = &negative_modifiers;
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut cursor).unwrap_err(),
        CodecError::NegativeLength(-1)
    );

    let mut too_many_modifiers = vec![1, 1, 0];
    too_many_modifiers.extend_from_slice(&0.0_f64.to_be_bytes());
    too_many_modifiers.write_varint((MAX_ATTRIBUTE_MODIFIERS + 1) as i32);
    let mut cursor: &[u8] = &too_many_modifiers;
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut cursor).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ATTRIBUTE_MODIFIERS + 1,
            max: MAX_ATTRIBUTE_MODIFIERS,
        }
    );
}

#[test]
fn update_attributes_rejects_invalid_registry_id_and_operation() {
    let mut negative_registry = vec![1, 1];
    negative_registry.write_varint(-1);
    let mut cursor: &[u8] = &negative_registry;
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut cursor).unwrap_err(),
        CodecError::NotSupported("negative attribute registry id")
    );

    let mut invalid_operation = vec![1, 1, 0];
    invalid_operation.extend_from_slice(&0.0_f64.to_be_bytes());
    invalid_operation.push(1);
    invalid_operation.extend_from_slice(&[3, b'x', b':', b'y']);
    invalid_operation.extend_from_slice(&0.0_f64.to_be_bytes());
    invalid_operation.push(3);
    let mut cursor: &[u8] = &invalid_operation;
    assert_eq!(
        ClientboundUpdateEntityAttributes::decode(&mut cursor).unwrap_err(),
        CodecError::NotSupported("unknown attribute modifier operation")
    );
}

#[test]
fn update_attributes_rejects_over_cap_encode_collections() {
    let snapshot = EntityAttributeSnapshot {
        attribute_id: AttributeId::new(0).unwrap(),
        base: 1.0,
        modifiers: Vec::new(),
    };
    let packet = ClientboundUpdateEntityAttributes {
        entity_id: 1,
        attributes: vec![snapshot; MAX_ENTITY_ATTRIBUTES + 1],
    };
    assert_eq!(
        packet.encode(&mut Vec::new()).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_ATTRIBUTES + 1,
            max: MAX_ENTITY_ATTRIBUTES,
        }
    );

    let packet = ClientboundUpdateEntityAttributes {
        entity_id: 1,
        attributes: vec![EntityAttributeSnapshot {
            attribute_id: AttributeId::new(0).unwrap(),
            base: 1.0,
            modifiers: vec![
                EntityAttributeModifier {
                    id: identifier("x:y"),
                    amount: 0.0,
                    operation: AttributeModifierOperation::AddValue,
                };
                MAX_ATTRIBUTE_MODIFIERS + 1
            ],
        }],
    };
    assert_eq!(
        packet.encode(&mut Vec::new()).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ATTRIBUTE_MODIFIERS + 1,
            max: MAX_ATTRIBUTE_MODIFIERS,
        }
    );
}

#[test]
fn set_equipment_matches_continuation_bit_and_item_stack_layout() {
    let packet = ClientboundSetEntityEquipment {
        entity_id: 300,
        equipment: vec![
            EntityEquipment {
                slot: EquipmentSlot::MainHand,
                item: ItemStack::EMPTY,
            },
            EntityEquipment {
                slot: EquipmentSlot::Head,
                item: ItemStack::new(5, 1),
            },
        ],
    };
    let expected = vec![
        0xAC, 0x02, 0x80, 0x00, // main hand, continuation, empty stack
        0x05, 0x01, 0x05, 0x00, 0x00, // head, one item, empty component patch
    ];

    let encoded = encode(&packet);
    assert_eq!(encoded, expected);
    assert_eq!(
        decode_exact::<ClientboundSetEntityEquipment>(&encoded),
        packet
    );
    assert_every_truncated_prefix_fails::<ClientboundSetEntityEquipment>(&encoded);
}

#[test]
fn set_equipment_rejects_invalid_lists_slots_and_continuations() {
    let empty = ClientboundSetEntityEquipment {
        entity_id: 1,
        equipment: Vec::new(),
    };
    assert_eq!(
        empty.encode(&mut Vec::new()).unwrap_err(),
        CodecError::NotSupported("entity equipment list cannot be empty")
    );

    let oversized = ClientboundSetEntityEquipment {
        entity_id: 1,
        equipment: vec![
            EntityEquipment {
                slot: EquipmentSlot::MainHand,
                item: ItemStack::EMPTY,
            };
            MAX_EQUIPMENT_ENTRIES + 1
        ],
    };
    assert_eq!(
        oversized.encode(&mut Vec::new()).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_EQUIPMENT_ENTRIES + 1,
            max: MAX_EQUIPMENT_ENTRIES,
        }
    );

    let mut unknown_slot: &[u8] = &[1, 8];
    assert_eq!(
        ClientboundSetEntityEquipment::decode(&mut unknown_slot).unwrap_err(),
        CodecError::NotSupported("unknown equipment slot")
    );

    let mut unterminated_continuation: &[u8] = &[1, 0x80, 0];
    assert!(
        ClientboundSetEntityEquipment::decode(&mut unterminated_continuation).is_err(),
        "continuation bit must require another equipment entry"
    );

    let mut too_many = vec![1];
    for _ in 0..MAX_EQUIPMENT_ENTRIES {
        too_many.extend_from_slice(&[0x80, 0x00]);
    }
    let mut cursor: &[u8] = &too_many;
    assert_eq!(
        ClientboundSetEntityEquipment::decode(&mut cursor).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_EQUIPMENT_ENTRIES + 1,
            max: MAX_EQUIPMENT_ENTRIES,
        }
    );
}

#[test]
fn update_and_remove_effect_packets_cover_flags_and_round_trip() {
    let update = ClientboundUpdateEntityEffect {
        entity_id: 300,
        effect_id: MobEffectId::new(5).unwrap(),
        amplifier: 2,
        duration_ticks: 600,
        flags: EntityEffectFlags {
            ambient: true,
            visible: true,
            show_icon: true,
            blend: true,
        },
    };
    let update_bytes = vec![0xAC, 0x02, 0x05, 0x02, 0xD8, 0x04, 0x0F];
    assert_eq!(encode(&update), update_bytes);
    assert_eq!(
        decode_exact::<ClientboundUpdateEntityEffect>(&update_bytes),
        update
    );
    assert_every_truncated_prefix_fails::<ClientboundUpdateEntityEffect>(&update_bytes);

    let no_flags = ClientboundUpdateEntityEffect {
        flags: EntityEffectFlags::default(),
        ..update
    };
    assert_eq!(
        decode_exact::<ClientboundUpdateEntityEffect>(&encode(&no_flags)),
        no_flags
    );

    let remove = ClientboundRemoveEntityEffect {
        entity_id: 300,
        effect_id: MobEffectId::new(5).unwrap(),
    };
    let remove_bytes = vec![0xAC, 0x02, 0x05];
    assert_eq!(encode(&remove), remove_bytes);
    assert_eq!(
        decode_exact::<ClientboundRemoveEntityEffect>(&remove_bytes),
        remove
    );
    assert_every_truncated_prefix_fails::<ClientboundRemoveEntityEffect>(&remove_bytes);
}

#[test]
fn effect_packets_reject_negative_registry_ids_and_reserved_flags() {
    let mut negative_update: &[u8] = &[1, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    assert_eq!(
        ClientboundUpdateEntityEffect::decode(&mut negative_update).unwrap_err(),
        CodecError::NotSupported("negative mob-effect registry id")
    );

    let mut reserved_flags: &[u8] = &[1, 0, 0, 0, 0x10];
    assert_eq!(
        ClientboundUpdateEntityEffect::decode(&mut reserved_flags).unwrap_err(),
        CodecError::NotSupported("unknown entity effect flag bits")
    );

    let mut negative_remove: &[u8] = &[1, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    assert_eq!(
        ClientboundRemoveEntityEffect::decode(&mut negative_remove).unwrap_err(),
        CodecError::NotSupported("negative mob-effect registry id")
    );
}

#[test]
fn set_entity_leash_covers_attached_and_detached_round_trips() {
    let attached = ClientboundSetEntityLeash {
        source_entity_id: 0x0102_0304,
        holder_entity_id: Some(0x0506_0708),
    };
    let attached_bytes = vec![1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(encode(&attached), attached_bytes);
    assert_eq!(
        decode_exact::<ClientboundSetEntityLeash>(&attached_bytes),
        attached
    );
    assert_every_truncated_prefix_fails::<ClientboundSetEntityLeash>(&attached_bytes);

    let detached = ClientboundSetEntityLeash {
        source_entity_id: 7,
        holder_entity_id: None,
    };
    let detached_bytes = vec![0, 0, 0, 7, 0, 0, 0, 0];
    assert_eq!(encode(&detached), detached_bytes);
    assert_eq!(
        decode_exact::<ClientboundSetEntityLeash>(&detached_bytes),
        detached
    );
}

#[test]
fn set_entity_leash_rejects_ambiguous_zero_holder() {
    let packet = ClientboundSetEntityLeash {
        source_entity_id: 1,
        holder_entity_id: Some(0),
    };
    assert_eq!(
        packet.encode(&mut Vec::new()).unwrap_err(),
        CodecError::NotSupported("entity leash holder id zero means detached")
    );
}
