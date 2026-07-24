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
fn serverbound_attack_id_and_layout_match_local_vanilla() {
    assert_eq!(ServerboundAttack::ID, 0x01);
    let packet = ServerboundAttack { entity_id: 123 };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x7B]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundAttack::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_interact_id_and_layout_match_local_vanilla() {
    assert_eq!(ServerboundInteract::ID, 0x1A);
    let packet = ServerboundInteract {
        entity_id: 123,
        hand: InteractionHand::MainHand,
        location: EntityVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        using_secondary_action: false,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x7B, 0x00, 0x00, 0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundInteract::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_interact_variants_round_trip() {
    round_trip(ServerboundInteract {
        entity_id: 1,
        hand: InteractionHand::MainHand,
        location: EntityVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        using_secondary_action: true,
    });
    round_trip(ServerboundInteract {
        entity_id: 2,
        hand: InteractionHand::OffHand,
        location: EntityVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        using_secondary_action: false,
    });
}

#[test]
fn serverbound_use_item_id_and_layout_match_javap() {
    assert_eq!(ServerboundUseItem::ID, 0x43);
    let packet = ServerboundUseItem {
        hand: InteractionHand::MainHand,
        sequence: 300,
        y_rot: 90.0,
        x_rot: -15.0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, 0xAC, 0x02, 0x42, 0xB4, 0x00, 0x00, 0xC1, 0x70, 0x00, 0x00
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundUseItem::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_client_information_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundClientInformation::ID, 0x0E);
    let packet = ServerboundClientInformation {
        information: ClientInformation {
            language: "ru_ru".to_string(),
            view_distance: 8,
            chat_visibility: ChatVisibility::Hidden,
            chat_colors: false,
            model_customisation: 0x55,
            main_hand: MainHand::Left,
            text_filtering_enabled: true,
            allows_listing: false,
            particle_status: ParticleStatus::Minimal,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x05, b'r', b'u', b'_', b'r', b'u', // ClientInformation.readUtf(16)
            8,    // readByte viewDistance
            2,    // readEnum(ChatVisiblity.HIDDEN)
            0,    // chatColors
            0x55, // readUnsignedByte modelCustomisation
            0,    // readEnum(HumanoidArm.LEFT)
            1,    // textFilteringEnabled
            0,    // allowsListing
            2,    // readEnum(ParticleStatus.MINIMAL)
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundClientInformation::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_custom_payload_brand_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundCustomPayload::ID, 0x16);
    let packet = ServerboundCustomPayload {
        payload: CustomPayload::Brand("vanilla".to_string()),
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x0f, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'b', b'r', b'a',
            b'n', b'd', 0x07, b'v', b'a', b'n', b'i', b'l', b'l', b'a',
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundCustomPayload::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn play_serverbound_custom_payload_rejects_oversized_unknown_body() {
    let mut buf = Vec::new();
    buf.write_identifier(&Identifier::parse("solaris:test").unwrap())
        .unwrap();
    buf.resize(buf.len() + 32_768, 0xEF);

    assert_eq!(
        ServerboundCustomPayload::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::StringTooLong {
            len: 32_768,
            max: 32_767,
        }
    );
}

#[test]
fn malformed_container_click_rejects_unknown_input_mode() {
    let mut buf = Vec::new();
    buf.write_varint(1); // container id
    buf.write_varint(1); // state id
    buf.write_i16(0); // slot
    buf.write_i8(0); // button
    buf.write_varint(99); // invalid ContainerInput

    let mut cursor: &[u8] = &buf;
    assert!(matches!(
        ServerboundContainerClick::decode(&mut cursor),
        Err(CodecError::NotSupported("unknown ContainerInput id"))
    ));
}

#[test]
fn malformed_container_click_rejects_changed_slot_overflow() {
    let mut buf = Vec::new();
    buf.write_varint(1);
    buf.write_varint(1);
    buf.write_i16(0);
    buf.write_i8(0);
    buf.write_varint(ContainerInput::Pickup.as_wire());
    buf.write_varint((MAX_CONTAINER_CLICK_CHANGED_SLOTS + 1) as i32);

    let mut cursor: &[u8] = &buf;
    assert!(matches!(
        ServerboundContainerClick::decode(&mut cursor),
        Err(CodecError::StringTooLong { .. })
    ));
}

#[test]
fn malformed_container_click_rejects_non_positive_actual_stack() {
    let mut buf = Vec::new();
    buf.write_varint(1);
    buf.write_varint(1);
    buf.write_i16(0);
    buf.write_i8(0);
    buf.write_varint(ContainerInput::Pickup.as_wire());
    buf.write_varint(0); // no changed slots
    buf.write_bool(true); // carried item is actual
    buf.write_varint(5); // item id
    buf.write_varint(0); // invalid count

    let mut cursor: &[u8] = &buf;
    assert!(matches!(
        ServerboundContainerClick::decode(&mut cursor),
        Err(CodecError::NotSupported(
            "HashedStack actual item with non-positive count"
        ))
    ));
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
fn level_event_id_and_wire_layout_match_local_vanilla() {
    assert_eq!(LevelEvent::ID, 0x2E);
    let position = pack_block_pos(-2, 70, 3);
    let packet = LevelEvent {
        event_id: 2001,
        position,
        data: 1234,
        global: false,
    };
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();

    let mut expected = Vec::new();
    expected.extend_from_slice(&2001_i32.to_be_bytes());
    expected.extend_from_slice(&position.to_be_bytes());
    expected.extend_from_slice(&1234_i32.to_be_bytes());
    expected.push(0);
    assert_eq!(encoded, expected);

    let mut cursor = encoded.as_slice();
    assert_eq!(LevelEvent::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
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
fn item_stack_damage_component_round_trips() {
    let pickaxe = ItemStack::new(777, 1).with_damage(12);
    let mut buf = Vec::new();
    pickaxe.encode(&mut buf).unwrap();
    // count=1, item_id=777, n_add=1, n_remove=0,
    // DataComponents.DAMAGE id=3, value=12. Per local javap:
    // DataComponentPatch$3 writes add/remove counts first, then
    // DataComponentType.STREAM_CODEC and the component StreamCodec;
    // DataComponents registers `damage` fourth after ids 0..2.
    assert_eq!(buf, vec![0x01, 0x89, 0x06, 0x01, 0x00, 0x03, 0x0C]);
    let mut cur: &[u8] = &buf;
    assert_eq!(ItemStack::decode(&mut cur).unwrap(), pickaxe);
    assert!(cur.is_empty());
}

#[test]
fn item_stack_enchantments_component_matches_vanilla_stream_codec() {
    let efficiency = Identifier::parse("minecraft:efficiency").unwrap();
    let pickaxe = ItemStack::new(777, 1).with_enchantment(efficiency, 1);

    let mut buf = Vec::new();
    pickaxe.encode(&mut buf).unwrap();

    assert_eq!(
        buf,
        vec![
            0x01, 0x89, 0x06, // count 1, item id 777
            0x01, 0x00, // one added component, none removed
            0x0D, // minecraft:enchantments component id
            0x01, // map size
            0x08, 0x01, // minecraft:efficiency registry id, level I
        ]
    );
    let mut cur: &[u8] = &buf;
    assert_eq!(ItemStack::decode(&mut cur).unwrap(), pickaxe);
    assert!(cur.is_empty());
}

#[test]
fn item_stack_custom_name_component_matches_26_1_2_component_stream_codec() {
    let named = ItemStack::new(5, 1).with_custom_name("Catalog Apple");

    let mut buf = Vec::new();
    named.encode(&mut buf).unwrap();

    // `minecraft:custom_name` is DataComponents registry id 6 in the local
    // 26.1.2 registry report. Its component payload is a network NBT text
    // component, as confirmed by DataComponents.CUSTOM_NAME's Component
    // STREAM_CODEC through javap.
    assert_eq!(
        buf,
        vec![
            0x01, 0x05, 0x01, 0x00, 0x06, // stack patch header and component id
            0x0a, // network-NBT root compound (the component codec has no root name)
            0x08, 0x00, 0x04, b't', b'e', b'x', b't',
            0x00, 0x0d, b'C', b'a', b't', b'a', b'l', b'o', b'g', b' ', b'A', b'p', b'p', b'l', b'e',
            0x00,
        ]
    );
    let mut cur: &[u8] = &buf;
    assert_eq!(ItemStack::decode(&mut cur).unwrap(), named);
    assert!(cur.is_empty());
}

#[test]
fn item_stack_item_model_component_matches_26_1_2_registry_codec() {
    let model = Identifier::parse("solaris_loader:loader_block").unwrap();
    let stack = ItemStack::new(5, 1).with_item_model(model);
    let mut buf = Vec::new();
    stack.encode(&mut buf).unwrap();

    assert_eq!(
        buf,
        vec![
            0x01, 0x05, 0x01, 0x00, 0x0A, 0x1b, b's', b'o', b'l', b'a', b'r', b'i', b's', b'_',
            b'l', b'o', b'a', b'd', b'e', b'r', b':', b'l', b'o', b'a', b'd', b'e', b'r', b'_',
            b'b', b'l', b'o', b'c', b'k',
        ]
    );
    let mut cur: &[u8] = &buf;
    assert_eq!(ItemStack::decode(&mut cur).unwrap(), stack);
    assert!(cur.is_empty());
}

#[test]
fn item_stack_decoder_refuses_unsupported_component_patches() {
    // count=1, item_id=1, n_add=1, n_remove=0, unsupported component id=4.
    let bytes: Vec<u8> = vec![0x01, 0x01, 0x01, 0x00, 0x04];
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
fn clientbound_container_close_and_data_layout_match_local_vanilla() {
    assert_eq!(ClientboundContainerClose::ID, 0x11);
    let close = ClientboundContainerClose { container_id: 3 };
    let mut buf = Vec::new();
    close.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x03]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundContainerClose::decode(&mut cursor).unwrap(),
        close
    );

    assert_eq!(ClientboundContainerSetData::ID, 0x13);
    let data = ClientboundContainerSetData {
        container_id: 3,
        id: 2,
        value: 200,
    };
    let mut buf = Vec::new();
    data.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x03, 0x00, 0x02, 0x00, 0xC8]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundContainerSetData::decode(&mut cursor).unwrap(),
        data
    );
}

#[test]
fn clientbound_open_screen_layout_matches_local_vanilla() {
    assert_eq!(ClientboundOpenScreen::ID, 0x3B);
    // Minimal network NBT component: Compound { text: "Furnace" }.
    let title_nbt = vec![
        0x0A, 0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x07, b'F', b'u', b'r', b'n', b'a',
        b'c', b'e', 0x00,
    ];
    let packet = ClientboundOpenScreen {
        container_id: 1,
        menu_type: 14,
        title_nbt,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[..2], &[0x01, 0x0E]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundOpenScreen::decode(&mut cursor).unwrap(), packet);
}

#[test]
fn serverbound_container_button_click_layout_matches_local_vanilla() {
    assert_eq!(ServerboundContainerButtonClick::ID, 0x11);
    let packet = ServerboundContainerButtonClick {
        container_id: 300,
        button_id: 2,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02, 0x02]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundContainerButtonClick::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_explode_matches_vanilla_tnt_fixture() {
    let packet = ClientboundExplode {
        center: EntityVec3 {
            x: 1.5,
            y: 64.0625,
            z: -2.5,
        },
        radius: 4.0,
        block_count: 1,
        knockback: Some(EntityVec3 {
            x: 0.25,
            y: 0.5,
            z: -0.75,
        }),
        explosion_particle_id: 22,
        sound_reference_id: 697,
        block_particles: vec![
            ExplosionBlockParticle {
                particle_id: 59,
                scaling: 0.5,
                speed: 1.0,
                weight: 1,
            },
            ExplosionBlockParticle {
                particle_id: 62,
                scaling: 1.0,
                speed: 1.0,
                weight: 1,
            },
        ],
    };
    let expected: [u8; 81] = [
        0x3f, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x50, 0x04, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xc0, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x01, 0x3f, 0xd0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0xe0, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xbf, 0xe8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x16, 0xba, 0x05,
        0x02, 0x3b, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x80, 0x00, 0x00, 0x01, 0x3e, 0x3f, 0x80, 0x00,
        0x00, 0x3f, 0x80, 0x00, 0x00, 0x01,
    ];

    assert_eq!(ClientboundExplode::ID, 0x24);
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();
    assert_eq!(encoded, expected);

    let mut cursor: &[u8] = &expected;
    assert_eq!(ClientboundExplode::decode(&mut cursor).unwrap(), packet);
    assert!(
        cursor.is_empty(),
        "decoder must consume the exact packet body"
    );
}

#[test]
fn clientbound_explode_round_trips_without_knockback() {
    round_trip(simple_explode_packet());
}

fn simple_explode_packet() -> ClientboundExplode {
    ClientboundExplode {
        center: EntityVec3 {
            x: 0.0,
            y: -64.0,
            z: 12.25,
        },
        radius: 0.0,
        block_count: 0,
        knockback: None,
        explosion_particle_id: 22,
        sound_reference_id: 697,
        block_particles: Vec::new(),
    }
}

fn explode_body_through_sound_holder(explosion_particle_id: i32, sound_holder: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.write_f64(0.0);
    bytes.write_f64(0.0);
    bytes.write_f64(0.0);
    bytes.write_f32(4.0);
    bytes.write_i32(0);
    bytes.write_bool(false);
    bytes.write_varint(explosion_particle_id);
    bytes.write_varint(sound_holder);
    bytes
}

#[test]
fn clientbound_explode_rejects_negative_block_particle_count() {
    let mut bytes = explode_body_through_sound_holder(22, 698);
    bytes.write_varint(-1);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err, CodecError::NegativeLength(-1));
}

#[test]
fn clientbound_explode_rejects_oversized_block_particle_count() {
    let mut bytes = explode_body_through_sound_holder(22, 698);
    bytes.write_varint((MAX_EXPLOSION_BLOCK_PARTICLES + 1) as i32);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(
        err,
        CodecError::StringTooLong {
            len: MAX_EXPLOSION_BLOCK_PARTICLES + 1,
            max: MAX_EXPLOSION_BLOCK_PARTICLES,
        }
    );
}

#[test]
fn clientbound_explode_rejects_inline_sound_holder() {
    let bytes = explode_body_through_sound_holder(22, 0);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("inline explosion sound holder")
    );
}

#[test]
fn clientbound_explode_encode_rejects_unsupported_explosion_particle() {
    let mut packet = simple_explode_packet();
    packet.explosion_particle_id = 23;

    let err = packet.encode(&mut Vec::new()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("unsupported simple explosion particle id")
    );
}

#[test]
fn clientbound_explode_decode_rejects_unsupported_explosion_particle() {
    let mut bytes = explode_body_through_sound_holder(23, 698);
    bytes.write_varint(0);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("unsupported simple explosion particle id")
    );
}

#[test]
fn clientbound_explode_encode_rejects_unsupported_block_particle() {
    let mut packet = simple_explode_packet();
    packet.block_particles.push(ExplosionBlockParticle {
        particle_id: 60,
        scaling: 1.0,
        speed: 1.0,
        weight: 1,
    });

    let err = packet.encode(&mut Vec::new()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("unsupported simple explosion particle id")
    );
}

#[test]
fn clientbound_explode_decode_rejects_unsupported_block_particle() {
    let mut bytes = explode_body_through_sound_holder(22, 698);
    bytes.write_varint(1);
    bytes.write_varint(60);
    bytes.write_f32(1.0);
    bytes.write_f32(1.0);
    bytes.write_varint(1);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("unsupported simple explosion particle id")
    );
}

#[test]
fn clientbound_explode_encode_rejects_negative_block_particle_weight() {
    let mut packet = simple_explode_packet();
    packet.block_particles.push(ExplosionBlockParticle {
        particle_id: 59,
        scaling: 1.0,
        speed: 1.0,
        weight: -1,
    });

    let err = packet.encode(&mut Vec::new()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("negative explosion block particle weight")
    );
}

#[test]
fn clientbound_explode_decode_rejects_negative_block_particle_weight() {
    let mut bytes = explode_body_through_sound_holder(22, 698);
    bytes.write_varint(1);
    bytes.write_varint(59);
    bytes.write_f32(1.0);
    bytes.write_f32(1.0);
    bytes.write_varint(-1);

    let err = ClientboundExplode::decode(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(
        err,
        CodecError::NotSupported("negative explosion block particle weight")
    );
}

#[test]
fn serverbound_container_click_layout_matches_local_vanilla() {
    assert_eq!(ServerboundContainerClick::ID, 0x12);
    let packet = ServerboundContainerClick {
        container_id: 0,
        state_id: 9,
        slot_num: 36,
        button_num: 0,
        container_input: ContainerInput::Pickup,
        changed_slots: vec![(36, HashedStack::empty())],
        carried_item: HashedStack::Actual {
            item_id: 7,
            count: 64,
            components: HashedStackComponentHashes::empty(),
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, 0x09, // container id, state id
            0x00, 0x24, // slot short 36
            0x00, // button byte
            0x00, // PICKUP
            0x01, // changed slot count
            0x00, 0x24, // changed slot key
            0x00, // changed slot hashed stack empty
            0x01, // carried item present
            0x07, 0x40, // item id, count
            0x00, 0x00, // hashed patch add/remove counts
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundContainerClick::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_container_click_rejects_invalid_input_and_hashed_count() {
    let invalid_input = [0x00, 0x01, 0x00, 0x24, 0x00, 0x07, 0x00, 0x00];
    let mut cursor: &[u8] = &invalid_input;
    assert!(matches!(
        ServerboundContainerClick::decode(&mut cursor),
        Err(CodecError::NotSupported(_))
    ));

    let invalid_hashed_count = [
        0x00, 0x01, // container id, state id
        0x00, 0x24, // slot
        0x00, // button
        0x00, // PICKUP
        0x00, // changed slots
        0x01, // carried item present
        0x07, // item id
        0x00, // invalid actual count
    ];
    let mut cursor: &[u8] = &invalid_hashed_count;
    assert!(matches!(
        ServerboundContainerClick::decode(&mut cursor),
        Err(CodecError::NotSupported(_))
    ));
}

fn container_click_with_hashed_components(added: usize, removed: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.write_varint(0); // container id
    buf.write_varint(1); // state id
    buf.write_i16(36); // slot
    buf.write_i8(0); // button
    buf.write_varint(ContainerInput::Pickup.as_wire());
    buf.write_varint(0); // changed slots
    buf.write_bool(true); // carried item present
    buf.write_varint(7); // item id
    buf.write_varint(1); // item count
    buf.write_varint(added as i32);
    for component_id in 0..added {
        buf.write_varint(component_id as i32);
        buf.write_i32(component_id as i32);
    }
    buf.write_varint(removed as i32);
    for component_id in 0..removed {
        buf.write_varint(component_id as i32);
    }
    buf
}

fn hashed_stack_with_components(added: usize, removed: usize) -> HashedStack {
    HashedStack::Actual {
        item_id: 7,
        count: 1,
        components: HashedStackComponentHashes {
            added: (0..added).map(|id| (id as i32, id as i32)).collect(),
            removed: (0..removed).map(|id| id as i32).collect(),
        },
    }
}

fn raw_container_click_with_stacks(
    changed_slots: &[(i16, HashedStack)],
    carried_item: &HashedStack,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.write_varint(0);
    buf.write_varint(1);
    buf.write_i16(36);
    buf.write_i8(0);
    buf.write_varint(ContainerInput::Pickup.as_wire());
    buf.write_varint(changed_slots.len() as i32);
    for (slot, stack) in changed_slots {
        buf.write_i16(*slot);
        stack.encode(&mut buf).unwrap();
    }
    carried_item.encode(&mut buf).unwrap();
    buf
}

fn container_click_packet(
    changed_slots: Vec<(i16, HashedStack)>,
    carried_item: HashedStack,
) -> ServerboundContainerClick {
    ServerboundContainerClick {
        container_id: 0,
        state_id: 1,
        slot_num: 36,
        button_num: 0,
        container_input: ContainerInput::Pickup,
        changed_slots,
        carried_item,
    }
}

#[test]
fn serverbound_container_click_accepts_zero_and_exact_changed_slot_ceiling() {
    for changed_len in [0, 128] {
        let packet = container_click_packet(
            (0..changed_len)
                .map(|slot| (slot as i16, HashedStack::empty()))
                .collect(),
            HashedStack::empty(),
        );
        let mut encoded = Vec::new();

        packet.encode(&mut encoded).unwrap();
        assert_eq!(
            ServerboundContainerClick::decode(&mut encoded.as_slice()).unwrap(),
            packet
        );
    }
}

#[test]
fn serverbound_container_click_encode_rejects_changed_slot_overflow_without_writing() {
    let packet = container_click_packet(
        (0..129).map(|slot| (slot, HashedStack::empty())).collect(),
        HashedStack::empty(),
    );
    let mut encoded = vec![0xA5, 0x5A];

    assert_eq!(
        packet.encode(&mut encoded).unwrap_err(),
        CodecError::StringTooLong { len: 129, max: 128 }
    );
    assert_eq!(encoded, [0xA5, 0x5A]);
}

#[test]
fn serverbound_container_click_decode_accepts_exact_aggregate_component_hash_budget() {
    let changed_slots: Vec<_> = (0..8)
        .map(|slot| (slot, hashed_stack_with_components(256, 256)))
        .collect();
    let packet = container_click_packet(changed_slots, HashedStack::empty());
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();

    let decoded = ServerboundContainerClick::decode(&mut buf.as_slice()).unwrap();
    assert_eq!(decoded, packet);
}

#[test]
fn serverbound_container_click_decode_rejects_aggregate_component_hash_overflow() {
    let changed_slots: Vec<_> = (0..8)
        .map(|slot| (slot, hashed_stack_with_components(256, 256)))
        .collect();
    let carried = hashed_stack_with_components(1, 0);
    let buf = raw_container_click_with_stacks(&changed_slots, &carried);

    assert_eq!(
        ServerboundContainerClick::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::StringTooLong {
            len: 4097,
            max: 4096,
        }
    );
}

#[test]
fn serverbound_container_click_encode_preflights_nested_and_aggregate_limits() {
    let cases = [
        (
            container_click_packet(
                vec![(0, hashed_stack_with_components(257, 0))],
                HashedStack::empty(),
            ),
            CodecError::StringTooLong { len: 257, max: 256 },
        ),
        (
            container_click_packet(
                vec![(0, hashed_stack_with_components(0, 257))],
                HashedStack::empty(),
            ),
            CodecError::StringTooLong { len: 257, max: 256 },
        ),
        (
            container_click_packet(
                (0..8)
                    .map(|slot| (slot, hashed_stack_with_components(256, 256)))
                    .collect(),
                hashed_stack_with_components(1, 0),
            ),
            CodecError::StringTooLong {
                len: 4097,
                max: 4096,
            },
        ),
        (
            container_click_packet(
                Vec::new(),
                HashedStack::Actual {
                    item_id: 7,
                    count: 0,
                    components: HashedStackComponentHashes::empty(),
                },
            ),
            CodecError::NotSupported("HashedStack actual item with non-positive count"),
        ),
        (
            container_click_packet(
                Vec::new(),
                HashedStack::Actual {
                    item_id: i32::MAX as u32 + 1,
                    count: 1,
                    components: HashedStackComponentHashes::empty(),
                },
            ),
            CodecError::NotSupported("hashed stack item id exceeds VarInt range"),
        ),
    ];

    for (packet, expected) in cases {
        let mut encoded = vec![0xA5, 0x5A];
        assert_eq!(packet.encode(&mut encoded).unwrap_err(), expected);
        assert_eq!(encoded, [0xA5, 0x5A]);
    }
}

#[test]
fn serverbound_container_click_decode_rejects_malformed_changed_slot_counts() {
    fn header() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.write_varint(0);
        buf.write_varint(1);
        buf.write_i16(36);
        buf.write_i8(0);
        buf.write_varint(ContainerInput::Pickup.as_wire());
        buf
    }

    let mut negative = header();
    negative.write_varint(-1);
    assert_eq!(
        ServerboundContainerClick::decode(&mut negative.as_slice()).unwrap_err(),
        CodecError::NegativeLength(-1)
    );

    let mut overlong = header();
    overlong.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80]);
    assert_eq!(
        ServerboundContainerClick::decode(&mut overlong.as_slice()).unwrap_err(),
        CodecError::VarIntTooLong
    );

    let mut truncated = header();
    truncated.push(0x80);
    assert!(matches!(
        ServerboundContainerClick::decode(&mut truncated.as_slice()),
        Err(CodecError::Underflow { .. })
    ));

    let mut max_without_payload = header();
    max_without_payload.write_varint(128);
    assert!(matches!(
        ServerboundContainerClick::decode(&mut max_without_payload.as_slice()),
        Err(CodecError::Underflow { .. })
    ));
}

#[test]
fn serverbound_container_click_decode_rejects_truncated_nested_hash_counts() {
    let mut buf = Vec::new();
    buf.write_varint(0);
    buf.write_varint(1);
    buf.write_i16(36);
    buf.write_i8(0);
    buf.write_varint(ContainerInput::Pickup.as_wire());
    buf.write_varint(0);
    buf.write_bool(true);
    buf.write_varint(7);
    buf.write_varint(1);
    buf.write_varint(256);

    assert!(matches!(
        ServerboundContainerClick::decode(&mut buf.as_slice()),
        Err(CodecError::Underflow { .. })
    ));
}

#[test]
fn serverbound_container_click_decode_rejects_malformed_nested_hash_counts() {
    fn through_actual_stack() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.write_varint(0);
        buf.write_varint(1);
        buf.write_i16(36);
        buf.write_i8(0);
        buf.write_varint(ContainerInput::Pickup.as_wire());
        buf.write_varint(0);
        buf.write_bool(true);
        buf.write_varint(7);
        buf.write_varint(1);
        buf
    }

    let mut negative_added = through_actual_stack();
    negative_added.write_varint(-1);
    assert_eq!(
        ServerboundContainerClick::decode(&mut negative_added.as_slice()).unwrap_err(),
        CodecError::NegativeLength(-1)
    );

    let mut overlong_added = through_actual_stack();
    overlong_added.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80]);
    assert_eq!(
        ServerboundContainerClick::decode(&mut overlong_added.as_slice()).unwrap_err(),
        CodecError::VarIntTooLong
    );

    let mut truncated_added = through_actual_stack();
    truncated_added.push(0x80);
    assert!(matches!(
        ServerboundContainerClick::decode(&mut truncated_added.as_slice()),
        Err(CodecError::Underflow { .. })
    ));

    let mut negative_removed = through_actual_stack();
    negative_removed.write_varint(0);
    negative_removed.write_varint(-1);
    assert_eq!(
        ServerboundContainerClick::decode(&mut negative_removed.as_slice()).unwrap_err(),
        CodecError::NegativeLength(-1)
    );

    let mut overlong_removed = through_actual_stack();
    overlong_removed.write_varint(0);
    overlong_removed.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80]);
    assert_eq!(
        ServerboundContainerClick::decode(&mut overlong_removed.as_slice()).unwrap_err(),
        CodecError::VarIntTooLong
    );

    let mut truncated_removed = through_actual_stack();
    truncated_removed.write_varint(0);
    truncated_removed.push(0x80);
    assert!(matches!(
        ServerboundContainerClick::decode(&mut truncated_removed.as_slice()),
        Err(CodecError::Underflow { .. })
    ));
}

#[test]
fn serverbound_container_click_accepts_exact_hashed_component_count_ceilings() {
    let buf = container_click_with_hashed_components(256, 256);
    let packet = ServerboundContainerClick::decode(&mut buf.as_slice()).unwrap();

    let HashedStack::Actual { components, .. } = packet.carried_item else {
        panic!("expected actual carried item");
    };
    assert_eq!(components.added.len(), 256);
    assert_eq!(components.removed.len(), 256);
}

#[test]
fn serverbound_container_click_rejects_added_hashed_component_count_over_ceiling() {
    let buf = container_click_with_hashed_components(257, 0);
    assert_eq!(
        ServerboundContainerClick::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::StringTooLong { len: 257, max: 256 }
    );
}

#[test]
fn serverbound_container_click_rejects_removed_hashed_component_count_over_ceiling() {
    let buf = container_click_with_hashed_components(0, 257);
    assert_eq!(
        ServerboundContainerClick::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::StringTooLong { len: 257, max: 256 }
    );
}

#[test]
fn hashed_stack_component_hashes_encode_accepts_exact_count_ceilings() {
    let components = HashedStackComponentHashes {
        added: (0..256).map(|id| (id, id)).collect(),
        removed: (0..256).collect(),
    };
    let mut encoded = Vec::new();

    components.encode(&mut encoded).unwrap();

    let decoded = HashedStackComponentHashes::decode(&mut encoded.as_slice()).unwrap();
    assert_eq!(decoded, components);
}

#[test]
fn hashed_stack_component_hashes_encode_rejects_added_over_ceiling_without_writing() {
    let components = HashedStackComponentHashes {
        added: (0..257).map(|id| (id, id)).collect(),
        removed: Vec::new(),
    };
    let mut encoded = vec![0xA5, 0x5A];

    assert_eq!(
        components.encode(&mut encoded).unwrap_err(),
        CodecError::StringTooLong { len: 257, max: 256 }
    );
    assert_eq!(encoded, [0xA5, 0x5A]);
}

#[test]
fn hashed_stack_component_hashes_encode_rejects_removed_over_ceiling_without_writing() {
    let components = HashedStackComponentHashes {
        added: vec![(1, 2)],
        removed: (0..257).collect(),
    };
    let mut encoded = vec![0xA5, 0x5A];

    assert_eq!(
        components.encode(&mut encoded).unwrap_err(),
        CodecError::StringTooLong { len: 257, max: 256 }
    );
    assert_eq!(encoded, [0xA5, 0x5A]);
}

#[test]
fn set_carried_item_round_trip() {
    round_trip(ServerboundSetCarriedItem { slot: 0 });
    round_trip(ServerboundSetCarriedItem { slot: 8 });
}

#[test]
fn serverbound_place_recipe_id_and_layout_match_javap() {
    assert_eq!(ServerboundPlaceRecipe::ID, 0x27);
    let packet = ServerboundPlaceRecipe {
        container_id: 0,
        recipe_display_id: 300,
        use_max_items: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x00, 0xAC, 0x02, 0x01]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundPlaceRecipe::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_container_close_layout_matches_local_vanilla() {
    assert_eq!(ServerboundContainerClose::ID, 0x13);
    let packet = ServerboundContainerClose { container_id: 7 };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x07]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundContainerClose::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_recipe_book_packets_match_local_vanilla() {
    assert_eq!(ServerboundRecipeBookChangeSettings::ID, 0x2E);
    let settings = ServerboundRecipeBookChangeSettings {
        book_type: RecipeBookType::Furnace,
        is_open: true,
        is_filtering: false,
    };
    let mut buf = Vec::new();
    settings.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x01, 0x01, 0x00]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundRecipeBookChangeSettings::decode(&mut cursor).unwrap(),
        settings
    );
    assert!(cursor.is_empty());

    assert_eq!(ServerboundRecipeBookSeenRecipe::ID, 0x2F);
    let seen = ServerboundRecipeBookSeenRecipe {
        recipe_display_id: 300,
    };
    let mut buf = Vec::new();
    seen.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundRecipeBookSeenRecipe::decode(&mut cursor).unwrap(),
        seen
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_recipe_book_packets_match_local_vanilla_2612() {
    assert_eq!(ClientboundRecipeBookSettings::ID, 0x4C);
    let settings = ClientboundRecipeBookSettings::default();
    let mut settings_bytes = Vec::new();
    settings.encode(&mut settings_bytes).unwrap();
    assert_eq!(settings_bytes, vec![0; 8]);
    let mut cursor: &[u8] = &settings_bytes;
    assert_eq!(
        ClientboundRecipeBookSettings::decode(&mut cursor).unwrap(),
        settings
    );
    assert!(cursor.is_empty());

    assert_eq!(ClientboundRecipeBookAdd::ID, 0x4A);
    let birch_logs = sample_identifier("minecraft:birch_logs");
    let packet = ClientboundRecipeBookAdd {
        entries: vec![RecipeBookEntry {
            display_id: 2,
            display: RecipeBookDisplay::Shapeless {
                ingredients: vec![RecipeBookSlotDisplay::Tag(birch_logs.clone())],
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 5,
                    count: 4,
                },
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 6 },
            },
            group: None,
            category_id: 0,
            crafting_requirements: Some(vec![RecipeBookIngredient::Tag(birch_logs)]),
            flags: 0,
        }],
        replace: true,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut expected = vec![0x01, 0x02, 0x00, 0x01, 0x06, 0x14];
    expected.extend_from_slice(b"minecraft:birch_logs");
    expected.extend_from_slice(&[0x05, 0x05, 0x04, 0x00, 0x00, 0x04, 0x06]);
    expected.extend_from_slice(&[0x00, 0x00, 0x01, 0x01, 0x00, 0x14]);
    expected.extend_from_slice(b"minecraft:birch_logs");
    expected.extend_from_slice(&[0x00, 0x01]);
    assert_eq!(bytes, expected);

    let mut cursor: &[u8] = &bytes;
    assert_eq!(
        ClientboundRecipeBookAdd::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_recipe_book_round_trips_supported_display_variants() {
    round_trip(ClientboundRecipeBookAdd {
        entries: vec![
            RecipeBookEntry {
                display_id: 7,
                display: RecipeBookDisplay::Shaped {
                    width: 2,
                    height: 1,
                    ingredients: vec![
                        RecipeBookSlotDisplay::Item { item_id: 1 },
                        RecipeBookSlotDisplay::Empty,
                    ],
                    result: RecipeBookSlotDisplay::ItemStack {
                        item_id: 2,
                        count: 3,
                    },
                    crafting_station: RecipeBookSlotDisplay::Item { item_id: 4 },
                },
                group: Some(3),
                category_id: 2,
                crafting_requirements: Some(vec![RecipeBookIngredient::Items(vec![1])]),
                flags: 0,
            },
            RecipeBookEntry {
                display_id: 8,
                display: RecipeBookDisplay::Furnace {
                    ingredient: RecipeBookSlotDisplay::Composite(vec![
                        RecipeBookSlotDisplay::Item { item_id: 9 },
                        RecipeBookSlotDisplay::Tag(sample_identifier("minecraft:logs")),
                    ]),
                    fuel: RecipeBookSlotDisplay::AnyFuel,
                    result: RecipeBookSlotDisplay::ItemStack {
                        item_id: 10,
                        count: 1,
                    },
                    crafting_station: RecipeBookSlotDisplay::Item { item_id: 11 },
                    duration: 200,
                    experience: 0.35,
                },
                group: None,
                category_id: 4,
                crafting_requirements: None,
                flags: 0,
            },
        ],
        replace: false,
    });
}

#[test]
fn clientbound_shaped_recipe_display_writes_ingredient_list_length() {
    let packet = ClientboundRecipeBookAdd {
        entries: vec![RecipeBookEntry {
            display_id: 7,
            display: RecipeBookDisplay::Shaped {
                width: 2,
                height: 1,
                ingredients: vec![
                    RecipeBookSlotDisplay::Item { item_id: 1 },
                    RecipeBookSlotDisplay::Empty,
                ],
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 2,
                    count: 3,
                },
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 4 },
            },
            group: Some(3),
            category_id: 2,
            crafting_requirements: Some(vec![RecipeBookIngredient::Items(vec![1])]),
            flags: 0,
        }],
        replace: true,
    };

    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();

    assert_eq!(
        bytes,
        vec![
            0x01, 0x07, 0x01, 0x02, 0x01, 0x02, 0x04, 0x01, 0x00, 0x05, 0x02, 0x03, 0x00, 0x00,
            0x04, 0x04, 0x04, 0x02, 0x01, 0x01, 0x02, 0x01, 0x00, 0x01,
        ]
    );
}

#[test]
fn serverbound_client_command_respawn_layout_matches_javap() {
    assert_eq!(ServerboundClientCommand::ID, 0x0C);
    let packet = ServerboundClientCommand {
        action: ClientCommandAction::PerformRespawn,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundClientCommand::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}
