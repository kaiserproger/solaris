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
    let runtime_telemetry = bound.runtime_telemetry_handle();
    let mut simulation_ticks = runtime_telemetry.subscribe_simulation_ticks();
    let mut player_attacks = runtime_telemetry.subscribe_player_attacks();
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

    let (full_hit_health, (), first_hit) = tokio::join!(
        wait_for_fresh_pvp_target_outcome(&mut bob, bob_identity),
        wait_for_pvp_hurt_and_attacker_health_fence(&mut alice, alice_identity, bob_identity,),
        wait_for_pvp_attack(&mut player_attacks, alice_identity as u64, bob_identity),
    );
    assert_eq!(full_hit_health, 19.0);

    let sword_id = embedded_item_id(&data, "minecraft:stone_sword");
    alice
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stone_sword 1 0".into(),
        })
        .await
        .expect("give PvP sword");
    wait_for_slot_stack(&mut alice, sword_id, 1).await;

    wait_for_pvp_simulation_tick(&mut simulation_ticks, first_hit.cooldown_tick + 6).await;
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

    let (partial_hit_health, (), partial_hit) = tokio::join!(
        wait_for_exact_pvp_health(&mut bob),
        wait_for_rejected_pvp_attacker_fence_without_hurt(
            &mut alice,
            alice_identity,
            bob_identity,
        ),
        wait_for_pvp_attack(&mut player_attacks, alice_identity as u64, bob_identity),
    );
    let expected_health = expected_resistant_sword_hit_health(&first_hit, &partial_hit);
    assert!(
        (partial_hit_health - expected_health).abs() < 0.000_1,
        "sword hit must use its observed processing tick and apply only the hurt-resistance difference; expected={expected_health} actual={partial_hit_health}"
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

async fn wait_for_pvp_attack(
    player_attacks: &mut tokio::sync::broadcast::Receiver<mc_net::PlayerAttackObservation>,
    attacker_session_id: u64,
    target_entity_id: i32,
) -> mc_net::PlayerAttackObservation {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let observation = player_attacks
                .recv()
                .await
                .expect("player attack publisher remains active");
            if observation.attacker_session_id == attacker_session_id
                && observation.target_entity_id == target_entity_id
            {
                return observation;
            }
        }
    })
        .await
        .expect("accepted PvP attack observation")
}

fn expected_resistant_sword_hit_health(
    first_hit: &mc_net::PlayerAttackObservation,
    partial_hit: &mc_net::PlayerAttackObservation,
) -> f32 {
    assert!(
        partial_hit.authority_sequence > first_hit.authority_sequence,
        "second attack must follow the first in authority order"
    );
    let elapsed_ticks = partial_hit
        .cooldown_tick
        .saturating_sub(first_hit.cooldown_tick);
    assert!(
        (6..12).contains(&elapsed_ticks),
        "partial sword hit must remain partially recharged; first={} partial={}",
        first_hit.cooldown_tick,
        partial_hit.cooldown_tick,
    );
    let authority_elapsed = partial_hit
        .authority_tick
        .saturating_sub(first_hit.authority_tick);
    assert!(
        authority_elapsed < 10,
        "partial sword hit must remain inside hurt resistance; first={} partial={}",
        first_hit.authority_tick,
        partial_hit.authority_tick,
    );
    match elapsed_ticks {
        6 => 17.918_4,
        7 => 17.56,
        8 => 17.150_4,
        9 => 16.689_6,
        10 => 16.177_6,
        11 => 15.614_4,
        _ => unreachable!("partial recharge range asserted above"),
    }
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

async fn wait_for_pvp_health_below(client: &mut Client, ceiling: f32) -> f32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("target health below shield-disabled ceiling");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let health = ClientboundSetHealth::decode(&mut body)
                .expect("decode shield-disabled target SetHealth")
                .health;
            if health < ceiling {
                return health;
            }
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

#[tokio::test]
async fn frontal_axe_block_disables_shield_until_exact_cooldown_expiry_over_wire() {
    let data = embedded_play_data();
    let axe_id = embedded_item_id(&data, "minecraft:stone_axe");
    let shield_id = embedded_item_id(&data, "minecraft:shield");
    let shutdown = mc_net::ShutdownHandle::default();
    let mut config = embedded_playable_config(&data, embedded_world(&data), "shield axe disable wire");
    config.biome_spawns = Arc::new(mc_data::biomes::BiomeSpawnRules::default());
    config.shutdown = shutdown.clone();
    let bound = mc_net::bind(config).await.expect("bind shield axe server");
    let addr = bound.local_addr().expect("shield axe local_addr");
    let telemetry = bound.runtime_telemetry_handle();
    let mut simulation_ticks = telemetry.subscribe_simulation_ticks();
    let mut player_attacks = telemetry.subscribe_player_attacks();
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut attacker, attacker_spawn) = connect_to_play(addr, "AxeWire").await;
    drain_until_chunk(&mut attacker, (0, 0)).await;
    let (mut defender, defender_spawn) = connect_to_play(addr, "ShieldWire").await;
    drain_until_chunk(&mut defender, (0, 0)).await;
    let (defender_entity, attacker_entity) = tokio::join!(
        wait_for_pvp_player_entity_id(&mut attacker, "ShieldWire"),
        wait_for_pvp_player_entity_id(&mut defender, "AxeWire"),
    );

    move_pvp_player_with_yaw_fence(
        &mut attacker,
        attacker_spawn.x,
        attacker_spawn.y,
        attacker_spawn.z - 1.0,
        0.0,
    )
    .await;
    move_pvp_player_with_yaw_fence(
        &mut defender,
        defender_spawn.x,
        defender_spawn.y,
        defender_spawn.z + 1.0,
        180.0,
    )
    .await;

    attacker
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stone_axe 1 0".into(),
        })
        .await
        .expect("give disabling axe");
    wait_for_slot_stack(&mut attacker, axe_id, 1).await;
    defender
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:shield 1 0".into(),
        })
        .await
        .expect("give defender shield");
    wait_for_slot_stack(&mut defender, shield_id, 1).await;
    defender
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::SwapItemWithOffhand,
            position: 0,
            direction: Direction::Down,
            sequence: 501,
        })
        .await
        .expect("equip defender shield");
    assert_offhand_swap_before_ack(&mut defender, 501, shield_id, true).await;

    let ready_tick = (*simulation_ticks.borrow()).saturating_add(30);
    wait_for_pvp_simulation_tick(&mut simulation_ticks, ready_tick).await;
    defender
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 502,
            y_rot: 180.0,
            x_rot: 0.0,
        })
        .await
        .expect("start frontal shield block");
    tokio::join!(
        wait_for_block_ack(&mut defender, 502),
        wait_for_pvp_shield_flags(&mut attacker, defender_entity, true),
    );
    let shield_ready_tick = (*simulation_ticks.borrow()).saturating_add(6);
    wait_for_pvp_simulation_tick(&mut simulation_ticks, shield_ready_tick).await;

    attacker
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: defender_entity,
        })
        .await
        .expect("send first disabling axe hit");
    let ((), (), first_hit) = tokio::join!(
        wait_for_axe_shield_block_target(&mut defender, shield_id, 10, 20.0),
        wait_for_pvp_shield_flags(&mut attacker, defender_entity, false),
        wait_for_pvp_attack(&mut player_attacks, attacker_entity as u64, defender_entity),
    );

    defender
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 503,
            y_rot: 180.0,
            x_rot: 0.0,
        })
        .await
        .expect("attempt shield use during cooldown");
    wait_for_block_ack(&mut defender, 503).await;
    defender
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: attacker_entity,
        })
        .await
        .expect("fence disabled shield use with defender attack");
    wait_for_pvp_attack_animation_without_shield_start(&mut attacker, defender_entity).await;

    wait_for_pvp_simulation_tick(
        &mut simulation_ticks,
        first_hit.cooldown_tick.saturating_add(30),
    )
    .await;
    attacker
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: defender_entity,
        })
        .await
        .expect("attack while shield remains disabled");
    let (damaged_health, second_hit) = tokio::join!(
        wait_for_pvp_health_below(&mut defender, 20.0),
        wait_for_pvp_attack(&mut player_attacks, attacker_entity as u64, defender_entity),
    );
    assert!(damaged_health < 20.0, "disabled shield must not block the next axe hit");
    assert!(second_hit.authority_tick < first_hit.authority_tick.saturating_add(100));

    wait_for_pvp_simulation_tick(
        &mut simulation_ticks,
        first_hit.authority_tick.saturating_add(100),
    )
    .await;
    defender
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 504,
            y_rot: 180.0,
            x_rot: 0.0,
        })
        .await
        .expect("restart shield use after cooldown expiry");
    tokio::join!(
        wait_for_block_ack(&mut defender, 504),
        wait_for_pvp_shield_flags(&mut attacker, defender_entity, true),
    );
    let shield_ready_tick = (*simulation_ticks.borrow()).saturating_add(6);
    wait_for_pvp_simulation_tick(&mut simulation_ticks, shield_ready_tick).await;
    attacker
        .write_packet(&mc_protocol::packets::play::ServerboundAttack {
            entity_id: defender_entity,
        })
        .await
        .expect("attack reactivated shield after cooldown");
    tokio::join!(
        wait_for_axe_shield_block_target(&mut defender, shield_id, 20, damaged_health),
        wait_for_pvp_shield_flags(&mut attacker, defender_entity, false),
        wait_for_pvp_attack(&mut player_attacks, attacker_entity as u64, defender_entity),
    );

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("shield axe server shutdown")
        .expect("shield axe server join")
        .expect("shield axe server serve");
}

async fn move_pvp_player_with_yaw_fence(
    client: &mut Client,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x,
            y,
            z,
            yaw,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move shield PvP player");
    client
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send shield PvP movement fence");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("shield PvP movement fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet = ClientboundSystemChat::decode(&mut body)
                .expect("decode shield PvP movement fence");
            if system_chat_text(&packet).contains("Runtime control:") {
                return;
            }
        }
    }
}

async fn wait_for_pvp_shield_flags(client: &mut Client, entity_id: i32, using: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("shield use metadata");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode shield use metadata");
            if packet.entity_id == entity_id
                && let Some(value) = packet.values.iter().find_map(|value| match value {
                    EntityDataValue::Byte { index, value }
                        if *index == LIVING_ENTITY_DATA_FLAGS_INDEX => Some(*value),
                    _ => None,
                })
                && (value & LIVING_ENTITY_FLAG_USING_ITEM != 0) == using
            {
                return;
            }
        }
    }
}

async fn wait_for_pvp_attack_animation_without_shield_start(client: &mut Client, entity_id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("disabled shield attack fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode disabled shield metadata");
            if packet.entity_id == entity_id
                && packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::Byte { index, value }
                            if *index == LIVING_ENTITY_DATA_FLAGS_INDEX && *value & LIVING_ENTITY_FLAG_USING_ITEM != 0
                    )
                })
            {
                panic!("shield use metadata must stay clear during axe cooldown");
            }
        } else if frame.id == EntityAnimation::ID {
            let mut body = frame.body;
            let packet = EntityAnimation::decode(&mut body)
                .expect("decode disabled shield swing fence");
            if packet.entity_id == entity_id {
                return;
            }
        }
    }
}

async fn wait_for_axe_shield_block_target(
    client: &mut Client,
    shield_id: u32,
    expected_damage: i32,
    expected_health: f32,
) {
    let mut saw_cooldown = false;
    let mut saw_shield_damage = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(saw_cooldown && saw_shield_damage) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("axe shield block publication");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundCooldown::ID {
            let mut body = frame.body;
            let packet = ClientboundCooldown::decode(&mut body)
                .expect("decode shield cooldown packet");
            assert_eq!(packet.cooldown_group.as_str(), "minecraft:shield");
            assert_eq!(packet.duration, 100);
            saw_cooldown = true;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode shield durability update");
            if packet.slot == 45 {
                assert_eq!(packet.item_stack.item_id, shield_id);
                assert_eq!(packet.item_stack.count, 1);
                assert_eq!(packet.item_stack.damage, Some(expected_damage));
                saw_shield_damage = true;
            }
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet = ClientboundSetHealth::decode(&mut body)
                .expect("decode unexpected shield-block health");
            assert_eq!(packet.health, expected_health);
        }
    }
}
