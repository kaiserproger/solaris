use super::{
    BlockEdit, BlockReport, BlockStateId, Chunk, ChunkPos, Identifier, ItemRegistry, ItemReport,
    ItemStack, LeafDecayDropRolls, ambient_random_tick_edit_allowed, natural_leaf_decay_drops,
    next_fire_state, next_leaf_decay_state, prop_schema, random_tick_edit_seeded, simple_block,
    state,
};
use std::sync::Arc;

#[test]
fn natural_random_tick_helpers_cover_leaves_grass_and_fire() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_leaves").unwrap(),
            properties: prop_schema(&[
                ("distance", &["6", "7"]),
                ("persistent", &["false", "true"]),
            ]),
            states: vec![
                state(2, true, &[("distance", "7"), ("persistent", "false")]),
                state(3, false, &[("distance", "7"), ("persistent", "true")]),
                state(4, false, &[("distance", "6"), ("persistent", "false")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:fire").unwrap(),
            properties: prop_schema(&[("age", &["14", "15"])]),
            states: vec![
                state(5, true, &[("age", "14")]),
                state(6, false, &[("age", "15")]),
            ],
        },
        simple_block(7, "minecraft:grass_block"),
    ])
    .unwrap();

    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(2)),
        Some(mc_world::BlockStateId(0))
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(3)),
        None
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(4)),
        None
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(5)),
        Some(mc_world::BlockStateId(6))
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(6)),
        Some(mc_world::BlockStateId(0))
    );
}

#[test]
fn fire_random_tick_spreads_to_common_fuel() {
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
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let fire = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let fuel = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(fire, BlockStateId(1)).unwrap();
    world.set_block_at(fuel, BlockStateId(3)).unwrap();

    let edits = random_tick_edit_seeded(
        blocks.as_ref(),
        &facts,
        &world,
        fire,
        BlockStateId(1),
        mc_data::block_facts::RandomTickFamily::Fire,
        0,
    )
    .unwrap();

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: fire,
                new_state: BlockStateId(2),
            },
            BlockEdit {
                pos: fuel,
                new_state: BlockStateId(1),
            },
        ]
    );
}

#[test]
fn protected_zone_rejects_only_ambient_fire_target() {
    let source = mc_world::BlockPos { x: -1, y: 64, z: 0 };
    let protected = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "claim",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        mc_script::ScriptPosition::try_new(15.0, 319.0, 15.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![zone]);

    assert!(ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Fire,
        source,
        source,
        Some(&protection),
    ));
    assert!(!ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Fire,
        source,
        protected,
        Some(&protection),
    ));
    assert!(ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Crop,
        source,
        protected,
        Some(&protection),
    ));
}

#[test]
fn natural_leaf_decay_uses_vanilla_base_drop_pools() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_leaves"),
        simple_block(2, "minecraft:jungle_leaves"),
        simple_block(3, "minecraft:pale_oak_leaves"),
        simple_block(4, "minecraft:mangrove_leaves"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:oak_sapling").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:jungle_sapling").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: Identifier::parse("minecraft:pale_oak_sapling").unwrap(),
            protocol_id: 12,
        },
        ItemReport {
            id: Identifier::parse("minecraft:stick").unwrap(),
            protocol_id: 13,
        },
        ItemReport {
            id: Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 14,
        },
    ]);

    let all_pools = LeafDecayDropRolls {
        sapling: 0,
        stick: 0,
        apple: 0,
        stick_count: 2,
    };
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(1), all_pools),
        vec![
            ItemStack::new(10, 1),
            ItemStack::new(13, 2),
            ItemStack::new(14, 1),
        ]
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(2), all_pools),
        vec![ItemStack::new(11, 1), ItemStack::new(13, 2)],
        "jungle leaves use the rarer sapling pool and never drop apples"
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(3), all_pools),
        vec![ItemStack::new(12, 1), ItemStack::new(13, 2)],
        "pale oak leaves have no apple pool in the 26.1.2 table"
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(4), all_pools),
        vec![ItemStack::new(13, 2)],
        "mangrove leaves do not drop propagules through decay"
    );

    let boundary_misses = LeafDecayDropRolls {
        sapling: 25,
        stick: 20,
        apple: 5,
        stick_count: 1,
    };
    assert!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(2), boundary_misses).is_empty(),
        "vanilla chances are strict 2.5%, 2%, and 0.5% thresholds"
    );
}
