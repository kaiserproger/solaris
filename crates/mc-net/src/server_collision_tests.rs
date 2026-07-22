use std::sync::Arc;

use mc_physics::BlockSampler;

use super::*;

#[test]
fn sampled_physics_uses_exact_pitcher_crop_collision_shape() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = reports
        .iter()
        .find(|block| block.id.as_str() == "minecraft:air")
        .and_then(|block| block.states.first())
        .map(|state| state.id)
        .expect("air state");
    let collision_shapes = mc_data::collision_shapes::vanilla_collision_shapes();
    let pitcher = reports
        .iter()
        .find(|block| block.id.as_str() == "minecraft:pitcher_crop")
        .and_then(|block| {
            block.states.iter().find(|state| {
                collision_shapes
                    .get(state.id)
                    .is_some_and(|shape| !shape.is_empty() && !shape.is_full_cube())
            })
        })
        .map(|state| state.id)
        .expect("partial pitcher crop state");
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        position,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(pitcher));
    let mut world = WorldStorage::in_memory(blocks);
    world.insert_generated_chunk(position, chunk).unwrap();
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(73),
        position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: true,
        kind: play::EntityPhysicsKind::Living,
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    let sampler = SampledPhysicsWorld {
        snapshot: input.snapshot,
    };
    let mut actual = Vec::new();
    sampler.collision_boxes_at(8, 64, 8, &mut |collision_box| {
        actual.push(collision_box.coordinates());
    });
    let expected = collision_shapes
        .get(pitcher)
        .expect("pitcher crop collision shape")
        .iter()
        .map(|collision_box| collision_box.coordinates())
        .collect::<Vec<_>>();

    assert!(
        !expected.is_empty(),
        "fixture must exercise a partial shape"
    );
    assert_ne!(
        expected,
        vec![[0, 0, 0, 4096, 4096, 4096]],
        "fixture must not be a full cube"
    );
    assert_eq!(actual, expected);
}
