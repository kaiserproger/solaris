use std::collections::{BTreeMap, HashSet};
use std::hint::black_box;
use std::time::Instant;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

use super::*;
use crate::login::LoggedInProfile;
use crate::play::session::EXPLOSIONS_PER_TICK;

const BACKGROUND_ENTITIES: usize = 4_096;
const QUEUED_EXPLOSIONS: usize = 64;
const IDLE_TICKS: u64 = 1_000;

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
}

fn block_report(id: &str, state_id: u32) -> BlockReport {
    BlockReport {
        id: Identifier::parse(id).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: state_id,
            default: true,
            properties: BTreeMap::new(),
        }],
    }
}

fn benchmark_block_reports() -> Vec<BlockReport> {
    vec![
        block_report("minecraft:air", 0),
        block_report("minecraft:stone", 1),
        block_report("minecraft:water", 2),
        block_report("minecraft:sand", 3),
        block_report("minecraft:campfire", 4),
        block_report("minecraft:tnt", 5),
        block_report("minecraft:dirt", 6),
    ]
}

fn fill_explosion_volume(storage: &mut WorldStorage) {
    for x in 7..=9 {
        for y in 63..=65 {
            for z in 7..=9 {
                storage
                    .set_block_at(BlockPos { x, y, z }, BlockStateId(6))
                    .unwrap();
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit O3 full explosion authority load benchmark"]
async fn explosion_authority_load_benchmark_report() {
    let blocks = Arc::new(BlockRegistry::from_report(&benchmark_block_reports()).unwrap());
    let items = mc_data::items::solaris_required_items();
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let chunk = ChunkPos { x: 0, z: 0 };
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    fill_explosion_volume(&mut storage);
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ExplosionLoadObserver"),
        name: "ExplosionLoadObserver".to_owned(),
    };
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8_192);
    let observer = sessions
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            outbound_tx,
            PlayerPose::new(20.5, 64.0, 8.5),
        )
        .0;
    sessions.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    assert!(sessions.mark_loaded(observer, (0, 0)).is_empty());

    for index in 0..BACKGROUND_ENTITIES {
        let grid_x = index % 64;
        let grid_z = index / 64;
        let position = Vec3::new(
            8.5 + (grid_x as f64 - 32.0) * 4.0,
            64.0,
            8.5 + (grid_z as f64 - 32.0) * 4.0,
        );
        black_box(sessions.spawn_command_entity(
            &SimulationAuthority::for_test(),
            5,
            "minecraft:cow".to_owned(),
            position,
        ));
    }

    let mut idle_us = Vec::with_capacity(IDLE_TICKS as usize);
    for tick in 0..IDLE_TICKS {
        let started = Instant::now();
        assert!(
            sessions
                .claim_due_primed_tnt(&SimulationAuthority::for_test(), tick)
                .is_empty()
        );
        idle_us.push(started.elapsed().as_micros());
    }

    let (_, mut owner) = simulation_channel();
    for index in 0..QUEUED_EXPLOSIONS {
        let offset = (index % 4) as f64 - 1.5;
        let spawn = sessions.spawn_chained_primed_tnt(
            &owner.authority,
            132,
            Vec3::new(8.5 + offset, 64.0, 8.5),
            Vec3::ZERO,
            1,
            BlockStateId(0),
        );
        dispatch_visibility_commands(spawn);
    }
    while outbound_rx.try_recv().is_ok() {}

    let mut explosion_resistance = vec![0.0; 29_873];
    explosion_resistance[6] = 0.5;
    let block_facts = BlockFactsTable::default().with_explosion_table(
        mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
            .unwrap(),
    );
    let materials = mc_physics::BlockMaterialIds::new(0, None, None);

    let profile_center = Vec3::new(8.5, 64.0, 8.5);
    let mut target_snapshot_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    for _ in 0..QUEUED_EXPLOSIONS {
        let started = Instant::now();
        black_box(sessions.explosion_entity_targets(&owner.authority, profile_center, 8.0));
        target_snapshot_us.push(started.elapsed().as_micros());
    }
    let mut constant_planner_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut constant_random = JavaLegacyRandom::new(0x51_0A_12);
    for _ in 0..QUEUED_EXPLOSIONS {
        let started = Instant::now();
        black_box(
            plan_explosion_candidates(profile_center, 4.0, &mut constant_random, |_| {
                Some(ExplosionBlockSample {
                    resistance: None,
                    explodable: true,
                })
            })
            .unwrap(),
        );
        constant_planner_us.push(started.elapsed().as_micros());
    }
    let mut world_planner_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut world_random = JavaLegacyRandom::new(0x51_0A_12);
    {
        let mut storage = world.lock().await;
        for _ in 0..QUEUED_EXPLOSIONS {
            let started = Instant::now();
            black_box(
                plan_explosion_candidates(profile_center, 4.0, &mut world_random, |position| {
                    let state = storage.get_block(position).ok().flatten()?;
                    let resistance = if state == BlockStateId(0) {
                        None
                    } else {
                        Some(block_facts.explosion_resistance(state.0)?)
                    };
                    Some(ExplosionBlockSample {
                        resistance,
                        explodable: true,
                    })
                })
                .unwrap(),
            );
            world_planner_us.push(started.elapsed().as_micros());
        }
    }
    let profile_targets = sessions.explosion_entity_targets(&owner.authority, profile_center, 8.0);
    let mut constant_impact_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    for _ in 0..QUEUED_EXPLOSIONS {
        let started = Instant::now();
        for target in &profile_targets {
            black_box(plan_entity_explosion_impact(
                profile_center,
                4.0,
                target.position,
                target.eye_position,
                target.aabb_min,
                target.aabb_max,
                |_| Some(Vec::new()),
            ));
        }
        constant_impact_us.push(started.elapsed().as_micros());
    }
    let mut world_impact_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    {
        let mut storage = world.lock().await;
        for _ in 0..QUEUED_EXPLOSIONS {
            let started = Instant::now();
            for target in &profile_targets {
                black_box(plan_entity_explosion_impact(
                    profile_center,
                    4.0,
                    target.position,
                    target.eye_position,
                    target.aabb_min,
                    target.aabb_max,
                    |position| explosion_collision_boxes(&mut storage, &materials, position),
                ));
            }
            world_impact_us.push(started.elapsed().as_micros());
        }
    }
    let mut mutation_prepare_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut mutation_apply_us = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut mutation_candidates = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut mutation_edits = Vec::with_capacity(QUEUED_EXPLOSIONS);
    let mut mutation_random = JavaLegacyRandom::new(0x51_0A_12);
    {
        let mut storage = world.lock().await;
        for _ in 0..QUEUED_EXPLOSIONS {
            fill_explosion_volume(&mut storage);
            let candidates =
                plan_explosion_candidates(profile_center, 4.0, &mut mutation_random, |position| {
                    let state = storage.get_block(position).ok().flatten()?;
                    let resistance = if state == BlockStateId(0) {
                        None
                    } else {
                        Some(block_facts.explosion_resistance(state.0)?)
                    };
                    Some(ExplosionBlockSample {
                        resistance,
                        explodable: true,
                    })
                })
                .unwrap();
            mutation_candidates.push(candidates.len() as u128);

            let mut positions = candidates.into_iter().collect::<Vec<_>>();
            positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
            mutation_random.shuffle(&mut positions);
            let prepare_started = Instant::now();
            let mut edits = Vec::new();
            let mut preconditions = Vec::new();
            for position in positions {
                let Some(state) = storage.get_block(position).unwrap() else {
                    continue;
                };
                if state == BlockStateId(0) {
                    continue;
                }
                let Some(token) = storage.block_mutation_token(position) else {
                    continue;
                };
                edits.push(BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                });
                preconditions.push(BlockEditPrecondition {
                    pos: position,
                    expected_state: state,
                    expected_token: token,
                });
            }
            mutation_prepare_us.push(prepare_started.elapsed().as_micros());
            mutation_edits.push(edits.len() as u128);

            let apply_started = Instant::now();
            black_box(apply_block_edit_batch_to_storage_conditionally(
                &mut storage,
                None,
                &edits,
                &preconditions,
            ));
            mutation_apply_us.push(apply_started.elapsed().as_micros());
        }
    }
    target_snapshot_us.sort_unstable();
    constant_planner_us.sort_unstable();
    world_planner_us.sort_unstable();
    constant_impact_us.sort_unstable();
    world_impact_us.sort_unstable();
    mutation_prepare_us.sort_unstable();
    mutation_apply_us.sort_unstable();
    mutation_candidates.sort_unstable();
    mutation_edits.sort_unstable();
    println!(
        "EXPLOSION_STAGE_BENCH samples={} nearby_targets={} target_snapshot_p50_us={} target_snapshot_p99_us={} constant_planner_p50_us={} constant_planner_p99_us={} world_planner_p50_us={} world_planner_p99_us={} constant_impact_p50_us={} constant_impact_p99_us={} world_impact_p50_us={} world_impact_p99_us={} candidates_p50={} candidates_p99={} edits_p50={} edits_p99={} mutation_prepare_p50_us={} mutation_prepare_p99_us={} mutation_apply_p50_us={} mutation_apply_p99_us={}",
        QUEUED_EXPLOSIONS,
        profile_targets.len(),
        percentile(&target_snapshot_us, 50),
        percentile(&target_snapshot_us, 99),
        percentile(&constant_planner_us, 50),
        percentile(&constant_planner_us, 99),
        percentile(&world_planner_us, 50),
        percentile(&world_planner_us, 99),
        percentile(&constant_impact_us, 50),
        percentile(&constant_impact_us, 99),
        percentile(&world_impact_us, 50),
        percentile(&world_impact_us, 99),
        percentile(&mutation_candidates, 50),
        percentile(&mutation_candidates, 99),
        percentile(&mutation_edits, 50),
        percentile(&mutation_edits, 99),
        percentile(&mutation_prepare_us, 50),
        percentile(&mutation_prepare_us, 99),
        percentile(&mutation_apply_us, 50),
        percentile(&mutation_apply_us, 99),
    );

    let mut burst_us = Vec::with_capacity(QUEUED_EXPLOSIONS.div_ceil(EXPLOSIONS_PER_TICK));
    let mut completed = 0;
    let mut mutated_volumes = 0;
    for _ in 1..=QUEUED_EXPLOSIONS.div_ceil(EXPLOSIONS_PER_TICK) as u64 {
        {
            let mut storage = world.lock().await;
            fill_explosion_volume(&mut storage);
        }
        owner.advance_world_time(&sessions, 1);
        let started = Instant::now();
        completed += owner
            .tick_primed_tnt(
                &sessions,
                Some(&world),
                None,
                &block_facts,
                ExplosionRegistries::new(&blocks, &items, &entity_types),
                Some(&materials),
                || None,
            )
            .await;
        burst_us.push(started.elapsed().as_micros());
        if world
            .lock()
            .await
            .get_block(BlockPos { x: 8, y: 64, z: 8 })
            .unwrap()
            == Some(BlockStateId(0))
        {
            mutated_volumes += 1;
        }
        while outbound_rx.try_recv().is_ok() {}
    }
    assert_eq!(completed, QUEUED_EXPLOSIONS);
    assert_eq!(mutated_volumes, QUEUED_EXPLOSIONS);
    assert_eq!(
        world
            .lock()
            .await
            .get_block(BlockPos { x: 8, y: 64, z: 8 })
            .unwrap(),
        Some(BlockStateId(0))
    );
    assert!(sessions.primed_tnt_fuses_for_test().is_empty());

    idle_us.sort_unstable();
    burst_us.sort_unstable();
    let burst_p99_us = percentile(&burst_us, 99);
    println!(
        "EXPLOSION_LOAD_BENCH background_entities={BACKGROUND_ENTITIES} queued_explosions={QUEUED_EXPLOSIONS} explosions_per_tick={EXPLOSIONS_PER_TICK} idle_ticks={IDLE_TICKS} idle_p50_us={} idle_p99_us={} burst_ticks={} burst_p50_us={} burst_p95_us={} burst_p99_us={burst_p99_us} burst_max_us={}",
        percentile(&idle_us, 50),
        percentile(&idle_us, 99),
        burst_us.len(),
        percentile(&burst_us, 50),
        percentile(&burst_us, 95),
        burst_us.last().copied().unwrap_or_default(),
    );
    assert!(
        burst_p99_us < 50_000,
        "bounded explosion authority exceeded one 50 ms tick at p99: {burst_p99_us} us"
    );
}
