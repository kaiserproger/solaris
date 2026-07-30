use super::{
    AppliedBlockEdit, BlockEditBatchOutcome, BlockReport, CampfireCookingState, ChunkPos,
    Direction, Identifier, ItemRegistry, ItemReport, ItemStack, ListTag, PendingSignEdit,
    PlayerPose, Tag, campfire_block_entity_update_nbt, cursor_y_relative_to_target,
    door_half_state, horizontal_facing_from_yaw, in_memory_button_world, placed_sign_edit,
    plan_block_placement, prop_schema, sign_block_entity_update_nbt, sign_placement_state,
    simple_block, solaris_required_blocks_report, state,
};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn door_half_state_builds_two_block_placement_states() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_door").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north", "south"]),
                ("half", &["lower", "upper"]),
                ("open", &["false"]),
                ("powered", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    3,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    4,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
            ],
        },
    ])
    .unwrap();
    let default = blocks.by_id(mc_world::BlockStateId(1)).unwrap();

    assert_eq!(
        door_half_state(&blocks, default, "lower", "south"),
        Some(mc_world::BlockStateId(3))
    );
    assert_eq!(
        door_half_state(&blocks, default, "upper", "south"),
        Some(mc_world::BlockStateId(4))
    );
    assert_eq!(horizontal_facing_from_yaw(180.0), "north");
}

fn oriented_placement_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap())
}

fn oriented_placement_state(
    blocks: &mc_world::BlockRegistry,
    block: &str,
    properties: &[(&str, &str)],
) -> mc_world::BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(block).unwrap(),
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|| panic!("missing canonical state {block} {properties:?}"))
}

fn torch_placement_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stone"),
            BlockReport {
                id: Identifier::parse("minecraft:oak_fence").unwrap(),
                properties: prop_schema(&[
                    ("east", &["false"]),
                    ("north", &["false"]),
                    ("south", &["false"]),
                    ("west", &["false"]),
                    ("waterlogged", &["false"]),
                ]),
                states: vec![state(
                    2,
                    true,
                    &[
                        ("east", "false"),
                        ("north", "false"),
                        ("south", "false"),
                        ("west", "false"),
                        ("waterlogged", "false"),
                    ],
                )],
            },
            simple_block(3, "minecraft:torch"),
            BlockReport {
                id: Identifier::parse("minecraft:wall_torch").unwrap(),
                properties: prop_schema(&[("facing", &["north", "south", "west", "east"])]),
                states: vec![
                    state(4, true, &[("facing", "north")]),
                    state(5, false, &[("facing", "south")]),
                    state(6, false, &[("facing", "west")]),
                    state(7, false, &[("facing", "east")]),
                ],
            },
        ])
        .unwrap(),
    )
}

fn torch_support_pos(pos: mc_world::BlockPos, direction: Direction) -> mc_world::BlockPos {
    match direction {
        Direction::North => mc_world::BlockPos {
            z: pos.z + 1,
            ..pos
        },
        Direction::South => mc_world::BlockPos {
            z: pos.z - 1,
            ..pos
        },
        Direction::West => mc_world::BlockPos {
            x: pos.x + 1,
            ..pos
        },
        Direction::East => mc_world::BlockPos {
            x: pos.x - 1,
            ..pos
        },
        Direction::Up => mc_world::BlockPos {
            y: pos.y - 1,
            ..pos
        },
        Direction::Down => mc_world::BlockPos {
            y: pos.y + 1,
            ..pos
        },
    }
}

fn plan_torch_placement(
    blocks: Arc<mc_world::BlockRegistry>,
    support_state: mc_world::BlockStateId,
    direction: Direction,
) -> Option<crate::play::block_placement::PlannedBlockPlacement> {
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(torch_support_pos(pos, direction), support_state)
        .expect("set torch support")
        .expect("replace torch support");
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);

    plan_block_placement(
        &blocks,
        mc_world::BlockStateId(3),
        Some(&snapshot),
        pos,
        PlayerPose::new(0.5, 64.0, 0.5),
        direction,
        0.5,
        mc_world::BlockStateId(0),
    )
}

fn plan_oriented_test_placement(
    blocks: Arc<mc_world::BlockRegistry>,
    placed_state: mc_world::BlockStateId,
    yaw: f32,
    direction: mc_protocol::packets::play::Direction,
    target_relative_hit_y: f32,
) -> mc_world::BlockStateId {
    let world = in_memory_button_world(Arc::clone(&blocks));
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.yaw = yaw;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };

    plan_block_placement(
        &blocks,
        placed_state,
        Some(&snapshot),
        pos,
        pose,
        direction,
        target_relative_hit_y,
        mc_world::BlockStateId(0),
    )
    .expect("ordinary oriented block placement plans")
    .edits[0]
        .new_state
}

#[test]
fn stair_placement_uses_yaw_and_cursor_height_for_all_facings_and_halves() {
    let blocks = oriented_placement_test_registry();
    let held = blocks
        .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
        .unwrap()
        .default;

    for (yaw, facing) in [
        (0.0, "south"),
        (90.0, "west"),
        (180.0, "north"),
        (270.0, "east"),
    ] {
        for (cursor_y, half) in [(0.25, "bottom"), (0.75, "top")] {
            assert_eq!(
                plan_oriented_test_placement(
                    Arc::clone(&blocks),
                    held,
                    yaw,
                    mc_protocol::packets::play::Direction::East,
                    cursor_y,
                ),
                oriented_placement_state(
                    &blocks,
                    "minecraft:oak_stairs",
                    &[
                        ("facing", facing),
                        ("half", half),
                        ("shape", "straight"),
                        ("waterlogged", "false"),
                    ],
                ),
            );
        }
    }
}

#[test]
fn slab_placement_uses_clicked_face_and_cursor_height() {
    let blocks = oriented_placement_test_registry();
    let held = blocks
        .block(&Identifier::parse("minecraft:oak_slab").unwrap())
        .unwrap()
        .default;

    for (direction, cursor_y, expected_type) in [
        (mc_protocol::packets::play::Direction::Up, 0.75, "bottom"),
        (mc_protocol::packets::play::Direction::Down, 0.25, "top"),
        (mc_protocol::packets::play::Direction::East, 0.25, "bottom"),
        (mc_protocol::packets::play::Direction::East, 0.5, "bottom"),
        (mc_protocol::packets::play::Direction::East, 0.75, "top"),
    ] {
        assert_eq!(
            plan_oriented_test_placement(Arc::clone(&blocks), held, 0.0, direction, cursor_y,),
            oriented_placement_state(
                &blocks,
                "minecraft:oak_slab",
                &[("type", expected_type), ("waterlogged", "false")],
            ),
        );
    }
}

#[test]
fn torch_placement_uses_the_clicked_horizontal_face_for_wall_facing() {
    let blocks = torch_placement_test_registry();

    for (direction, expected_state) in [
        (Direction::North, 4),
        (Direction::South, 5),
        (Direction::West, 6),
        (Direction::East, 7),
    ] {
        let plan = plan_torch_placement(Arc::clone(&blocks), mc_world::BlockStateId(1), direction)
            .expect("full sturdy support permits wall torch placement");
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(
            plan.edits[0].new_state,
            mc_world::BlockStateId(expected_state)
        );
    }
}

#[test]
fn torch_placement_on_top_uses_the_standing_state() {
    let plan = plan_torch_placement(
        torch_placement_test_registry(),
        mc_world::BlockStateId(1),
        Direction::Up,
    )
    .expect("full sturdy top support permits standing torch placement");

    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.edits[0].new_state, mc_world::BlockStateId(3));
}

#[test]
fn torch_placement_rejects_non_full_support_faces() {
    let blocks = torch_placement_test_registry();

    assert!(plan_torch_placement(blocks, mc_world::BlockStateId(2), Direction::East).is_none());
}

#[test]
fn torch_placement_rejects_downward_faces() {
    assert!(
        plan_torch_placement(
            torch_placement_test_registry(),
            mc_world::BlockStateId(1),
            Direction::Down,
        )
        .is_none()
    );
}

#[test]
fn placement_cursor_height_is_relative_to_the_placed_target() {
    assert_eq!(cursor_y_relative_to_target(64, 64, 0.5), 0.5);
    assert_eq!(cursor_y_relative_to_target(64, 65, 1.0), 0.0);
    assert_eq!(cursor_y_relative_to_target(64, 63, 0.0), 1.0);
}

#[test]
fn noncanonical_stair_family_fails_closed() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:incomplete_stairs").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["north", "south"]),
                    ("half", &["bottom", "top"]),
                ]),
                states: vec![state(1, true, &[("facing", "north"), ("half", "bottom")])],
            },
        ])
        .unwrap(),
    );
    let world = in_memory_button_world(Arc::clone(&blocks));
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);

    assert!(
        plan_block_placement(
            &blocks,
            mc_world::BlockStateId(1),
            Some(&snapshot),
            mc_world::BlockPos { x: 1, y: 64, z: 1 },
            PlayerPose::new(0.5, 64.0, 0.5),
            mc_protocol::packets::play::Direction::East,
            0.75,
            mc_world::BlockStateId(0),
        )
        .is_none()
    );
}

#[test]
fn sign_placement_sets_wall_facing_and_floor_rotation() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0", "4"])]),
            states: vec![
                state(1, true, &[("rotation", "0")]),
                state(2, false, &[("rotation", "4")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north", "east"])]),
            states: vec![
                state(3, true, &[("facing", "north")]),
                state(4, false, &[("facing", "east")]),
            ],
        },
    ])
    .unwrap();
    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.yaw = 90.0;

    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            pose,
            Direction::Up,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            pose,
            Direction::East,
        ),
        Some(mc_world::BlockStateId(4))
    );

    assert_eq!(
        placed_sign_edit(
            &blocks,
            &BlockEditBatchOutcome {
                applied: vec![AppliedBlockEdit {
                    pos: mc_world::BlockPos { x: 1, y: 2, z: 3 },
                    previous: mc_world::BlockStateId(0),
                    new_state: mc_world::BlockStateId(2),
                }],
                resulting_tokens: HashMap::from([(
                    mc_world::BlockPos { x: 1, y: 2, z: 3 },
                    mc_world::BlockMutationToken {
                        chunk_instance_id: 7,
                        version: 11,
                    },
                )]),
                ..BlockEditBatchOutcome::default()
            },
        ),
        Some(PendingSignEdit {
            position: mc_world::BlockPos { x: 1, y: 2, z: 3 },
            state: mc_world::BlockStateId(2),
            token: mc_world::BlockMutationToken {
                chunk_instance_id: 7,
                version: 11,
            },
            is_front_text: true,
        })
    );
}

#[test]
fn sign_update_nbt_matches_vanilla_plain_text_shape() {
    let tag = sign_block_entity_update_nbt(
        &[
            "Hello".to_string(),
            "World".to_string(),
            String::new(),
            "!".to_string(),
        ],
        true,
    );

    assert_eq!(
        tag,
        Tag::Compound(vec![
            (
                "front_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String("Hello".into()),
                                Tag::String("World".into()),
                                Tag::String(String::new()),
                                Tag::String("!".into()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            (
                "back_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            ("is_waxed".into(), Tag::Byte(0)),
        ])
    );
}

#[test]
fn campfire_update_nbt_contains_visible_cooking_items_only() {
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: cooked_porkchop,
            protocol_id: 11,
        },
    ]);
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(10, 1), ItemStack::new(11, 1), 2));

    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(0)),
                    ("id".into(), Tag::String(porkchop.as_str().to_string())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        )]))
    );

    assert!(!cooking.tick().changed);
    let tick = cooking.tick();
    assert!(tick.changed);
    assert_eq!(tick.completed, vec![ItemStack::new(11, 1)]);
    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag::empty()),
        )]))
    );
}
