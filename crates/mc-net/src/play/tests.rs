use super::*;
use std::collections::BTreeMap;

use crate::play::chunk_stream::{hostile_chunk_spawns, passive_chunk_spawns, prioritized_spiral};
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::ItemReport;

fn props(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn player_pose_metadata_reports_swimming_and_shared_flags() {
    let mut pose = PlayerPose::new(0.5, 62.0, 0.5);
    pose.in_water = true;
    pose.swimming = true;
    pose.sprinting = true;

    assert_eq!(pose.entity_pose(), EntityPose::Swimming);
    assert_eq!(pose.shared_flags() & 0x08, 0x08);
}

fn state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: props(properties),
    }
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, true, &[])],
    }
}

fn prop_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(key, values)| {
            (
                (*key).to_string(),
                values.iter().map(|value| (*value).to_string()).collect(),
            )
        })
        .collect()
}

fn crop_test_reports() -> Vec<BlockReport> {
    let mut farmland_properties = BTreeMap::new();
    farmland_properties.insert(
        "moisture".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );
    let mut wheat_properties = BTreeMap::new();
    wheat_properties.insert(
        "age".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );

    let mut reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        simple_block(2, "minecraft:water"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: farmland_properties,
            states: (0..=7)
                .map(|moisture| {
                    state(
                        3 + moisture,
                        moisture == 0,
                        &[("moisture", &moisture.to_string())],
                    )
                })
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            properties: wheat_properties,
            states: (0..=7)
                .map(|age| state(11 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
    ];
    reports.sort_by_key(|block| block.states.first().map(|state| state.id).unwrap_or(0));
    reports
}

fn crop_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&crop_test_reports()).unwrap()
}

fn fluid_block(first_id: u32, name: &str, max_level: u8) -> BlockReport {
    let mut properties = BTreeMap::new();
    properties.insert(
        "level".to_string(),
        (0..=max_level).map(|level| level.to_string()).collect(),
    );
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties,
        states: (0..=max_level)
            .map(|level| {
                state(
                    first_id + u32::from(level),
                    level == 0,
                    &[("level", &level.to_string())],
                )
            })
            .collect(),
    }
}

fn fluid_test_reports() -> Vec<BlockReport> {
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        fluid_block(2, "minecraft:water", 7),
        fluid_block(10, "minecraft:lava", 3),
        simple_block(14, "minecraft:obsidian"),
        simple_block(15, "minecraft:cobblestone"),
    ]
}

fn fluid_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&fluid_test_reports()).unwrap()
}

fn fluid_test_facts() -> mc_data::block_facts::BlockFactsTable {
    mc_data::block_facts::BlockFactsTable::from_blocks_report(&fluid_test_reports())
}

#[test]
fn entity_tick_cadence_matches_vanilla_cow_tracking() {
    assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
    assert_eq!(mc_physics::TICK_SECONDS, 0.05);
    assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 1);
}

#[test]
fn gamemode_command_parses_names_and_numeric_modes() {
    assert_eq!(
        parse_gamemode_command("gamemode survival"),
        Some(GameMode::Survival)
    );
    assert_eq!(
        parse_gamemode_command("gamemode creative"),
        Some(GameMode::Creative)
    );
    assert_eq!(
        parse_gamemode_command("gamemode adventure"),
        Some(GameMode::Adventure)
    );
    assert_eq!(
        parse_gamemode_command("gamemode spectator"),
        Some(GameMode::Spectator)
    );
    assert_eq!(
        parse_gamemode_command("gamemode 1"),
        Some(GameMode::Creative)
    );
}

#[test]
fn gamemode_command_rejects_unknown_or_extra_args() {
    assert_eq!(parse_gamemode_command("time set day"), None);
    assert_eq!(parse_gamemode_command("gamemode nope"), None);
    assert_eq!(parse_gamemode_command("gamemode creative other"), None);
}

#[test]
fn client_view_distance_is_clamped_to_server_policy() {
    assert_eq!(clamp_client_view_distance(12, 8), 8);
    assert_eq!(clamp_client_view_distance(6, 10), 6);
    assert_eq!(clamp_client_view_distance(0, 10), 2);
    assert_eq!(clamp_client_view_distance(-8, 1), 2);
}

#[test]
fn debug_commands_parse_survival_mutations_and_give() {
    assert_eq!(
        parse_debug_command("debug survival damage 7.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Damage(7.5)))
    );
    assert_eq!(
        parse_debug_command("debug survival heal"),
        Some(DebugCommand::Survival(SurvivalCommand::Heal(20.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival feed 2 0.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Feed {
            food: 2,
            saturation: 0.5
        }))
    );
    assert_eq!(
        parse_debug_command("debug survival exhaust 4"),
        Some(DebugCommand::Survival(SurvivalCommand::Exhaust(4.0)))
    );
    assert_eq!(
        parse_debug_command("debug give minecraft:dirt 64 1"),
        Some(DebugCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 64,
            hotbar_slot: 1,
        })
    );
    assert_eq!(parse_debug_command("damage 7.5"), None);
    assert_eq!(parse_debug_command("debug survival damage bad"), None);
}

#[test]
fn admin_dispatcher_parses_slash_commands_and_permissions() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    assert_eq!(
        parse_admin_command("/gamemode creative", op),
        Ok(AdminCommand::GameMode(GameMode::Creative))
    );
    assert_eq!(
        parse_admin_command("give minecraft:dirt 12", op),
        Ok(AdminCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 12,
        })
    );
    assert_eq!(
        parse_admin_command("/tp 1.5 70 -2", op),
        Ok(AdminCommand::Teleport {
            x: 1.5,
            y: 70.0,
            z: -2.0,
        })
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie", op),
        Ok(AdminCommand::Summon {
            entity: mc_data::Identifier::parse("minecraft:zombie").unwrap(),
            x: None,
            y: None,
            z: None,
        })
    );
    assert_eq!(parse_admin_command("/kill", op), Ok(AdminCommand::Kill));
    assert_eq!(
        parse_admin_command("/gamemode creative", not_op),
        Err(CommandError::PermissionDenied)
    );
    assert_eq!(
        parse_admin_command("/gamemode", op),
        Err(CommandError::Usage(
            "Usage: /gamemode <survival|creative|adventure|spectator>"
        ))
    );
    assert_eq!(
        parse_admin_command("/doesnotexist", op),
        Err(CommandError::Unknown)
    );
}

#[test]
fn command_tree_and_suggestions_are_permission_aware() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    let tree = command_tree_packet(op);
    assert_eq!(tree.root_index, 0);
    assert_eq!(
        tree.nodes[0].children,
        vec![1, 6, 8, 10, 11, 12, 13, 15, 17]
    );
    assert_eq!(
        command_tree_packet(not_op).nodes[0].children,
        Vec::<i32>::new()
    );

    let root = command_suggestions("/g", op);
    assert_eq!(root.start, 1);
    assert_eq!(root.length, 1);
    assert_eq!(
        root.suggestions,
        vec!["gamemode".to_string(), "give".to_string()]
    );

    let modes = command_suggestions("/gamemode c", op);
    assert_eq!(modes.start, 10);
    assert_eq!(modes.length, 1);
    assert_eq!(modes.suggestions, vec!["creative".to_string()]);

    assert!(command_suggestions("/g", not_op).suggestions.is_empty());
}

#[test]
fn local_dev_profiles_are_op_capable_for_now() {
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "op_probe".to_string(),
    };

    let permissions = crate::server::CommandPermissionConfig::new(Vec::<String>::new(), true)
        .permissions_for(&profile);

    assert!(permissions.can_change_game_mode());
    assert!(permissions.can_use_admin_commands());
}

#[test]
fn item_to_block_table_is_registry_derived() {
    use std::collections::BTreeMap;

    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 42,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 43,
        },
    ]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ])
    .unwrap();

    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.resolve(42), Some(mc_world::BlockStateId(1)));
    assert_eq!(table.resolve(43), None);
}

#[test]
fn wheat_seeds_place_wheat_on_farmland_only() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 50,
    }]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let farmland = Identifier::parse("minecraft:farmland").unwrap();
    let farmland_state = blocks
        .by_name_and_props(&farmland, &[("moisture".to_string(), "0".to_string())])
        .unwrap();
    let dirt_state = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(table.resolve(50), None);
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(11))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::North, &blocks),
        None
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, dirt_state, Direction::Up, &blocks),
        None
    );
}

#[test]
fn sign_items_choose_floor_or_wall_sign_for_clicked_face() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:oak_sign").unwrap(),
        protocol_id: 70,
    }]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0"])]),
            states: vec![state(1, true, &[("rotation", "0")])],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(2, true, &[("facing", "north")])],
        },
    ])
    .unwrap();

    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Up,
            &blocks,
        ),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::North,
            &blocks,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Down,
            &blocks,
        ),
        None
    );
}

#[test]
fn bucket_items_resolve_fluid_sources() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:bucket").unwrap(),
            protocol_id: 60,
        },
        ItemReport {
            id: Identifier::parse("minecraft:water_bucket").unwrap(),
            protocol_id: 61,
        },
        ItemReport {
            id: Identifier::parse("minecraft:lava_bucket").unwrap(),
            protocol_id: 62,
        },
    ]);
    let blocks = fluid_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.empty_bucket_item(), Some(60));
    assert_eq!(table.bucket_fluid_kind(61), Some(FluidKind::Water));
    assert_eq!(table.bucket_fluid_kind(62), Some(FluidKind::Lava));
    assert_eq!(
        table.fluid_source_state(FluidKind::Water),
        Some(BlockStateId(2))
    );
    assert_eq!(
        table.fluid_source_state(FluidKind::Lava),
        Some(BlockStateId(10))
    );
}

#[test]
fn bucket_replacement_updates_single_held_stack_only() {
    let mut inventory = PlayerInventory::empty();
    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 61,
            count: 1,
            damage: None,
        },
    );

    let (next, changed) = plan_bucket_replacement(&inventory, 0, 60, 16).unwrap();

    assert_eq!(next.held(0).item_id, 60);
    assert_eq!(next.held(0).count, 1);
    assert_eq!(
        changed,
        vec![(PlayerInventory::HOTBAR_BASE, next.held(0).clone())]
    );

    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 60,
            count: 2,
            damage: None,
        },
    );
    assert!(plan_bucket_replacement(&inventory, 0, 61, 1).is_none());
}

#[test]
fn fluid_tick_flows_sideways_when_blocked_below() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(edits.len(), 4);
    assert!(edits.iter().all(|edit| edit.new_state == BlockStateId(3)));
}

#[test]
fn unsupported_flow_decays_to_air() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(4)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        pos,
        BlockStateId(4),
        facts.fluid(4).unwrap(),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0)
        }]
    );
}

#[test]
fn scheduling_fluid_edits_uses_current_tick_delay() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    schedule_fluid_ticks_near_applied(
        &mut world,
        &facts,
        100,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    );

    let ticks = world.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[test]
fn water_lava_interactions_make_solid_blocks() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let water_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let lava_source_pos = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(water_pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(lava_source_pos, BlockStateId(10))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        water_pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_source_pos,
            new_state: BlockStateId(14),
        }]
    );

    world
        .set_block_at(lava_source_pos, BlockStateId(0))
        .unwrap();
    let lava_flow_pos = mc_world::BlockPos { x: 4, y: 63, z: 4 };
    world.set_block_at(lava_flow_pos, BlockStateId(11)).unwrap();
    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        lava_flow_pos,
        BlockStateId(11),
        facts.fluid(11).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_flow_pos,
            new_state: BlockStateId(1),
        }]
    );
}

#[test]
fn wheat_random_tick_advances_age_until_mature() {
    let blocks = crop_test_registry();

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(11)),
        Some(mc_world::BlockStateId(12))
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(17)),
        Some(mc_world::BlockStateId(18))
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(18)),
        None
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(1)),
        None
    );
}

#[test]
fn farmland_trample_requires_landing_on_block() {
    let old_pose = PlayerPose::new(2.7, 3.0, -1.2);
    let landed = PlayerPose {
        y: 1.0,
        flags: MovePlayerFlags::new(true, false),
        ..old_pose
    };
    let hovering = PlayerPose {
        flags: MovePlayerFlags::new(false, false),
        ..landed
    };

    assert_eq!(
        farmland_trample_pos(old_pose, landed),
        Some(mc_world::BlockPos { x: 2, y: 0, z: -2 })
    );
    assert_eq!(farmland_trample_pos(old_pose, hovering), None);
    assert_eq!(farmland_trample_pos(landed, landed), None);
}

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
fn interactive_toggle_helpers_preserve_other_properties() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_trapdoor").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("open", &["false", "true"]),
                ("waterlogged", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("open", "false"),
                        ("waterlogged", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("open", "true"),
                        ("waterlogged", "false"),
                    ],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("facing", &["north"]), ("powered", &["false", "true"])]),
            states: vec![
                state(3, true, &[("facing", "north"), ("powered", "false")]),
                state(4, false, &[("facing", "north"), ("powered", "true")]),
            ],
        },
    ])
    .unwrap();

    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            "open"
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            "powered"
        ),
        Some(mc_world::BlockStateId(4))
    );
}

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
}

#[test]
fn bed_respawn_pose_uses_block_above_bed() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
    ])
    .unwrap();
    let pose = bed_respawn_pose(
        mc_world::BlockPos { x: 3, y: 64, z: -2 },
        blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
    );

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, -1.5, 180.0));
}

#[test]
fn common_container_paper_cuts_resolve_to_existing_menus() {
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:furnace"),
        Some("Furnace")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:smoker"),
        Some("Smoker")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:blast_furnace"),
        Some("Blast Furnace")
    );
}

#[test]
fn hostile_melee_requires_moving_toward_player() {
    let hostile = |velocity: Vec3| ServerEntitySnapshot {
        id: mc_entity::EntityId(7),
        uuid: uuid::Uuid::nil(),
        type_id: 1,
        type_name: "minecraft:zombie".into(),
        position: Vec3::ZERO,
        rotation: mc_entity::Rotation::ZERO,
        velocity,
        on_ground: true,
        item_stack: None,
        experience_value: None,
    };
    let player = Vec3::new(1.0, 0.0, 0.0);

    assert!(hostile_can_melee_player(
        &hostile(Vec3::new(0.2, 0.0, 0.0)),
        player
    ));
    assert!(!hostile_can_melee_player(
        &hostile(Vec3::new(-0.2, 0.0, 0.0)),
        player
    ));
    assert!(!hostile_can_melee_player(&hostile(Vec3::ZERO), player));
}

#[test]
fn hostile_melee_reaches_player_one_block_above() {
    let hostile = ServerEntitySnapshot {
        id: mc_entity::EntityId(7),
        uuid: uuid::Uuid::nil(),
        type_id: 1,
        type_name: "minecraft:zombie".into(),
        position: Vec3::new(0.0, 64.0, 0.0),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::new(0.2, 0.0, 0.0),
        on_ground: true,
        item_stack: None,
        experience_value: None,
    };

    assert!(hostile_can_melee_player(
        &hostile,
        Vec3::new(1.0, 65.0, 0.0)
    ));
    assert!(!hostile_can_melee_player(
        &hostile,
        Vec3::new(1.0, 67.0, 0.0)
    ));
}

include!("tests/inventory_and_survival.rs");
include!("tests/spawning_and_world.rs");
