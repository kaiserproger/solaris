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
    assert_eq!(ServerboundClientInformation::decode(&mut cursor).unwrap(), packet);
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
    assert_eq!(ServerboundCustomPayload::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
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
        0x0A, 0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x07, b'F', b'u', b'r', b'n',
        b'a', b'c', b'e', 0x00,
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
