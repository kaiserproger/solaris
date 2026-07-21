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

#[tokio::test]
async fn embedded_creeper_fuses_explodes_and_damages_player_over_wire() {
    let data = embedded_play_data();
    let creeper_entity_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:creeper").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded creeper entity type");
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&data.report)
        .with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(vec![0.0; 29_873])
                .expect("test explosion resistance table"),
        );
    let mut cfg = embedded_playable_config(&data, embedded_world(&data), "creeper combat wire");
    cfg.block_facts = Arc::new(block_facts);
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "CreeperWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let creeper_position = (sync.x, sync.y, sync.z + 1.5);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:creeper {} {} {}",
                creeper_position.0, creeper_position.1, creeper_position.2
            ),
        })
        .await
        .expect("summon creeper");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut creeper_id = None;
    let mut saw_remove = false;
    let mut saw_explosion = false;
    let mut damaged_health = None;
    while creeper_id.is_none() || !saw_remove || !saw_explosion || damaged_health.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "creeper explosion: {error}; creeper={creeper_id:?} remove={saw_remove} explosion={saw_explosion} health={damaged_health:?}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode creeper AddEntity");
            if packet.entity_type_id == creeper_entity_type {
                creeper_id = Some(packet.entity_id);
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode creeper removal");
            saw_remove = creeper_id.is_some_and(|id| packet.entity_ids.contains(&id));
        } else if frame.id == ClientboundExplode::ID {
            let mut body = frame.body;
            let packet = ClientboundExplode::decode(&mut body).expect("decode creeper explosion");
            assert_eq!(packet.radius, 3.0);
            saw_explosion = true;
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode creeper damage health");
            if packet.health < 20.0 {
                damaged_health = Some(packet.health);
            }
        }
    }

    assert!(damaged_health.unwrap() < 20.0);
}
