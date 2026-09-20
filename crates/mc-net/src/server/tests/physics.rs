use super::super::*;
use super::support::*;
use std::future::Future;

#[tokio::test]
async fn entity_physics_keeps_a_common_herd_batch_inline() {
    let resources = ChunkPipelineResources::with_limits(1, 16);
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks: HashMap::new(),
        materials: Arc::new(BlockMaterialIds::new(0, None, None)),
        blocks: None,
        powder_snow_states: Box::default(),
        collision_direct_lookup_compatible: false,
    });
    let inputs = (0..198)
        .map(|id| EntityPhysicsInput {
            query: play::EntityPhysicsQuery {
                id: mc_entity::EntityId(id),
                position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
                velocity: mc_entity::Vec3::ZERO,
                aabb: mc_physics::Aabb::COW,
                on_ground: false,
                fall_distance: 0.0,
                goal_fence: mc_entity::EntityGoalFence::Idle,
                kind: play::EntityPhysicsKind::Default,
            },
            snapshot: Arc::clone(&snapshot),
            complete_samples: false,
        })
        .collect();

    let steps = step_entity_physics_inputs(resources.clone(), inputs).await;

    assert_eq!(steps.len(), 198);
    assert_eq!(resources.metrics().snapshot().max_cpu_active, 0);
}

#[tokio::test]
async fn entity_physics_does_not_wait_for_busy_chunk_workers() {
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let _busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Default,
    };
    let inputs = vec![EntityPhysicsInput {
        query,
        snapshot: Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::new(),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
            powder_snow_states: Box::default(),
            collision_direct_lookup_compatible: false,
        }),
        complete_samples: false,
    }];

    let steps = tokio::time::timeout(
        Duration::from_secs(1),
        step_entity_physics_inputs(resources, inputs),
    )
    .await
    .expect("entity physics waited for a chunk worker");

    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].id, query.id);
    assert_eq!(steps[0].position, query.position);
}

#[test]
fn item_entity_released_underwater_settles_at_surface_without_oscillation() {
    struct WaterColumnSampler {
        floor_y: i32,
        water_top_y: i32,
    }
    impl mc_physics::BlockSampler for WaterColumnSampler {
        fn material_at(&self, _x: i32, y: i32, _z: i32) -> mc_physics::BlockMaterial {
            if y < self.floor_y {
                mc_physics::BlockMaterial::Solid
            } else if y < self.water_top_y {
                mc_physics::BlockMaterial::Water
            } else {
                mc_physics::BlockMaterial::Air
            }
        }
    }

    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(1),
        position: mc_entity::Vec3::new(0.5, 61.0, 0.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb {
            half_width: 0.125,
            height: 0.25,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Item,
    };
    let config = physics_config_for_query(query);
    let sampler = WaterColumnSampler {
        floor_y: 60,
        water_top_y: 64,
    };
    let mut body = mc_physics::EntityBody {
        position: mc_physics::Vec3::new(0.5, 61.0, 0.5),
        velocity: mc_physics::Vec3::new(0.0, 0.0, 0.0),
        aabb: query.aabb,
        on_ground: false,
    };

    let mut max_feet = f64::MIN;
    let (mut band_min, mut band_max) = (f64::MAX, f64::MIN);
    for tick in 0..400 {
        let result = mc_physics::step_entity(body, &sampler, config);
        body = result.body;
        max_feet = max_feet.max(body.position.y);
        if tick >= 300 {
            band_min = band_min.min(body.position.y);
            band_max = band_max.max(body.position.y);
        }
    }

    assert!(
        body.position.y > 63.2,
        "item must rise from 61.0 to the top water cell, ended at {}",
        body.position.y
    );
    assert!(
        max_feet < 64.15,
        "item must never be tossed above the surface, peaked at {max_feet}"
    );
    assert!(
        band_max - band_min < 0.2,
        "surface rest must be a gentle bob without a limit cycle, band {band_min}..{band_max}"
    );
}

#[tokio::test]
async fn large_entity_physics_waits_for_cpu_push_without_blocking_owner_work() {
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks: HashMap::new(),
        materials: Arc::new(BlockMaterialIds::new(0, None, None)),
        blocks: None,
        powder_snow_states: Box::default(),
        collision_direct_lookup_compatible: false,
    });
    let inputs = (0..257)
        .map(|id| EntityPhysicsInput {
            query: play::EntityPhysicsQuery {
                id: mc_entity::EntityId(id),
                position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
                velocity: mc_entity::Vec3::ZERO,
                aabb: mc_physics::Aabb::COW,
                on_ground: false,
                fall_distance: 0.0,
                goal_fence: mc_entity::EntityGoalFence::Idle,
                kind: play::EntityPhysicsKind::Default,
            },
            snapshot: Arc::clone(&snapshot),
            complete_samples: false,
        })
        .collect();
    let mut physics = std::pin::pin!(step_entity_physics_inputs(resources.clone(), inputs,));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(physics.as_mut(), cx).is_pending(),
            "large physics batch ran inline while shared CPU was busy"
        );
        std::task::Poll::Ready(())
    })
    .await;

    let (owner_work_tx, owner_work_rx) = tokio::sync::oneshot::channel();
    owner_work_tx.send(()).unwrap();
    tokio::select! {
        biased;
        steps = &mut physics => panic!(
            "large physics completed before CPU release: {} steps",
            steps.len()
        ),
        result = owner_work_rx => result.unwrap(),
    }

    drop(busy_worker);
    let steps = physics.await;
    assert_eq!(steps.len(), 257);
    assert_eq!(resources.metrics().snapshot().max_cpu_active, 1);
}

#[tokio::test]
async fn background_entity_physics_leaves_simulation_owner_responsive() {
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks: HashMap::new(),
        materials: Arc::new(BlockMaterialIds::new(0, None, None)),
        blocks: None,
        powder_snow_states: Box::default(),
        collision_direct_lookup_compatible: false,
    });
    let queries = (0..257)
        .map(|id| play::EntityPhysicsQuery {
            id: mc_entity::EntityId(id),
            position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: false,
            fall_distance: 0.0,
            goal_fence: mc_entity::EntityGoalFence::Idle,
            kind: play::EntityPhysicsKind::Default,
        })
        .collect::<Vec<_>>();
    let inputs = queries
        .iter()
        .copied()
        .map(|query| EntityPhysicsInput {
            query,
            snapshot: Arc::clone(&snapshot),
            complete_samples: false,
        })
        .collect();
    let mut physics = spawn_entity_physics_job(9, queries, None, resources, inputs);
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(std::pin::Pin::new(&mut physics), cx).is_pending(),
            "background physics completed while CPU admission was occupied"
        );
        std::task::Poll::Ready(())
    })
    .await;

    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let barrier = tokio::spawn(async move { simulation.save_barrier(false).await });
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
    assert!(barrier.await.unwrap().is_ok());

    drop(busy_worker);
    let completed = physics.await.unwrap();
    assert_eq!(completed.tick, 9);
    assert_eq!(completed.expected.len(), 257);
    assert_eq!(completed.steps.len(), 257);
}

#[tokio::test]
async fn background_scheduled_blocks_leave_simulation_owner_responsive() {
    let reports = [
        report("minecraft:air", &[], &[(0, true, &[])]),
        report(
            "minecraft:stone_button",
            &[
                ("face", &["wall"]),
                ("facing", &["east"]),
                ("powered", &["false", "true"]),
            ],
            &[
                (
                    1,
                    true,
                    &[("face", "wall"), ("facing", "east"), ("powered", "false")],
                ),
                (
                    2,
                    false,
                    &[("face", "wall"), ("facing", "east"), ("powered", "true")],
                ),
            ],
        ),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            chunk_position,
            mc_world::Chunk::empty(
                chunk_position,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let mut positions = (1..=14)
        .flat_map(|z| (1..=14).map(move |x| mc_world::BlockPos { x, y: 64, z }))
        .collect::<Vec<_>>();
    positions
        .extend((1..=6).flat_map(|z| (1..=10).map(move |x| mc_world::BlockPos { x, y: 65, z })));
    assert_eq!(positions.len(), 256);
    for position in positions {
        storage
            .set_block_at(position, mc_world::BlockStateId(2))
            .unwrap();
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                9,
                0,
            ))
            .unwrap();
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(Mutex::new(storage));
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("dimensions/minecraft/overworld/region")).unwrap();
    let mut config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::new(ItemRegistry::default()),
        canonical_entity_types(),
    );
    config.world = Some(Arc::clone(&world));
    let config = Arc::new(config);
    let sessions = Arc::new(play::SessionRegistry::new());
    sessions.register_loaded_for_server_test("ScheduledWorker", (0, 0));

    let resources = ChunkPipelineResources::with_limits(1, 1);
    let admission = sessions
        .try_begin_scheduled_block_ticks()
        .expect("first scheduled block admission succeeds");
    let duplicate = play::run_scheduled_block_ticks_background(
        &config,
        &sessions,
        play::SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        None,
        9,
        256,
    )
    .await;
    assert_eq!(duplicate.drained, 0);
    assert_eq!(duplicate.applied, 0);
    drop(admission);

    let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
    let block_tick = spawn_scheduled_block_tick_job(
        9,
        256,
        Arc::clone(&config),
        Arc::clone(&sessions),
        Some(world_read.clone()),
        Some(world_mutation.clone()),
        None,
        resources.clone(),
    );

    let (simulation, mut owner) = play::simulation_channel();
    let mut barrier = tokio::spawn(async move { simulation.save_barrier(false).await });
    let mut waiting = Box::pin(await_scheduled_block_tick_job_with_commands(
        block_tick,
        &mut owner,
        &config,
        &sessions,
        Some(&world_read),
        Some(&world_mutation),
        &resources,
    ));
    let barrier_result = tokio::select! {
        biased;
        result = &mut waiting => panic!(
            "scheduled block batch completed before CPU release: {:?}",
            result.0.map(|completed| completed.report)
        ),
        result = &mut barrier => result.unwrap(),
    };
    assert!(barrier_result.is_ok());

    drop(busy_worker);
    let (completed, commands) = waiting.await;
    let completed = completed.unwrap();
    eprintln!(
        "scheduled block background batch: drained={} applied={} elapsed_us={}",
        completed.report.drained, completed.report.applied, completed.elapsed_us
    );
    assert_eq!(commands.report.processed, 1);
    assert_eq!(commands.report.remaining_depth, 0);
    assert_eq!(completed.tick, 9);
    assert_eq!(completed.report.drained, 256);
    assert_eq!(completed.report.applied, 256);
    assert_eq!(
        world.lock().await.get_cached_block(position),
        Some(mc_world::BlockStateId(1))
    );
}

#[test]
fn entity_physics_snapshot_becomes_stale_after_world_edit() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let mut world = WorldStorage::in_memory(blocks);
    let chunk = mc_world::ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            mc_world::Chunk::empty(
                chunk,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let world_read = world.read_view();
    let captured = world_read.snapshot_chunks(&[chunk]);
    let snapshot = EntityPhysicsSnapshot {
        chunks: HashMap::from([(chunk, captured.chunk(chunk))]),
        materials: Arc::new(BlockMaterialIds::new(0, None, None)),
        blocks: None,
        powder_snow_states: Box::default(),
        collision_direct_lookup_compatible: false,
    };

    assert!(entity_physics_snapshot_is_current(&world_read, &snapshot));
    world
        .set_block_at(
            mc_world::BlockPos { x: 1, y: 64, z: 1 },
            mc_world::BlockStateId(1),
        )
        .unwrap();
    assert!(!entity_physics_snapshot_is_current(&world_read, &snapshot));
}

#[test]
fn arrow_physics_samples_chunk_boundary_and_wires_block_hit_fact() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let stone = state_id(&reports, "minecraft:stone", &[]);
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::in_memory(blocks);
    for chunk_x in [0, 1] {
        let position = mc_world::ChunkPos { x: chunk_x, z: 0 };
        world
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(position, mc_world::BlockStateId(air), biome.clone()),
            )
            .unwrap();
    }
    world
        .set_block_at(
            mc_world::BlockPos { x: 16, y: 64, z: 8 },
            mc_world::BlockStateId(stone),
        )
        .unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(71),
        position: mc_entity::Vec3::new(15.5, 64.25, 8.5),
        velocity: mc_entity::Vec3::new(1.0, 0.0, 0.0),
        aabb: mc_physics::Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::ArrowProjectile {
            revision: None,
            embedded_block: None,
        },
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    assert!(input.complete_samples);
    assert!(
        input
            .snapshot
            .chunks
            .contains_key(&mc_world::ChunkPos { x: 0, z: 0 })
    );
    assert!(
        input
            .snapshot
            .chunks
            .contains_key(&mc_world::ChunkPos { x: 1, z: 0 })
    );
    let snapshot = Arc::clone(&input.snapshot);
    let step = step_sampled_entity(input);

    let facts = arrow_physics_facts_from_steps(1, &[query], &snapshot, &[step]);

    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].arrow_id, query.id);
    let block_hit = facts[0].block_hit.expect("arrow endpoint hits stone");
    assert_eq!(block_hit.block_state, mc_world::BlockStateId(stone));
    assert_eq!(
        block_hit.block_position,
        mc_entity::projectile_26_1_2::BlockPosition::new(16, 64, 8)
    );
    assert_eq!(block_hit.location, step.position);
}

#[test]
fn arrow_endpoint_sampler_uses_exact_block_collision_shape() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let slab = state_id(
        &reports,
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        position,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(slab));
    let mut world = WorldStorage::in_memory(blocks);
    world.insert_generated_chunk(position, chunk).unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(72),
        position: mc_entity::Vec3::new(8.5, 64.5, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::ArrowProjectile {
            revision: None,
            embedded_block: Some(mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8)),
        },
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    let snapshot = Arc::clone(&input.snapshot);
    let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(&snapshot));

    assert_eq!(
        collision_block_touching_arrow_endpoint(
            &sampler,
            mc_entity::Vec3::new(8.5, 64.5, 8.5),
            query.aabb,
        ),
        Some((
            mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8),
            slab
        ))
    );
    assert_eq!(
        collision_block_touching_arrow_endpoint(
            &sampler,
            mc_entity::Vec3::new(8.5, 64.500_000_002, 8.5),
            query.aabb,
        ),
        None
    );
    let embedded = play::EntityPhysicsStep {
        id: query.id,
        position: mc_entity::Vec3::new(8.5, 64.500_000_002, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        on_ground: true,
        horizontal_collision: false,
    };
    let fact = arrow_physics_facts_from_steps(4, &[query], &snapshot, &[embedded])[0];
    assert!(fact.embedded_in_block);
    assert_eq!(fact.current_block_state, mc_world::BlockStateId(slab));
    assert!(!fact.should_fall);
    assert!(fact.block_hit.is_none());
}

#[test]
fn arrow_environment_sampler_propagates_water_and_support_loss() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let water = state_id(&reports, "minecraft:water", &[]);
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        position,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(water));
    let mut world = WorldStorage::in_memory(blocks);
    world.insert_generated_chunk(position, chunk).unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(74),
        position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::ArrowProjectile {
            revision: None,
            embedded_block: Some(mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8)),
        },
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    let snapshot = Arc::clone(&input.snapshot);
    let step = step_sampled_entity(input);

    let fact = arrow_physics_facts_from_steps(9, &[query], &snapshot, &[step])[0];

    assert!(fact.in_water);
    assert!(fact.in_water_or_rain);
    assert!(!fact.embedded_in_block);
    assert_eq!(fact.current_block_state, mc_world::BlockStateId(water));
    assert!(fact.should_fall);
    for component in [
        fact.fall_velocity_scale.x,
        fact.fall_velocity_scale.y,
        fact.fall_velocity_scale.z,
    ] {
        assert!((0.0..0.2).contains(&component));
    }
}

#[test]
fn hurting_projectile_physics_applies_inertia_before_collision_move() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut world = WorldStorage::in_memory(blocks);
    world
        .insert_generated_chunk(
            position,
            mc_world::Chunk::empty(
                position,
                mc_world::BlockStateId(air),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(75),
        position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
        velocity: mc_entity::Vec3::new(0.1, 0.0, 0.0),
        aabb: mc_physics::Aabb {
            half_width: 0.15625,
            height: 0.3125,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::HurtingProjectile {
            revision: Some(0),
            acceleration_power_bits: 0.1_f64.to_bits(),
        },
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    assert!(input.complete_samples);
    let snapshot = Arc::clone(&input.snapshot);
    let step = step_sampled_entity(input);
    let expected = mc_entity::projectile_26_1_2::next_hurting_projectile_velocity(
        mc_entity::projectile_26_1_2::Vec3::new(0.1, 0.0, 0.0),
        0.1,
        false,
    )
    .unwrap();

    assert_eq!(step.velocity.x.to_bits(), expected.x.to_bits());
    assert_eq!(step.velocity.y.to_bits(), expected.y.to_bits());
    assert_eq!(step.velocity.z.to_bits(), expected.z.to_bits());
    assert_eq!(
        step.position.x.to_bits(),
        (query.position.x + expected.x).to_bits()
    );
    let fact = hurting_projectile_physics_facts_from_steps(&[query], &snapshot, &[step])[0];
    assert!(!fact.in_water);
    assert!(fact.block_hit.is_none());
}

#[test]
fn stale_arrow_snapshot_fact_is_rejected_after_world_mutation() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let mut world = WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk = mc_world::ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            mc_world::Chunk::empty(
                chunk,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world
        .set_block_at(
            mc_world::BlockPos { x: 8, y: 64, z: 8 },
            mc_world::BlockStateId(1),
        )
        .unwrap();
    let world_read = world.read_view();
    let captured = world_read.snapshot_chunks(&[chunk]);
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks: HashMap::from([(chunk, captured.chunk(chunk))]),
        materials: Arc::new(BlockMaterialIds::new(0, None, None)),
        blocks: None,
        powder_snow_states: Box::default(),
        collision_direct_lookup_compatible: false,
    });
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(73),
        position: mc_entity::Vec3::new(8.5, 64.25, 7.5),
        velocity: mc_entity::Vec3::new(0.0, 0.0, 1.0),
        aabb: mc_physics::Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::ArrowProjectile {
            revision: None,
            embedded_block: None,
        },
    };
    let step = play::EntityPhysicsStep {
        id: query.id,
        position: mc_entity::Vec3::new(8.5, 64.25, 7.75),
        velocity: mc_entity::Vec3::ZERO,
        on_ground: true,
        horizontal_collision: false,
    };
    assert!(
        arrow_physics_facts_from_steps(1, &[query], &snapshot, &[step])[0]
            .block_hit
            .is_some()
    );

    world
        .set_block_at(
            mc_world::BlockPos { x: 8, y: 64, z: 8 },
            mc_world::BlockStateId(0),
        )
        .unwrap();

    assert!(!entity_physics_snapshot_is_current(&world_read, &snapshot));
}

#[tokio::test]
async fn entity_physics_sampling_does_not_wait_for_world_writer() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(
        tmp.path()
            .join("dimensions")
            .join("minecraft")
            .join("overworld")
            .join("region"),
    )
    .unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    let world = Arc::clone(config.world.as_ref().unwrap());
    let world_read = {
        let mut storage = world.lock().await;
        let cpos = mc_world::ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(
                    cpos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.read_view()
    };
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Default,
    };
    let queries = [query];
    let resources = ChunkPipelineResources::with_limits(1, 16);
    let world_writer = world.lock().await;
    let inputs = prepare_entity_physics_inputs(&config, Some(&world_read), &queries);
    let mut physics = Box::pin(step_entity_physics_inputs(resources, inputs));

    std::future::poll_fn(|cx| match std::future::Future::poll(physics.as_mut(), cx) {
        std::task::Poll::Ready(steps) => {
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].id, query.id);
            std::task::Poll::Ready(())
        }
        std::task::Poll::Pending => {
            panic!("entity physics sampling waited for the world writer")
        }
    })
    .await;

    drop(world_writer);
}

#[test]
fn runtime_tick_attribution_includes_sheep_grazing() {
    let sample = RuntimeTickSample {
        tick_us: 1_000,
        world_time_us: 0,
        sheep_grazing_us: 123,
        animal_breeding_us: 0,
        hostile_attacks_us: 0,
        entity_goals_us: 0,
        entity_physics_us: 0,
        entity_dispatch_us: 0,
        campfire_tick_us: 0,
        inhabited_time_us: 0,
        entity_save_us: 0,
        random_tick_us: 0,
        block_tick_us: 0,
        fluid_tick_us: 0,
    };

    assert_eq!(runtime_attributed_tick_us(&sample, 7, 11), 141);
}

#[test]
fn runtime_work_input_uses_the_exact_pushed_percentile_window() {
    let mut window = RuntimeTickMetricsWindow::with_capacity(4);
    for _ in 0..3 {
        window.record(RuntimeTickSample {
            tick_us: 10_000,
            world_time_us: 10,
            sheep_grazing_us: 10,
            animal_breeding_us: 10,
            hostile_attacks_us: 10,
            entity_goals_us: 1_000,
            entity_physics_us: 1_000,
            entity_dispatch_us: 1_000,
            campfire_tick_us: 10,
            inhabited_time_us: 10,
            entity_save_us: 10,
            random_tick_us: 500,
            block_tick_us: 100,
            fluid_tick_us: 100,
        });
    }
    let spike = RuntimeTickSample {
        tick_us: 90_000,
        world_time_us: 10,
        sheep_grazing_us: 10,
        animal_breeding_us: 10,
        hostile_attacks_us: 10,
        entity_goals_us: 40_000,
        entity_physics_us: 20_000,
        entity_dispatch_us: 10_000,
        campfire_tick_us: 10,
        inhabited_time_us: 10,
        entity_save_us: 10,
        random_tick_us: 500,
        block_tick_us: 100,
        fluid_tick_us: 100,
    };
    window.record(spike);
    let percentiles = window.snapshot().expect("computed window");

    let input = runtime_work_input(&percentiles, false);

    assert_eq!(input.tick_p95_us, spike.tick_us);
    assert_eq!(input.entity_goals_p95_us, spike.entity_goals_us);
    assert_eq!(input.entity_physics_p95_us, spike.entity_physics_us);
    assert_eq!(input.entity_dispatch_p95_us, spike.entity_dispatch_us);
    assert_eq!(input.random_tick_p95_us, 500);
}

#[tokio::test(start_paused = true)]
async fn runtime_control_tick_applies_sustained_memory_pressure_after_one_minute() {
    let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
        crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        },
    );
    let control = crate::RuntimeControlHandle::new_with_memory_pressure(
        crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                memory_pressure_percent: 50,
                ..crate::AutoscalePolicy::default()
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 16,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        },
        memory_pressure.clone(),
    );
    let input = runtime_control_tick_input(49_001);
    assert_eq!(input.tick_ms, 50);
    assert_eq!(input.memory_used_mb, 0);
    assert_eq!(input.memory_limit_mb, 0);

    let resources = ChunkPipelineResources::with_limits(1, 8);
    let sessions = play::SessionRegistry::new();
    let initial_owner_lanes = sessions.entity_owner_lane_count();
    let shutdown = ShutdownHandle::default();
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 49_001).unwrap();
    assert_eq!(decision.pressure, Some(crate::AutoscalePressure::Memory));
    assert_eq!(decision.action, crate::AutoscaleAction::Hold);
    assert_eq!(resources.prepare_limit(), 7);
    tokio::time::advance(Duration::from_secs(60)).await;
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 49_001).unwrap();
    assert_eq!(decision.action, crate::AutoscaleAction::Hold);
    assert_eq!(resources.prepare_limit(), 7);
    tokio::time::advance(Duration::from_secs(1)).await;
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 49_001).unwrap();
    assert_eq!(decision.action, crate::AutoscaleAction::ScaleDown);
    assert_eq!(decision.limits.chunk_send_rate, 15);
    assert_eq!(resources.prepare_limit(), 6);
    assert_eq!(sessions.entity_owner_lane_count(), initial_owner_lanes);
    assert_eq!(
        control.snapshot().last_decision.pressure,
        Some(crate::AutoscalePressure::Memory)
    );
    assert_eq!(
        control.memory_pressure_observation().sample,
        crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        }
    );

    memory_pressure.set_sample(crate::memory_pressure::MemoryPressureSnapshot {
        used_mb: 100,
        limit_mb: 1_000,
    });
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 10_000).unwrap();
    assert_eq!(decision.action, crate::AutoscaleAction::Hold);
    tokio::time::advance(Duration::from_secs(60)).await;
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 10_000).unwrap();
    assert_eq!(decision.action, crate::AutoscaleAction::Hold);
    assert_eq!(decision.limits.chunk_send_rate, 15);
    assert_eq!(resources.prepare_limit(), 6);
    tokio::time::advance(Duration::from_secs(1)).await;
    let decision =
        observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 10_000).unwrap();
    assert_eq!(decision.action, crate::AutoscaleAction::ScaleUp);
    assert_eq!(decision.limits.chunk_send_rate, 16);
    assert_eq!(resources.prepare_limit(), 7);
}

#[test]
fn broken_pipe_is_graceful_disconnect() {
    let err = ConnectionError::Io(std::io::Error::from(ErrorKind::BrokenPipe));
    assert!(is_client_disconnect(&err));
}

#[test]
fn codec_error_is_not_graceful_disconnect() {
    let err = ConnectionError::UnexpectedPacketId {
        state: mc_protocol::State::Login,
        expected: 1,
        got: 2,
    };
    assert!(!is_client_disconnect(&err));
}

#[test]
fn material_ids_treat_generated_vegetation_as_passable() {
    let reports = vec![
        report("minecraft:air", &[], &[(0, true, &[])]),
        report("minecraft:stone", &[], &[(1, true, &[])]),
        report(
            "minecraft:water",
            &[("level", &["0", "1"])],
            &[(2, true, &[("level", "0")]), (8, false, &[("level", "1")])],
        ),
        report(
            "minecraft:lava",
            &[("level", &["0", "1"])],
            &[(3, true, &[("level", "0")]), (9, false, &[("level", "1")])],
        ),
        report("minecraft:short_grass", &[], &[(4, true, &[])]),
        report("minecraft:poppy", &[], &[(5, true, &[])]),
        report(
            "minecraft:sugar_cane",
            &[("age", &["0", "1"])],
            &[(6, true, &[("age", "0")]), (7, false, &[("age", "1")])],
        ),
    ];
    let registry = BlockRegistry::from_report(&reports).unwrap();
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let ids = material_ids(&registry, &facts);

    assert_eq!(ids.classify(1), BlockMaterial::Solid);
    assert_eq!(ids.classify(2), BlockMaterial::Water);
    assert_eq!(ids.classify(3), BlockMaterial::Lava);
    assert_eq!(ids.classify(4), BlockMaterial::Air);
    assert_eq!(ids.classify(5), BlockMaterial::Air);
    assert_eq!(ids.classify(6), BlockMaterial::Air);
    assert_eq!(ids.classify(7), BlockMaterial::Air);
    assert_eq!(ids.classify(8), BlockMaterial::Water);
    assert_eq!(ids.classify(9), BlockMaterial::Lava);
}

#[test]
fn entity_physics_refuses_unloaded_boundary_samples() {
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(15.8, 64.0, 0.5),
        velocity: mc_entity::Vec3::new(1.0, 0.0, 0.0),
        aabb: mc_physics::Aabb::COW,
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Default,
    };
    let step = step_sampled_entity(EntityPhysicsInput {
        query,
        snapshot: Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::new(),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
            powder_snow_states: Box::default(),
            collision_direct_lookup_compatible: false,
        }),
        complete_samples: false,
    });

    assert_eq!(step.position, query.position);
    assert_eq!(step.velocity, mc_entity::Vec3::ZERO);
}

#[test]
fn living_entity_physics_pushes_horizontal_collision() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let materials = BlockMaterialIds::new(0, None, None);
    let mut storage = WorldStorage::in_memory(registry);
    let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for x in 0..mc_world::SECTION_DIM as u8 {
        for z in 0..mc_world::SECTION_DIM as u8 {
            let _ = chunk.set_block(x, 63, z, mc_world::BlockStateId(1));
        }
    }
    let _ = chunk.set_block(9, 64, 8, mc_world::BlockStateId(1));
    let _ = chunk.set_block(9, 65, 8, mc_world::BlockStateId(1));
    storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
        velocity: mc_entity::Vec3::new(20.0, 0.0, 0.0),
        aabb: mc_physics::Aabb::COW,
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Living,
    };

    let step = step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

    assert!(step.horizontal_collision);
    assert!((step.position.x - 8.55).abs() < 1.0e-9);
    assert_eq!(step.position.y, query.position.y);
    assert_eq!(step.position.z, query.position.z);
    assert_eq!(step.velocity, mc_entity::Vec3::ZERO);
    assert!(step.on_ground);
}

#[test]
fn living_entity_walks_across_farmland_at_its_collision_height() {
    let reports = [
        report("minecraft:air", &[], &[(0, true, &[])]),
        report(
            "minecraft:farmland",
            &[("moisture", &["0", "7"])],
            &[
                (1, true, &[("moisture", "0")]),
                (2, false, &[("moisture", "7")]),
            ],
        ),
    ];
    let registry = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&registry, &facts);
    let mut storage = WorldStorage::in_memory(registry);
    let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for x in 0..mc_world::SECTION_DIM as u8 {
        for z in 0..mc_world::SECTION_DIM as u8 {
            let _ = chunk.set_block(x, 64, z, mc_world::BlockStateId(2));
        }
    }
    storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
    let farmland_top = 64.0 + 15.0 / 16.0;
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.5, farmland_top, 8.5),
        velocity: mc_entity::Vec3::new(2.0, 0.0, 0.0),
        aabb: mc_physics::Aabb::COW,
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Living,
    };

    let step = step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

    assert!(step.position.x > query.position.x);
    assert_eq!(step.position.y, farmland_top);
    assert!(!step.horizontal_collision);
    assert!(step.on_ground);
}

#[test]
fn sampled_entity_moves_through_empty_side_of_isolated_fence() {
    let (mut storage, materials) = isolated_oak_fence_physics_world();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.0, 64.0, 8.1),
        velocity: mc_entity::Vec3::new(20.0, 0.0, 0.0),
        aabb: mc_physics::Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Living,
    };

    let step = step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

    assert!(step.position.x > query.position.x);
    assert!(!step.horizontal_collision);
}

#[test]
fn sampled_entity_collides_with_overheight_center_of_isolated_fence() {
    let (mut storage, materials) = isolated_oak_fence_physics_world();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.0, 65.3, 8.5),
        velocity: mc_entity::Vec3::new(30.0, 0.0, 0.0),
        aabb: mc_physics::Aabb {
            half_width: 0.2,
            height: 0.3,
        },
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Living,
    };

    let step = step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

    assert!(step.horizontal_collision);
    assert!((step.position.x - 9.175).abs() < 1.0e-9);
}

#[test]
fn entity_physics_uses_only_cached_chunks_for_sampling() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let facts = BlockFactsTable::default();
    let materials = material_ids(&registry, &facts);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage =
        WorldStorage::in_memory(Arc::clone(&registry)).with_generator(Arc::new(FlatGenerator {
            air: mc_world::BlockStateId(0),
            ground: mc_world::BlockStateId(1),
            biome,
        }));
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(42),
        position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Default,
    };

    let input = sample_entity_physics_input(query, &mut storage, &materials);
    assert!(!input.complete_samples);

    storage
        .get_chunk(mc_world::ChunkPos { x: 0, z: 0 })
        .expect("generate spawn chunk")
        .expect("spawn chunk generated");
    let input = sample_entity_physics_input(query, &mut storage, &materials);
    assert!(input.complete_samples);
    let step = step_sampled_entity(input);

    assert!(step.position.y < query.position.y);
    assert!(step.velocity.y < 0.0);
}

#[test]
fn entity_physics_fetches_each_cached_chunk_once_per_snapshot() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory(Arc::clone(&registry));
    storage
        .insert_generated_chunk(
            mc_world::ChunkPos { x: 0, z: 0 },
            mc_world::Chunk::empty(
                mc_world::ChunkPos { x: 0, z: 0 },
                mc_world::BlockStateId(0),
                biome,
            ),
        )
        .unwrap();
    let queries = [8.5, 8.6, 24.5, 24.6].map(|x| play::EntityPhysicsQuery {
        id: mc_entity::EntityId(x as i32),
        position: mc_entity::Vec3::new(x, 64.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: true,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        kind: play::EntityPhysicsKind::Default,
    });
    let plans = entity_physics_sample_plans(&queries);
    let fetches = std::cell::Cell::new(0);

    let chunks = entity_physics_chunk_snapshots(&plans, |cpos| {
        fetches.set(fetches.get() + 1);
        storage.cached_chunk_snapshot(cpos)
    });

    assert_eq!(fetches.get(), 2);
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .get(&mc_world::ChunkPos { x: 0, z: 0 })
            .is_some_and(Option::is_some)
    );
    assert!(
        chunks
            .get(&mc_world::ChunkPos { x: 1, z: 0 })
            .is_some_and(Option::is_none)
    );
}

#[test]
fn entity_physics_batch_sampling_shares_chunk_snapshots() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let facts = BlockFactsTable::default();
    let materials = material_ids(&registry, &facts);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage =
        WorldStorage::in_memory(Arc::clone(&registry)).with_generator(Arc::new(FlatGenerator {
            air: mc_world::BlockStateId(0),
            ground: mc_world::BlockStateId(1),
            biome,
        }));
    storage
        .get_chunk(mc_world::ChunkPos { x: 0, z: 0 })
        .expect("generate spawn chunk")
        .expect("spawn chunk generated");
    let queries = (0..49)
        .map(|idx| play::EntityPhysicsQuery {
            id: mc_entity::EntityId(idx + 1),
            position: mc_entity::Vec3::new(
                8.5 + f64::from(idx % 7) * 0.05,
                66.0,
                8.5 + f64::from(idx / 7) * 0.05,
            ),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: false,
            fall_distance: 0.0,
            goal_fence: mc_entity::EntityGoalFence::Idle,
            kind: play::EntityPhysicsKind::Default,
        })
        .collect::<Vec<_>>();

    let plans = entity_physics_sample_plans(&queries);
    let chunks = entity_physics_chunk_snapshots(&plans, |cpos| storage.cached_chunk_snapshot(cpos));
    let registry = storage.registry_arc();
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks,
        materials: Arc::new(materials),
        powder_snow_states: powder_snow_state_ids(&registry),
        collision_direct_lookup_compatible: collision_direct_lookup_compatible(&registry),
        blocks: Some(registry),
    });
    let inputs = entity_physics_inputs_from_snapshot(plans, snapshot);

    assert_eq!(inputs.len(), queries.len());
    assert!(inputs.iter().all(|input| input.complete_samples));
    assert!(Arc::ptr_eq(&inputs[0].snapshot, &inputs[1].snapshot));
    assert_eq!(inputs[0].snapshot.chunks.len(), 1);
}

#[test]
fn vanilla_registry_collision_direct_lookup_matches_all_table_states() {
    let registry = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded vanilla report builds a registry");
    let table = mc_data::collision_shapes::vanilla_collision_shapes();

    for state in 0..table.covered_state_count() {
        let state = u32::try_from(state).expect("covered state id fits u32");
        let block = registry
            .by_id(mc_world::BlockStateId(state))
            .expect("vanilla registry covers every collision table state");
        assert_eq!(
            table.get_for_state(state, &block.block.id, &block.properties),
            table.get(state),
            "state {state} diverges between fingerprint and direct lookup"
        );
    }

    assert!(collision_direct_lookup_compatible(&registry));
}

#[test]
fn synthetic_registry_keeps_fingerprint_collision_fallback() {
    let empty = BlockRegistry::from_report(&[]).unwrap();
    assert!(!collision_direct_lookup_compatible(&empty));

    let reports = vec![report("solaris:test_block", &[], &[(0, true, &[])])];
    let synthetic = BlockRegistry::from_report(&reports).unwrap();
    assert!(!collision_direct_lookup_compatible(&synthetic));
}

#[test]
fn physics_collision_class_fast_path_matches_fingerprint_route() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let stone = state_id(&reports, "minecraft:stone", &[]);
    let slab = state_id(
        &reports,
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let covered =
        u32::try_from(mc_data::collision_shapes::vanilla_collision_shapes().covered_state_count())
            .expect("covered state count fits u32");
    for state in 0..covered {
        let x = (state % 16) as u8;
        let z = ((state / 16) % 16) as u8;
        let y = 64 + i32::try_from(state / 256).expect("cell y fits i32");
        let _ = chunk.set_block(x, y, z, mc_world::BlockStateId(state));
    }
    let _ = chunk.set_block(3, 64, 3, mc_world::BlockStateId(stone));
    let _ = chunk.set_block(5, 64, 5, mc_world::BlockStateId(slab));
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
    storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
    let (fast, legacy) = collision_route_samplers(&storage, &blocks, &materials, chunk_pos);

    assert_eq!(collect_collision_boxes(&fast, 1, 63, 1), Vec::new());
    assert_eq!(collect_collision_boxes(&legacy, 1, 63, 1), Vec::new());
    assert_eq!(
        collect_collision_boxes(&fast, 3, 64, 3),
        vec![BlockCollisionBox::FULL_BLOCK]
    );
    assert_eq!(
        collect_collision_boxes(&legacy, 3, 64, 3),
        vec![BlockCollisionBox::FULL_BLOCK]
    );
    let slab_boxes = collect_collision_boxes(&legacy, 5, 64, 5);
    assert_eq!(slab_boxes.len(), 1);
    assert_ne!(slab_boxes[0], BlockCollisionBox::FULL_BLOCK);
    assert_eq!(collect_collision_boxes(&fast, 5, 64, 5), slab_boxes);

    for state in 0..covered {
        let x = i32::from((state % 16) as u8);
        let z = i32::from(((state / 16) % 16) as u8);
        let y = 64 + i32::try_from(state / 256).expect("cell y fits i32");
        assert_eq!(
            collect_collision_boxes(&fast, x, y, z),
            collect_collision_boxes(&legacy, x, y, z),
            "state {state} diverges between collision-class fast path and fingerprint route"
        );
    }
}

#[test]
fn physics_powder_snow_entity_contexts_match_between_collision_routes() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let powder_snow = state_id(&reports, "minecraft:powder_snow", &[]);
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(powder_snow));
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
    storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
    let chunks = HashMap::from([(chunk_pos, storage.cached_chunk_snapshot(chunk_pos))]);
    let make_sampler = |compatible: bool,
                        entity_bottom: f64,
                        fall_distance: f64,
                        powder_snow_collision: PowderSnowCollision| {
        SampledPhysicsWorld {
            snapshot: Arc::new(EntityPhysicsSnapshot {
                chunks: chunks.clone(),
                materials: Arc::new(materials.clone()),
                blocks: Some(Arc::clone(&blocks)),
                powder_snow_states: powder_snow_state_ids(&blocks),
                collision_direct_lookup_compatible: compatible,
            }),
            entity_bottom,
            fall_distance,
            powder_snow_collision,
        }
    };

    let contexts = [
        ("none-resting", 64.5, 0.0, PowderSnowCollision::None),
        ("none-falling", 64.5, 3.0, PowderSnowCollision::None),
        (
            "falling-block",
            64.5,
            0.0,
            PowderSnowCollision::FallingBlock,
        ),
        (
            "walkable-on-crust",
            65.5,
            0.0,
            PowderSnowCollision::WalkableMob,
        ),
        (
            "walkable-sinking",
            64.5,
            0.0,
            PowderSnowCollision::WalkableMob,
        ),
    ];
    for (label, entity_bottom, fall_distance, powder_snow_collision) in contexts {
        let fast = make_sampler(true, entity_bottom, fall_distance, powder_snow_collision);
        let legacy = make_sampler(false, entity_bottom, fall_distance, powder_snow_collision);
        assert_eq!(
            collect_collision_boxes(&fast, 8, 64, 8),
            collect_collision_boxes(&legacy, 8, 64, 8),
            "powder snow context {label} diverges between routes"
        );
    }

    let crust =
        BlockCollisionBox::from_fixed_4096([0, 0, 0, 4096, (0.9_f32 * 4096.0) as i16, 4096])
            .expect("powder snow crust box is valid");
    assert_eq!(
        collect_collision_boxes(
            &make_sampler(true, 64.5, 3.0, PowderSnowCollision::None),
            8,
            64,
            8
        ),
        vec![crust]
    );
    assert_eq!(
        collect_collision_boxes(
            &make_sampler(true, 64.5, 0.0, PowderSnowCollision::FallingBlock),
            8,
            64,
            8
        ),
        vec![BlockCollisionBox::FULL_BLOCK]
    );
    assert_eq!(
        collect_collision_boxes(
            &make_sampler(true, 65.5, 0.0, PowderSnowCollision::WalkableMob),
            8,
            64,
            8
        ),
        vec![BlockCollisionBox::FULL_BLOCK]
    );
    assert!(
        collect_collision_boxes(
            &make_sampler(true, 64.5, 0.0, PowderSnowCollision::None),
            8,
            64,
            8
        )
        .is_empty(),
        "ordinary entities sink: recorded powder snow shape stays empty"
    );
    assert!(
        collect_collision_boxes(
            &make_sampler(true, 64.5, 0.0, PowderSnowCollision::WalkableMob),
            8,
            64,
            8
        )
        .is_empty(),
        "tagged mobs inside powder snow keep sinking"
    );
}

#[test]
fn cached_material_ids_recovers_poisoned_mutex_state() {
    let cache = PHYSICS_MATERIAL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    let poisoned = std::panic::catch_unwind(|| {
        let _guard = cache.lock().unwrap();
        panic!("inject material cache poison");
    });
    assert!(poisoned.is_err());

    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let config = ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "poisoned-material-cache-test".into(),
        max_players: 0,
        view_distance: 0,
        data: Arc::new(mc_data::testing::stub()),
        blocks,
        world: None,
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: canonical_entity_types(),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    };

    let first = cached_material_ids(&config);
    let second = cached_material_ids(&config);

    assert!(Arc::ptr_eq(&first, &second));
    assert!(cache.lock().is_ok());
}

#[tokio::test]
async fn serve_shutdown_drains_without_starting_final_save() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    let shutdown = config.shutdown.clone();
    let bound = bind(config).await.expect("bind");
    let save = bound.save_handle();
    let serve = tokio::spawn(bound.serve());

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(2), serve)
        .await
        .expect("serve drain exits after shutdown")
        .expect("serve task joins")
        .expect("serve drain succeeds");

    let metadata = tmp.path().join("solaris").join("world.dat");
    assert!(
        !metadata.exists(),
        "serve drain must not perform the final save"
    );

    let report = save.save_all_after_drain().await;
    assert!(report.is_ok(), "single final save failed: {report:?}");
    assert!(report.world_metadata_saved);
    assert!(metadata.exists());
}

#[tokio::test]
async fn public_run_performs_final_save_after_successful_drain() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    config.shutdown.request();

    run(config).await.expect("public run drains and saves");

    assert!(tmp.path().join("solaris").join("world.dat").exists());
}

#[tokio::test]
async fn run_bound_propagates_final_save_failure() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    let shutdown = config.shutdown.clone();
    let bound = bind(config).await.expect("bind");
    std::fs::create_dir(tmp.path().join("solaris/world.dat")).unwrap();
    shutdown.request();

    let error = serve_then_final_save(bound)
        .await
        .expect_err("final save failure reaches run caller");

    assert_eq!(error.kind(), ErrorKind::Other);
    assert!(error.to_string().contains("final save failed"));
}

#[tokio::test]
async fn serve_error_still_runs_final_save_without_masking_primary_error() {
    let save_called = Arc::new(AtomicBool::new(false));
    let save_called_by_future = Arc::clone(&save_called);
    let save = async move {
        save_called_by_future.store(true, Ordering::SeqCst);
        SaveAllReport {
            players_saved: 1,
            entities_saved: 2,
            chunks_flushed: 3,
            world_metadata_saved: true,
            timings: SaveAllTimings::default(),
            errors: vec!["final save failed".to_owned()],
        }
    };

    let error = finish_serve_with_final_save(
        Err(std::io::Error::new(
            ErrorKind::ConnectionAborted,
            "listener accept failed",
        )),
        save,
    )
    .await
    .expect_err("primary serve error remains visible");

    assert!(save_called.load(Ordering::SeqCst));
    assert_eq!(error.kind(), ErrorKind::ConnectionAborted);
    assert_eq!(error.to_string(), "listener accept failed");
}

#[tokio::test]
async fn run_bound_drains_admitted_simulation_mutation_before_final_save() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    let shutdown = config.shutdown.clone();
    let bound = bind(config).await.expect("bind");
    let simulation = bound.simulation.clone();
    let mut mutation = Box::pin(simulation.set_world_time_server_owned(73));

    std::future::poll_fn(|context| {
        assert!(
            mutation.as_mut().poll(context).is_pending(),
            "mutation must remain in flight until the simulation owner starts"
        );
        std::task::Poll::Ready(())
    })
    .await;
    shutdown.request();

    serve_then_final_save(bound)
        .await
        .expect("drain and final save succeed");
    mutation
        .await
        .expect("admitted mutation completes during drain");
    let metadata = play::persistence::load_world_metadata(tmp.path())
        .unwrap()
        .expect("world metadata saved");
    assert!(
        metadata.world_time >= 73,
        "final save must include the admitted world-time mutation"
    );
}

#[tokio::test]
async fn entity_owner_failure_is_push_published_and_typed_connection_panic_is_fatal() {
    let sessions = Arc::new(play::SessionRegistry::new());
    let mut failure = sessions.subscribe_entity_owner_failure();
    let reported = sessions
        .report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::WorkerPanicked);

    failure
        .changed()
        .await
        .expect("owner fatal watch remains open");
    assert_eq!(
        failure.borrow_and_update().as_ref().copied(),
        Some(reported)
    );

    let task_sessions = Arc::clone(&sessions);
    let join = tokio::spawn(async move {
        let _ = task_sessions.entity_owner_status_for_test();
    })
    .await
    .expect_err("owner fatal blocks the connection task through typed unwind");
    let error = connection_task_join_error(join);

    assert!(is_entity_owner_serve_error(&error));
    assert!(error.to_string().contains("WorkerPanicked"));
}

#[tokio::test]
async fn unrelated_connection_task_panic_is_terminal_without_claiming_owner_uncertainty() {
    let join = tokio::spawn(async {
        panic!("injected connection task panic");
    })
    .await
    .expect_err("connection task panic produces a join error");

    let error = connection_task_join_error(join);

    assert_eq!(error.kind(), ErrorKind::Other);
    assert!(!is_entity_owner_serve_error(&error));
    assert_eq!(error.to_string(), "connection task panicked");
}

#[tokio::test]
async fn entity_owner_serve_error_skips_clean_final_save() {
    let save_called = Arc::new(AtomicBool::new(false));
    let save_called_by_future = Arc::clone(&save_called);
    let save = async move {
        save_called_by_future.store(true, Ordering::SeqCst);
        SaveAllReport {
            players_saved: 0,
            entities_saved: 0,
            chunks_flushed: 0,
            world_metadata_saved: false,
            timings: SaveAllTimings::default(),
            errors: Vec::new(),
        }
    };
    let primary = entity_owner_serve_error(mc_entity::RegionOwnerLaneError::OutcomeUnknown);

    let error = finish_serve_with_final_save(Err(primary), save)
        .await
        .expect_err("uncertain owner state must remain terminal");

    assert!(!save_called.load(Ordering::SeqCst));
    assert!(is_entity_owner_serve_error(&error));
    assert!(error.to_string().contains("OutcomeUnknown"));
}

#[tokio::test]
async fn authoritative_runtime_poison_is_terminal_and_skips_clean_final_save() {
    let lock = Arc::new(std::sync::Mutex::new(()));
    let poisoned = Arc::clone(&lock);
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.lock().unwrap();
        panic!("inject authoritative runtime poison");
    })
    .join();
    let before = crate::runtime_lock_poison_metrics_snapshot().authoritative_poison;
    let task_lock = Arc::clone(&lock);
    let join = tokio::spawn(async move {
        drop(crate::lock_policy::lock_authoritative_mutex(
            &task_lock,
            "test.runtime_authority",
        ));
    })
    .await
    .expect_err("poisoned runtime authority must unwind its task");
    let primary = runtime_task_join_error("connection", join);

    assert!(is_uncertain_runtime_serve_error(&primary));
    assert!(primary.to_string().contains("test.runtime_authority"));
    assert!(crate::runtime_lock_poison_metrics_snapshot().authoritative_poison > before);

    let save_called = Arc::new(AtomicBool::new(false));
    let save_called_by_future = Arc::clone(&save_called);
    let save = async move {
        save_called_by_future.store(true, Ordering::SeqCst);
        SaveAllReport {
            players_saved: 0,
            entities_saved: 0,
            chunks_flushed: 0,
            world_metadata_saved: false,
            timings: SaveAllTimings::default(),
            errors: Vec::new(),
        }
    };
    let error = finish_serve_with_final_save(Err(primary), save)
        .await
        .expect_err("poisoned runtime state must remain terminal");

    assert!(!save_called.load(Ordering::SeqCst));
    assert!(is_uncertain_runtime_serve_error(&error));
}

#[tokio::test]
async fn unrelated_command_task_panic_is_a_terminal_drain_error() {
    let result = tokio::spawn(async {
        panic!("injected command task failure");
        #[allow(unreachable_code)]
        "test command"
    })
    .await;

    let error = log_command_task_exit(result, true).expect_err("join failure propagates");

    assert_eq!(error.kind(), ErrorKind::Other);
    assert!(!is_entity_owner_serve_error(&error));
    assert_eq!(error.to_string(), "command task panicked");
}

#[tokio::test]
async fn typed_owner_panic_from_command_or_entity_task_remains_owner_fatal() {
    let sessions = Arc::new(play::SessionRegistry::new());
    sessions.report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::OutcomeUnknown);

    let command_sessions = Arc::clone(&sessions);
    let command_result = tokio::spawn(async move {
        let _ = command_sessions.entity_owner_status_for_test();
        #[allow(unreachable_code)]
        "script command"
    })
    .await;
    let command_error =
        log_command_task_exit(command_result, false).expect_err("typed panic propagates");
    assert!(is_entity_owner_serve_error(&command_error));
    assert!(command_error.to_string().contains("OutcomeUnknown"));

    let entity_sessions = Arc::clone(&sessions);
    let entity_result = tokio::spawn(async move {
        let _ = entity_sessions.entity_owner_status_for_test();
    })
    .await;
    let shutdown = ShutdownHandle::default();
    let entity_error =
        handle_entity_ticker_exit(&shutdown, entity_result).expect_err("typed panic propagates");
    assert!(is_entity_owner_serve_error(&entity_error));
    assert!(entity_error.to_string().contains("OutcomeUnknown"));
    assert!(shutdown.is_requested());
}

#[tokio::test]
async fn periodic_save_worker_failure_is_a_drain_error() {
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let mut started = Some(started);
    let worker = crate::dirty_flush::DirtyFlushCoordinator::spawn(move || {
        let started = started.take().expect("one flush invocation is expected");
        async move {
            started.send(()).expect("test observes worker start");
            panic!("injected periodic save worker failure");
        }
    });
    worker.notifier().request();
    started_rx.await.expect("worker reports start");

    let error = drain_periodic_save_worker(Some(worker))
        .await
        .expect_err("periodic worker failure propagates");

    assert_eq!(error.kind(), ErrorKind::Other);
    assert!(!is_uncertain_runtime_serve_error(&error));
    assert_eq!(error.to_string(), "periodic save task panicked");
}

#[tokio::test]
async fn accept_failure_requests_shutdown_and_runtime_drain() {
    let shutdown = ShutdownHandle::default();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let config = ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "accept-failure-drain-test".into(),
        max_players: 0,
        view_distance: 0,
        data: Arc::new(mc_data::testing::stub()),
        blocks,
        world: None,
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: canonical_entity_types(),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy {
            chunk_worker_threads: 8,
            runtime_control: Some(crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 4,
                    chunk_send_rate: 8,
                    chunk_load_rate: 16,
                    chunk_generate_rate: 16,
                },
            }),
            ..ChunkPipelinePolicy::default()
        },
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };

    let bound = bind(config).await.expect("bind");
    let resources = bound.chunk_pipeline_resources.clone();
    let runtime_control = bound
        .runtime_control_handle()
        .expect("runtime control enabled");
    let error = handle_accept_failure(
        std::io::Error::new(ErrorKind::ConnectionAborted, "injected accept failure"),
        &shutdown,
        Some(&runtime_control),
        &resources,
        &bound.sessions,
    );

    assert_eq!(error.kind(), ErrorKind::ConnectionAborted);
    assert!(shutdown.is_requested());
    assert!(runtime_control.snapshot().draining);
    assert_eq!(resources.prepare_limit(), 1);
}

#[tokio::test]
async fn entity_ticker_drain_waits_for_in_flight_tick() {
    let entered_tick = Arc::new(Notify::new());
    let release_tick = Arc::new(Notify::new());
    let task_entered = Arc::clone(&entered_tick);
    let task_release = Arc::clone(&release_tick);
    let ticker = tokio::spawn(async move {
        task_entered.notify_waiters();
        task_release.notified().await;
    });
    entered_tick.notified().await;

    let mut drain = std::pin::pin!(drain_entity_ticker(ticker));
    let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
    probe_tx.send(()).unwrap();
    tokio::select! {
        biased;
        result = &mut drain => panic!("entity ticker drain returned before the in-flight tick completed: {result:?}"),
        result = probe_rx => result.unwrap(),
    }

    release_tick.notify_waiters();
    drain.await.expect("entity ticker drains");
}

#[tokio::test]
async fn entity_ticker_timeout_cancels_task_before_returning() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let ticker = tokio::spawn(async move {
        let _held_tx = held_tx;
        entered_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    entered_rx.await.unwrap();

    let error = drain_entity_ticker_with_timeout(ticker, Duration::ZERO)
        .await
        .expect_err("entity ticker timeout fails the drain");

    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(held_rx.await.is_err(), "ticker task must be dropped");
}

#[tokio::test]
async fn late_owner_panic_during_connection_drain_remains_owner_fatal() {
    let sessions = Arc::new(play::SessionRegistry::new());
    sessions.report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::WorkerPanicked);
    let mut connections = tokio::task::JoinSet::new();
    connections.spawn(async move {
        let _ = sessions.entity_owner_status_for_test();
    });

    let error = drain_connections_with_timeout(&mut connections, Duration::from_secs(1))
        .await
        .expect_err("late owner panic must fail the drain");

    assert!(connections.is_empty());
    assert!(is_entity_owner_serve_error(&error));
    assert!(error.to_string().contains("WorkerPanicked"));
}

#[tokio::test]
async fn connection_timeout_cancels_tasks_before_returning() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let mut connections = tokio::task::JoinSet::new();
    connections.spawn(async move {
        let _held_tx = held_tx;
        entered_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    entered_rx.await.unwrap();

    let error = drain_connections_with_timeout(&mut connections, Duration::ZERO)
        .await
        .expect_err("connection timeout fails the drain");

    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(connections.is_empty());
    assert!(held_rx.await.is_err(), "connection task must be dropped");
}

#[test]
fn console_stop_requests_drain_and_shutdown() {
    let shutdown = ShutdownHandle::default();
    let runtime_control = RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 4,
            chunk_send_rate: 8,
            chunk_load_rate: 16,
            chunk_generate_rate: 16,
        },
    });
    let resources = ChunkPipelineResources::with_limits(1, 4);
    let sessions = play::SessionRegistry::new();

    request_stop(&shutdown, Some(&runtime_control), &resources, &sessions);

    assert!(runtime_control.snapshot().draining);
    assert!(shutdown.is_requested());
}

#[tokio::test]
async fn console_stop_requests_shutdown_without_early_save() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(ItemRegistry::default()),
        canonical_entity_types(),
    );
    let runtime_control = RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 4,
            chunk_send_rate: 8,
            chunk_load_rate: 16,
            chunk_generate_rate: 16,
        },
    });
    let resources = ChunkPipelineResources::with_limits(1, 4);
    let sessions = Arc::new(play::SessionRegistry::new());
    let (simulation, mut owner) = play::simulation_channel();
    let control = OperatorControlHandle {
        sessions: Arc::clone(&sessions),
        simulation,
        shutdown: config.shutdown.clone(),
        runtime_control: Some(runtime_control.clone()),
        resources,
        operators: config.command_permissions.operator_identities(),
        whitelist: config.command_permissions.whitelist_identities(),
    };
    control.request_stop();
    assert!(config.shutdown.is_requested());
    assert!(runtime_control.snapshot().draining);
    let metadata = tmp.path().join("solaris").join("world.dat");
    assert!(
        !metadata.exists(),
        "console stop must not save before the runtime drain"
    );

    owner.shutdown();
    let report = save_all_after_drain_with_context("test final save", &config, &sessions).await;
    assert!(report.is_ok(), "post-drain final save failed: {report:?}");
    assert!(metadata.exists());
}
