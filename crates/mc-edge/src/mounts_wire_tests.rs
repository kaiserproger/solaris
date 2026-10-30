use super::*;
use mc_protocol::packets::play::{
    ClientboundMoveVehicle, ClientboundSetEntityLeash, ClientboundSetPassengers,
};
fn body(packet: &PlannedPacket) -> bytes::Bytes {
    let mut frame = BytesMut::from(packet.data.as_ref());
    mc_protocol::frame::try_decode_frame(&mut frame, Compression::Threshold(256))
        .unwrap()
        .unwrap()
        .body
}
#[test]
fn sleeping_villager_publishes_pose_and_bed_head_then_clears_them() {
    let entity = EntityId(42);
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:villager")
        .unwrap()
        .protocol_id;
    let sleeping = EntityAppearance {
        pose: 2,
        extra: vec![(
            14,
            mc_core::boundary::EntityDataValue::OptionalBlockPos(Some([8, 201, 12])),
        )],
        ..Default::default()
    };
    let awake = EntityAppearance {
        extra: vec![(
            14,
            mc_core::boundary::EntityDataValue::OptionalBlockPos(None),
        )],
        ..Default::default()
    };
    for (appearance, previous, pose, bed) in [
        (
            &sleeping,
            None,
            EntityPose::Sleeping,
            Some(mc_protocol::packets::play::pack_block_pos(8, 201, 12)),
        ),
        (&awake, Some(&sleeping), EntityPose::Standing, None),
    ] {
        let mut packets = Vec::new();
        push_appearance_for(
            &mut packets,
            ConnectionId(7),
            entity,
            appearance,
            type_id,
            previous,
        );
        let packet = packets
            .iter()
            .find(|packet| packet.id == ClientboundSetEntityData::ID)
            .unwrap();
        let decoded = ClientboundSetEntityData::decode(&mut body(packet)).unwrap();
        assert!(decoded.values.contains(&WireEntityDataValue::Pose {
            index: ENTITY_DATA_POSE_INDEX,
            pose,
        }));
        assert!(decoded.values.contains(&WireEntityDataValue::OptionalBlockPosition {
            index: 14,
            value: bed,
        }));
    }
}

#[test]
fn pairing_and_skipped_snapshot_links_converge_with_explicit_detach_packets() {
    let conn = ConnectionId(7);
    let entity = EntityId(42);
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:horse")
        .unwrap()
        .protocol_id;
    let attached = EntityAppearance {
        passengers: vec![EntityId(7)],
        leash_holder: Some(EntityId(8)),
        ..Default::default()
    };
    let mut packets = Vec::new();
    push_appearance_for(&mut packets, conn, entity, &attached, type_id, None);
    let passenger = packets
        .iter()
        .find(|p| p.id == ClientboundSetPassengers::ID)
        .unwrap();
    assert_eq!(
        ClientboundSetPassengers::decode(&mut body(passenger))
            .unwrap()
            .passenger_ids,
        vec![7]
    );
    let leash = packets
        .iter()
        .find(|p| p.id == ClientboundSetEntityLeash::ID)
        .unwrap();
    assert_eq!(
        ClientboundSetEntityLeash::decode(&mut body(leash))
            .unwrap()
            .holder_entity_id,
        Some(8)
    );
    let detached = EntityAppearance::default();
    packets.clear();
    push_appearance_for(
        &mut packets,
        conn,
        entity,
        &detached,
        type_id,
        Some(&attached),
    );
    let passenger = packets
        .iter()
        .find(|p| p.id == ClientboundSetPassengers::ID)
        .unwrap();
    assert!(
        ClientboundSetPassengers::decode(&mut body(passenger))
            .unwrap()
            .passenger_ids
            .is_empty()
    );
    let leash = packets
        .iter()
        .find(|p| p.id == ClientboundSetEntityLeash::ID)
        .unwrap();
    assert!(
        ClientboundSetEntityLeash::decode(&mut body(leash))
            .unwrap()
            .holder_entity_id
            .is_none()
    );
}
#[test]
fn tame_owner_reference_and_vehicle_correction_use_exact_protocol_codecs() {
    let owner = uuid::Uuid::new_v4();
    let appearance = EntityAppearance {
        extra: vec![(
            19,
            mc_core::boundary::EntityDataValue::OptionalLivingEntityReference(Some(owner)),
        )],
        ..Default::default()
    };
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:wolf")
        .unwrap()
        .protocol_id;
    let mut packets = Vec::new();
    push_appearance_for(
        &mut packets,
        ConnectionId(7),
        EntityId(42),
        &appearance,
        type_id,
        None,
    );
    let metadata = packets
        .iter()
        .find(|p| p.id == ClientboundSetEntityData::ID)
        .unwrap();
    let decoded = ClientboundSetEntityData::decode(&mut body(metadata)).unwrap();
    assert!(decoded.values.iter().any(|v|matches!(v,WireEntityDataValue::OptionalLivingEntityReference { index:19,value:Some(id) } if *id==owner)));
    let change = mc_core::boundary::PlayerChange::VehicleCorrected {
        position: [1.0, 65.0, -3.0],
        yaw: 90.0,
        pitch: 2.0,
    };
    let mut viewer = ViewerState::new(ConnectionId(7), EntityId(7), [0.0; 3]);
    let packets = plan_player_changes(&[change], &mut viewer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, 0x3a);
    let decoded = ClientboundMoveVehicle::decode(&mut body(&packets[0])).unwrap();
    assert_eq!(
        (decoded.x, decoded.y, decoded.z, decoded.yaw, decoded.pitch),
        (1.0, 65.0, -3.0, 90.0, 2.0)
    );
}

#[test]
fn leash_knot_wire_spawn_uses_integer_attachment_not_render_position() {
    let entity = EntityView {
        id: EntityId(42),
        uuid: uuid::Uuid::nil(),
        type_id: mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:leash_knot")
            .unwrap()
            .protocol_id,
        position: [1.5, 65.375, -2.5],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: false,
        appearance: Arc::new(EntityAppearance::default()),
    };
    assert_eq!(entity_spawn_position(&entity), [1.0, 65.0, -3.0]);
}
