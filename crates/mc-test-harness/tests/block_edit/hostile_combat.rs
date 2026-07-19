#[tokio::test]
async fn embedded_skeleton_arrow_damages_player_over_wire() {
    let data = embedded_play_data();
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let skeleton_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:skeleton").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded skeleton entity type");
    let arrow_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded arrow entity type");
    let cfg = embedded_playable_config(&data, embedded_world(&data), "skeleton combat wire");
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "SkelCombatWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let skeleton_position = (sync.x, sync.y, sync.z + 4.0);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:skeleton {} {} {}",
                skeleton_position.0, skeleton_position.1, skeleton_position.2
            ),
        })
        .await
        .expect("summon skeleton");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_skeleton = false;
    let mut saw_arrow = false;
    let mut damaged_health = None;
    while !saw_skeleton || !saw_arrow || damaged_health.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "skeleton arrow damage: {error}; skeleton={saw_skeleton} arrow={saw_arrow} health={damaged_health:?}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode combat AddEntity");
            if packet.entity_type_id == skeleton_entity_type
                && (packet.x - skeleton_position.0).abs() < 0.01
                && (packet.y - skeleton_position.1).abs() < 0.01
                && (packet.z - skeleton_position.2).abs() < 0.01
            {
                saw_skeleton = true;
            } else if packet.entity_type_id == arrow_entity_type && saw_skeleton {
                saw_arrow = true;
            }
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode skeleton damage health");
            if packet.health < 20.0 {
                damaged_health = Some(packet.health);
            }
        }
    }

    assert!(damaged_health.unwrap() < 20.0);
}
