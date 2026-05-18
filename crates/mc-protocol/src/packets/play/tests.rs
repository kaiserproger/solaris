use super::*;
use crate::packets::{ChatVisibility, MainHand, ParticleStatus, ResourcePackAction};

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
fn clientbound_respawn_id_and_layout_match_javap() {
    assert_eq!(ClientboundRespawn::ID, 0x52);
    let packet = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: sample_identifier("overworld"),
        hashed_seed: 0,
        game_mode: 0,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        data_to_keep: 0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, // dimension type id
            0x13, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'o', b'v', b'e',
            b'r', b'w', b'o', b'r', b'l', b'd', 0, 0, 0, 0, 0, 0, 0, 0, 0,    // game mode
            0xFF, // previous game mode
            0, 0,  // debug/flat
            0,  // no death location
            0,  // portal cooldown
            63, // sea level
            0,  // data to keep
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundRespawn::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
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
    assert_eq!(ClientboundSetEntityData::ID, 0x63);
    assert_eq!(ClientboundTakeItemEntity::ID, 0x7C);
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
fn set_entity_data_item_stack_layout_matches_javap() {
    let packet = ClientboundSetEntityData {
        entity_id: 300,
        values: vec![EntityDataValue::ItemStack {
            index: ITEM_ENTITY_DATA_ITEM_INDEX,
            stack: ItemStack::new(5, 1),
        }],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0xAC,
            0x02,
            ITEM_ENTITY_DATA_ITEM_INDEX,
            ENTITY_DATA_ITEM_STACK_SERIALIZER_ID as u8,
            0x01,
            0x05,
            0x00,
            0x00,
            0xFF,
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_entity_data_player_pose_layout_matches_javap() {
    let packet = ClientboundSetEntityData {
        entity_id: 7,
        values: vec![
            EntityDataValue::Byte {
                index: ENTITY_DATA_SHARED_FLAGS_INDEX,
                value: 0x0A,
            },
            EntityDataValue::Pose {
                index: ENTITY_DATA_POSE_INDEX,
                pose: EntityPose::Swimming,
            },
        ],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![7, 0, 0, 0x0A, 6, 20, 3, 0xFF]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn take_item_entity_layout_matches_javap() {
    let packet = ClientboundTakeItemEntity {
        item_entity_id: 300,
        player_entity_id: 2,
        amount: 1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02, 0x02, 0x01]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundTakeItemEntity::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_swing_layout_matches_javap() {
    assert_eq!(ServerboundSwing::ID, 0x3F);
    let packet = ServerboundSwing {
        hand: InteractionHand::MainHand,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundSwing::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
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
fn move_entity_pos_id_and_wire_layout_match_server_javap() {
    assert_eq!(MoveEntityPos::ID, 0x35);
    let packet = MoveEntityPos {
        entity_id: 300,
        delta_x: 4,
        delta_y: -8,
        delta_z: 12,
        on_ground: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![0xAC, 0x02, 0x00, 0x04, 0xFF, 0xF8, 0x00, 0x0C, 0x01]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(MoveEntityPos::decode(&mut cursor).unwrap(), packet);
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
fn clientbound_set_time_id_and_empty_clock_map_layout_match_javap() {
    assert_eq!(ClientboundSetTime::ID, 0x71);
    let packet = ClientboundSetTime { game_time: 6000 };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, [6000_i64.to_be_bytes().as_slice(), &[0]].concat());

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSetTime::decode(&mut cursor).unwrap(), packet);
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
fn serverbound_chat_ack_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundChatAck::ID, 0x06);
    let packet = ServerboundChatAck { offset: 300 };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundChatAck::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn command_suggestion_packets_match_local_decompiled_sources() {
    assert_eq!(ServerboundCommandSuggestion::ID, 0x0F);
    assert_eq!(ClientboundCommandSuggestions::ID, 0x0F);

    let request = ServerboundCommandSuggestion {
        id: 42,
        command: "gamemode ".to_string(),
    };
    let mut buf = Vec::new();
    request.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            42, 0x09, b'g', b'a', b'm', b'e', b'm', b'o', b'd', b'e', b' ',
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundCommandSuggestion::decode(&mut cursor).unwrap(),
        request
    );
    assert!(cursor.is_empty());

    let response = ClientboundCommandSuggestions {
        id: request.id,
        start: request.command.len() as i32,
        length: 0,
        suggestions: Vec::new(),
    };
    let mut buf = Vec::new();
    response.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![42, 9, 0, 0]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundCommandSuggestions::decode(&mut cursor).unwrap(),
        response
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_commands_literal_and_greedy_string_tree_matches_javap_layout() {
    assert_eq!(ClientboundCommands::ID, 0x10);
    let packet = ClientboundCommands {
        nodes: vec![
            CommandNode::root(vec![1, 3]),
            CommandNode::literal("gamemode", vec![2], false).restricted(true),
            CommandNode::literal("creative", Vec::new(), true).restricted(true),
            CommandNode::literal("give", vec![4], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
        ],
        root_index: 0,
    };

    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 5); // node list length
    assert_eq!(buf[1], 0); // root flags
    assert_eq!(buf[2], 2); // root child count
    assert_eq!(buf[3], 1);
    assert_eq!(buf[4], 3);
    assert_eq!(buf[5], 0x21); // restricted literal, not executable

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundCommands::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_system_chat_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ClientboundSystemChat::ID, 0x79);
    let packet = ClientboundSystemChat {
        content_nbt: vec![0x0A, 0x00],
        overlay: false,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x0A, 0x00, 0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSystemChat::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn client_tick_chunk_batch_and_player_loaded_packets_match_local_decompiled_sources() {
    assert_eq!(ServerboundChunkBatchReceived::ID, 0x0B);
    assert_eq!(ServerboundClientTickEnd::ID, 0x0D);
    assert_eq!(ServerboundPlayerLoaded::ID, 0x2C);

    let batch = ServerboundChunkBatchReceived {
        desired_chunks_per_tick: 12.5,
    };
    let mut buf = Vec::new();
    batch.encode(&mut buf).unwrap();
    assert_eq!(buf, 12.5_f32.to_be_bytes());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundChunkBatchReceived::decode(&mut cursor).unwrap(),
        batch
    );
    assert!(cursor.is_empty());

    let mut buf = Vec::new();
    ServerboundClientTickEnd.encode(&mut buf).unwrap();
    assert!(buf.is_empty());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundClientTickEnd::decode(&mut cursor).unwrap(),
        ServerboundClientTickEnd
    );
    assert!(cursor.is_empty());

    let mut buf = Vec::new();
    ServerboundPlayerLoaded.encode(&mut buf).unwrap();
    assert!(buf.is_empty());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundPlayerLoaded::decode(&mut cursor).unwrap(),
        ServerboundPlayerLoaded
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_resource_pack_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundResourcePack::ID, 0x31);
    let packet = ServerboundResourcePack {
        status: ResourcePackStatus {
            id: uuid::Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff),
            action: ResourcePackAction::FailedReload,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x06,
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundResourcePack::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_player_command_id_and_layout_match_protocol_dump() {
    assert_eq!(ServerboundPlayerCommand::ID, 0x2A);
    let packet = ServerboundPlayerCommand {
        entity_id: 123,
        action: PlayerCommandAction::StartSprinting,
        data: 0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![123, 3, 0]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundPlayerCommand::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_player_input_id_and_layout_match_javap() {
    assert_eq!(ServerboundPlayerInput::ID, 0x2B);
    let packet = ServerboundPlayerInput {
        input: PlayerInput {
            forward: true,
            backward: false,
            left: false,
            right: true,
            jump: true,
            shift: false,
            sprint: true,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x59]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundPlayerInput::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn player_abilities_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundPlayerAbilities::ID, 0x40);
    let packet = ClientboundPlayerAbilities {
        invulnerable: true,
        flying: false,
        can_fly: true,
        instabuild: true,
        flying_speed: 0.05,
        walking_speed: 0.1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 0x0d);
    assert_eq!(&buf[1..5], &0.05_f32.to_be_bytes());
    assert_eq!(&buf[5..9], &0.1_f32.to_be_bytes());

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundPlayerAbilities::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_health_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundSetHealth::ID, 0x68);
    let packet = ClientboundSetHealth {
        health: 20.0,
        food: 20,
        saturation: 5.0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[0..4], &20.0_f32.to_be_bytes());
    assert_eq!(buf[4], 20);
    assert_eq!(&buf[5..9], &5.0_f32.to_be_bytes());

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSetHealth::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn set_experience_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundSetExperience::ID, 0x67);
    let packet = ClientboundSetExperience {
        experience_progress: 0.5,
        total_experience: 9,
        experience_level: 1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[0..4], &0.5_f32.to_be_bytes());
    assert_eq!(buf[4], 9);
    assert_eq!(buf[5], 1);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetExperience::decode(&mut cursor).unwrap(),
        packet
    );
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

include!("tests/serverbound_and_slots.rs");
