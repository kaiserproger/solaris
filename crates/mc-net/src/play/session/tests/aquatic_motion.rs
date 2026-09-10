use super::*;
use mc_physics::{BlockMaterial, BlockSampler, EntityBody, PhysicsConfig};

struct WaterSnapshot<'a> {
    snapshot: &'a mc_world::WorldReadSnapshot,
    materials: &'a mc_physics::BlockMaterialIds,
}

impl BlockSampler for WaterSnapshot<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.snapshot
            .get_cached_block(mc_world::BlockPos { x, y, z })
            .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
    }
}

#[test]
fn fresh_legacy_fish_and_squid_move_without_collision_history_and_remain_submerged() {
    let water = vanilla_block_state_id("minecraft:water", &[("level", "0")]);
    let mut blocks = Vec::new();
    for x in 1..=8 {
        for z in 1..=8 {
            for y in 62..=66 {
                blocks.push((x, y, z, water));
            }
        }
    }
    let (world_read, materials) = vanilla_collision_pathing_world(&blocks);
    let materials = materials.with_water_states(vec![water]);
    let snapshot = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let sampler = WaterSnapshot {
        snapshot: &snapshot,
        materials: &materials,
    };
    for species in ["minecraft:cod", "minecraft:squid"] {
        let registry = SessionRegistry::new();
        let player = register_test_session(&registry, "FreshSwimmer");
        assert!(registry.mark_loaded(player, (0, 0)).is_empty());
        let start = Vec3::new(3.5, 65.5, 3.5);
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            species.to_owned(),
            start,
        );
        let mut furthest_horizontal: f64 = 0.0;
        for tick in 1..=200 {
            // No explicit terrain-pathing admission: fresh swimmers must sample
            // water before they have ever collided with a solid block.
            assert_eq!(registry.terrain_pathing_entity_count(), 0);
            let queries = registry.tick_entities_and_collect_physics_queries_with_terrain(
                tick,
                &world_read,
                &materials,
            );
            assert_eq!(queries.len(), 1);
            let query = queries[0];
            let config = match query.kind {
                EntityPhysicsKind::FishLiving => PhysicsConfig::fish_entity(),
                EntityPhysicsKind::SquidLiving => PhysicsConfig::squid_entity(),
                other => panic!("{species} selected non-species travel: {other:?}"),
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
                config,
            );
            let position = Vec3::new(
                stepped.body.position.x,
                stepped.body.position.y,
                stepped.body.position.z,
            );
            assert_eq!(
                mc_entity::aquatic_motion::water_occupancy(position, query.aabb, |x, y, z| Ok(
                    sampler.material_at(x, y, z)
                )),
                PathingProbeResult::Walkable,
                "{species} left water at tick {tick}: {position:?}"
            );
            furthest_horizontal =
                furthest_horizontal.max((position.x - start.x).hypot(position.z - start.z));
            registry.apply_entity_physics_if_current_and_dispatch(
                tick,
                &queries,
                &[EntityPhysicsStep {
                    id: query.id,
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
        }
        assert!(
            furthest_horizontal > 0.5,
            "{species} never started swimming without collision-history admission"
        );
    }
}
