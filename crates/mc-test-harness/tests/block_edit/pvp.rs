#[tokio::test]
async fn melee_pvp_damages_only_the_observed_target_player_over_wire() {
    let data = embedded_play_data();
    let mut config = embedded_playable_config(&data, embedded_world(&data), "melee PvP wire");
    config.biome_spawns = Arc::new(mc_data::biomes::BiomeSpawnRules::default());
    config.item_facts = Arc::new(mc_data::item_components::ItemFactsTable::from_entries([(
        mc_data::Identifier::parse("minecraft:stone_sword").unwrap(),
        mc_data::item_components::ItemFacts {
            max_damage: Some(131),
            weapon: true,
            weapon_damage_per_attack: Some(1),
            attack_damage_modifier: Some(4.0),
            attack_speed_modifier: Some(-2.4),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, alice_spawn) = connect_to_play(addr, "PvpAlice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;
    let (mut bob, bob_spawn) = connect_to_play(addr, "PvpBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;

    let (bob_identity, alice_identity) = tokio::join!(
        wait_for_pvp_player_entity_id(&mut alice, "PvpBob"),
        wait_for_pvp_player_entity_id(&mut bob, "PvpAlice"),
    );
    assert_ne!(alice_identity, bob_identity);

    move_pvp_player_with_fence(
        &mut alice,
        alice_spawn.x,
        alice_spawn.y,
        alice_spawn.z - 1.0,
    )
    .await;
    move_pvp_player_with_fence(&mut bob, bob_spawn.x, bob_spawn.y, bob_spawn.z + 1.0).await;

    alice
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: bob_identity,
        })
        .await
        .expect("attack observed Bob entity id");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send post-attack command fence");

    let (full_hit_health, ()) = tokio::join!(
        wait_for_fresh_pvp_target_outcome(&mut bob, bob_identity),
        wait_for_pvp_hurt_and_attacker_health_fence(&mut alice, alice_identity, bob_identity,),
    );
    assert_eq!(full_hit_health, 19.0);

    let post_first_hit_tick = *simulation_ticks.borrow_and_update();
    let sword_id = embedded_item_id(&data, "minecraft:stone_sword");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stone_sword 1 0".into(),
        })
        .await
        .expect("give PvP sword");
    wait_for_slot_stack(&mut alice, sword_id, 1).await;

    wait_for_pvp_simulation_tick(&mut simulation_ticks, post_first_hit_tick + 5).await;
    alice
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: bob_identity,
        })
        .await
        .expect("send partially recharged sword attack");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send partial-attack command fence");

    let (partial_hit_health, ()) = tokio::join!(
        wait_for_exact_pvp_health(&mut bob),
        wait_for_rejected_pvp_attacker_fence_without_hurt(
            &mut alice,
            alice_identity,
            bob_identity,
        ),
    );
    assert!(
        (partial_hit_health - 18.225_6).abs() < 0.000_1,
        "five-tick sword hit must use the +0.5 sample and apply only the hurt-resistance difference; health={partial_hit_health}"
    );
    alice
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: bob_identity,
        })
        .await
        .expect("send hurt-resistant follow-up attack");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send rejected follow-up command fence");

    wait_for_rejected_pvp_attacker_fence_without_hurt(&mut alice, alice_identity, bob_identity)
        .await;

    alice
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: bob_identity,
        })
        .await
        .expect("send later attack-action fence");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send later attack-action command fence");

    tokio::join!(
        wait_for_rejected_pvp_hit_without_health_or_hurt_until_second_attack_animation_fence(
            &mut bob,
            alice_identity,
            bob_identity,
            partial_hit_health,
        ),
        wait_for_rejected_pvp_attacker_fence_without_hurt(&mut alice, alice_identity, bob_identity,),
    );
}

async fn wait_for_pvp_simulation_tick(
    simulation_ticks: &mut tokio::sync::watch::Receiver<u64>,
    target_tick: u64,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while *simulation_ticks.borrow_and_update() < target_tick {
            simulation_ticks
                .changed()
                .await
                .expect("simulation tick publisher remains active");
        }
    })
    .await
    .expect("simulation reaches partial sword recharge tick");
}

async fn move_pvp_player_with_fence(client: &mut Client, x: f64, y: f64, z: f64) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move PvP player to deterministic position");
    client
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send PvP movement command fence");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("PvP movement command fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet = ClientboundSystemChat::decode(&mut body)
                .expect("decode PvP movement command fence");
            if system_chat_text(&packet).contains("Runtime control:") {
                return;
            }
        }
    }
}

async fn wait_for_pvp_player_entity_id(client: &mut Client, expected_name: &str) -> i32 {
    let mut expected_uuid = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("other player PlayerInfoUpdate and AddEntity");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == mc_protocol::packets::play::PlayerInfoUpdate::ID {
            let mut body = frame.body;
            let packet = mc_protocol::packets::play::PlayerInfoUpdate::decode(&mut body)
                .expect("decode PvP PlayerInfoUpdate");
            for entry in packet.entries {
                if entry.name == expected_name {
                    expected_uuid = Some(entry.profile_id);
                }
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode PvP player AddEntity");
            if expected_uuid == Some(packet.uuid) {
                return packet.entity_id;
            }
        }
    }
}

async fn wait_for_fresh_pvp_target_outcome(client: &mut Client, target_entity_id: i32) -> f32 {
    let mut health = None;
    let mut saw_motion = false;
    let mut saw_hurt_event = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while health.is_none() || !saw_motion || !saw_hurt_event {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("target health and motion after melee PvP attack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet = ClientboundSetHealth::decode(&mut body).expect("decode target SetHealth");
            health = Some(packet.health);
        } else if frame.id == SetEntityMotion::ID {
            const WIRE_BASE_KNOCKBACK: f64 = 0.399_987_792_223_646_55;
            let mut body = frame.body;
            let packet = SetEntityMotion::decode(&mut body).expect("decode target melee motion");
            assert_eq!(packet.entity_id, target_entity_id);
            assert!(
                packet.movement.x.abs() < 1.0e-9,
                "unexpected melee motion: {:?}",
                packet.movement
            );
            assert!(
                (packet.movement.y - WIRE_BASE_KNOCKBACK).abs() < 1.0e-12,
                "unexpected melee motion: {:?}",
                packet.movement
            );
            assert!(
                (packet.movement.z - WIRE_BASE_KNOCKBACK).abs() < 1.0e-12,
                "unexpected melee motion: {:?}",
                packet.movement
            );
            saw_motion = true;
        } else if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let packet = EntityEvent::decode(&mut body).expect("decode target PvP hurt event");
            saw_hurt_event |= packet.entity_id == target_entity_id && packet.event_id == 2;
        }
    }
    health.expect("target health observed")
}

async fn wait_for_exact_pvp_health(client: &mut Client) -> f32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("target health after stronger resistant hit");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            return ClientboundSetHealth::decode(&mut body)
                .expect("decode resistant target SetHealth")
                .health;
        }
    }
}

async fn wait_for_rejected_pvp_hit_without_health_or_hurt_until_second_attack_animation_fence(
    client: &mut Client,
    attacker_entity_id: i32,
    target_entity_id: i32,
    minimum_health: f32,
) {
    let mut matching_attacker_swings = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("hurt-resisted target event fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode unexpected PvP health");
            assert!(
                packet.health + 0.000_1 >= minimum_health,
                "weaker same-window PvP hit reduced health before the later attack-action fence; health={}",
                packet.health
            );
        } else if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let packet = EntityEvent::decode(&mut body).expect("decode rejected-hit hurt event");
            if packet.entity_id == target_entity_id && packet.event_id == 2 {
                panic!(
                    "weaker same-window PvP hit emitted a hurt event before the later attack-action fence"
                );
            }
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let packet = SetEntityMotion::decode(&mut body)
                .expect("decode rejected-hit target-side motion");
            assert_ne!(
                packet.entity_id, target_entity_id,
                "resisted PvP hits must not knock back the target"
            );
        } else if frame.id == EntityAnimation::ID {
            let mut body = frame.body;
            let packet =
                EntityAnimation::decode(&mut body).expect("decode later PvP attack-action fence");
            if packet.entity_id == attacker_entity_id
                && packet.action == EntityAnimationAction::SwingMainHand
            {
                matching_attacker_swings += 1;
                // Each attack queues its swing before its victim DamagePlayer command. The
                // second swing therefore fences completion of the first rejected command.
                if matching_attacker_swings == 2 {
                    return;
                }
            }
        }
    }
}

async fn wait_for_rejected_pvp_attacker_fence_without_hurt(
    client: &mut Client,
    attacker_entity_id: i32,
    target_entity_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected-hit attacker command fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let packet =
                EntityEvent::decode(&mut body).expect("decode rejected-hit observer event");
            assert!(
                packet.entity_id != target_entity_id || packet.event_id != 2,
                "weaker same-window PvP hit must not publish a hurt event to observers"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode attacker SetHealth");
            assert_eq!(
                packet.health, 20.0,
                "rejected attack must not damage the attacker"
            );
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let packet = SetEntityMotion::decode(&mut body)
                .expect("decode rejected-hit attacker-side motion");
            assert_ne!(packet.entity_id, attacker_entity_id);
            assert_ne!(packet.entity_id, target_entity_id);
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet = ClientboundSystemChat::decode(&mut body)
                .expect("decode rejected-hit command fence");
            if system_chat_text(&packet).contains("Runtime control:") {
                return;
            }
        }
    }
}

async fn wait_for_pvp_hurt_and_attacker_health_fence(
    client: &mut Client,
    attacker_entity_id: i32,
    target_entity_id: i32,
) {
    let mut saw_target_hurt = false;
    let mut saw_command_fence = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(saw_target_hurt && saw_command_fence) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("target hurt event and post-attack command fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let packet = EntityEvent::decode(&mut body).expect("decode PvP hurt event");
            saw_target_hurt |= packet.entity_id == target_entity_id && packet.event_id == 2;
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode attacker SetHealth");
            assert_eq!(
                packet.health, 20.0,
                "attacking another player must not damage the attacker"
            );
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let packet = SetEntityMotion::decode(&mut body)
                .expect("decode unexpected attacker-side melee motion");
            assert_ne!(packet.entity_id, attacker_entity_id);
            assert_ne!(packet.entity_id, target_entity_id);
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet = ClientboundSystemChat::decode(&mut body).expect("decode command fence");
            saw_command_fence |= system_chat_text(&packet).contains("Runtime control:");
        }
    }
}
