use std::sync::Arc;

use mc_physics::BlockSampler;

use super::*;

fn powder_snow_fixture(
    reports: Vec<mc_data::blocks::BlockReport>,
) -> (WorldStorage, BlockMaterialIds, u32) {
    let air = reports
        .iter()
        .find(|block| block.id.as_str() == "minecraft:air")
        .and_then(|block| block.states.first())
        .map(|state| state.id)
        .expect("air state");
    let powder_snow = reports
        .iter()
        .find(|block| block.id.as_str() == "minecraft:powder_snow")
        .and_then(|block| block.states.first())
        .map(|state| state.id)
        .expect("powder snow state");
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&blocks, &facts);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        position,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(powder_snow));
    let mut world = WorldStorage::in_memory(blocks);
    world.insert_generated_chunk(position, chunk).unwrap();
    (world, materials, powder_snow)
}

fn powder_snow_boxes(
    world: &mut WorldStorage,
    materials: &BlockMaterialIds,
    kind: play::EntityPhysicsKind,
    entity_bottom: f64,
    fall_distance: f64,
) -> Vec<[i16; 6]> {
    let query = play::EntityPhysicsQuery {
        id: mc_entity::EntityId(74),
        position: mc_entity::Vec3::new(8.5, entity_bottom, 8.5),
        velocity: mc_entity::Vec3::ZERO,
        aabb: mc_physics::Aabb::COW,
        on_ground: false,
        fall_distance,
        kind,
    };
    let input = sample_entity_physics_input(query, world, materials);
    let sampler = SampledPhysicsWorld::for_query(input.snapshot, query);
    let mut boxes = Vec::new();
    sampler.collision_boxes_at(8, 64, 8, &mut |collision_box| {
        boxes.push(collision_box.coordinates());
    });
    boxes
}

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
        fall_distance: 0.0,
        kind: play::EntityPhysicsKind::Living,
    };
    let input = sample_entity_physics_input(query, &mut world, &materials);
    let sampler = SampledPhysicsWorld::for_query(input.snapshot, input.query);
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

#[test]
fn sampled_physics_applies_vanilla_powder_snow_entity_context() {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let (mut world, materials, _) = powder_snow_fixture(reports);
    let full_cube = vec![[0, 0, 0, 4096, 4096, 4096]];

    assert!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::Living,
            65.0,
            0.0,
        )
        .is_empty(),
        "ordinary living entities sink into powder snow"
    );
    assert_eq!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::PowderSnowWalkableLiving,
            65.0,
            0.0,
        ),
        full_cube,
        "tagged mobs stand on powder snow from above"
    );
    assert!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::PowderSnowWalkableLiving,
            64.5,
            0.0,
        )
        .is_empty(),
        "tagged mobs already inside powder snow keep sinking"
    );
    assert_eq!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::FallingBlock,
            64.5,
            0.0,
        ),
        full_cube,
        "a short-falling block uses powder snow's full base shape"
    );
    assert_eq!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::FallingBlock,
            64.5,
            3.0,
        ),
        vec![[0, 0, 0, 4096, 3686, 4096]],
        "a long-falling block uses powder snow's 0.9F shape first"
    );
    assert_eq!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::Living,
            64.5,
            3.0,
        ),
        vec![[0, 0, 0, 4096, 3686, 4096]],
        "a long-falling ordinary entity also uses the 0.9F shape"
    );
}

#[test]
fn sampled_powder_snow_context_keeps_exact_state_fingerprint_fence() {
    let mut reports = mc_data::blocks::solaris_required_blocks_report();
    let powder_snow = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:powder_snow")
        .expect("powder snow report");
    powder_snow
        .properties
        .insert("solaris_test".to_string(), vec!["mismatch".to_string()]);
    powder_snow.states[0]
        .properties
        .insert("solaris_test".to_string(), "mismatch".to_string());
    let (mut world, materials, _) = powder_snow_fixture(reports);

    assert_eq!(
        powder_snow_boxes(
            &mut world,
            &materials,
            play::EntityPhysicsKind::Living,
            64.5,
            0.0,
        ),
        vec![[0, 0, 0, 4096, 4096, 4096]],
        "a fingerprint mismatch must use the conservative custom-state fallback"
    );
}
