#[test]
fn passive_spawn_planner_keeps_water_mobs_off_land() {
    use std::collections::BTreeMap;

    let plains = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let ocean = mc_data::Identifier::parse("minecraft:ocean").unwrap();
    let pig = mc_data::Identifier::parse("minecraft:pig").unwrap();
    let cod = mc_data::Identifier::parse("minecraft:cod").unwrap();
    let rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
        plains.clone(),
        BTreeMap::from([
            (
                "creature".to_string(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: pig.clone(),
                    min_count: 2,
                    max_count: 2,
                    weight: 1,
                }],
            ),
            (
                "water_ambient".to_string(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: cod.clone(),
                    min_count: 4,
                    max_count: 4,
                    weight: 1,
                }],
            ),
        ]),
    )]));
    let entity_types = mc_data::entity_types::EntityTypeRegistry::from_report(&[
        mc_data::entity_types::EntityTypeReport {
            id: pig,
            protocol_id: 1,
        },
        mc_data::entity_types::EntityTypeReport {
            id: cod,
            protocol_id: 2,
        },
    ]);
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        plains.clone(),
    );
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    let water = mc_world::BlockStateId(2);
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = chunk.set_block(lx, 64, lz, grass);
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        Some(grass),
        Some(water),
        &passable,
        &rules,
        &entity_types,
    );

    assert!(!spawns.is_empty());
    assert!(
        spawns
            .iter()
            .all(|spawn| spawn.entity_type_name == "minecraft:pig")
    );
    assert!(spawns.iter().all(|spawn| spawn.position.y == 65.0));
    assert!(spawns.iter().all(|spawn| spawn.entity_type_id == 1));
    assert!(spawns.iter().all(|spawn| {
        let fx = spawn.position.x.fract();
        let fz = spawn.position.z.fract();
        (0.48..=0.51).contains(&fx) && (0.48..=0.51).contains(&fz)
    }));

    let mut unsupported_chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains.clone());
    for lx in 3..=12 {
        for lz in 3..=12 {
            if (lx + lz) % 2 == 0 {
                let _ = unsupported_chunk.set_block(lx, 64, lz, grass);
            }
        }
    }
    let unsupported_spawns = plan_passive_herd(
        &unsupported_chunk,
        Some(grass),
        None,
        &passable,
        &rules,
        &entity_types,
    );
    assert!(unsupported_spawns.is_empty());

    let ocean_rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
        ocean.clone(),
        BTreeMap::from([(
            "water_ambient".to_string(),
            vec![mc_data::biomes::BiomeSpawnEntry {
                entity_type: mc_data::Identifier::parse("minecraft:cod").unwrap(),
                min_count: 3,
                max_count: 3,
                weight: 1,
            }],
        )]),
    )]));
    let mut ocean_chunk =
        Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), ocean.clone());
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = ocean_chunk.set_block(lx, DEFAULT_SEA_LEVEL, lz, water);
        }
    }

    let spawns = plan_passive_herd(
        &ocean_chunk,
        Some(grass),
        Some(water),
        &passable,
        &ocean_rules,
        &entity_types,
    );

    assert!(
        spawns
            .iter()
            .all(|spawn| spawn.entity_type_name == "minecraft:cod")
    );
    assert!(spawns.iter().all(|spawn| {
        let lx = spawn.position.x.floor() as u8;
        let lz = spawn.position.z.floor() as u8;
        ocean_chunk.get_block(lx, spawn.position.y as i32, lz) == Some(water)
    }));

    let water_only_chunk_pos = (1..64)
        .map(|x| (x, 0))
        .find(|&pos| !passive_chunk_spawns(pos))
        .expect("non-passive chunk sample");
    let mut water_only_chunk = Chunk::empty(
        ChunkPos {
            x: water_only_chunk_pos.0,
            z: water_only_chunk_pos.1,
        },
        mc_world::BlockStateId(0),
        ocean,
    );
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = water_only_chunk.set_block(lx, DEFAULT_SEA_LEVEL, lz, water);
        }
    }

    let spawns = plan_passive_herd(
        &water_only_chunk,
        Some(grass),
        Some(water),
        &passable,
        &ocean_rules,
        &entity_types,
    );

    assert!(
        spawns
            .iter()
            .any(|spawn| spawn.entity_type_name == "minecraft:cod"),
        "water mobs should not be throttled by sparse land passive spawning"
    );
    assert!(spawns.iter().all(|spawn| {
        let lx = (spawn.position.x.floor() as i32 - water_only_chunk.pos.x * 16) as u8;
        let lz = (spawn.position.z.floor() as i32 - water_only_chunk.pos.z * 16) as u8;
        water_only_chunk.get_block(lx, spawn.position.y as i32, lz) == Some(water)
    }));
}

#[test]
fn hostile_spawn_planner_uses_multiple_monster_facts() {
    use std::collections::{BTreeMap, HashSet};

    let plains = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let zombie = mc_data::Identifier::parse("minecraft:zombie").unwrap();
    let skeleton = mc_data::Identifier::parse("minecraft:skeleton").unwrap();
    let spider = mc_data::Identifier::parse("minecraft:spider").unwrap();
    let chicken = mc_data::Identifier::parse("minecraft:chicken").unwrap();
    let rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
        plains.clone(),
        BTreeMap::from([(
            "monster".to_string(),
            vec![
                mc_data::biomes::BiomeSpawnEntry {
                    entity_type: zombie.clone(),
                    min_count: 1,
                    max_count: 1,
                    weight: 1,
                },
                mc_data::biomes::BiomeSpawnEntry {
                    entity_type: skeleton.clone(),
                    min_count: 1,
                    max_count: 1,
                    weight: 1,
                },
                mc_data::biomes::BiomeSpawnEntry {
                    entity_type: spider.clone(),
                    min_count: 1,
                    max_count: 1,
                    weight: 1,
                },
                mc_data::biomes::BiomeSpawnEntry {
                    entity_type: chicken.clone(),
                    min_count: 1,
                    max_count: 1,
                    weight: 1,
                },
            ],
        )]),
    )]));
    let entity_types = mc_data::entity_types::EntityTypeRegistry::from_report(&[
        mc_data::entity_types::EntityTypeReport {
            id: zombie,
            protocol_id: 1,
        },
        mc_data::entity_types::EntityTypeReport {
            id: skeleton,
            protocol_id: 2,
        },
        mc_data::entity_types::EntityTypeReport {
            id: spider,
            protocol_id: 3,
        },
        mc_data::entity_types::EntityTypeReport {
            id: chicken,
            protocol_id: 4,
        },
    ]);
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains);
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = chunk.set_block(lx, 64, lz, grass);
        }
    }

    let spawns = plan_passive_herd(&chunk, Some(grass), None, &passable, &rules, &entity_types);

    let spawned_types: HashSet<_> = spawns
        .iter()
        .map(|spawn| spawn.entity_type_name.as_str())
        .collect();
    assert!(spawned_types.contains("minecraft:zombie"));
    assert!(spawned_types.contains("minecraft:skeleton"));
    assert!(spawned_types.contains("minecraft:spider"));
    assert!(!spawned_types.contains("minecraft:chicken"));
    assert!(spawns.iter().all(|spawn| spawn.hostile));
}

#[test]
fn hostile_spawn_planner_requires_cover_outside_bootstrap_chunk() {
    use std::collections::BTreeMap;

    let chunk_pos = (1..128)
        .map(|x| (x, 0))
        .find(|&chunk| hostile_chunk_spawns(chunk) && passive_chunk_spawns(chunk))
        .expect("hostile chunk sample");
    let plains = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let zombie = mc_data::Identifier::parse("minecraft:zombie").unwrap();
    let rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
        plains.clone(),
        BTreeMap::from([(
            "monster".to_string(),
            vec![mc_data::biomes::BiomeSpawnEntry {
                entity_type: zombie.clone(),
                min_count: 1,
                max_count: 1,
                weight: 1,
            }],
        )]),
    )]));
    let entity_types = mc_data::entity_types::EntityTypeRegistry::from_report(&[
        mc_data::entity_types::EntityTypeReport {
            id: zombie,
            protocol_id: 1,
        },
    ]);
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    let stone = mc_world::BlockStateId(2);
    let mut open = Chunk::empty(
        ChunkPos {
            x: chunk_pos.0,
            z: chunk_pos.1,
        },
        mc_world::BlockStateId(0),
        plains.clone(),
    );
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = open.set_block(lx, 64, lz, grass);
        }
    }
    let mut covered = open.clone();
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = covered.set_block(lx, 67, lz, stone);
        }
    }

    let open_spawns = plan_passive_herd(&open, Some(grass), None, &passable, &rules, &entity_types);
    let covered_spawns = plan_passive_herd(
        &covered,
        Some(grass),
        None,
        &passable,
        &rules,
        &entity_types,
    );

    assert!(open_spawns.is_empty());
    assert_eq!(covered_spawns.len(), 1);
    assert_eq!(covered_spawns[0].entity_type_name, "minecraft:zombie");
}

#[test]
fn creative_and_spectator_modes_grant_client_abilities() {
    let creative = player_abilities_for_mode(GameMode::Creative);
    assert!(creative.invulnerable);
    assert!(creative.can_fly);
    assert!(creative.instabuild);
    assert!(!creative.flying);

    let spectator = player_abilities_for_mode(GameMode::Spectator);
    assert!(spectator.invulnerable);
    assert!(spectator.can_fly);
    assert!(spectator.flying);
    assert!(!spectator.instabuild);
}

#[test]
fn survival_like_modes_revoke_client_abilities() {
    let survival = player_abilities_for_mode(GameMode::Survival);
    assert!(!survival.invulnerable);
    assert!(!survival.can_fly);
    assert!(!survival.instabuild);
    assert!(!survival.flying);

    let adventure = player_abilities_for_mode(GameMode::Adventure);
    assert_eq!(survival, adventure);
}

#[test]
fn full_survival_state_maps_to_health_packet() {
    assert_eq!(
        SurvivalState::FULL.as_packet(),
        ClientboundSetHealth {
            health: 20.0,
            food: 20,
            saturation: 5.0,
        }
    );
}

#[test]
fn fall_damage_starts_after_three_blocks_on_landing() {
    let mut old_pose = PlayerPose::new(0.0, 70.0, 0.0);
    old_pose.flags = MovePlayerFlags::new(false, false);
    let mut landing = PlayerPose::new(0.0, 65.5, 0.0);
    landing.flags = MovePlayerFlags::new(true, false);

    assert_eq!(fall_damage_amount(old_pose, landing), 1.0);

    let mut short_landing = PlayerPose::new(0.0, 67.5, 0.0);
    short_landing.flags = MovePlayerFlags::new(true, false);
    assert_eq!(fall_damage_amount(old_pose, short_landing), 0.0);

    old_pose.flags = MovePlayerFlags::new(true, false);
    assert_eq!(fall_damage_amount(old_pose, landing), 0.0);
}

#[test]
fn survival_damage_heal_and_death_are_clamped() {
    let mut state = SurvivalState::FULL;

    state.apply_damage(7.5);
    assert_eq!(state.health, 12.5);
    assert!(!state.is_dead());

    state.heal(100.0);
    assert_eq!(state.health, SurvivalState::MAX_HEALTH);

    state.apply_damage(100.0);
    assert_eq!(state.health, 0.0);
    assert!(state.is_dead());
}

#[test]
fn survival_exhaustion_drains_saturation_before_food() {
    let mut state = SurvivalState {
        health: 20.0,
        food: 20,
        saturation: 1.0,
        exhaustion: 0.0,
    };

    state.add_exhaustion(4.0);
    assert_eq!(state.saturation, 0.0);
    assert_eq!(state.food, 20);
    assert_eq!(state.exhaustion, 0.0);

    state.add_exhaustion(8.0);
    assert_eq!(state.food, 18);
    assert_eq!(state.saturation, 0.0);
}

#[test]
fn survival_food_addition_clamps_to_food_level() {
    let mut state = SurvivalState {
        health: 20.0,
        food: 18,
        saturation: 1.0,
        exhaustion: 0.0,
    };

    state.add_food(10, 30.0);

    assert_eq!(state.food, 20);
    assert_eq!(state.saturation, 20.0);
}

#[test]
fn pack_block_pos_round_trip() {
    // The packed-i64 representation is bit-exact what vanilla wants.
    // Just confirm the formula does not panic and that nominal
    // origin packs to 0.
    assert_eq!(pack_block_pos(0, 0, 0), 0);
    assert_ne!(pack_block_pos(1, 0, 0), 0);
    assert_ne!(pack_block_pos(0, 1, 0), 0);
    assert_ne!(pack_block_pos(0, 0, 1), 0);
}

#[test]
fn spawn_chunk_pos_matches_origin() {
    // SPAWN_(X,Z) = (0.5, 0.5); the containing chunk is (0, 0).
    assert_eq!(spawn_chunk_pos(), (0, 0));
}

#[test]
fn spawn_y_uses_chunk_heightmap_without_block_light_table() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains);
    let top_y = 72;
    chunk
        .highest_opaque
        .set(0, 0, (top_y - mc_world::MIN_Y + 1) as u32);

    assert_eq!(spawn_y_from_chunk(&mut chunk, None), Some(74.0));
}

#[test]
fn underwater_break_refills_target_with_water() {
    let air = mc_world::BlockStateId(0);
    let water = mc_world::BlockStateId(2);
    let stone = mc_world::BlockStateId(1);

    assert_eq!(
        break_replacement_from_neighbours([Some(water), None, None, None, None], air, water),
        water
    );
    assert_eq!(
        break_replacement_from_neighbours([Some(stone), None, None, None, None], air, water),
        air
    );
}

#[test]
fn chunk_pos_from_coords_uses_floor_division() {
    assert_eq!(chunk_pos_from_coords(0.0, 0.0), (0, 0));
    assert_eq!(chunk_pos_from_coords(15.999, 15.999), (0, 0));
    assert_eq!(chunk_pos_from_coords(16.0, -0.001), (1, -1));
    assert_eq!(chunk_pos_from_coords(-0.001, -16.0), (-1, -1));
    assert_eq!(chunk_pos_from_coords(-16.001, 32.0), (-2, 2));
}

#[test]
fn session_registry_drops_prepared_cache_with_last_ticket() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "tester".to_string(),
    };
    let (id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    registry.cache_prepared_chunk(
        (0, 0),
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );
    assert!(registry.prepared_chunk((0, 0)).is_some());

    let _ = registry.unregister(id);

    assert!(registry.prepared_chunk((0, 0)).is_none());
}

#[test]
fn session_registry_invalidates_multiple_prepared_chunks() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "tester".to_string(),
    };
    let _ = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    for chunk in [(0, 0), (1, 0)] {
        registry.cache_prepared_chunk(
            chunk,
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                herd_spawns: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );
        assert!(registry.prepared_chunk(chunk).is_some());
    }

    registry.invalidate_prepared_chunks(&HashSet::from([(0, 0), (1, 0)]));

    assert!(registry.prepared_chunk((0, 0)).is_none());
    assert!(registry.prepared_chunk((1, 0)).is_none());
}

#[test]
fn hostile_entities_follow_nearby_player_position() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "target".to_string(),
    };
    let _ = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, DEFAULT_SPAWN_Y + 1.0, 0.5),
    );
    let _ = registry.ensure_chunk_herd(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 1,
            entity_type_name: "minecraft:zombie".to_string(),
            position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
            hostile: true,
        }],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    let zombie = queries
        .iter()
        .find(|query| query.position.x == 0.5 && query.position.z == 0.5)
        .expect("zombie physics query");

    assert!(zombie.velocity.x > 0.0);
    assert_eq!(zombie.velocity.z, 0.0);
}

#[test]
fn chunk_herd_materialization_applies_caps_and_player_distance() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "nearby".to_string(),
    };
    let _ = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let spawns = (0..10)
        .map(|slot| HerdSpawn {
            chunk: (0, 0),
            slot,
            entity_type_id: 1,
            entity_type_name: if slot < 5 {
                "minecraft:zombie".to_string()
            } else {
                "minecraft:cow".to_string()
            },
            position: if slot == 0 {
                Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5)
            } else {
                Vec3::new(4.5 + f64::from(slot), DEFAULT_SPAWN_Y, 0.5)
            },
            hostile: slot < 5,
        })
        .collect::<Vec<_>>();

    let _ = registry.ensure_chunk_herd((0, 0), &spawns);
    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), MAX_HOSTILE_SPAWNS_PER_CHUNK + 5);
    assert!(queries.iter().all(|query| query.position.x != 0.5));
}

#[test]
fn chunk_herd_materialization_dedupes_restored_herd_uuid() {
    let registry = SessionRegistry::new();
    let uuid = herd_uuid((0, 0), 0);
    let restored = mc_entity::EntitySnapshot {
        id: EntityId(42),
        uuid,
        type_id: 1,
        type_name: "minecraft:cow".to_string(),
        position: Vec3::new(20.5, DEFAULT_SPAWN_Y, 0.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        lifecycle: EntityLifecycle::Alive,
        health: 10.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
    };

    assert_eq!(registry.restore_persisted_entities([restored]), 1);
    let _ = registry.ensure_chunk_herd(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 1,
            entity_type_name: "minecraft:cow".to_string(),
            position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
            hostile: false,
        }],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].id, EntityId(42));
}

#[test]
fn loaded_recipients_for_chunks_can_include_origin() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel(8);
    let (bob_tx, _bob_rx) = mpsc::channel(8);
    let alice = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "Alice".to_string(),
    };
    let bob = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "Bob".to_string(),
    };
    let (alice_id, _) = registry.register(
        &alice,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        alice_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let (bob_id, _) = registry.register(
        &bob,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        bob_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(alice_id, (0, 0));
    let _ = registry.mark_loaded(bob_id, (0, 0));
    let chunks = HashSet::from([(0, 0)]);

    let mut without_origin: Vec<_> = registry
        .loaded_recipients_for_chunks(&chunks, Some(alice_id))
        .into_iter()
        .map(|recipient| recipient.id)
        .collect();
    without_origin.sort_unstable();
    let mut with_origin: Vec<_> = registry
        .loaded_recipients_for_chunks(&chunks, None)
        .into_iter()
        .map(|recipient| recipient.id)
        .collect();
    with_origin.sort_unstable();

    assert_eq!(without_origin, vec![bob_id]);
    assert_eq!(with_origin, vec![alice_id, bob_id]);
}

#[test]
fn session_registry_reports_ticketed_chunks_sorted() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "tester".to_string(),
    };
    let (id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(1, 0), (0, -1), (0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );

    assert_eq!(
        registry.ticketed_chunks_sorted(),
        vec![(0, -1), (0, 0), (1, 0)]
    );

    let _ = registry.unregister(id);
    assert!(registry.ticketed_chunks_sorted().is_empty());
}

#[test]
fn random_tick_sampling_is_deterministic_for_seed_tick_and_chunks() {
    let policy = RandomTickPolicy {
        random_tick_speed: 2,
        chunk_budget: 2,
        seed: 99,
    };
    let chunks = vec![(0, 0), (1, 0), (2, 0)];

    let first = sample_random_tick_positions(policy, 7, &chunks);
    let second = sample_random_tick_positions(policy, 7, &chunks);

    assert_eq!(first, second);
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|sample| sample.pos.y >= mc_world::MIN_Y));
    assert!(first.iter().all(|sample| sample.pos.y < mc_world::MAX_Y));
}

#[test]
fn random_tick_sampling_rotates_chunk_budget() {
    let policy = RandomTickPolicy {
        random_tick_speed: 1,
        chunk_budget: 2,
        seed: 0,
    };
    let chunks = vec![(0, 0), (1, 0), (2, 0)];

    let tick_zero = sample_random_tick_positions(policy, 0, &chunks)
        .into_iter()
        .map(|sample| sample.chunk)
        .collect::<Vec<_>>();
    let tick_one = sample_random_tick_positions(policy, 1, &chunks)
        .into_iter()
        .map(|sample| sample.chunk)
        .collect::<Vec<_>>();

    assert_eq!(tick_zero, vec![(0, 0), (1, 0)]);
    assert_eq!(tick_one, vec![(1, 0), (2, 0)]);
}

#[test]
fn random_tick_speed_zero_disables_sampling() {
    let policy = RandomTickPolicy {
        random_tick_speed: 0,
        chunk_budget: 2,
        seed: 0,
    };

    assert!(sample_random_tick_positions(policy, 0, &[(0, 0)]).is_empty());
}

#[test]
fn session_registry_spawns_and_despawns_visible_players() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel(8);
    let (bob_tx, _bob_rx) = mpsc::channel(8);
    let alice = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "Alice".to_string(),
    };
    let bob = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "Bob".to_string(),
    };

    let (alice_id, _) = registry.register(
        &alice,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        alice_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    assert!(registry.mark_loaded(alice_id, (0, 0)).is_empty());

    let (bob_id, dispatches) = registry.register(
        &bob,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        bob_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == alice_id
            && matches!(
                &dispatch.command,
                OutboundCommand::SpawnPlayer(player)
                    if player.session_id == bob_id && player.name == "Bob"
            )
    }));

    let dispatches = registry.mark_loaded(bob_id, (0, 0));
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == bob_id
            && matches!(
                &dispatch.command,
                OutboundCommand::SpawnPlayer(player)
                    if player.session_id == alice_id && player.name == "Alice"
            )
    }));

    let dispatches = registry.update_pose(
        bob_id,
        PlayerPose {
            x: 48.5,
            y: DEFAULT_SPAWN_Y,
            z: 0.5,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        },
    );
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == alice_id
            && matches!(
                &dispatch.command,
                OutboundCommand::DespawnPlayer(player) if player.session_id == bob_id
            )
    }));
}

#[test]
fn session_registry_unregister_removes_visible_player() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel(8);
    let (bob_tx, _bob_rx) = mpsc::channel(8);
    let alice = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "Alice".to_string(),
    };
    let bob = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "Bob".to_string(),
    };

    let (alice_id, _) = registry.register(
        &alice,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        alice_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(alice_id, (0, 0));
    let (bob_id, _) = registry.register(
        &bob,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        bob_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );

    let dispatches = registry.unregister(bob_id);
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == alice_id
            && matches!(
                &dispatch.command,
                OutboundCommand::DespawnPlayer(player) if player.session_id == bob_id
            )
    }));
}

#[test]
fn spiral_chunks_starts_at_centre() {
    let mut iter = spiral_chunks(3, -7, 2);
    assert_eq!(iter.next(), Some((3, -7)));
}

#[test]
fn spiral_chunks_covers_every_cell_in_window() {
    for vd in 0..=4 {
        let collected: std::collections::HashSet<(i32, i32)> =
            spiral_chunks(0, 0, vd).collect();
        let expected_count = ((2 * vd + 1) as usize).pow(2);
        assert_eq!(
            collected.len(),
            expected_count,
            "vd={vd}: spiral should yield {expected_count} unique cells",
        );
        for dz in -vd..=vd {
            for dx in -vd..=vd {
                assert!(
                    collected.contains(&(dx, dz)),
                    "vd={vd}: missing cell ({dx},{dz})"
                );
            }
        }
    }
}

#[test]
fn spiral_chunks_ring_order_monotonic() {
    // Within the iteration, the chebyshev distance must be
    // non-decreasing. That's the property that makes the
    // perceptual spread feel like a spiral rather than a scan.
    let mut last_ring = -1i32;
    for (dx, dz) in spiral_chunks(0, 0, 3) {
        let r = dx.abs().max(dz.abs());
        assert!(
            r >= last_ring,
            "non-monotonic ring sequence at cell ({dx},{dz}): r={r} < last={last_ring}"
        );
        last_ring = r;
    }
}

#[test]
fn prioritized_spiral_prefers_player_look_direction_within_ring() {
    let south: Vec<_> = prioritized_spiral(0, 0, 2, 0.0)
        .map(|(cx, cz, _)| (cx, cz))
        .collect();
    assert_eq!(south[0], (0, 0));
    assert_eq!(south[1], (0, 1));
    assert!(
        south.iter().position(|chunk| *chunk == (0, 1)).unwrap()
            < south.iter().position(|chunk| *chunk == (0, -1)).unwrap()
    );

    let west: Vec<_> = prioritized_spiral(0, 0, 2, 90.0)
        .map(|(cx, cz, _)| (cx, cz))
        .collect();
    assert_eq!(west[1], (-1, 0));
    assert!(
        west.iter().position(|chunk| *chunk == (-1, 0)).unwrap()
            < west.iter().position(|chunk| *chunk == (1, 0)).unwrap()
    );
}

#[test]
fn spawn_dimension_prefers_alphabetical_first() {
    let data = mc_data::testing::stub();
    let (id, name, all) = spawn_dimension(&data).unwrap();
    assert_eq!(id, 0);
    assert_eq!(name.as_str(), "minecraft:alpha");
    assert_eq!(all.len(), 2);
}

#[test]
fn block_delta_plan_keeps_single_edit_as_block_update() {
    let delta = BlockDelta {
        x: 1,
        y: -60,
        z: 2,
        state_id: mc_world::BlockStateId(1),
    };

    assert_eq!(
        plan_block_delta_packets(&[delta]),
        vec![BlockDeltaPacket::Single(delta)]
    );
}

#[test]
fn block_delta_plan_groups_multiple_changes_in_same_section() {
    let first = BlockDelta {
        x: -1,
        y: -60,
        z: 2,
        state_id: mc_world::BlockStateId(1),
    };
    let second = BlockDelta {
        x: -2,
        y: -61,
        z: 3,
        state_id: mc_world::BlockStateId(2),
    };

    assert_eq!(
        plan_block_delta_packets(&[first, second]),
        vec![BlockDeltaPacket::Section {
            section_x: -1,
            section_y: -4,
            section_z: 0,
            changes: vec![first, second],
        }]
    );
}

#[test]
fn block_delta_plan_does_not_section_pack_singletons() {
    let first = BlockDelta {
        x: 0,
        y: 0,
        z: 0,
        state_id: mc_world::BlockStateId(1),
    };
    let second = BlockDelta {
        x: 16,
        y: 0,
        z: 0,
        state_id: mc_world::BlockStateId(2),
    };

    assert_eq!(
        plan_block_delta_packets(&[first, second]),
        vec![
            BlockDeltaPacket::Single(first),
            BlockDeltaPacket::Single(second)
        ]
    );
}
