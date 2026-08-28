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
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
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
        &[],
        Some(&[water]),
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
    assert!(spawns.iter().all(|spawn| spawn.entity_type_id == 100));
    assert!(spawns.iter().all(|spawn| {
        let fx = spawn.position.x.fract();
        let fz = spawn.position.z.fract();
        (0.48..=0.51).contains(&fx) && (0.48..=0.51).contains(&fz)
    }));

    let mut unsupported_chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        plains.clone(),
    );
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
        &[],
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
    let mut ocean_chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        ocean.clone(),
    );
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = ocean_chunk.set_block(lx, DEFAULT_SEA_LEVEL, lz, water);
        }
    }

    let spawns = plan_passive_herd(
        &ocean_chunk,
        Some(grass),
        &[],
        Some(&[water]),
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
        &[],
        Some(&[water]),
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
fn water_spawn_planner_uses_mid_column_and_all_water_states() {
    use std::collections::BTreeMap;

    let ocean = mc_data::Identifier::parse("minecraft:ocean").unwrap();
    let cod = mc_data::Identifier::parse("minecraft:cod").unwrap();
    let rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
        ocean.clone(),
        BTreeMap::from([(
            "water_ambient".to_string(),
            vec![mc_data::biomes::BiomeSpawnEntry {
                entity_type: cod.clone(),
                min_count: 1,
                max_count: 1,
                weight: 1,
            }],
        )]),
    )]));
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let water_source = mc_world::BlockStateId(2);
    let water_flowing = mc_world::BlockStateId(3);
    let mut chunk = Chunk::empty(ChunkPos { x: 1, z: 0 }, mc_world::BlockStateId(0), ocean);
    for lx in 3..=12 {
        for lz in 3..=12 {
            for y in 50_i32..=58 {
                let state = if y % 2 == 0 {
                    water_source
                } else {
                    water_flowing
                };
                let _ = chunk.set_block(lx, y, lz, state);
            }
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        None,
        &[],
        Some(&[water_source, water_flowing]),
        &[mc_world::BlockStateId(0)],
        &rules,
        &entity_types,
    );

    assert!(!spawns.is_empty());
    assert!(
        spawns
            .iter()
            .all(|spawn| (53.0..=55.0).contains(&spawn.position.y))
    );
    assert!(spawns.iter().all(|spawn| {
        let lx = (spawn.position.x.floor() as i32 - chunk.pos.x * 16) as u8;
        let lz = (spawn.position.z.floor() as i32 - chunk.pos.z * 16) as u8;
        matches!(
            chunk.get_block(lx, spawn.position.y as i32, lz),
            Some(state) if state == water_source || state == water_flowing
        )
    }));
}

#[test]
fn hostile_spawn_planner_uses_multiple_monster_facts() {
    use std::collections::{BTreeMap, HashSet};

    let chunk_pos = (1..128)
        .map(|x| (x, 0))
        .find(|&chunk| hostile_chunk_spawns(chunk) && passive_chunk_spawns(chunk))
        .expect("hostile chunk sample");
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
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let mut chunk = Chunk::empty(
        ChunkPos {
            x: chunk_pos.0,
            z: chunk_pos.1,
        },
        mc_world::BlockStateId(0),
        plains,
    );
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    let stone = mc_world::BlockStateId(2);
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = chunk.set_block(lx, 64, lz, grass);
            let _ = chunk.set_block(lx, 67, lz, stone);
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        Some(grass),
        &[],
        None,
        &passable,
        &rules,
        &entity_types,
    );

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
fn hostile_spawn_planner_prepares_open_surface_candidate_for_night() {
    use std::collections::BTreeMap;

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
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains);
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = chunk.set_block(lx, 64, lz, grass);
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        Some(grass),
        &[],
        None,
        &passable,
        &rules,
        &entity_types,
    );

    assert_eq!(spawns.len(), 1);
    assert!(spawns[0].hostile);
    assert_eq!(spawns[0].entity_type_name, "minecraft:zombie");
}

#[test]
fn hostile_spawn_planner_surface_candidate_does_not_require_cover() {
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
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
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

    let open_spawns = plan_passive_herd(
        &open,
        Some(grass),
        &[],
        None,
        &passable,
        &rules,
        &entity_types,
    );
    let covered_spawns = plan_passive_herd(
        &covered,
        Some(grass),
        &[],
        None,
        &passable,
        &rules,
        &entity_types,
    );

    assert_eq!(open_spawns.len(), 1);
    assert_eq!(open_spawns[0].entity_type_name, "minecraft:zombie");
    assert_eq!(covered_spawns.len(), 1);
    assert_eq!(covered_spawns[0].entity_type_name, "minecraft:zombie");
}

#[test]
fn hostile_spawn_planner_does_not_depend_on_passive_chunk_selection() {
    use std::collections::BTreeMap;

    let chunk_pos = (-4..=4)
        .flat_map(|x| (-4..=4).map(move |z| (x, z)))
        .find(|&chunk| hostile_chunk_spawns(chunk) && !passive_chunk_spawns(chunk))
        .expect("hostile-only chunk sample");
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
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let passable = vec![mc_world::BlockStateId(0)];
    let grass = mc_world::BlockStateId(1);
    let mut chunk = Chunk::empty(
        ChunkPos {
            x: chunk_pos.0,
            z: chunk_pos.1,
        },
        mc_world::BlockStateId(0),
        plains,
    );
    for lx in 3..=12 {
        for lz in 3..=12 {
            let _ = chunk.set_block(lx, 64, lz, grass);
            let _ = chunk.set_block(lx, 67, lz, mc_world::BlockStateId(2));
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        Some(grass),
        &[],
        None,
        &passable,
        &rules,
        &entity_types,
    );

    assert!(
        spawns
            .iter()
            .any(|spawn| spawn.hostile && spawn.entity_type_name == "minecraft:zombie"),
        "covered hostile-only chunks must still seed playable combat"
    );
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
fn fall_damage_uses_accumulated_airborne_height() {
    let mut takeoff = PlayerPose::new(0.0, 70.0, 0.0);
    takeoff.flags = MovePlayerFlags::new(true, false);
    let mut mid_fall = PlayerPose::new(0.0, 66.0, 0.0);
    mid_fall.flags = MovePlayerFlags::new(false, false);
    refresh_player_fall_state(takeoff, &mut mid_fall);

    let mut landing = PlayerPose::new(0.0, 64.0, 0.0);
    landing.flags = MovePlayerFlags::new(true, false);

    assert_eq!(fall_damage_amount(mid_fall, landing), 3.0);
}

#[test]
fn survival_damage_heal_and_death_are_clamped() {
    let mut state = SurvivalState::FULL;

    state.apply_damage(7.5);
    assert_eq!(state.health, 12.5);
    assert!(!state.is_dead());

    state.heal(100.0);
    assert_eq!(state.health, mc_entity::player_survival_26_1_2::MAX_HEALTH);

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
        remaining_fire_ticks: 0,
    };

    assert!(!state.add_exhaustion(3.0));
    assert_eq!(state.saturation, 1.0);
    assert_eq!(state.food, 20);
    assert_eq!(state.exhaustion, 3.0);

    assert!(state.add_exhaustion(1.0));
    assert_eq!(state.saturation, 0.0);
    assert_eq!(state.food, 20);
    assert_eq!(state.exhaustion, 0.0);

    assert!(state.add_exhaustion(8.0));
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
        remaining_fire_ticks: 0,
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

#[tokio::test]
async fn spawn_position_reads_published_chunk_while_world_writer_is_held() {
    let blocks = Arc::new(BlockRegistry::from_report(&[simple_block(0, "minecraft:air")]).unwrap());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let top_y = 72;
    chunk
        .highest_opaque
        .set(0, 0, (top_y - mc_world::MIN_Y + 1) as u32);
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let world_read = world.read_view();
    let config = Arc::new(simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy::default(),
        Arc::new(mc_data::block_facts::BlockFactsTable::default()),
    ));
    let world = Arc::clone(config.world.as_ref().unwrap());
    let writer = world.lock().await;
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn({
        let config = Arc::clone(&config);
        async move {
            let position = spawn_position(&config, Some(&world_read));
            let _ = finished_tx.send(position);
        }
    });

    let position = tokio::time::timeout(Duration::from_secs(1), finished_rx)
        .await
        .expect("published spawn lookup waited for the world writer")
        .expect("spawn lookup task dropped its result");
    drop(writer);
    task.await.expect("spawn lookup task failed");

    assert_eq!(position, (SPAWN_X, 74.0, SPAWN_Z));
}

#[tokio::test]
async fn spawn_position_uses_published_nonzero_world_spawn() {
    let blocks = Arc::new(fluid_test_registry());
    let spawn = mc_world::WorldSpawn::new(160, -80);
    let position = spawn.chunk();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks)).with_spawn(spawn);
    let mut chunk = Chunk::empty(
        position,
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_block(0, 64, 0, mc_world::BlockStateId(1));
    chunk
        .highest_opaque
        .set(0, 0, (64 - mc_world::MIN_Y + 1) as u32);
    world.commit_chunk_snapshot(position, chunk).unwrap();
    let world_read = world.read_view();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy::default(),
        Arc::new(fluid_test_facts()),
    );

    assert_eq!(world_read.spawn(), spawn);
    assert_eq!(
        spawn_position(&config, Some(&world_read)),
        (160.5, 66.0, -79.5)
    );
}

#[tokio::test]
async fn spawn_position_chooses_nearest_dry_column_instead_of_origin_water() {
    let blocks = Arc::new(fluid_test_registry());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_block(0, 60, 0, mc_world::BlockStateId(1));
    for y in 61..=63 {
        chunk.set_block(0, y, 0, mc_world::BlockStateId(2));
    }
    chunk
        .highest_opaque
        .set(0, 0, (60 - mc_world::MIN_Y + 1) as u32);
    chunk.set_block(2, 64, 0, mc_world::BlockStateId(1));
    chunk
        .highest_opaque
        .set(2, 0, (64 - mc_world::MIN_Y + 1) as u32);
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let world_read = world.read_view();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy::default(),
        Arc::new(fluid_test_facts()),
    );

    assert_eq!(spawn_position(&config, Some(&world_read)), (2.5, 66.0, 0.5));
}

#[tokio::test]
async fn spawn_position_skips_collidable_body_space_and_hazardous_support() {
    let reports = solaris_required_blocks_report();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let state = |name: &str| {
        blocks
            .block(&Identifier::parse(name).unwrap())
            .unwrap()
            .default
    };
    let air = state("minecraft:air");
    let stone = state("minecraft:stone");
    let glass = state("minecraft:glass");
    let magma = state("minecraft:magma_block");
    let oak_leaves = state("minecraft:oak_leaves");
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for (x, support) in [(0, stone), (1, magma), (2, oak_leaves), (3, stone)] {
        chunk.set_block(x, 64, 0, support);
        chunk
            .highest_opaque
            .set(x, 0, (64 - mc_world::MIN_Y + 1) as u32);
    }
    chunk.set_block(0, 66, 0, glass);
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let world_read = world.read_view();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy::default(),
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );

    assert_eq!(spawn_position(&config, Some(&world_read)), (3.5, 66.0, 0.5));
}

#[test]
fn break_replacement_next_to_water_uses_flowing_state() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let target = mc_world::BlockPos { x: 8, y: 63, z: 0 };
    let plains = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(
            mc_world::ChunkPos { x: 0, z: 0 },
            Chunk::empty(
                mc_world::ChunkPos { x: 0, z: 0 },
                mc_world::BlockStateId(0),
                plains,
            ),
        )
        .unwrap();
    world
        .set_block_at(
            mc_world::BlockPos { x: 7, y: 63, z: 0 },
            mc_world::BlockStateId(2),
        )
        .unwrap();

    assert_eq!(
        supported_flow_state(&registry, &facts, &world, target, facts.fluid(2).unwrap()),
        Some(mc_world::BlockStateId(3))
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
fn passable_block_names_cover_common_flowers() {
    assert!(passable_block_name("minecraft:blue_orchid"));
    assert!(passable_block_name("minecraft:lily_of_the_valley"));
    assert!(passable_block_name("minecraft:rose_bush"));
    assert!(!passable_block_name("minecraft:flower_pot"));
    assert!(!passable_block_name("minecraft:stone"));
}

#[test]
fn passable_block_names_cover_non_colliding_crops() {
    for name in [
        "minecraft:wheat",
        "minecraft:carrots",
        "minecraft:potatoes",
        "minecraft:beetroots",
        "minecraft:torchflower_crop",
        "minecraft:pitcher_crop",
        "minecraft:melon_stem",
        "minecraft:attached_melon_stem",
        "minecraft:pumpkin_stem",
        "minecraft:attached_pumpkin_stem",
        "minecraft:sweet_berry_bush",
        "minecraft:nether_wart",
    ] {
        assert!(passable_block_name(name), "{name}");
    }
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
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    registry.cache_prepared_chunk(
        (0, 0),
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
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
                hydrated_campfires: Vec::new(),
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
    registry.set_world_time(NIGHT_START_TICK);
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "target".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(4.5, DEFAULT_SPAWN_Y + 1.0, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.mark_loaded(session_id, (1, 0));
    let _ = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 1,
            entity_type_name: "minecraft:zombie".to_string(),
            position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
            hostile: true,
            sheep_color: None,
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
fn passive_herd_wander_uses_each_entity_movement_speed() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "observer".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[
            HerdSpawn {
                chunk: (0, 0),
                slot: 0,
                entity_type_id: 1,
                entity_type_name: "minecraft:cow".to_string(),
                position: Vec3::new(8.5, DEFAULT_SPAWN_Y, 8.5),
                hostile: false,
                sheep_color: None,
            },
            HerdSpawn {
                chunk: (0, 0),
                slot: 1,
                entity_type_id: 2,
                entity_type_name: "minecraft:sheep".to_string(),
                position: Vec3::new(9.5, DEFAULT_SPAWN_Y, 8.5),
                hostile: false,
                sheep_color: None,
            },
            HerdSpawn {
                chunk: (0, 0),
                slot: 2,
                entity_type_id: 3,
                entity_type_name: "minecraft:chicken".to_string(),
                position: Vec3::new(10.5, DEFAULT_SPAWN_Y, 8.5),
                hostile: false,
                sheep_color: None,
            },
        ],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    for (x, expected_speed) in [(8.5, 2.0), (9.5, 2.3), (10.5, 2.5)] {
        let query = queries
            .iter()
            .find(|query| query.position.x == x && query.position.z == 8.5)
            .expect("passive mob physics query");
        assert!((query.velocity.horizontal_len() - expected_speed).abs() < 1.0e-9);
    }
}

#[test]
fn natural_sheep_color_reaches_authoritative_ecs_and_spawn_snapshot() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "observer".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));

    let dispatches = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 2,
            entity_type_name: "minecraft:sheep".to_string(),
            position: Vec3::new(8.5, DEFAULT_SPAWN_Y, 8.5),
            hostile: false,
            sheep_color: Some(mc_entity::SheepColor::Brown),
        }],
    );
    let spawned = dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity),
            _ => None,
        })
        .expect("natural sheep spawn dispatch");
    let wool = spawned
        .animal
        .and_then(|animal| animal.sheep_wool)
        .expect("spawn snapshot carries sheep wool state");

    assert_eq!(wool.color, mc_entity::SheepColor::Brown);
    assert_eq!(
        wool.packed_metadata(),
        i8::try_from(mc_entity::SheepColor::Brown.id()).unwrap()
    );
    assert_eq!(
        registry
            .server_entity_snapshot(spawned.id)
            .and_then(|entity| entity.animal)
            .and_then(|animal| animal.sheep_wool)
            .map(|wool| wool.color),
        Some(mc_entity::SheepColor::Brown)
    );
}

#[test]
fn hostile_pathing_keeps_the_next_tick_inside_loaded_chunk() {
    let registry = SessionRegistry::new();
    registry.set_world_time(NIGHT_START_TICK);
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "target".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (1, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(20.5, DEFAULT_SPAWN_Y + 1.0, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 1,
            entity_type_name: "minecraft:zombie".to_string(),
            position: Vec3::new(15.5, DEFAULT_SPAWN_Y, 0.5),
            hostile: true,
            sheep_color: None,
        }],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    let zombie = queries
        .iter()
        .find(|query| query.position.x == 15.5 && query.position.z == 0.5)
        .expect("zombie physics query");

    assert_ne!(zombie.velocity, Vec3::ZERO);
    assert!(zombie.position.x + zombie.velocity.x * PathingBudget::TICK_SECONDS < 16.0);
}

#[test]
fn hostile_pathing_keeps_full_speed_while_next_tick_remains_loaded() {
    let registry = SessionRegistry::new();
    registry.set_world_time(NIGHT_START_TICK);
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "target".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (1, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(20.5, DEFAULT_SPAWN_Y + 1.0, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 1,
            entity_type_name: "minecraft:zombie".to_string(),
            position: Vec3::new(14.9, DEFAULT_SPAWN_Y, 0.5),
            hostile: true,
            sheep_color: None,
        }],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    let zombie = queries
        .iter()
        .find(|query| query.position.x == 14.9 && query.position.z == 0.5)
        .expect("zombie physics query");

    assert_ne!(zombie.velocity, Vec3::ZERO);
    assert!((zombie.velocity.x - HOSTILE_FOLLOW_SPEED).abs() < 1.0e-9);
    assert!(zombie.position.x + zombie.velocity.x * PathingBudget::TICK_SECONDS < 16.0);
}

#[test]
fn chunk_herd_materialization_applies_caps_and_player_distance() {
    let registry = SessionRegistry::new();
    registry.set_world_time(NIGHT_START_TICK);
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "nearby".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.mark_loaded(session_id, (1, 0));
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
            sheep_color: None,
        })
        .collect::<Vec<_>>();

    let _ = registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns);
    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), MAX_HOSTILE_SPAWNS_PER_CHUNK + 5);
    assert!(queries.iter().all(|query| query.position.x != 0.5));
}

#[test]
fn restored_herd_uuid_dedupes_without_suppressing_missing_night_spawn() {
    let registry = SessionRegistry::new();
    registry.set_world_time(NIGHT_START_TICK);
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "observer".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.mark_loaded(session_id, (1, 0));
    let uuid = herd_uuid((0, 0), 0);
    let restored = mc_entity::EntitySnapshot {
        id: EntityId(42),
        uuid,
        type_id: 1,
        type_name: "minecraft:cow".to_string(),
        position: Vec3::new(8.5, DEFAULT_SPAWN_Y, 0.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: EntityLifecycle::Alive,
        health: 10.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
        retained: mc_entity::EntityRetainedState::default(),
    };

    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(0, [restored])),
        1
    );
    let _ = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[
            HerdSpawn {
                chunk: (0, 0),
                slot: 0,
                entity_type_id: 1,
                entity_type_name: "minecraft:cow".to_string(),
                position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
                hostile: false,
                sheep_color: None,
            },
            HerdSpawn {
                chunk: (0, 0),
                slot: 1,
                entity_type_id: 2,
                entity_type_name: "minecraft:zombie".to_string(),
                position: Vec3::new(2.5, DEFAULT_SPAWN_Y, 0.5),
                hostile: true,
                sheep_color: None,
            },
        ],
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    assert_eq!(queries.len(), 2);
    assert!(queries.iter().any(|query| query.id == EntityId(42)));
    assert!(queries.iter().any(|query| query.id != EntityId(42)));
}

#[test]
fn entity_physics_skips_persisted_entities_without_loaded_players() {
    let registry = SessionRegistry::new();
    let restored = mc_entity::EntitySnapshot {
        id: EntityId(42),
        uuid: uuid::Uuid::from_u128(42),
        type_id: 1,
        type_name: "minecraft:cow".to_string(),
        position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: EntityLifecycle::Alive,
        health: 10.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
        retained: mc_entity::EntityRetainedState::default(),
    };

    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(0, [restored])),
        1
    );

    assert!(
        registry
            .tick_entities_and_collect_physics_queries(1)
            .is_empty()
    );
}

#[test]
fn entity_physics_skips_loaded_entities_outside_simulation_distance() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "observer".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (5, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.mark_loaded(session_id, (5, 0));
    let near = mc_entity::EntitySnapshot {
        id: EntityId(1),
        uuid: uuid::Uuid::from_u128(1),
        type_id: 1,
        type_name: "minecraft:cow".to_string(),
        position: Vec3::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: EntityLifecycle::Alive,
        health: 10.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
        retained: mc_entity::EntityRetainedState::default(),
    };
    let far = mc_entity::EntitySnapshot {
        id: EntityId(2),
        uuid: uuid::Uuid::from_u128(2),
        type_id: 1,
        type_name: "minecraft:cow".to_string(),
        position: Vec3::new(5.0 * 16.0 + 0.5, DEFAULT_SPAWN_Y, 0.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: EntityLifecycle::Alive,
        health: 10.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
        retained: mc_entity::EntityRetainedState::default(),
    };

    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(0, [near, far])),
        2
    );
    let queries = registry.tick_entities_and_collect_physics_queries_with_simulation_distance(1, 4);

    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].id, EntityId(1));
}

#[test]
fn entity_simulation_distance_handles_extreme_chunk_coordinates() {
    let far_negative_chunk = Vec3::new(f64::from(i32::MIN) * 16.0, DEFAULT_SPAWN_Y, 0.0);

    assert!(!session::entity_is_near_player_chunk(
        (i32::MAX, 0),
        &[far_negative_chunk],
        32,
    ));
}

#[test]
fn entity_physics_includes_every_active_entity_under_work_pressure() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "observer".to_string(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y + 1.0, 0.5),
    );
    let _ = registry.mark_loaded(session_id, (0, 0));
    let _ = registry.mark_loaded(session_id, (1, 0));
    let entities = (0..49)
        .map(|idx| mc_entity::EntitySnapshot {
            id: EntityId(1000 + idx),
            uuid: uuid::Uuid::from_u128(1000 + idx as u128),
            type_id: 1,
            type_name: if idx == 0 {
                "minecraft:zombie".to_string()
            } else {
                "minecraft:cow".to_string()
            },
            position: Vec3::new(
                0.5 + f64::from(idx % 7),
                DEFAULT_SPAWN_Y,
                0.5 + f64::from(idx / 7),
            ),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 10.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(0, entities)),
        49
    );
    let queries = registry.tick_entities_and_collect_physics_queries_with_pathing_budget(1, 8);

    assert_eq!(queries.len(), 49);
    assert!(queries.iter().any(|query| query.id == EntityId(1000)));
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

fn simulation_tick_test_config(
    blocks: Arc<BlockRegistry>,
    world: mc_world::WorldStorage,
    random_tick: RandomTickPolicy,
    block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
) -> crate::server::ServerConfig {
    crate::server::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "random-tick-test".into(),
        max_players: 1,
        view_distance: 0,
        data: Arc::new(mc_data::testing::stub()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts,
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick,
        command_permissions: crate::server::CommandPermissionConfig::new(
            Vec::<String>::new(),
            true,
        ),
        loader_manifest: None,
        shutdown: crate::server::ShutdownHandle::default(),
    }
}

#[tokio::test]
async fn sheep_grazing_animates_changes_grass_and_regrows_wool() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        simple_block(2, "minecraft:grass_block"),
        simple_block(3, "minecraft:short_grass"),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let grass_pos = mc_world::BlockPos { x: 0, y: 63, z: 0 };
    assert_eq!(
        chunk.set_block(0, grass_pos.y, 0, mc_world::BlockStateId(2)),
        Some(mc_world::BlockStateId(0))
    );
    world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    let config = simulation_tick_test_config(
        Arc::clone(&blocks),
        world,
        RandomTickPolicy::default(),
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );

    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "GrazingObserver".to_owned(),
    };
    let (tx, mut outbound) = mpsc::channel(32);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(sessions.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn = sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let sheep_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected sheep spawn, got {other:?}"),
    };
    dispatch_visibility_commands(spawn);
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == sheep_id
    ));
    assert!(sessions.set_sheep_sheared_for_test(sheep_id, true));

    let start_tick = (1..=2_000)
        .find(|tick| session::sheep_grazing_starts_on_tick(sheep_id, *tick, false))
        .expect("adult sheep receives a deterministic grazing opportunity");
    let (_simulation, owner) = simulation_channel();
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_read = shared_world.lock().await.read_view();
    let world_mutation = shared_world.lock().await.mutation_view();
    let world_writer = shared_world.lock().await;
    let mut start = Box::pin(owner.run_sheep_grazing(
        &config,
        &sessions,
        Some(&world_read),
        Some(&world_mutation),
        start_tick,
    ));
    let started = std::future::poll_fn(|cx| match Future::poll(start.as_mut(), cx) {
        Poll::Ready(report) => Poll::Ready(report),
        Poll::Pending => panic!("sheep grazing start waited for the world writer"),
    })
    .await;
    drop(world_writer);
    assert_eq!(started.started, 1);
    assert_eq!(started.ate, 0);
    assert!(matches!(
        outbound.recv().await,
        Some(OutboundCommand::EntityEvent {
            entity_id,
            event_id: 10,
        }) if entity_id == sheep_id.0
    ));

    for tick in (start_tick + 1)..(start_tick + 36) {
        let report = owner
            .run_sheep_grazing(
                &config,
                &sessions,
                Some(&world_read),
                Some(&world_mutation),
                tick,
            )
            .await;
        assert_eq!(report.ate, 0);
    }
    let world_writer = shared_world.lock().await;
    let mut finish = Box::pin(owner.run_sheep_grazing(
        &config,
        &sessions,
        Some(&world_read),
        Some(&world_mutation),
        start_tick + 36,
    ));
    let ate = std::future::poll_fn(|cx| match Future::poll(finish.as_mut(), cx) {
        Poll::Ready(report) => Poll::Ready(report),
        Poll::Pending => panic!("sheep grazing commit waited for the world writer"),
    })
    .await;
    drop(world_writer);
    assert_eq!(ate.ate, 1);

    assert_eq!(
        config
            .world
            .as_ref()
            .unwrap()
            .lock()
            .await
            .get_cached_block(grass_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(
        sessions
            .server_entity_snapshot(sheep_id)
            .and_then(|entity| entity.animal)
            .and_then(|animal| animal.sheep_wool)
            .is_some_and(|wool| !wool.sheared)
    );

    let mut saw_block_delta = false;
    let mut saw_wool_metadata = false;
    while let Ok(command) = outbound.try_recv() {
        match command {
            OutboundCommand::BlockDeltas(deltas) => {
                saw_block_delta |= deltas.iter().any(|delta| {
                    delta.x == grass_pos.x
                        && delta.y == grass_pos.y
                        && delta.z == grass_pos.z
                        && delta.state_id == mc_world::BlockStateId(1)
                });
            }
            OutboundCommand::UpdateEntityData(entity) if entity.id == sheep_id => {
                saw_wool_metadata |= entity
                    .animal
                    .and_then(|animal| animal.sheep_wool)
                    .is_some_and(|wool| !wool.sheared);
            }
            _ => {}
        }
    }
    assert!(saw_block_delta);
    assert!(saw_wool_metadata);
}

#[tokio::test]
async fn random_ticks_ignore_ticketed_chunks_until_loaded() {
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
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:wheat"),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut first_chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    first_chunk.set_block(0, 0, 0, mc_world::BlockStateId(1));
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, first_chunk)
        .unwrap();
    let mut second_chunk = Chunk::empty(
        ChunkPos { x: 1, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    second_chunk.set_block(0, 0, 0, mc_world::BlockStateId(1));
    world
        .commit_chunk_snapshot(ChunkPos { x: 1, z: 0 }, second_chunk)
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 1,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let (_simulation, owner) = simulation_channel();
    let world_read = config.world.as_ref().unwrap().lock().await.read_view();

    let unloaded = owner
        .run_random_ticks_with_budget(
            &config,
            &registry,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: None,
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            1,
        )
        .await;
    assert_eq!(unloaded.sampled, 0);

    let _ = registry.mark_loaded(id, (0, 0));
    let _ = registry.mark_loaded(id, (1, 0));
    let limited = owner
        .run_random_ticks_with_budget(
            &config,
            &registry,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: None,
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            1,
        )
        .await;
    assert_eq!(limited.sampled, 1);

    let expanded = owner
        .run_random_ticks_with_budget(
            &config,
            &registry,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: None,
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            2,
        )
        .await;
    assert_eq!(expanded.sampled, 2);
}

#[tokio::test]
async fn random_tick_owner_keeps_protected_fuel_while_source_fire_ages() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:fire").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: vec![
                state(1, true, &[("age", "0")]),
                state(2, false, &[("age", "1")]),
            ],
        },
        simple_block(3, "minecraft:oak_log"),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let (policy, source) = (0..10_000)
        .find_map(|seed| {
            let policy = RandomTickPolicy {
                simulation_distance: DEFAULT_VIEW_DISTANCE,
                random_tick_speed: 1,
                chunk_budget: 1,
                fluid_tick_budget: 1,
                save_interval_ticks: 20,
                friendly_spawn_interval_ticks: 400,
                hostile_spawn_interval_ticks: 20,
                seed,
            };
            let source = sample_random_tick_positions(policy, 0, &[(0, 0)])
                .into_iter()
                .find(|sample| {
                    (64..80).contains(&sample.pos.y)
                        && (1..15).contains(&sample.pos.x)
                        && (1..15).contains(&sample.pos.z)
                })?
                .pos;
            random_tick_candidate_seed(seed, 0, source, 0)
                .is_multiple_of(3)
                .then_some((policy, source))
        })
        .expect("bounded seed search finds an interior fire sample");
    let fuel = mc_world::BlockPos {
        x: source.x + 1,
        ..source
    };

    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    world
        .commit_chunk_snapshot(
            chunk_pos,
            Chunk::empty(
                chunk_pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world.set_block_at(source, BlockStateId(1)).unwrap();
    world.set_block_at(fuel, BlockStateId(3)).unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );

    let sessions = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(
        &LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "tester".to_string(),
        },
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));

    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "claim",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(
            f64::from(fuel.x),
            f64::from(fuel.y),
            f64::from(fuel.z),
        )
        .unwrap(),
        mc_script::ScriptPosition::try_new(
            f64::from(fuel.x),
            f64::from(fuel.y),
            f64::from(fuel.z),
        )
        .unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![zone]);
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_read = shared_world.lock().await.read_view();
    let world_mutation = shared_world.lock().await.mutation_view();
    let (_simulation, owner) = simulation_channel();

    let report = owner
        .run_random_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: None,
                light: config.block_light.as_ref(),
            },
            Some(&protection),
            0,
            1,
        )
        .await;

    assert_eq!(report.eligible, 1);
    assert_eq!(report.applied, 1);
    let storage = shared_world.lock().await;
    assert_eq!(storage.get_cached_block(source), Some(BlockStateId(2)));
    assert_eq!(storage.get_cached_block(fuel), Some(BlockStateId(3)));
}

#[tokio::test]
async fn inert_random_tick_pass_does_not_wait_for_world_writer() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    world
        .commit_chunk_snapshot(
            chunk_pos,
            Chunk::empty(
                chunk_pos,
                mc_world::BlockStateId(1),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 3,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "InertRandomTick");
    let (_simulation, owner) = simulation_channel();
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_read = shared_world.lock().await.read_view();
    let world_writer = shared_world.lock().await;
    let mut pass = Box::pin(owner.run_random_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: None,
            cpu: None,
            light: config.block_light.as_ref(),
        },
        None,
        0,
        1,
    ));

    std::future::poll_fn(|cx| match Future::poll(pass.as_mut(), cx) {
        Poll::Ready(report) => {
            assert_eq!(report, RandomTickReport::default());
            Poll::Ready(())
        }
        Poll::Pending => panic!("inert random-tick prefilter waited for the world writer"),
    })
    .await;

    drop(world_writer);
}

fn mutating_random_tick_fixture(
    session_name: &str,
) -> (
    ServerConfig,
    SessionRegistry,
    RandomTickPolicy,
    RandomTickSample,
    mc_world::WorldReadView,
) {
    let reports = crop_test_reports();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 1,
        chunk_budget: 1,
        fluid_tick_budget: 1,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };
    let sample = sample_random_tick_positions(policy, 0, &[(0, 0)])[0];
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(
        chunk
            .set_block(
                sample.pos.x.rem_euclid(16) as u8,
                sample.pos.y,
                sample.pos.z.rem_euclid(16) as u8,
                mc_world::BlockStateId(11),
            )
            .is_some()
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let world_read = world.read_view();
    let config = simulation_tick_test_config(
        blocks,
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, session_name);
    (config, sessions, policy, sample, world_read)
}

#[tokio::test]
async fn mutating_random_tick_planning_does_not_wait_for_world_writer() {
    let (config, sessions, _policy, _sample, world_read) =
        mutating_random_tick_fixture("MutatingRandomTick");
    let (_simulation, owner) = simulation_channel();
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_mutation = shared_world.lock().await.mutation_view();
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let world_writer = shared_world.lock().await;
    RANDOM_TICK_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut pass = Box::pin(owner.run_random_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        None,
        0,
        1,
    ));

    std::future::poll_fn(|cx| match Future::poll(pass.as_mut(), cx) {
        Poll::Ready(report) => {
            assert_eq!(report.applied, 1);
            Poll::Ready(())
        }
        Poll::Pending => panic!("resident random-tick commit waited for the world writer"),
    })
    .await;
    RANDOM_TICK_PLANNING_COMPLETION_COUNT.with(|count| assert_eq!(count.get(), 1));

    drop(world_writer);
}

#[tokio::test]
async fn checkpoint_only_random_ticks_in_distinct_regions_do_not_wait_for_world_writer() {
    let reports = crop_test_reports();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 1,
        chunk_budget: 2,
        fluid_tick_budget: 1,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };
    let chunk_positions = [(0, 0), (8, 0)];
    let samples = sample_random_tick_positions(policy, 0, &chunk_positions);
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for &(x, z) in &chunk_positions {
        let chunk_pos = ChunkPos { x, z };
        let mut chunk = Chunk::empty(
            chunk_pos,
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        for sample in samples.iter().filter(|sample| {
            sample.chunk == (x, z)
                && (4..124).contains(&sample.pos.x.rem_euclid(128))
                && (4..124).contains(&sample.pos.z.rem_euclid(128))
        }) {
            chunk.set_block(
                sample.pos.x.rem_euclid(16) as u8,
                sample.pos.y,
                sample.pos.z.rem_euclid(16) as u8,
                BlockStateId(11),
            );
        }
        world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    }
    let config = simulation_tick_test_config(
        blocks,
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(122),
        name: "RegionalRandomTick".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let loaded = HashSet::from(chunk_positions);
    let (session, _) = sessions.register(
        &profile,
        (0, 0),
        8,
        loaded.clone(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session, chunk);
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, mut owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);
    let world_writer = shared_world.lock().await;
    let mut pass = Box::pin(owner.run_random_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        None,
        0,
        2,
    ));
    let entered_task = tokio::task::spawn_blocking(move || {
        [entered_rx.recv().unwrap(), entered_rx.recv().unwrap()]
    });
    let entered = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            entered = entered_task => entered.unwrap(),
            _ = &mut pass => panic!("random-tick fanout completed before worker probe"),
        }
    })
    .await
    .expect("both random-tick workers enter before either release");
    assert_ne!(entered[0], entered[1]);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(1), pass)
        .await
        .expect("distinct random-tick regions complete without the world writer");
    drop(world_writer);

    assert!(report.applied >= 2);
    for chunk in chunk_positions {
        assert!(
            samples
                .iter()
                .filter(|sample| sample.chunk == chunk)
                .any(|sample| {
                    world_read
                        .block_mutation_snapshot(sample.pos)
                        .is_some_and(|(state, _)| state == BlockStateId(12))
                })
        );
    }
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert!(reopened.decode_pending(&pending).unwrap().is_empty());
}

#[tokio::test]
async fn resident_random_tick_uses_periodic_checkpoint_instead_of_per_tick_wal() {
    let (config, sessions, _policy, sample, world_read) =
        mutating_random_tick_fixture("DurableRandomTick");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let (_simulation, owner) = simulation_channel();
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_mutation = shared_world.lock().await.mutation_view();
    let world_writer = shared_world.lock().await;
    let report = owner
        .run_random_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            1,
        )
        .await;

    assert_eq!(report.applied, 1);
    assert_eq!(sessions.world_chunk_journal_watermark(), None);
    assert_eq!(
        world_read.get_cached_block(sample.pos),
        Some(mc_world::BlockStateId(12))
    );
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert!(reopened.decode_pending(&pending).unwrap().is_empty());

    drop(world_writer);
}

#[tokio::test]
async fn boundary_random_tick_coordinator_fallback_uses_periodic_checkpoint() {
    let reports = crop_test_reports();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 1,
        chunk_budget: 1,
        fluid_tick_budget: 1,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };
    let sample = sample_random_tick_positions(policy, 0, &[(0, 0)])
        .into_iter()
        .find(|sample| sample.pos.x.rem_euclid(128) == 0 || sample.pos.z.rem_euclid(128) == 0)
        .expect("seed zero samples the region boundary belt");
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_block(
        sample.pos.x.rem_euclid(16) as u8,
        sample.pos.y,
        sample.pos.z.rem_euclid(16) as u8,
        BlockStateId(11),
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DurableBoundaryRandomTick");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();

    let report = owner
        .run_random_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            1,
        )
        .await;

    assert_eq!(report.applied, 1);
    assert_eq!(
        world_read.get_cached_block(sample.pos),
        Some(BlockStateId(12))
    );
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert!(reopened.decode_pending(&pending).unwrap().is_empty());
}

#[test]
fn random_tick_planning_preserves_repeated_sample_order() {
    let (config, _sessions, policy, sample, world_read) =
        mutating_random_tick_fixture("RepeatedRandomTick");
    let candidates = [
        RandomTickCandidate {
            sample,
            state: mc_world::BlockStateId(11),
        },
        RandomTickCandidate {
            sample,
            state: mc_world::BlockStateId(11),
        },
    ];
    let planning_chunks = random_tick_planning_chunks(&candidates);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);

    let mut plans = plan_random_tick_region_edits(&config, policy, 0, &snapshot, &candidates);

    assert_eq!(plans.len(), 1);
    let plan = plans.pop().unwrap();
    assert_eq!(plan.eligible, 2);
    assert_eq!(
        plan.edits
            .iter()
            .map(|edit| edit.new_state)
            .collect::<Vec<_>>(),
        vec![mc_world::BlockStateId(12), mc_world::BlockStateId(13)]
    );
}

#[test]
fn random_tick_region_planning_preserves_boundary_barrier_order() {
    let reports = crop_test_reports();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 1,
        chunk_budget: 1,
        fluid_tick_budget: 1,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };
    let positions = [
        mc_world::BlockPos { x: 4, y: 64, z: 4 },
        mc_world::BlockPos { x: 0, y: 64, z: 4 },
        mc_world::BlockPos { x: 5, y: 64, z: 4 },
    ];
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for position in positions {
        chunk.set_block(
            position.x.rem_euclid(16) as u8,
            position.y,
            position.z.rem_euclid(16) as u8,
            BlockStateId(11),
        );
    }
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let world_read = world.read_view();
    let config = simulation_tick_test_config(
        blocks,
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let candidates = positions.map(|pos| RandomTickCandidate {
        sample: RandomTickSample { chunk: (0, 0), pos },
        state: BlockStateId(11),
    });
    let snapshot = world_read.snapshot_chunks(&random_tick_planning_chunks(&candidates));

    let plans = plan_random_tick_region_edits(&config, policy, 0, &snapshot, &candidates);

    assert_eq!(
        plans
            .iter()
            .map(|plan| plan.edits.iter().map(|edit| edit.pos).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        positions.map(|position| vec![position])
    );
}

#[test]
fn random_tick_fanout_requires_sample_region_to_own_plan() {
    let plan = RandomTickPlan {
        edits: vec![BlockEdit {
            pos: mc_world::BlockPos {
                x: 8 * 16 + 4,
                y: 64,
                z: 4,
            },
            new_state: BlockStateId(1),
        }],
        ..RandomTickPlan::default()
    };

    assert!(random_tick_plan_fits_resident_region(&plan));
    assert!(!random_tick_plan_fits_region(RegionKey::new(0, 0), &plan));
}

#[tokio::test]
async fn random_tick_commit_rejects_changed_snapshot() {
    let (config, _sessions, policy, sample, world_read) =
        mutating_random_tick_fixture("StaleRandomTick");
    let candidates = [RandomTickCandidate {
        sample,
        state: mc_world::BlockStateId(11),
    }];
    let snapshot = world_read.snapshot_chunks(&random_tick_planning_chunks(&candidates));
    let plan = plan_random_tick_edits(&config, policy, 0, &snapshot, &candidates);
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let world_mutation = shared_world.lock().await.mutation_view();
    let mut world_writer = shared_world.lock().await;
    assert_eq!(
        world_writer
            .set_block_at(sample.pos, mc_world::BlockStateId(13))
            .unwrap(),
        Some(mc_world::BlockStateId(11))
    );

    drop(world_writer);
    let (edits, preconditions) = random_tick_resident_inputs(&plan, None).unwrap();
    assert_eq!(
        world_mutation.apply_block_edits_conditionally(&edits, &preconditions, &[], None, Some(1),),
        mc_world::ResidentBlockEditBatchResult::Stale
    );
    assert_eq!(
        shared_world.lock().await.get_cached_block(sample.pos),
        Some(mc_world::BlockStateId(13))
    );
}

#[tokio::test]
async fn random_leaf_decay_spawns_deterministic_natural_drop() {
    let reports = leaf_distance_test_reports();
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let mut selected = None;
    for seed in 0..10_000 {
        let policy = RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 1,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed,
        };
        let sample = sample_random_tick_positions(policy, 0, &[(0, 0)])[0];
        let rolls = leaf_decay_drop_rolls(seed, 0, sample.pos);
        if rolls.sapling < 50 && rolls.stick >= 20 && rolls.apple >= 5 {
            selected = Some((policy, sample));
            break;
        }
    }
    let (policy, sample) = selected.expect("bounded seed range contains a sapling roll");

    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(
        chunk
            .set_block(
                sample.pos.x as u8,
                sample.pos.y,
                sample.pos.z as u8,
                mc_world::BlockStateId(8),
            )
            .is_some()
    );
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();

    let mut config = simulation_tick_test_config(
        Arc::clone(&blocks),
        world,
        policy,
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    config.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:oak_sapling").unwrap(),
        protocol_id: 10,
    }]));
    config.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());

    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "LeafDecayDrop");
    let _ = sessions.mark_loaded(session, (0, 0));
    let (_simulation, owner) = simulation_channel();
    let world_read = config.world.as_ref().unwrap().lock().await.read_view();

    let report = owner
        .run_random_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: None,
                cpu: None,
                light: config.block_light.as_ref(),
            },
            None,
            0,
            1,
        )
        .await;

    assert_eq!(report.applied, 1);
    assert_eq!(
        config
            .world
            .as_ref()
            .unwrap()
            .lock()
            .await
            .get_cached_block(sample.pos),
        Some(mc_world::BlockStateId(0))
    );
    assert_eq!(
        sessions.pressure_snapshot().server_entities,
        1,
        "an applied natural decay must create its rolled sapling entity"
    );
}

#[test]
fn unsupported_dense_flow_search_visits_each_position_once() {
    struct CountingPlanningWorld<'a> {
        inner: &'a mc_world::WorldStorage,
        reads: Cell<usize>,
    }

    impl BlockPlanningRead for CountingPlanningWorld<'_> {
        fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<BlockStateId> {
            self.reads.set(self.reads.get() + 1);
            self.inner.get_cached_block(pos)
        }

        fn block_mutation_token(
            &self,
            pos: mc_world::BlockPos,
        ) -> Option<mc_world::BlockMutationToken> {
            self.inner.block_mutation_token(pos)
        }
    }

    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_pos,
            Chunk::empty(
                chunk_pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    for x in 1_i32..=15 {
        for z in 1_i32..=15 {
            let distance = x.abs_diff(8).saturating_add(z.abs_diff(8)).clamp(1, 7);
            storage
                .set_block_at(
                    mc_world::BlockPos { x, y: 64, z },
                    BlockStateId(2 + distance),
                )
                .unwrap();
        }
    }
    let world = CountingPlanningWorld {
        inner: &storage,
        reads: Cell::new(0),
    };
    let target = mc_world::BlockPos { x: 15, y: 64, z: 8 };
    let fluid = facts.fluid(9).unwrap();

    assert_eq!(
        supported_flow_state(registry.as_ref(), &facts, &world, target, fluid),
        Some(BlockStateId(0))
    );
    assert!(
        world.reads.get() < 1_000,
        "bounded source search made {} block reads",
        world.reads.get()
    );
}

#[tokio::test]
async fn scheduled_fluid_ticks_ignore_ticketed_chunks_until_loaded() {
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
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:water").unwrap(),
            properties: prop_schema(&[("level", &["0"])]),
            states: vec![state(1, true, &[("level", "0")])],
        },
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let pos = mc_world::BlockPos {
        x: 4,
        y: DEFAULT_SEA_LEVEL,
        z: 4,
    };
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(4, DEFAULT_SEA_LEVEL, 4, mc_world::BlockStateId(1));
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    world
        .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            pos,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            0,
            0,
        ))
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 0,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        block_facts,
    );

    let unloaded = run_scheduled_fluid_ticks(&config, &registry, 0).await;
    assert_eq!(unloaded.drained, 0);

    let _ = registry.mark_loaded(id, (0, 0));
    SCHEDULED_FLUID_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(0));
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = shared_world.lock().await;
    let mut fluid_tick = Box::pin(owner.run_scheduled_fluid_ticks_with_budget(
        &config,
        &registry,
        Some(&world_read),
        Some(&world_mutation),
        0,
        1,
    ));
    let loaded = std::future::poll_fn(|cx| match Future::poll(fluid_tick.as_mut(), cx) {
        Poll::Ready(report) => Poll::Ready(report),
        Poll::Pending => panic!("resident scheduled-fluid commit waited for the world writer"),
    })
    .await;
    drop(world_writer);
    assert_eq!(loaded.drained, 1);
    SCHEDULED_FLUID_PLANNING_WITHOUT_WRITER_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "scheduled-fluid neighbour planning must not hold the world writer"
        );
    });
}

#[tokio::test]
async fn resident_scheduled_fluid_tick_stays_off_the_synchronous_journal_path() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:water").unwrap(),
            properties: prop_schema(&[("level", &["0", "1"])]),
            states: vec![
                state(1, true, &[("level", "0")]),
                state(2, false, &[("level", "1")]),
            ],
        },
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    let source = mc_world::BlockPos {
        x: 4,
        y: DEFAULT_SEA_LEVEL,
        z: 4,
    };
    let target = mc_world::BlockPos {
        y: source.y - 1,
        ..source
    };
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(
        chunk
            .set_block(4, source.y, 4, mc_world::BlockStateId(1))
            .is_some()
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    world
        .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            source,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            0,
            0,
        ))
        .unwrap();
    let config = simulation_tick_test_config(
        Arc::clone(&blocks),
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 0,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        block_facts,
    );
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DurableFluidTick");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        blocks,
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal.clone());

    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = shared_world.lock().await;
    let report = owner
        .run_scheduled_fluid_ticks_with_budget(
            &config,
            &sessions,
            Some(&world_read),
            Some(&world_mutation),
            0,
            1,
        )
        .await;
    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    assert_eq!(
        journal.watermark(),
        None,
        "deterministic fluid simulation must not fsync the crash journal inside a game tick"
    );
    let snapshot = world_read.snapshot_chunks(&[chunk_pos]);
    let chunk = snapshot.chunk(chunk_pos).unwrap();
    assert_eq!(
        chunk.get_block(4, target.y, 4),
        Some(mc_world::BlockStateId(2))
    );
    assert!(
        chunk
            .scheduled_fluid_ticks()
            .iter()
            .all(|tick| tick.trigger_tick > 0)
    );
    assert!(!chunk.scheduled_fluid_ticks().is_empty());

    drop(world_writer);
}

#[tokio::test]
async fn region_boundary_scheduled_fluid_tick_uses_exact_coordinator_fallback() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:water").unwrap(),
            properties: prop_schema(&[("level", &["0", "1"])]),
            states: vec![
                state(1, true, &[("level", "0")]),
                state(2, false, &[("level", "1")]),
            ],
        },
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let source = mc_world::BlockPos {
        x: 0,
        y: DEFAULT_SEA_LEVEL,
        z: 4,
    };
    let target = mc_world::BlockPos {
        y: source.y - 1,
        ..source
    };
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(
        chunk
            .set_block(0, source.y, 4, mc_world::BlockStateId(1))
            .is_some()
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    world
        .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            source,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            0,
            0,
        ))
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 0,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &reports,
        )),
    );
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "BoundaryFluidTick");

    let report = run_scheduled_fluid_ticks(&config, &sessions, 0).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let mut storage = config.world.as_ref().unwrap().lock().await;
    assert_eq!(
        storage.get_cached_block(target),
        Some(mc_world::BlockStateId(2))
    );
    assert!(
        storage
            .scheduled_fluid_ticks(chunk_pos)
            .unwrap()
            .unwrap()
            .iter()
            .all(|tick| tick.trigger_tick > 0)
    );
}

#[tokio::test]
async fn stale_scheduled_fluid_plan_keeps_due_tick_without_edit() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:water").unwrap(),
            properties: prop_schema(&[("level", &["0", "1"])]),
            states: vec![
                state(1, true, &[("level", "0")]),
                state(2, false, &[("level", "1")]),
            ],
        },
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    let source = mc_world::BlockPos {
        x: 4,
        y: DEFAULT_SEA_LEVEL,
        z: 4,
    };
    let target = mc_world::BlockPos {
        y: source.y - 1,
        ..source
    };
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(
        chunk
            .set_block(4, source.y, 4, mc_world::BlockStateId(1))
            .is_some()
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    world
        .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            source,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            0,
            0,
        ))
        .unwrap();
    let config = simulation_tick_test_config(
        blocks,
        world,
        RandomTickPolicy {
            simulation_distance: DEFAULT_VIEW_DISTANCE,
            random_tick_speed: 0,
            chunk_budget: 1,
            fluid_tick_budget: 1,
            save_interval_ticks: 20,
            friendly_spawn_interval_ticks: 400,
            hostile_spawn_interval_ticks: 20,
            seed: 0,
        },
        block_facts,
    );
    let shared_world = Arc::clone(config.world.as_ref().unwrap());
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let loaded_snapshot = world_read.snapshot_chunks(&[chunk_pos]);
    let due = due_scheduled_fluid_ticks(&loaded_snapshot, &[chunk_pos], 0, 1);
    assert_eq!(due.len(), 1);
    let planning_chunks = scheduled_fluid_planning_chunks(&due);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);
    let plan = plan_scheduled_fluid_tick_edits(&config, 0, &snapshot, &due);
    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: target,
            new_state: mc_world::BlockStateId(2),
        }]
    );

    let mut storage = shared_world.lock().await;
    assert_eq!(
        storage
            .set_block_at(source, mc_world::BlockStateId(0))
            .unwrap(),
        Some(mc_world::BlockStateId(1))
    );
    drop(storage);
    let (edits, preconditions) =
        resident_block_edit_inputs(&plan.edits, &plan.preconditions, None).unwrap();
    assert_eq!(
        world_mutation.apply_fluid_tick_plan_conditionally(&mc_world::ResidentFluidTickPlan {
            consumed_ticks: &due,
            edits: &edits,
            preconditions: &preconditions,
            scheduled_ticks: &plan.scheduled_fluid_ticks,
            light_table: None,
            leaf_trigger_tick: Some(1),
        },),
        mc_world::ResidentBlockEditBatchResult::Stale
    );
    let mut storage = shared_world.lock().await;
    assert_eq!(
        storage.get_cached_block(target),
        Some(mc_world::BlockStateId(0))
    );
    let restored = storage.scheduled_fluid_ticks(chunk_pos).unwrap().unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].pos, source);
    assert_eq!(restored[0].fluid.as_str(), "minecraft:water");
}

#[test]
fn random_tick_sampling_is_deterministic_for_seed_tick_and_chunks() {
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 2,
        chunk_budget: 2,
        fluid_tick_budget: 256,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 99,
    };
    let chunks = vec![(0, 0), (1, 0), (2, 0)];

    let first = sample_random_tick_positions(policy, 7, &chunks);
    let second = sample_random_tick_positions(policy, 7, &chunks);

    assert_eq!(first, second);
    assert_eq!(first.len(), 4 * mc_world::SECTION_COUNT);
    assert!(first.iter().all(|sample| sample.pos.y >= mc_world::MIN_Y));
    assert!(first.iter().all(|sample| sample.pos.y < mc_world::MAX_Y));
}

#[test]
fn random_tick_section_filter_skips_inert_palettes_and_keeps_crops() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        simple_block(2, "minecraft:wheat"),
    ];
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let inert =
        mc_world::ChunkSection::filled(mc_world::BlockStateId(1), mc_world::BlockStateId(0));
    let mut crop = inert.clone();
    crop.set(1, 1, 1, mc_world::BlockStateId(2));

    assert!(!section_may_random_tick(&inert, &facts));
    assert!(section_may_random_tick(&crop, &facts));
}

#[test]
fn random_tick_speed_applies_to_every_world_section() {
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 2,
        chunk_budget: 1,
        fluid_tick_budget: 256,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 99,
    };

    let samples = sample_random_tick_positions(policy, 7, &[(0, 0)]);
    let mut per_section = vec![0usize; mc_world::SECTION_COUNT];
    for sample in samples {
        let section = ((sample.pos.y - mc_world::MIN_Y) as usize) / mc_world::SECTION_DIM;
        per_section[section] += 1;
    }

    assert_eq!(per_section, vec![2; mc_world::SECTION_COUNT]);
}

#[test]
fn random_tick_sampling_rotates_chunk_budget() {
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 1,
        chunk_budget: 2,
        fluid_tick_budget: 256,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };
    let chunks = vec![(0, 0), (1, 0), (2, 0)];

    let mut tick_zero = sample_random_tick_positions(policy, 0, &chunks)
        .into_iter()
        .map(|sample| sample.chunk)
        .collect::<Vec<_>>();
    tick_zero.dedup();
    let mut tick_one = sample_random_tick_positions(policy, 1, &chunks)
        .into_iter()
        .map(|sample| sample.chunk)
        .collect::<Vec<_>>();
    tick_one.dedup();

    assert_eq!(tick_zero, vec![(0, 0), (1, 0)]);
    assert_eq!(tick_one, vec![(1, 0), (2, 0)]);
}

#[test]
fn random_tick_speed_zero_disables_sampling() {
    let policy = RandomTickPolicy {
        simulation_distance: DEFAULT_VIEW_DISTANCE,
        random_tick_speed: 0,
        chunk_budget: 2,
        fluid_tick_budget: 256,
        save_interval_ticks: 20,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    };

    assert!(sample_random_tick_positions(policy, 0, &[(0, 0)]).is_empty());
}

#[test]
fn simulation_tick_policy_normalizes_deferred_work_budgets() {
    let policy = RandomTickPolicy {
        simulation_distance: 0,
        random_tick_speed: 0,
        chunk_budget: 0,
        fluid_tick_budget: 0,
        save_interval_ticks: 0,
        friendly_spawn_interval_ticks: 400,
        hostile_spawn_interval_ticks: 20,
        seed: 0,
    }
    .normalized();

    assert_eq!(policy.chunk_budget, 1);
    assert_eq!(policy.fluid_tick_budget, 1);
    assert_eq!(policy.save_interval_ticks, 1);
    assert_eq!(policy.simulation_distance, crate::MIN_VIEW_DISTANCE);
}

#[test]
fn light_inert_block_edits_do_not_request_full_relight() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:age_zero"),
            simple_block(2, "minecraft:age_one"),
            simple_block(3, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let table = mc_data::block_light::BlockLightTable::from_arrays(
        "test",
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 15],
        vec![true, true, true, false],
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let baked = mc_world::light::ChunkLight::filled(15, 0);
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(1),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_baked_light(&baked);
    chunk.dirty = false;
    world
        .commit_chunk_snapshot(ChunkPos { x: 0, z: 0 }, chunk)
        .unwrap();
    let pos = mc_world::BlockPos {
        x: 1,
        y: DEFAULT_SEA_LEVEL,
        z: 1,
    };
    let mut outcome = BlockEditBatchOutcome::default();

    apply_block_edit_to_storage(
        &mut world,
        Some(&table),
        &BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(2),
        },
        &mut outcome,
    );

    assert_eq!(outcome.edit_chunks, HashSet::from([(0, 0)]));
    assert!(outcome.light_edit_chunks.is_empty());
    assert_eq!(
        mc_world::light::ChunkLight::from_section_lights(
            &world
                .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .section_lights
        ),
        Some(baked)
    );

    apply_block_edit_to_storage(
        &mut world,
        Some(&table),
        &BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(3),
        },
        &mut outcome,
    );

    assert_eq!(outcome.light_edit_chunks, HashSet::from([(0, 0)]));
    let updates = collect_incremental_light_updates_for_applied_edits(&mut world, &table, &outcome);
    assert_eq!(updates.len(), 1);
    assert!(
        mc_world::light::ChunkLight::from_section_lights(
            &world
                .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .section_lights
        )
        .is_some()
    );
}

#[test]
fn large_single_chunk_light_batch_encodes_only_final_chunk_state() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let table = mc_data::block_light::BlockLightTable::from_arrays(
        "test",
        vec![0, 0],
        vec![0, 15],
        vec![true, false],
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let baked = mc_world::light::ChunkLight::filled(15, 0);
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_baked_light(&baked);
    chunk.dirty = false;
    world.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    let mut outcome = BlockEditBatchOutcome::default();

    for x in 1..=4 {
        for z in 1..=4 {
            apply_block_edit_to_storage(
                &mut world,
                Some(&table),
                &BlockEdit {
                    pos: mc_world::BlockPos {
                        x,
                        y: DEFAULT_SEA_LEVEL,
                        z,
                    },
                    new_state: mc_world::BlockStateId(1),
                },
                &mut outcome,
            );
        }
    }
    assert_eq!(outcome.applied.len(), 16);
    OUTBOUND_LIGHT_UPDATE_ENCODING_COUNT.with(|count| count.set(0));
    OUTBOUND_LIGHT_NEIGHBOURHOOD_CAPTURE_COUNT.with(|count| count.set(0));

    let updates = collect_incremental_light_updates_for_applied_edits(&mut world, &table, &outcome);
    let encoding_count = OUTBOUND_LIGHT_UPDATE_ENCODING_COUNT.with(std::cell::Cell::get);
    let neighbourhood_capture_count =
        OUTBOUND_LIGHT_NEIGHBOURHOOD_CAPTURE_COUNT.with(std::cell::Cell::get);

    assert_eq!(updates.len(), 1);
    assert_eq!(encoding_count, updates.len());
    assert_eq!(neighbourhood_capture_count, 1);
    assert_eq!(updates[0].pos, chunk_pos);
    let final_chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
    let mut refs = [[None; 3]; 3];
    refs[1][1] = Some(final_chunk.as_ref());
    let expected = compute_chunk_light_in(&mut LightWorkspace::new(), refs, &table);
    assert_eq!(updates[0].light, expected);
    assert_eq!(
        mc_world::light::ChunkLight::from_section_lights(
            &world
                .cached_chunk_snapshot(chunk_pos)
                .unwrap()
                .section_lights
        ),
        Some(updates[0].light.clone())
    );
}

#[test]
fn server_origin_relight_compute_does_not_hold_world_writer() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let table = Arc::new(mc_data::block_light::BlockLightTable::from_arrays(
        "test",
        vec![0, 0],
        vec![0, 15],
        vec![true, false],
    ));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let mut chunk = Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_baked_light(&mc_world::light::ChunkLight::filled(15, 0));
    chunk.dirty = false;
    storage.commit_chunk_snapshot(chunk_pos, chunk).unwrap();
    let mut outcome = BlockEditBatchOutcome::default();
    apply_block_edit_to_storage(
        &mut storage,
        Some(&table),
        &BlockEdit {
            pos: mc_world::BlockPos { x: 1, y: 64, z: 1 },
            new_state: mc_world::BlockStateId(1),
        },
        &mut outcome,
    );

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    sessions.install_server_relight_compute_probe(reached_tx, resume_rx);

    let compute_world = Arc::clone(&world);
    let compute_sessions = Arc::clone(&sessions);
    let compute_table = Arc::clone(&table);
    let compute_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(collect_server_origin_light_updates(
                &compute_world,
                &compute_sessions,
                &compute_table,
                &outcome,
            ))
    });

    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("server relight reaches the compute boundary");
    let writer_available = if let Ok(mut writer) = world.try_lock() {
        writer
            .set_block_at(
                mc_world::BlockPos { x: 2, y: 64, z: 2 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
        true
    } else {
        false
    };
    resume_tx.send(()).expect("release server relight compute");
    let updates = compute_thread.join().expect("server relight joins");

    assert!(
        writer_available,
        "server-origin relight compute must run after releasing the world writer"
    );
    assert_eq!(updates.len(), 1);
    let storage = world.try_lock().expect("relight released world writer");
    let current = storage.cached_chunk_snapshot(chunk_pos).unwrap();
    let mut refs = [[None; 3]; 3];
    refs[1][1] = Some(current.as_ref());
    let expected = compute_chunk_light_in(&mut LightWorkspace::new(), refs, &table);
    assert_eq!(updates[0].light, expected);
    assert_eq!(
        mc_world::light::ChunkLight::from_section_lights(&current.section_lights),
        Some(expected)
    );
}

#[test]
fn outbound_block_delta_batching_preserves_next_different_command() {
    let (tx, mut rx) = mpsc::channel(8);
    tx.try_send(OutboundCommand::BlockDeltas(vec![BlockDelta {
        x: 1,
        y: 2,
        z: 3,
        state_id: BlockStateId(4),
    }]))
    .unwrap();
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 9 })
        .unwrap();
    let mut pending = std::collections::VecDeque::new();

    let batched = collect_block_delta_batch(
        vec![BlockDelta {
            x: 0,
            y: 0,
            z: 0,
            state_id: BlockStateId(1),
        }],
        &mut rx,
        &mut pending,
    );

    assert_eq!(batched.len(), 2);
    assert!(matches!(
        pending.pop_front(),
        Some(OutboundCommand::AnimatePlayer { entity_id: 9 })
    ));
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
            flags: MovePlayerFlags::new(true, false),
            ..PlayerPose::new(48.5, DEFAULT_SPAWN_Y, 0.5)
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
        let collected: std::collections::HashSet<(i32, i32)> = spiral_chunks(0, 0, vd).collect();
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
fn spiral_chunks_caps_untrusted_view_distance() {
    let chunks: Vec<_> = spiral_chunks(0, 0, crate::MAX_VIEW_DISTANCE + 1).collect();
    let diameter = (2 * crate::MAX_VIEW_DISTANCE + 1) as usize;

    assert_eq!(chunks.len(), diameter * diameter);
    assert_eq!(
        chunks.iter().copied().collect::<HashSet<_>>().len(),
        chunks.len()
    );
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
fn spawn_dimension_falls_back_to_alphabetical_first_for_stubs() {
    let data = mc_data::testing::stub();
    let (id, name, all) = spawn_dimension(&data).unwrap();
    assert_eq!(id, 0);
    assert_eq!(name.as_str(), "minecraft:alpha");
    assert_eq!(all.len(), 2);
}

#[test]
fn spawn_dimension_prefers_overworld_when_present() {
    let registry = mc_data::Registry {
        id: mc_data::Identifier::parse("minecraft:dimension_type").unwrap(),
        entries: vec![
            mc_data::Identifier::parse("minecraft:the_nether").unwrap(),
            mc_data::Identifier::parse("minecraft:overworld").unwrap(),
            mc_data::Identifier::parse("minecraft:the_end").unwrap(),
        ],
    };
    let data = mc_data::VanillaData::from_registries("", vec![registry]);

    let (id, name, all) = spawn_dimension(&data).unwrap();

    assert_eq!(id, 1);
    assert_eq!(name.as_str(), "minecraft:overworld");
    assert_eq!(all.len(), 3);
}

#[test]
fn spawn_dimension_rejects_empty_registry() {
    let registry = mc_data::Registry {
        id: mc_data::Identifier::parse("minecraft:dimension_type").unwrap(),
        entries: Vec::new(),
    };
    let data = mc_data::VanillaData::from_registries("", vec![registry]);

    assert!(spawn_dimension(&data).is_none());
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
