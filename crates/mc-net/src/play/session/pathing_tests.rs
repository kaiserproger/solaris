use super::*;

struct CollisionContactCase {
    name: &'static str,
    block: (i32, i32, i32, u32),
    position: Vec3,
    full_cube: bool,
}

fn vanilla_block_state_id(block_name: &str, properties: &[(&str, &str)]) -> u32 {
    let blocks = mc_data::blocks::solaris_required_blocks_report();
    let block = blocks
        .iter()
        .find(|block| block.id.as_str() == block_name)
        .unwrap_or_else(|| panic!("missing vanilla block {block_name}"));
    block
        .states
        .iter()
        .find(|state| {
            state.properties.len() == properties.len()
                && properties.iter().all(|(name, value)| {
                    state
                        .properties
                        .get(*name)
                        .is_some_and(|actual| actual == value)
                })
        })
        .map(|state| state.id)
        .unwrap_or_else(|| panic!("missing vanilla state for {block_name}"))
}

fn collision_contact_cases() -> [CollisionContactCase; 10] {
    let stone = vanilla_block_state_id("minecraft:stone", &[]);
    let slab = vanilla_block_state_id(
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let double_slab = vanilla_block_state_id(
        "minecraft:stone_slab",
        &[("type", "double"), ("waterlogged", "false")],
    );
    let stair = vanilla_block_state_id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    let fence = vanilla_block_state_id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );

    [
        CollisionContactCase {
            name: "full block",
            block: (1, 63, 1, stone),
            position: Vec3::new(1.5, 64.0, 1.5),
            full_cube: true,
        },
        CollisionContactCase {
            name: "bottom slab",
            block: (3, 63, 1, slab),
            position: Vec3::new(3.5, 63.5, 1.5),
            full_cube: false,
        },
        CollisionContactCase {
            name: "directional stair",
            block: (5, 63, 1, stair),
            position: Vec3::new(5.5, 64.0, 1.45),
            full_cube: false,
        },
        CollisionContactCase {
            name: "isolated fence",
            block: (7, 63, 1, fence),
            position: Vec3::new(7.5, 64.5, 1.5),
            full_cube: false,
        },
        CollisionContactCase {
            name: "table-backed double slab",
            block: (9, 63, 1, double_slab),
            position: Vec3::new(9.5, 64.0, 1.5),
            full_cube: true,
        },
        CollisionContactCase {
            name: "negative-Y full block",
            block: (1, -2, 1, stone),
            position: Vec3::new(1.5, -1.0, 1.5),
            full_cube: true,
        },
        CollisionContactCase {
            name: "negative-Y bottom slab",
            block: (3, -2, 1, slab),
            position: Vec3::new(3.5, -1.5, 1.5),
            full_cube: false,
        },
        CollisionContactCase {
            name: "negative-Y directional stair",
            block: (5, -2, 1, stair),
            position: Vec3::new(5.5, -1.0, 1.45),
            full_cube: false,
        },
        CollisionContactCase {
            name: "negative-Y isolated fence",
            block: (7, -2, 1, fence),
            position: Vec3::new(7.5, -0.5, 1.5),
            full_cube: false,
        },
        CollisionContactCase {
            name: "negative-Y table-backed double slab",
            block: (9, -2, 1, double_slab),
            position: Vec3::new(9.5, -1.0, 1.5),
            full_cube: true,
        },
    ]
}

fn with_collision_probe(
    test: impl FnOnce(&LoadedChunkPathingProbe<'_>, EntityId, &[CollisionContactCase]),
) {
    let cases = collision_contact_cases();
    let air = vanilla_block_state_id("minecraft:air", &[]);
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded vanilla blocks build a registry"),
    );
    let mut world = mc_world::WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for case in &cases {
        let (x, y, z, state) = case.block;
        let _ = chunk.set_block(x as u8, y, z as u8, BlockStateId(state));
    }
    world.insert_generated_chunk(chunk_pos, chunk).unwrap();

    let materials = mc_physics::BlockMaterialIds::new(air, None, None);
    let world_read = world.read_view();
    let snapshot = world_read.snapshot_chunks(&[chunk_pos]);
    let entity_id = EntityId(1);
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::from([entity_id]);
    let entity_aabbs = HashMap::from([(entity_id, mc_physics::Aabb::COW)]);
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        Some(LoadedTerrainPathingProbe::new(&snapshot, &materials)),
    );
    test(&probe, entity_id, &cases);
}

#[test]
fn matches_vanilla_support_contact_truth_table() {
    with_collision_probe(|probe, entity_id, cases| {
        for case in cases {
            let top = case.position.y;
            let support_limit_y = case.position.y + SUPPORT_CONTACT_DEPTH;
            let positive_y = top.is_sign_positive();
            let truth_table = [
                (
                    "body overlap beyond tolerance",
                    top - 2.0 * VOXEL_SHAPE_MERGE_TOLERANCE,
                    PathingProbeResult::Blocked,
                ),
                (
                    "body overlap within tolerance",
                    top - 0.5 * VOXEL_SHAPE_MERGE_TOLERANCE,
                    if case.full_cube {
                        PathingProbeResult::Blocked
                    } else {
                        PathingProbeResult::Walkable
                    },
                ),
                (
                    "top next_down",
                    top.next_down(),
                    if case.full_cube {
                        PathingProbeResult::Blocked
                    } else {
                        PathingProbeResult::Walkable
                    },
                ),
                ("top exact", top, PathingProbeResult::Walkable),
                ("top next_up", top.next_up(), PathingProbeResult::Walkable),
                (
                    "tolerance inside",
                    support_limit_y - 2.0 * VOXEL_SHAPE_MERGE_TOLERANCE,
                    PathingProbeResult::Walkable,
                ),
                (
                    "tolerance outside",
                    support_limit_y - 0.5 * VOXEL_SHAPE_MERGE_TOLERANCE,
                    if case.full_cube {
                        PathingProbeResult::Walkable
                    } else {
                        PathingProbeResult::Blocked
                    },
                ),
                (
                    "support limit next_down",
                    support_limit_y.next_down(),
                    if case.full_cube && positive_y {
                        PathingProbeResult::Walkable
                    } else {
                        PathingProbeResult::Blocked
                    },
                ),
                (
                    "support limit exact",
                    support_limit_y,
                    PathingProbeResult::Blocked,
                ),
                (
                    "support limit next_up",
                    support_limit_y.next_up(),
                    PathingProbeResult::Blocked,
                ),
            ];

            for (point, feet_y, expected) in truth_table {
                assert_eq!(
                    probe.can_entity_stand_at(
                        entity_id,
                        Vec3::new(case.position.x, feet_y, case.position.z),
                    ),
                    expected,
                    "{} at {point} ({feet_y:.17})",
                    case.name
                );
            }
        }
    });
}

#[test]
fn support_candidate_y_bounds_include_negative_one_and_a_half_block_shape() {
    let feet_y = (-0.5_f64 + SUPPORT_CONTACT_DEPTH).next_down();
    let (min_y, max_y) = LoadedChunkPathingProbe::support_candidate_y_bounds(feet_y, 1.5);

    assert!(min_y <= -2, "fence cell y=-2 must be scanned");
    assert!(max_y >= -2, "fence cell y=-2 must be scanned");
}

#[test]
fn collision_shape_identity_mismatch_falls_back_to_full_cube() {
    let slab = vanilla_block_state_id(
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let table = mc_data::collision_shapes::vanilla_collision_shapes();
    let canonical = canonical_pathing_state_fact(slab).expect("slab has pathing state facts");
    assert!(matches!(
        LoadedChunkPathingProbe::collision_shape_with_facts(slab, Some(canonical), table),
        PathingCollisionShape::Voxel(_)
    ));

    let renamed = CanonicalPathingStateFact {
        block: Identifier::parse("minecraft:renamed_stone_slab").unwrap(),
        properties: canonical.properties.clone(),
    };
    let custom = CanonicalPathingStateFact {
        block: Identifier::parse("solaris:stone_slab").unwrap(),
        properties: canonical.properties.clone(),
    };
    let wrong_properties = CanonicalPathingStateFact {
        block: canonical.block.clone(),
        properties: Box::from([
            ("type".to_string(), "top".to_string()),
            ("waterlogged".to_string(), "false".to_string()),
        ]),
    };
    let reordered_properties = CanonicalPathingStateFact {
        block: canonical.block.clone(),
        properties: canonical.properties.iter().cloned().rev().collect(),
    };

    for facts in [renamed, custom, wrong_properties, reordered_properties] {
        assert!(matches!(
            LoadedChunkPathingProbe::collision_shape_with_facts(slab, Some(&facts), table),
            PathingCollisionShape::FullCube
        ));
    }
    assert!(matches!(
        LoadedChunkPathingProbe::collision_shape_with_facts(slab, None, table),
        PathingCollisionShape::FullCube
    ));
}

#[test]
fn canonical_double_slab_is_a_strict_table_backed_full_cube() {
    let double_slab = vanilla_block_state_id(
        "minecraft:stone_slab",
        &[("type", "double"), ("waterlogged", "false")],
    );
    let table = mc_data::collision_shapes::vanilla_collision_shapes();
    let facts = canonical_pathing_state_fact(double_slab).expect("double slab has pathing facts");

    assert!(matches!(
        LoadedChunkPathingProbe::collision_shape_with_facts(double_slab, Some(facts), table),
        PathingCollisionShape::FullCube
    ));
}

#[test]
fn pathing_table_prewarm_materializes_canonical_state_facts() {
    assert!(prewarm_canonical_pathing_state_facts() > 0);
}

#[test]
fn unloaded_collision_contact_remains_fail_closed() {
    let active_chunks = HashSet::new();
    let terrain_pathing_entities = HashSet::new();
    let entity_aabbs = HashMap::new();
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        None,
    );

    assert_eq!(
        probe.can_entity_stand_at(EntityId(1), Vec3::new(0.5, 64.0, 0.5)),
        PathingProbeResult::Unloaded
    );
}

#[test]
fn nonfinite_collision_contact_remains_fail_closed() {
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::new();
    let entity_aabbs = HashMap::new();
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        None,
    );

    for position in [
        Vec3::new(f64::NAN, 64.0, 0.5),
        Vec3::new(0.5, f64::INFINITY, 0.5),
        Vec3::new(0.5, 64.0, f64::NEG_INFINITY),
    ] {
        assert_eq!(
            probe.can_entity_stand_at(EntityId(1), position),
            PathingProbeResult::Blocked
        );
    }
}
