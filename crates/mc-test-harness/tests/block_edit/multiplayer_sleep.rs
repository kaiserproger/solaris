use mc_protocol::packets::play::{PlayerInfoRemove, PlayerInfoUpdate};
use uuid::Uuid;

#[tokio::test]
async fn two_clients_sleep_quorum_pushes_pose_and_morning_to_both() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let mut config = embedded_playable_config(&data, world, "multiplayer sleep quorum");
    config.biome_spawns = Arc::new(mc_data::biomes::BiomeSpawnRules::default());
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));

    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, alice_sync) = connect_to_play(addr, "SleepAlice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;
    let (mut bob, _) = connect_to_play(addr, "SleepBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;

    let bed_pos = mc_world::BlockPos {
        x: alice_sync.x.floor() as i32 + 1,
        y: alice_sync.y.floor() as i32 - 1,
        z: alice_sync.z.floor() as i32,
    };
    let bob_bed_pos = mc_world::BlockPos {
        x: bed_pos.x + 2,
        ..bed_pos
    };
    let bed_states = north_facing_bed_states(&data);
    seed_two_block_bed(&world_handle, bed_pos, bed_states).await;
    seed_two_block_bed(&world_handle, bob_bed_pos, bed_states).await;

    alice
        .write_packet(&ServerboundChatCommand {
            command: "time set night".into(),
        })
        .await
        .expect("set night");
    wait_for_exact_world_time(&mut alice, 13_000).await;

    use_sleep_test_bed(&mut alice, bed_pos, 1).await;
    let alice_id = wait_for_sleeping_before_morning(&mut alice, Some(1)).await;
    let observed_alice_id = wait_for_sleeping_before_morning(&mut bob, None).await;
    assert_eq!(observed_alice_id, alice_id);

    use_sleep_test_bed(&mut bob, bob_bed_pos, 2).await;
    let (alice_result, bob_result) = tokio::join!(
        wait_for_sleep_quorum_delivery(&mut alice, alice_id, bed_pos, bob_bed_pos, None),
        wait_for_sleep_quorum_delivery(&mut bob, alice_id, bed_pos, bob_bed_pos, Some(2)),
    );
    assert_eq!(alice_result.bob_id, bob_result.bob_id);
    assert_ne!(alice_result.bob_id, alice_id);
    assert!(alice_result.wake_position.1 >= f64::from(bed_pos.y));
    assert!(bob_result.wake_position.1 >= f64::from(bob_bed_pos.y));
}

#[tokio::test]
async fn lowering_sleep_percentage_wakes_the_only_sleeper_once_for_both_clients() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let mut config = embedded_playable_config(&data, world, "sleep gamerule transition");
    config.biome_spawns = Arc::new(mc_data::biomes::BiomeSpawnRules::default());
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));

    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, alice_sync) = connect_to_play(addr, "SleepRuleAlice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;
    let (mut bob, _) = connect_to_play(addr, "SleepRuleBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;

    let bed_pos = mc_world::BlockPos {
        x: alice_sync.x.floor() as i32 + 1,
        y: alice_sync.y.floor() as i32 - 1,
        z: alice_sync.z.floor() as i32,
    };
    let bed_states = north_facing_bed_states(&data);
    seed_two_block_bed(&world_handle, bed_pos, bed_states).await;

    alice
        .write_packet(&ServerboundChatCommand {
            command: "time set night".into(),
        })
        .await
        .expect("set night");
    wait_for_exact_world_time(&mut alice, 13_000).await;

    use_sleep_test_bed(&mut alice, bed_pos, 31).await;
    let alice_id = wait_for_sleeping_before_morning(&mut alice, Some(31)).await;
    assert_eq!(
        wait_for_sleeping_before_morning(&mut bob, None).await,
        alice_id
    );

    bob.write_packet(&ServerboundChatCommand {
        command: "gamerule players_sleeping_percentage 50".into(),
    })
    .await
    .expect("lower sleeping percentage");

    let (alice_wake, bob_wake) = tokio::join!(
        wait_for_single_sleeper_wake(&mut alice, alice_id, bed_pos, false, true),
        wait_for_single_sleeper_wake(&mut bob, alice_id, bed_pos, true, false),
    );
    assert_eq!(alice_wake.morning_publications, 1);
    assert_eq!(bob_wake.morning_publications, 1);
    assert!(alice_wake.saw_wake_position);
    assert!(!bob_wake.saw_wake_position);

    bob.write_packet(&ServerboundChatCommand {
        command: "time set day".into(),
    })
    .await
    .expect("send post-wake time fence");
    let (alice_extra_mornings, bob_extra_mornings) = tokio::join!(
        count_mornings_until_time_fence(&mut alice, 1_000),
        count_mornings_until_time_fence(&mut bob, 1_000),
    );
    assert_eq!(alice_extra_mornings, 0);
    assert_eq!(bob_extra_mornings, 0);

    let world = world_handle.lock().await;
    assert_eq!(world.get_cached_block(bed_pos), Some(bed_states.foot));
    assert_eq!(
        world.get_cached_block(bed_states.head_pos(bed_pos)),
        Some(bed_states.head)
    );
}

#[tokio::test]
async fn disconnecting_sleeping_player_releases_bed_for_other_client() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let mut config = embedded_playable_config(&data, world, "disconnecting sleeping player");
    config.biome_spawns = Arc::new(mc_data::biomes::BiomeSpawnRules::default());
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));

    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, alice_sync) = connect_to_play(addr, "SleepDropAlice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;
    let (mut bob, _) = connect_to_play(addr, "SleepDropBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;
    let bob_identity = wait_for_player_identity(&mut alice, "SleepDropBob").await;
    let alice_identity = wait_for_player_identity(&mut bob, "SleepDropAlice").await;
    let (_below_quorum_client, _) = connect_to_play(addr, "SleepDropWatch").await;

    let foot = mc_world::BlockPos {
        x: alice_sync.x.floor() as i32 + 1,
        y: alice_sync.y.floor() as i32 - 1,
        z: alice_sync.z.floor() as i32,
    };
    let bed_states = north_facing_bed_states(&data);
    seed_two_block_bed(&world_handle, foot, bed_states).await;

    alice
        .write_packet(&ServerboundChatCommand {
            command: "time set night".into(),
        })
        .await
        .expect("set night");
    wait_for_exact_world_time(&mut alice, 13_000).await;

    use_sleep_test_bed(&mut alice, foot, 21).await;
    assert_eq!(
        wait_for_sleeping_before_morning(&mut alice, Some(21)).await,
        alice_identity.entity_id
    );
    assert_eq!(
        wait_for_sleeping_before_morning(&mut bob, None).await,
        alice_identity.entity_id
    );
    {
        let world = world_handle.lock().await;
        assert_eq!(world.get_cached_block(foot), Some(bed_states.occupied_foot));
        assert_eq!(
            world.get_cached_block(bed_states.head_pos(foot)),
            Some(bed_states.occupied_head)
        );
    }

    drop(alice);
    wait_for_player_disconnect(&mut bob, alice_identity).await;
    {
        let world = world_handle.lock().await;
        assert_eq!(world.get_cached_block(foot), Some(bed_states.foot));
        assert_eq!(
            world.get_cached_block(bed_states.head_pos(foot)),
            Some(bed_states.head)
        );
    }

    use_sleep_test_bed(&mut bob, foot, 22).await;
    assert_eq!(
        wait_for_sleeping_before_morning(&mut bob, Some(22)).await,
        bob_identity.entity_id
    );
    {
        let world = world_handle.lock().await;
        assert_eq!(world.get_cached_block(foot), Some(bed_states.occupied_foot));
        assert_eq!(
            world.get_cached_block(bed_states.head_pos(foot)),
            Some(bed_states.occupied_head)
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct NorthFacingBedStates {
    foot: mc_world::BlockStateId,
    head: mc_world::BlockStateId,
    occupied_foot: mc_world::BlockStateId,
    occupied_head: mc_world::BlockStateId,
}

impl NorthFacingBedStates {
    fn head_pos(self, foot: mc_world::BlockPos) -> mc_world::BlockPos {
        mc_world::BlockPos {
            z: foot.z - 1,
            ..foot
        }
    }
}

fn north_facing_bed_states(data: &EmbeddedPlayData) -> NorthFacingBedStates {
    let bed_id = mc_data::Identifier::parse("minecraft:white_bed").unwrap();
    let state = |occupied: &str, part: &str| {
        data.blocks
            .by_name_and_props(
                &bed_id,
                &[
                    ("facing".into(), "north".into()),
                    ("occupied".into(), occupied.into()),
                    ("part".into(), part.into()),
                ],
            )
            .unwrap_or_else(|| panic!("north-facing bed occupied={occupied} part={part} state"))
    };
    NorthFacingBedStates {
        foot: state("false", "foot"),
        head: state("false", "head"),
        occupied_foot: state("true", "foot"),
        occupied_head: state("true", "head"),
    }
}

async fn seed_two_block_bed(
    world_handle: &Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    foot: mc_world::BlockPos,
    states: NorthFacingBedStates,
) {
    let mut world = world_handle.lock().await;
    world
        .set_block_at(foot, states.foot)
        .expect("seed bed foot")
        .expect("bed foot chunk is loaded");
    world
        .set_block_at(states.head_pos(foot), states.head)
        .expect("seed bed head")
        .expect("bed head chunk is loaded");
}

#[derive(Debug, Clone, Copy)]
struct PlayerIdentity {
    entity_id: i32,
    profile_id: Uuid,
}

async fn wait_for_player_identity(client: &mut Client, expected_name: &str) -> PlayerIdentity {
    let mut expected_uuid = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for player identity");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == PlayerInfoUpdate::ID {
            let mut body = frame.body;
            let packet = PlayerInfoUpdate::decode(&mut body).expect("decode PlayerInfoUpdate");
            for entry in packet.entries {
                if entry.name == expected_name {
                    expected_uuid = Some(entry.profile_id);
                }
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode AddEntity");
            if expected_uuid == Some(packet.uuid) {
                return PlayerIdentity {
                    entity_id: packet.entity_id,
                    profile_id: packet.uuid,
                };
            }
        }
    }
}

async fn wait_for_player_disconnect(client: &mut Client, identity: PlayerIdentity) {
    let mut saw_entity_remove = false;
    let mut saw_info_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_entity_remove && saw_info_remove) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for sleeping player disconnect");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode RemoveEntities");
            saw_entity_remove |= packet.entity_ids.contains(&identity.entity_id);
        } else if frame.id == PlayerInfoRemove::ID {
            let mut body = frame.body;
            let packet = PlayerInfoRemove::decode(&mut body).expect("decode PlayerInfoRemove");
            saw_info_remove |= packet.profile_ids.contains(&identity.profile_id);
        }
    }
}

#[tokio::test]
async fn second_client_cannot_sleep_in_the_other_half_of_an_occupied_bed() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let config = embedded_playable_config(&data, world, "occupied multiplayer bed");
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));
    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, sync) = connect_to_play(addr, "OccupiedAlice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;
    let (mut bob, _) = connect_to_play(addr, "OccupiedBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;
    let foot = mc_world::BlockPos {
        x: sync.x.floor() as i32 + 1,
        y: sync.y.floor() as i32 - 1,
        z: sync.z.floor() as i32,
    };
    let head = mc_world::BlockPos {
        z: foot.z - 1,
        ..foot
    };
    let bed_states = north_facing_bed_states(&data);
    seed_two_block_bed(&world_handle, foot, bed_states).await;
    alice
        .write_packet(&ServerboundChatCommand {
            command: "time set night".into(),
        })
        .await
        .expect("set night");
    wait_for_exact_world_time(&mut alice, 13_000).await;

    use_sleep_test_bed(&mut alice, foot, 11).await;
    wait_for_sleeping_before_morning(&mut alice, Some(11)).await;
    wait_for_sleeping_before_morning(&mut bob, None).await;
    {
        let world = world_handle.lock().await;
        assert_eq!(world.get_cached_block(foot), Some(bed_states.occupied_foot));
        assert_eq!(world.get_cached_block(head), Some(bed_states.occupied_head));
    }
    use_sleep_test_bed(&mut bob, head, 12).await;
    wait_for_occupied_bed_rejection(&mut bob, 12).await;
    alice
        .write_packet(&ServerboundPlayerCommand {
            entity_id: 0,
            action: PlayerCommandAction::StopSleeping,
            data: 0,
        })
        .await
        .expect("stop sleeping");
    wait_for_bed_wake_position(&mut alice).await;
    {
        let world = world_handle.lock().await;
        assert_eq!(world.get_cached_block(foot), Some(bed_states.foot));
        assert_eq!(world.get_cached_block(head), Some(bed_states.head));
    }
}

#[tokio::test]
async fn suffocating_block_above_either_bed_half_rejects_sleep_over_wire() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let mut config = embedded_playable_config(&data, world, "obstructed bed");
    config.block_light = Some(Arc::new(
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&data.report),
    ));
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));
    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "ObstructedSleep").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let foot = mc_world::BlockPos {
        x: sync.x.floor() as i32 + 1,
        y: sync.y.floor() as i32 - 1,
        z: sync.z.floor() as i32,
    };
    let head = mc_world::BlockPos {
        z: foot.z - 1,
        ..foot
    };
    let bed_id = mc_data::Identifier::parse("minecraft:white_bed").unwrap();
    let bed_state = |part: &str| {
        data.blocks
            .by_name_and_props(
                &bed_id,
                &[
                    ("facing".into(), "north".into()),
                    ("occupied".into(), "false".into()),
                    ("part".into(), part.into()),
                ],
            )
            .unwrap_or_else(|| panic!("north-facing bed {part} state"))
    };
    {
        let mut world = world_handle.lock().await;
        world
            .set_block_at(foot, bed_state("foot"))
            .expect("seed bed foot")
            .expect("bed foot chunk is loaded");
        world
            .set_block_at(head, bed_state("head"))
            .expect("seed bed head")
            .expect("bed head chunk is loaded");
        world
            .set_block_at(
                mc_world::BlockPos {
                    y: foot.y + 1,
                    ..foot
                },
                embedded_block_state(&data, "minecraft:stone"),
            )
            .expect("seed bed obstruction")
            .expect("bed obstruction chunk is loaded");
    }

    use_sleep_test_bed(&mut client, foot, 13).await;
    wait_for_obstructed_bed_rejection(&mut client, 13).await;
}

async fn wait_for_obstructed_bed_rejection(client: &mut Client, expected_ack: i32) {
    let mut saw_ack = false;
    let mut saw_obstructed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("obstructed bed rejection");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode obstructed bed ack");
            saw_ack |= packet.sequence == expected_ack;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode obstructed bed rejection");
            let text = system_chat_text(&packet);
            assert!(!text.contains("Respawn point set"));
            if text.contains("bed is obstructed") {
                assert!(packet.overlay);
                saw_obstructed = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode entity data during obstructed rejection");
            assert!(!packet.values.iter().any(|value| matches!(
                value,
                EntityDataValue::Pose {
                    pose: EntityPose::Sleeping,
                    ..
                }
            )));
        }
        if saw_ack && saw_obstructed {
            return;
        }
    }
}

async fn wait_for_occupied_bed_rejection(client: &mut Client, expected_ack: i32) {
    let mut saw_ack = false;
    let mut saw_occupied = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("occupied bed rejection");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode occupied bed ack");
            saw_ack |= packet.sequence == expected_ack;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode occupied bed rejection");
            if system_chat_text(&packet).contains("bed is occupied") {
                assert!(packet.overlay);
                saw_occupied = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode entity data during occupied rejection");
            assert!(!packet.values.iter().any(|value| matches!(
                value,
                EntityDataValue::Pose {
                    pose: EntityPose::Sleeping,
                    ..
                }
            )));
        }
        if saw_ack && saw_occupied {
            return;
        }
    }
}

async fn wait_for_bed_wake_position(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("bed wake position");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            SynchronizePlayerPosition::decode(&mut frame.body).expect("decode bed wake position");
            return;
        }
    }
}

#[tokio::test]
async fn nearby_monster_rejects_sleep_without_publishing_sleeping_pose() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let config = embedded_playable_config(&data, world, "monster blocks sleep");
    let world_handle = Arc::clone(config.world.as_ref().expect("embedded world handle"));

    let bound = mc_net::bind(config).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "UnsafeSleepAlice").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let bed_pos = mc_world::BlockPos {
        x: sync.x.floor() as i32 + 1,
        y: sync.y.floor() as i32 - 1,
        z: sync.z.floor() as i32,
    };
    seed_two_block_bed(&world_handle, bed_pos, north_facing_bed_states(&data)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "time set night".into(),
        })
        .await
        .expect("set night");
    wait_for_exact_world_time(&mut client, 13_000).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:zombie {} {} {}",
                bed_pos.x + 7,
                bed_pos.y + 4,
                bed_pos.z + 7
            ),
        })
        .await
        .expect("summon nearby zombie");
    wait_for_summoned_entity(&mut client).await;

    use_sleep_test_bed(&mut client, bed_pos, 7).await;
    wait_for_monster_sleep_rejection(&mut client, 7).await;
}

async fn wait_for_summoned_entity(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("summoned entity visibility");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            return;
        }
    }
}

async fn wait_for_monster_sleep_rejection(client: &mut Client, expected_ack: i32) {
    let mut saw_ack = false;
    let mut saw_rejection = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("monster sleep rejection");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode bed ack");
            saw_ack |= packet.sequence == expected_ack;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode monster sleep rejection");
            if system_chat_text(&packet).contains("monsters are nearby") {
                assert!(
                    packet.overlay,
                    "vanilla bed rejection is an overlay message"
                );
                saw_rejection = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode entity data while rejecting sleep");
            assert!(
                !packet.values.iter().any(|value| matches!(
                    value,
                    EntityDataValue::Pose {
                        pose: EntityPose::Sleeping,
                        ..
                    }
                )),
                "rejected sleep must not publish a sleeping pose"
            );
        }
        if saw_ack && saw_rejection {
            return;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SingleSleeperWakeWireResult {
    morning_publications: usize,
    saw_wake_position: bool,
}

async fn wait_for_single_sleeper_wake(
    client: &mut Client,
    sleeper_id: i32,
    bed: mc_world::BlockPos,
    expect_gamerule_feedback: bool,
    expect_wake_position: bool,
) -> SingleSleeperWakeWireResult {
    let mut saw_gamerule_feedback = !expect_gamerule_feedback;
    let mut saw_standing = false;
    let mut saw_wake_position = false;
    let mut morning_publications = 0;
    let mut saw_bed_release = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("gamerule-driven sleep wake");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode gamerule bed release");
            saw_bed_release |= unpack_block_pos(packet.position) == (bed.x, bed.y, bed.z);
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet = SectionBlocksUpdate::decode(&mut body)
                .expect("decode gamerule section bed release");
            saw_bed_release |= section_update_contains(&packet, bed);
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode gamerule sleep feedback");
            saw_gamerule_feedback |=
                system_chat_text(&packet) == "players_sleeping_percentage = 50";
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode gamerule wake entity data");
            if packet.entity_id == sleeper_id {
                let standing = packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::Pose {
                            pose: EntityPose::Standing,
                            ..
                        }
                    )
                });
                assert!(
                    !standing || saw_bed_release,
                    "bed release must precede Standing metadata"
                );
                saw_standing |= standing;
            }
        } else if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetTime::decode(&mut body).expect("decode gamerule wake world time");
            if packet.game_time == 24_000 {
                morning_publications += 1;
            }
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            SynchronizePlayerPosition::decode(&mut body).expect("decode gamerule wake position");
            assert!(saw_bed_release, "bed release must precede wake teleport");
            saw_wake_position = true;
        }

        if saw_gamerule_feedback
            && saw_standing
            && morning_publications > 0
            && (!expect_wake_position || saw_wake_position)
        {
            return SingleSleeperWakeWireResult {
                morning_publications,
                saw_wake_position,
            };
        }
    }
}

async fn count_mornings_until_time_fence(client: &mut Client, fence_time: i64) -> usize {
    let mut morning_publications = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("post-wake time fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetTime::decode(&mut body).expect("decode post-wake world time");
            if packet.game_time == 24_000 {
                morning_publications += 1;
            } else if packet.game_time == fence_time {
                return morning_publications;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SleepQuorumWireResult {
    bob_id: i32,
    wake_position: (f64, f64, f64),
}

async fn use_sleep_test_bed(client: &mut Client, pos: mc_world::BlockPos, sequence: i32) {
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(pos.x, pos.y, pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("use bed");
}

async fn wait_for_exact_world_time(client: &mut Client, expected: i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("world time update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body).expect("decode SetTime");
            if packet.game_time == expected {
                return;
            }
        }
    }
}

async fn wait_for_sleeping_before_morning(client: &mut Client, expected_ack: Option<i32>) -> i32 {
    let mut saw_ack = expected_ack.is_none();
    let mut sleeping_entity = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("waiting sleep pose");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode bed ack");
            if expected_ack == Some(packet.sequence) {
                saw_ack = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode sleep system chat");
            let text = system_chat_text(&packet);
            assert!(
                !text.contains("bed is occupied"),
                "sleeping bed was rejected: {text}"
            );
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetEntityData::decode(&mut body).expect("decode sleeping entity data");
            if packet.values.iter().any(|value| {
                matches!(
                    value,
                    EntityDataValue::Pose {
                        pose: EntityPose::Sleeping,
                        ..
                    }
                )
            }) {
                sleeping_entity = Some(packet.entity_id);
            }
        } else if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body).expect("decode waiting SetTime");
            assert_ne!(
                packet.game_time, 24_000,
                "one sleeping player must not satisfy a two-player quorum"
            );
        }
        if saw_ack && let Some(entity_id) = sleeping_entity {
            return entity_id;
        }
    }
}

async fn wait_for_sleep_quorum_delivery(
    client: &mut Client,
    alice_id: i32,
    alice_bed: mc_world::BlockPos,
    bob_bed: mc_world::BlockPos,
    expected_ack: Option<i32>,
) -> SleepQuorumWireResult {
    let own_bed = if expected_ack.is_none() {
        alice_bed
    } else {
        bob_bed
    };
    let mut saw_ack = expected_ack.is_none();
    let mut bob_id = None;
    let mut standing_entities = HashSet::new();
    let mut saw_morning = false;
    let mut wake_position = None;
    let mut released_beds = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("sleep quorum delivery");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode quorum bed release");
            let position = unpack_block_pos(packet.position);
            for bed in [alice_bed, bob_bed] {
                if position == (bed.x, bed.y, bed.z) {
                    released_beds.insert(bed);
                }
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet =
                SectionBlocksUpdate::decode(&mut body).expect("decode quorum section bed release");
            for bed in [alice_bed, bob_bed] {
                if section_update_contains(&packet, bed) {
                    released_beds.insert(bed);
                }
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode quorum bed ack");
            if expected_ack == Some(packet.sequence) {
                saw_ack = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetEntityData::decode(&mut body).expect("decode quorum entity data");
            for value in packet.values {
                match value {
                    EntityDataValue::Pose {
                        pose: EntityPose::Sleeping,
                        ..
                    } if packet.entity_id != alice_id => bob_id = Some(packet.entity_id),
                    EntityDataValue::Pose {
                        pose: EntityPose::Standing,
                        ..
                    } => {
                        let expected_bed = if packet.entity_id == alice_id {
                            alice_bed
                        } else {
                            bob_bed
                        };
                        assert!(
                            released_beds.contains(&expected_bed),
                            "bed release must precede Standing metadata for entity {}",
                            packet.entity_id
                        );
                        standing_entities.insert(packet.entity_id);
                    }
                    _ => {}
                }
            }
        } else if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body).expect("decode quorum SetTime");
            if packet.game_time == 24_000 {
                saw_morning = true;
            }
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let packet =
                SynchronizePlayerPosition::decode(&mut body).expect("decode safe wake position");
            assert!(
                released_beds.contains(&own_bed),
                "own bed release must precede wake teleport"
            );
            wake_position = Some((packet.x, packet.y, packet.z));
        }

        if saw_ack
            && saw_morning
            && standing_entities.contains(&alice_id)
            && let (Some(bob_id), Some(wake_position)) = (bob_id, wake_position)
            && standing_entities.contains(&bob_id)
        {
            return SleepQuorumWireResult {
                bob_id,
                wake_position,
            };
        }
    }
}

fn section_update_contains(packet: &SectionBlocksUpdate, bed: mc_world::BlockPos) -> bool {
    section_pos_matches(packet.section_pos, (bed.x, bed.y, bed.z))
        && packet
            .changes
            .iter()
            .any(|change| change.relative_pos == pack_section_relative_pos(bed.x, bed.y, bed.z))
}

fn section_pos_matches(section_pos: i64, target: (i32, i32, i32)) -> bool {
    let sx = unpack_signed_section_coord(section_pos >> 42, 22);
    let sy = unpack_signed_section_coord(section_pos, 20);
    let sz = unpack_signed_section_coord(section_pos >> 20, 22);
    sx == target.0.div_euclid(16) && sy == target.1.div_euclid(16) && sz == target.2.div_euclid(16)
}

fn unpack_signed_section_coord(value: i64, bits: u8) -> i32 {
    let mask = (1_i64 << bits) - 1;
    let sign = 1_i64 << (bits - 1);
    let value = value & mask;
    if value & sign == 0 {
        value as i32
    } else {
        (value - (1_i64 << bits)) as i32
    }
}
