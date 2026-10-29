use super::{Y, at, block, give, held, plant, put, setup, use_on};
use crate::boundary::BlockPos;
use crate::plants;

fn pos(x: i32, dy: i32, z: i32) -> BlockPos {
    BlockPos { x, y: Y + dy, z }
}

fn apply(state: &mut crate::tick::TickState, plan: plants::Plan) {
    assert!(
        plan.expects
            .iter()
            .all(|(pos, id)| state.world.block_at(pos.x, pos.y, pos.z) == Some(*id))
    );
    for (pos, id) in plan.writes {
        state.world.set_block(pos.x, pos.y, pos.z, id);
    }
    state.world.fold_overlay();
}

#[test]
fn s9_sniffer_crops_pop_when_farmland_is_removed() {
    for (name, props) in [
        ("torchflower_crop", vec![("age", "0")]),
        ("pitcher_crop", vec![("age", "0"), ("half", "lower")]),
    ] {
        let (mut state, _) = setup();
        plant(&mut state, 2, Y, 2, "farmland", &[("moisture", "7")]);
        plant(&mut state, 2, Y + 1, 2, name, &props);
        let air = state.registry.states.air;
        put(&mut state, 2, Y, 2, air);
        plants::neighbor_updates(&mut state, pos(2, 0, 2), &mut Vec::new(), &mut Vec::new());
        assert_eq!(at(&state, 2, Y + 1, 2), air, "{name}");
    }
}

#[test]
fn s9_propagules_pop_when_support_is_removed() {
    for hanging in [false, true] {
        let (mut state, _) = setup();
        let props = [
            ("age", "4"),
            ("hanging", if hanging { "true" } else { "false" }),
            ("stage", "0"),
            ("waterlogged", "false"),
        ];
        plant(&mut state, 2, Y + 1, 2, "mangrove_propagule", &props);
        let support_y = if hanging { Y + 2 } else { Y };
        let air = state.registry.states.air;
        put(&mut state, 2, support_y, 2, air);
        plants::neighbor_updates(
            &mut state,
            BlockPos {
                x: 2,
                y: support_y,
                z: 2,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(at(&state, 2, Y + 1, 2), air);
    }
}

#[test]
fn s9_floor_bell_pops_when_support_is_removed() {
    let (mut state, _) = setup();
    let stone = state.registry.states.stone;
    put(&mut state, 2, Y, 2, stone);
    plant(
        &mut state,
        2,
        Y + 1,
        2,
        "bell",
        &[
            ("attachment", "floor"),
            ("facing", "north"),
            ("powered", "false"),
        ],
    );
    let air = state.registry.states.air;
    put(&mut state, 2, Y, 2, air);
    plants::neighbor_updates(&mut state, pos(2, 0, 2), &mut Vec::new(), &mut Vec::new());
    assert_eq!(at(&state, 2, Y + 1, 2), air);
}

#[test]
fn s9_double_wall_bell_becomes_single_before_last_support_is_removed() {
    let (mut state, _) = setup();
    let stone = state.registry.states.stone;
    put(&mut state, 2, Y + 1, 1, stone);
    put(&mut state, 2, Y + 1, 3, stone);
    plant(
        &mut state,
        2,
        Y + 1,
        2,
        "bell",
        &[
            ("attachment", "double_wall"),
            ("facing", "north"),
            ("powered", "false"),
        ],
    );
    let air = state.registry.states.air;
    put(&mut state, 2, Y + 1, 1, air);
    plants::neighbor_updates(&mut state, pos(2, 1, 1), &mut Vec::new(), &mut Vec::new());
    assert_eq!(
        at(&state, 2, Y + 1, 2),
        block(
            &state,
            "bell",
            &[
                ("attachment", "single_wall"),
                ("facing", "south"),
                ("powered", "false")
            ]
        )
    );
    put(&mut state, 2, Y + 1, 3, air);
    plants::neighbor_updates(&mut state, pos(2, 1, 3), &mut Vec::new(), &mut Vec::new());
    assert_eq!(at(&state, 2, Y + 1, 2), air);
}

#[test]
fn columns_obey_height_and_support_and_cascade_once() {
    let (mut state, _) = setup();
    let sand = block(&state, "sand", &[]);
    put(&mut state, 2, Y, 2, sand);
    plant(&mut state, 2, Y + 1, 2, "cactus", &[("age", "0")]);
    for dy in 1..3 {
        let id = block(&state, "cactus", &[("age", "15")]);
        let plan = plants::random_growth(&state, pos(2, dy, 2), id, 0).expect("cactus growth");
        apply(&mut state, plan);
    }
    let top = at(&state, 2, Y + 3, 2);
    assert!(plants::random_growth(&state, pos(2, 3, 2), top, 0).is_none());
    let stone = state.registry.states.stone;
    put(&mut state, 3, Y + 1, 2, stone);
    assert!(!plants::can_stand(
        &state,
        at(&state, 2, Y + 1, 2),
        pos(2, 1, 2)
    ));
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    plants::neighbor_updates(&mut state, pos(3, 1, 2), &mut writes, &mut spawned);
    for dy in 1..=3 {
        assert_eq!(at(&state, 2, Y + dy, 2), state.registry.states.air);
    }
    let first = spawned.len();
    plants::neighbor_updates(&mut state, pos(3, 1, 2), &mut writes, &mut spawned);
    assert_eq!(spawned.len(), first, "no second loot credit");
}

#[test]
fn cane_needs_water_and_stops_at_three() {
    let (mut state, _) = setup();
    plant(&mut state, 2, Y + 1, 2, "sugar_cane", &[("age", "0")]);
    let id = at(&state, 2, Y + 1, 2);
    assert!(!plants::can_stand(&state, id, pos(2, 1, 2)));
    let water = state.registry.states.water_levels[0];
    put(&mut state, 3, Y, 2, water);
    assert!(plants::can_stand(&state, id, pos(2, 1, 2)));
    for dy in 1..3 {
        let id = block(&state, "sugar_cane", &[("age", "15")]);
        let plan = plants::random_growth(&state, pos(2, dy, 2), id, 0).unwrap();
        apply(&mut state, plan);
    }
    let id = at(&state, 2, Y + 3, 2);
    assert!(plants::random_growth(&state, pos(2, 3, 2), id, 0).is_none());
    let air = state.registry.states.air;
    put(&mut state, 3, Y, 2, air);
    plants::neighbor_updates(&mut state, pos(3, 0, 2), &mut Vec::new(), &mut Vec::new());
    assert_eq!(at(&state, 2, Y + 3, 2), air);
}

#[test]
fn cane_and_cactus_age_sixteen_random_ticks_before_extending() {
    // Vanilla SugarCaneBlock/CactusBlock.randomTick: ages 0..15 advance the
    // stalk's own `age`; only the 16th tick (age 15) adds the next segment.
    for (name, support) in [("sugar_cane", "sand"), ("cactus", "sand")] {
        let (mut state, _) = setup();
        let sand = block(&state, support, &[]);
        put(&mut state, 2, Y, 2, sand);
        if name == "sugar_cane" {
            let water = state.registry.states.water_levels[0];
            put(&mut state, 3, Y, 2, water);
        }
        plant(&mut state, 2, Y + 1, 2, name, &[("age", "0")]);
        for tick in 0..15 {
            let id = at(&state, 2, Y + 1, 2);
            let plan = plants::random_growth(&state, pos(2, 1, 2), id, 0).expect(name);
            apply(&mut state, plan);
            assert_eq!(
                at(&state, 2, Y + 2, 2),
                state.registry.states.air,
                "{name} tick {tick}: ageing never extends"
            );
        }
        assert_eq!(
            at(&state, 2, Y + 1, 2),
            block(&state, name, &[("age", "15")])
        );
        let id = at(&state, 2, Y + 1, 2);
        let plan = plants::random_growth(&state, pos(2, 1, 2), id, 0).expect(name);
        apply(&mut state, plan);
        assert_eq!(
            at(&state, 2, Y + 2, 2),
            block(&state, name, &[("age", "0")])
        );
        assert_eq!(
            at(&state, 2, Y + 1, 2),
            block(&state, name, &[("age", "0")])
        );
    }
}

#[test]
fn bamboo_seedling_and_terminal_height_are_native() {
    let (mut state, _) = setup();
    plant(&mut state, 2, Y + 1, 2, "bamboo_sapling", &[]);
    let id = at(&state, 2, Y + 1, 2);
    let plan = plants::random_growth(&state, pos(2, 1, 2), id, 0).unwrap();
    apply(&mut state, plan);
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, at(&state, 2, Y + 2, 2), "leaves"),
        Some("small")
    );
    for dy in 1..=16 {
        plant(
            &mut state,
            2,
            Y + dy,
            2,
            "bamboo",
            &[("age", "1"), ("leaves", "none"), ("stage", "0")],
        );
    }
    let id = at(&state, 2, Y + 16, 2);
    assert!(plants::random_growth(&state, pos(2, 16, 2), id, 0).is_none());
    assert_eq!(at(&state, 2, Y + 17, 2), state.registry.states.air);
}

#[test]
fn stems_fruit_and_return_to_mature_after_fruit_breaks() {
    let (mut state, _) = setup();
    plant(&mut state, 2, Y, 2, "farmland", &[("moisture", "7")]);
    plant(&mut state, 2, Y + 1, 2, "melon_stem", &[("age", "7")]);
    let id = at(&state, 2, Y + 1, 2);
    let plan = plants::random_growth(&state, pos(2, 1, 2), id, 0).unwrap();
    apply(&mut state, plan);
    assert_eq!(at(&state, 2, Y + 1, 1), block(&state, "melon", &[]));
    assert_eq!(
        at(&state, 2, Y + 1, 2),
        block(&state, "attached_melon_stem", &[("facing", "north")])
    );
    let air = state.registry.states.air;
    put(&mut state, 2, Y + 1, 1, air);
    plants::neighbor_updates(&mut state, pos(2, 1, 1), &mut Vec::new(), &mut Vec::new());
    assert_eq!(at(&state, 2, Y + 1, 2), id);
}

#[test]
fn extra_crops_and_cocoa_support() {
    let (mut state, _) = setup();
    plant(&mut state, 2, Y, 2, "soul_sand", &[]);
    plant(&mut state, 2, Y + 1, 2, "nether_wart", &[("age", "0")]);
    let id = at(&state, 2, Y + 1, 2);
    state.day_time = 18_000;
    let plan = plants::random_growth(&state, pos(2, 1, 2), id, 0).expect("wart ignores light");
    apply(&mut state, plan);
    assert_eq!(
        at(&state, 2, Y + 1, 2),
        block(&state, "nether_wart", &[("age", "1")])
    );
    assert!(plants::bonemeal(&state, pos(2, 1, 2), 0).is_none());
    plant(
        &mut state,
        3,
        Y + 1,
        2,
        "cocoa",
        &[("age", "0"), ("facing", "east")],
    );
    let cocoa = at(&state, 3, Y + 1, 2);
    assert!(!plants::can_stand(&state, cocoa, pos(3, 1, 2)));
    plant(&mut state, 4, Y + 1, 2, "jungle_log", &[("axis", "y")]);
    assert!(plants::can_stand(&state, cocoa, pos(3, 1, 2)));
    let plan = plants::bonemeal(&state, pos(3, 1, 2), 0).unwrap();
    apply(&mut state, plan);
    assert_eq!(
        crate::farming::prop(&state.registry.blocks, at(&state, 3, Y + 1, 2), "age"),
        Some("1")
    );
}

#[test]
fn all_old_tree_families_and_two_by_two_requirements() {
    for family in ["oak", "birch", "spruce", "jungle", "acacia", "dark_oak"] {
        let (mut state, _) = setup();
        let name = format!("{family}_sapling");
        plant(&mut state, 2, Y + 1, 2, &name, &[("stage", "1")]);
        if family == "dark_oak" {
            assert!(
                plants::bonemeal(&state, pos(2, 1, 2), 0).is_none(),
                "single dark oak"
            );
        }
        if matches!(family, "dark_oak" | "spruce" | "jungle") {
            for (x, z) in [(2, 3), (3, 2), (3, 3)] {
                plant(&mut state, x, Y + 1, z, &name, &[("stage", "1")]);
            }
        }
        let plan = plants::bonemeal(&state, pos(2, 1, 2), 0).expect("old tree family");
        assert!(
            plan.expects.iter().any(|(pos, _)| pos.y > Y + 1),
            "room guarded"
        );
        apply(&mut state, plan);
        let log = block(&state, &format!("{family}_log"), &[("axis", "y")]);
        assert_eq!(at(&state, 2, Y + 1, 2), log);
        if matches!(family, "dark_oak" | "spruce" | "jungle") {
            assert_eq!(at(&state, 3, Y + 1, 3), log);
        }
    }
}

#[test]
fn blocked_tree_refuses_without_spending_bonemeal() {
    let (mut state, player) = setup();
    plant(&mut state, 1, Y + 1, 0, "spruce_sapling", &[("stage", "1")]);
    let stone = state.registry.states.stone;
    put(&mut state, 1, Y + 2, 0, stone);
    give(&mut state, player, "bone_meal", 2);
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(held(&state, player).count, 2);
    assert_eq!(at(&state, 1, Y + 2, 0), stone);
}

#[test]
fn bonemeal_tree_and_stem_use_whole_plan_and_one_item() {
    let (mut state, player) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    plant(&mut state, 1, Y + 1, 0, "birch_sapling", &[("stage", "1")]);
    give(&mut state, player, "bone_meal", 3);
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(
        at(&state, 1, Y + 1, 0),
        block(&state, "birch_log", &[("axis", "y")])
    );
    assert_eq!(held(&state, player).count, 2);
    let (mut state, player) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    plant(&mut state, 1, Y, 0, "farmland", &[("moisture", "7")]);
    plant(&mut state, 1, Y + 1, 0, "pumpkin_stem", &[("age", "6")]);
    give(&mut state, player, "bone_meal", 3);
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(held(&state, player).count, 2);
    assert_eq!(at(&state, 1, Y + 1, -1), block(&state, "pumpkin", &[]));
    assert_eq!(
        at(&state, 1, Y + 1, 0),
        block(&state, "attached_pumpkin_stem", &[("facing", "north")])
    );
}

#[test]
fn native_random_sampler_reaches_extra_plants_and_support_cascade() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), super::FAST.into());
    plant(&mut state, 1, Y, 0, "soul_sand", &[]);
    plant(&mut state, 1, Y + 1, 0, "nether_wart", &[("age", "0")]);
    // Vanilla advances one age per received random tick gated 1/10 (pinned
    // 26.1.2 NetherWartBlock bytecode: nextInt(10) == 0); at accelerated
    // speed the wart must mature within a generous bound, not on an exact
    // tick — the old exact-age-3-at-20 oracle over-fit deterministic RNG.
    let mature = block(&state, "nether_wart", &[("age", "3")]);
    let mut grown = false;
    for _ in 0..200 {
        super::run(&mut state, 1);
        if at(&state, 1, Y + 1, 0) == mature {
            grown = true;
            break;
        }
    }
    assert!(grown, "accelerated wart matures");
    super::dig(&mut state, 1, Y, 0);
    assert_eq!(at(&state, 1, Y + 1, 0), state.registry.states.air);
}

#[test]
fn chorus_extends_branches_stops_and_loses_support() {
    let (mut state, _) = setup();
    plant(&mut state, 4, Y, 4, "end_stone", &[]);
    plant(&mut state, 4, Y + 1, 4, "chorus_flower", &[("age", "0")]);
    for dy in 1..=4 {
        let at = pos(4, dy, 4);
        let id = state.world.block_at(at.x, at.y, at.z).unwrap();
        let plan = plants::random_growth(&state, at, id, 0).expect("chorus extends");
        apply(&mut state, plan);
    }
    let at = pos(4, 5, 4);
    let id = state.world.block_at(at.x, at.y, at.z).unwrap();
    let plan = plants::random_growth(&state, at, id, 0).expect("chorus branches");
    apply(&mut state, plan);
    assert_eq!(
        crate::farming::prop(
            &state.registry.blocks,
            super::at(&state, 3, Y + 5, 4),
            "age"
        ),
        Some("1")
    );
    plant(&mut state, 8, Y, 8, "end_stone", &[]);
    plant(&mut state, 8, Y + 1, 8, "chorus_flower", &[("age", "5")]);
    let dead = super::at(&state, 8, Y + 1, 8);
    assert!(plants::random_growth(&state, pos(8, 1, 8), dead, 0).is_none());
    assert!(plants::bonemeal(&state, pos(8, 1, 8), 0).is_none());
    let air = state.registry.states.air;
    put(&mut state, 4, Y, 4, air);
    plants::neighbor_updates(&mut state, pos(4, 0, 4), &mut Vec::new(), &mut Vec::new());
    for dy in 1..=5 {
        assert_eq!(super::at(&state, 4, Y + dy, 4), air);
    }
    assert_eq!(super::at(&state, 3, Y + 5, 4), air);
}

#[test]
fn native_random_sampler_grows_chorus_flowers() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), super::FAST.into());
    plant(&mut state, 4, Y, 4, "end_stone", &[]);
    plant(&mut state, 4, Y + 1, 4, "chorus_flower", &[("age", "0")]);
    super::run(&mut state, 20);
    let id = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(at(&state, 4, Y + 1, 4)))
        .unwrap();
    assert_eq!(id.block.id.path(), "chorus_plant");
}
