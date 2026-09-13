use super::*;
use mc_physics::{BlockMaterial, BlockSampler, EntityBody, PhysicsConfig};

struct TerrainSnapshot<'a> {
    snapshot: &'a mc_world::WorldReadSnapshot,
    materials: &'a mc_physics::BlockMaterialIds,
}

impl BlockSampler for TerrainSnapshot<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.snapshot
            .get_cached_block(mc_world::BlockPos { x, y, z })
            .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
    }
}

fn oak_leaves_state() -> u32 {
    mc_data::blocks::solaris_required_blocks_report()
        .iter()
        .find(|block| block.id.as_str() == "minecraft:oak_leaves")
        .and_then(|block| block.states.iter().find(|state| state.default))
        .map(|state| state.id)
        .expect("oak_leaves default state")
}

/// Live world geometry captured around a natural oak at trunk (-59, 68..71, -61),
/// shifted into chunk (0, 0). The canopy's lowest leaves sit at the sheep's head
/// height, so a sheep beside the trunk cannot stand under it.
fn live_oak_blocks() -> Vec<(i32, i32, i32, u32)> {
    const OX: i32 = 69;
    const OZ: i32 = 68;
    let leaves = oak_leaves_state();
    let log = vanilla_block_state_id("minecraft:oak_log", &[("axis", "y")]);
    let grass = vanilla_block_state_id("minecraft:grass_block", &[("snowy", "false")]);
    let dirt = vanilla_block_state_id("minecraft:dirt", &[]);
    let mut blocks = Vec::new();
    for x in -63..=-54 {
        for z in -65..=-56 {
            blocks.push((x + OX, 66, z + OZ, dirt));
            blocks.push((x + OX, 67, z + OZ, grass));
        }
    }
    for y in 68..=71 {
        blocks.push((-59 + OX, y, -61 + OZ, log));
    }
    let canopy: &[(i32, i32)] = &[
        (-61, -62),
        (-61, -61),
        (-61, -60),
        (-60, -63),
        (-60, -62),
        (-60, -61),
        (-60, -60),
        (-60, -59),
        (-59, -63),
        (-59, -62),
        (-59, -60),
        (-58, -63),
        (-58, -62),
        (-58, -61),
        (-58, -60),
        (-58, -59),
        (-57, -62),
        (-57, -61),
        (-57, -60),
    ];
    for &(x, z) in canopy {
        blocks.push((x + OX, 69, z + OZ, leaves));
        blocks.push((x + OX, 70, z + OZ, leaves));
        blocks.push((x + OX, 71, z + OZ, leaves));
    }
    for &(x, z) in &[
        (-60, -61),
        (-59, -62),
        (-59, -60),
        (-58, -61),
        (-58, -60),
        (-57, -61),
    ] {
        blocks.push((x + OX, 72, z + OZ, leaves));
    }
    blocks
}

/// Runs one sheep beside the live oak with the given goal and returns
/// `(net horizontal displacement, total absolute yaw change, ticks moved)`.
fn run_sheep_beside_live_oak(goal: mc_entity::GoalState, ticks: u64) -> (f64, f64, u64) {
    let blocks = live_oak_blocks();
    let (world_read, materials) = vanilla_collision_pathing_world(&blocks);
    let snapshot = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let sampler = TerrainSnapshot {
        snapshot: &snapshot,
        materials: &materials,
    };
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("MobSpinObserver"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(1.5, 68.0, 1.5),
    );
    registry.mark_loaded(player, (0, 0));
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        100,
        "minecraft:sheep".to_owned(),
        Vec3::new(-61.5 + 69.0, 68.0, -60.5 + 68.0),
    );
    let sheep = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = registry.lock_entities("point sheep at target");
        assert!(entities.set_goal(sheep, goal));
    }

    let mut start = None;
    let mut last_yaw: Option<f32> = None;
    let mut yaw_total = 0.0_f64;
    let mut moved_ticks = 0_u64;
    for tick in 1..=ticks {
        let queries = registry.tick_entities_and_collect_physics_queries_with_terrain(
            tick,
            &world_read,
            &materials,
        );
        let Some(query) = queries.iter().find(|query| query.id == sheep) else {
            continue;
        };
        let stepped = mc_physics::step_entity(
            EntityBody {
                position: mc_physics::Vec3::new(
                    query.position.x,
                    query.position.y,
                    query.position.z,
                ),
                velocity: mc_physics::Vec3::new(
                    query.velocity.x,
                    query.velocity.y,
                    query.velocity.z,
                ),
                aabb: query.aabb,
                on_ground: query.on_ground,
            },
            &sampler,
            PhysicsConfig::living_entity(),
        );
        let position = Vec3::new(
            stepped.body.position.x,
            stepped.body.position.y,
            stepped.body.position.z,
        );
        if query.velocity.horizontal_len() > f64::EPSILON {
            moved_ticks += 1;
        }
        registry.apply_entity_physics_if_current_and_dispatch(
            tick,
            &queries,
            &[EntityPhysicsStep {
                id: sheep,
                position,
                velocity: Vec3::new(
                    stepped.body.velocity.x,
                    stepped.body.velocity.y,
                    stepped.body.velocity.z,
                ),
                on_ground: stepped.body.on_ground,
                horizontal_collision: stepped.horizontal_collision,
            }],
        );
        let snapshot = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == sheep)
            .expect("sheep snapshot")
            .snapshot;
        if start.is_none() {
            start = Some(snapshot.position);
        }
        if let Some(previous) = last_yaw {
            yaw_total +=
                f64::from(((snapshot.rotation.yaw - previous + 180.0).rem_euclid(360.0)) - 180.0)
                    .abs();
        }
        last_yaw = Some(snapshot.rotation.yaw);
    }
    let end = registry.persisted_entity_records()[0].snapshot.position;
    let start = start.expect("sheep observed");
    (
        (end.x - start.x).hypot(end.z - start.z),
        yaw_total,
        moved_ticks,
    )
}

#[test]
fn sheep_beside_a_leaf_canopy_keeps_walking_to_a_reachable_target() {
    let (net, yaw_total, moved_ticks) = run_sheep_beside_live_oak(
        mc_entity::GoalState::FollowPosition {
            target: Vec3::new(-61.5 + 69.0, 68.0, -74.5 + 68.0),
            speed: 2.3,
        },
        120,
    );

    assert!(
        net > 3.0,
        "a sheep beside leaves must walk to a reachable target (moved {net:.2})"
    );
    assert!(
        yaw_total < 360.0,
        "walking must not spin the sheep (yaw changed {yaw_total:.1} degrees)"
    );
    assert!(moved_ticks > 60, "sheep must keep moving, not stall");
}

#[test]
fn sheep_beside_a_leaf_canopy_does_not_spin_at_an_unreachable_target() {
    // Target sits under the canopy where the sheep's head cannot fit, so no
    // path can ever reach it. Before the fix the sheep repeatedly stepped away
    // and back, turning through hundreds of degrees in place.
    let (net, yaw_total, _) = run_sheep_beside_live_oak(
        mc_entity::GoalState::FollowPosition {
            target: Vec3::new(-59.5 + 69.0, 68.0, -61.5 + 68.0),
            speed: 2.3,
        },
        120,
    );

    assert!(
        yaw_total < 180.0,
        "an unreachable target must not spin the sheep (yaw changed {yaw_total:.1} degrees)"
    );
    assert!(
        net < 1.0,
        "the sheep must stop, not oscillate around the leaves (moved {net:.2})"
    );
}

/// A wide spider spawned overlapping leaf blocks (its 1.4-wide body reaches the
/// neighbouring leaf columns) must walk out and continue, never rotate in place.
#[test]
fn spider_spawned_inside_leaves_walks_out() {
    let leaves = oak_leaves_state();
    let stone = vanilla_block_state_id("minecraft:stone", &[]);
    let mut blocks = Vec::new();
    for x in 0..16 {
        for z in 0..16 {
            blocks.push((x, 67, z, stone));
        }
    }
    // The spider's own column and two neighbours are leaves, so half-step
    // probes stay inside the overlapped block: only a full-block escape works.
    for &(x, z) in &[(8, 8), (7, 8), (8, 7)] {
        blocks.push((x, 68, z, leaves));
        blocks.push((x, 69, z, leaves));
    }
    let (world_read, materials) = vanilla_collision_pathing_world(&blocks);
    let snapshot = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let sampler = TerrainSnapshot {
        snapshot: &snapshot,
        materials: &materials,
    };
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("MobSpinObserver"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(1.5, 68.0, 1.5),
    );
    registry.mark_loaded(player, (0, 0));
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        100,
        "minecraft:spider".to_owned(),
        Vec3::new(8.5, 68.0, 8.5),
    );
    let spider = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = registry.lock_entities("point spider out of the leaves");
        assert!(entities.set_goal(
            spider,
            mc_entity::GoalState::FollowPosition {
                target: Vec3::new(3.5, 68.0, 12.5),
                speed: 3.0,
            },
        ));
    }
    let start = registry.persisted_entity_records()[0].snapshot.position;
    let mut last_yaw: Option<f32> = None;
    let mut yaw_total = 0.0_f64;
    for tick in 1..=160u64 {
        let queries = registry.tick_entities_and_collect_physics_queries_with_terrain(
            tick,
            &world_read,
            &materials,
        );
        let Some(query) = queries.iter().find(|query| query.id == spider) else {
            continue;
        };
        let stepped = mc_physics::step_entity(
            EntityBody {
                position: mc_physics::Vec3::new(
                    query.position.x,
                    query.position.y,
                    query.position.z,
                ),
                velocity: mc_physics::Vec3::new(
                    query.velocity.x,
                    query.velocity.y,
                    query.velocity.z,
                ),
                aabb: query.aabb,
                on_ground: query.on_ground,
            },
            &sampler,
            PhysicsConfig::living_entity(),
        );
        let position = Vec3::new(
            stepped.body.position.x,
            stepped.body.position.y,
            stepped.body.position.z,
        );
        registry.apply_entity_physics_if_current_and_dispatch(
            tick,
            &queries,
            &[EntityPhysicsStep {
                id: spider,
                position,
                velocity: Vec3::new(
                    stepped.body.velocity.x,
                    stepped.body.velocity.y,
                    stepped.body.velocity.z,
                ),
                on_ground: stepped.body.on_ground,
                horizontal_collision: stepped.horizontal_collision,
            }],
        );
        let snapshot = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == spider)
            .expect("spider snapshot")
            .snapshot;
        if let Some(previous) = last_yaw {
            yaw_total +=
                f64::from(((snapshot.rotation.yaw - previous + 180.0).rem_euclid(360.0)) - 180.0)
                    .abs();
        }
        last_yaw = Some(snapshot.rotation.yaw);
    }
    let end = registry.persisted_entity_records()[0].snapshot.position;
    let net = (end.x - start.x).hypot(end.z - start.z);
    assert!(
        net > 2.0,
        "a spider spawned inside leaves must walk out (moved {net:.2})"
    );
    assert!(
        yaw_total < 720.0,
        "escaping must not spin the spider (yaw changed {yaw_total:.1} degrees)"
    );
}
