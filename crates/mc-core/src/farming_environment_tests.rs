use super::*;
use crate::farming_tests::{Y, block, put, setup};

fn pos(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

fn set_result(outcome: Option<Outcome>, at: BlockPos, expected: u32) {
    assert!(matches!(outcome, Some(Outcome::Set(edits)) if edits.contains(&(at, expected))));
}

#[test]
fn s9_torchflower_random_growth_and_bonemeal() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 1, 2);
    let farm = block(&state, "farmland", &[("moisture", "7")]);
    let young = block(&state, "torchflower_crop", &[("age", "0")]);
    let grown = block(&state, "torchflower_crop", &[("age", "1")]);
    put(&mut state, 2, Y, 2, farm);
    put(&mut state, at.x, at.y, at.z, young);
    assert_eq!(state.registry.farming.kind(young), Kind::Crop);
    assert!((0..100).any(|seed| matches!(crop(&state, at, young, &mut Roll::new(seed)), Some(Outcome::Set(edits)) if edits.contains(&(at, grown)))));
    assert_eq!(bone_meal_state(&state, at, 0), Some(grown));
    put(&mut state, at.x, at.y, at.z, grown);
    let flower = block(&state, "torchflower", &[]);
    assert_eq!(bone_meal_state(&state, at, 0), Some(flower));
    assert!((0..100).any(|seed| matches!(crop(&state, at, grown, &mut Roll::new(seed)), Some(Outcome::Set(edits)) if edits.contains(&(at, flower)))));
}

#[test]
fn s9_dry_farmland_under_torchflower_stays_farmland() {
    let (mut state, _) = setup();
    let at = pos(-5, Y, -5);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    let crop = block(&state, "torchflower_crop", &[("age", "0")]);
    put(&mut state, at.x, at.y, at.z, dry);
    put(&mut state, at.x, at.y + 1, at.z, crop);
    assert!(farmland(&state, at, dry).is_none());
}

#[test]
fn s9_dry_farmland_under_pitcher_stays_farmland() {
    let (mut state, _) = setup();
    let at = pos(-5, Y, -5);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    let crop = block(&state, "pitcher_crop", &[("age", "0"), ("half", "lower")]);
    put(&mut state, at.x, at.y, at.z, dry);
    put(&mut state, at.x, at.y + 1, at.z, crop);
    assert!(farmland(&state, at, dry).is_none());
}

#[test]
fn s9_pitcher_random_growth_creates_upper_half_at_age_three() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 1, 2);
    let farm = block(&state, "farmland", &[("moisture", "7")]);
    let young = block(&state, "pitcher_crop", &[("age", "2"), ("half", "lower")]);
    let lower = block(&state, "pitcher_crop", &[("age", "3"), ("half", "lower")]);
    let upper = block(&state, "pitcher_crop", &[("age", "3"), ("half", "upper")]);
    put(&mut state, 2, Y, 2, farm);
    put(&mut state, at.x, at.y, at.z, young);
    assert_eq!(state.registry.farming.kind(young), Kind::Plant);
    assert!((0..100).any(|seed| matches!(crop(&state, at, young, &mut Roll::new(seed)), Some(Outcome::Set(edits)) if edits.contains(&(at, lower)) && edits.contains(&(pos(at.x, at.y + 1, at.z), upper)))));
}

#[test]
fn gap_waterlogged_irrigation_and_exposed_rain_hydrate_farmland() {
    let (mut state, _) = setup();
    let at = pos(-5, Y, -5);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, at.x, at.y, at.z, dry);
    let slab = block(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "true")],
    );
    put(&mut state, -1, Y, -5, slab);
    set_result(farmland(&state, at, dry), at, wet);
    let air = state.registry.states.air;
    put(&mut state, -1, Y, -5, air);
    state.weather = (crate::commands::Weather::Rain, 1000);
    set_result(farmland(&state, at, dry), at, wet);
    let stone = state.registry.states.stone;
    put(&mut state, -5, Y + 2, -5, stone);
    set_result(farmland(&state, at, dry), at, state.registry.states.dirt);
}

#[test]
fn gap_covered_wet_farmland_becomes_dirt_on_neighbor_update() {
    let (mut state, _) = setup();
    let at = pos(2, Y, 2);
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, at.x, at.y, at.z, wet);
    let stone = state.registry.states.stone;
    put(&mut state, 2, Y + 1, 2, stone);
    crate::plants::neighbor_updates(
        &mut state,
        pos(2, Y + 1, 2),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(
        state.world.block_at(at.x, at.y, at.z),
        Some(state.registry.states.dirt)
    );
}

#[test]
fn gap_opaque_cover_kills_grass_but_glass_does_not() {
    let (mut state, _) = setup();
    let at = pos(-5, Y, -5);
    let grass_id = state.registry.states.grass_block;
    put(&mut state, at.x, at.y, at.z, grass_id);
    let stone = state.registry.states.stone;
    put(&mut state, -5, Y + 1, -5, stone);
    set_result(
        grass(&state, at, &mut Roll::new(0)),
        at,
        state.registry.states.dirt,
    );
    let glass = block(&state, "glass", &[]);
    put(&mut state, -5, Y + 1, -5, glass);
    assert!(
        !matches!(grass(&state, at, &mut Roll::new(0)), Some(Outcome::Set(edits)) if edits.contains(&(at, state.registry.states.dirt)))
    );
}

#[test]
fn gap_dark_grass_does_not_spread() {
    let (mut state, _) = setup();
    let at = pos(0, Y, 0);
    let grass_id = state.registry.states.grass_block;
    put(&mut state, 0, Y, 0, grass_id);
    let stone = state.registry.states.stone;
    for x in -2i32..=2 {
        for z in -2i32..=2 {
            for y in Y + 1..=Y + 3 {
                if x.abs() == 2 || z.abs() == 2 || y == Y + 3 {
                    state.world.set_block(x, y, z, stone);
                }
            }
        }
    }
    state.world.fold_overlay();
    assert_eq!(state.raw_brightness(0, Y + 1, 0), 0);
    for seed in 0..100 {
        assert!(grass(&state, at, &mut Roll::new(seed)).is_none());
    }
}

#[test]
fn gap_mature_stems_require_soil_and_growth_roll() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 1, 2);
    let farm = block(&state, "farmland", &[("moisture", "7")]);
    let stem = block(&state, "melon_stem", &[("age", "7")]);
    put(&mut state, 2, Y, 2, farm);
    put(&mut state, at.x, at.y, at.z, stem);
    assert!(crate::plants::random_growth(&state, at, stem, 1).is_none());
    let air = state.registry.states.air;
    for side in crate::plants::horizontal(at) {
        put(&mut state, side.x, side.y - 1, side.z, air);
    }
    assert!(crate::plants::random_growth(&state, at, stem, 0).is_none());
}

#[test]
fn gap_bamboo_bonemeal_extends_from_lower_stalk_and_refuses_obstruction() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 1, 2);
    let bamboo = block(
        &state,
        "bamboo",
        &[("age", "0"), ("leaves", "none"), ("stage", "0")],
    );
    put(&mut state, 2, Y + 1, 2, bamboo);
    put(&mut state, 2, Y + 2, 2, bamboo);
    let plan = crate::plants::bonemeal(&state, at, 0).expect("bamboo accepts bone meal");
    assert!(plan.writes.iter().any(|(at, _)| at.y == Y + 3));
    assert!(!plan.writes.iter().any(|(at, _)| at.y > Y + 4));
    let stone = state.registry.states.stone;
    put(&mut state, 2, Y + 3, 2, stone);
    assert!(crate::plants::bonemeal(&state, at, 0).is_none());
}

#[test]
fn gap_cherry_and_pale_oak_grow_with_correct_trunk_width() {
    for family in ["cherry", "pale_oak"] {
        let (mut state, _) = setup();
        let at = pos(2, Y + 1, 2);
        let sapling = block(&state, &format!("{family}_sapling"), &[("stage", "1")]);
        put(&mut state, at.x, at.y, at.z, sapling);
        if family == "pale_oak" {
            assert!(crate::plants::bonemeal(&state, at, 0).is_none());
            for (x, z) in [(2, 3), (3, 2), (3, 3)] {
                put(&mut state, x, Y + 1, z, sapling);
            }
        }
        let plan = crate::plants::bonemeal(&state, at, 0).expect("tree family supported");
        let log = block(&state, &format!("{family}_log"), &[("axis", "y")]);
        assert!(plan.writes.contains(&(at, log)));
        if family == "pale_oak" {
            assert!(plan.writes.contains(&(pos(3, Y + 1, 3), log)));
        }
    }
}

#[test]
fn gap_mycelium_spreads_its_own_family() {
    let (mut state, _) = setup();
    let at = pos(-5, Y, -5);
    let id = block(&state, "mycelium", &[("snowy", "false")]);
    put(&mut state, at.x, at.y, at.z, id);
    assert!(kind_of("mycelium").ticks());
    let edits = (0..100)
        .find_map(|seed| grass(&state, at, &mut Roll::new(seed)))
        .expect("spread");
    assert!(matches!(edits, Outcome::Set(cells) if cells.iter().all(|(_, grown)| *grown == id)));
}

#[test]
fn gap_kelp_and_cave_vines_extend_with_age_and_keep_old_body() {
    for (name, dy, body) in [
        ("kelp", 1, "kelp_plant"),
        ("cave_vines", -1, "cave_vines_plant"),
    ] {
        let (mut state, _) = setup();
        let at = pos(2, Y + 3, 2);
        let props = if name == "kelp" {
            vec![("age", "0")]
        } else {
            vec![("age", "0"), ("berries", "false")]
        };
        let id = block(&state, name, &props);
        put(&mut state, at.x, at.y, at.z, id);
        let target = pos(2, at.y + dy, 2);
        if name == "kelp" {
            let water = state.registry.states.water;
            put(&mut state, target.x, target.y, target.z, water);
        }
        let plan = (0..1000)
            .find_map(|seed| crate::plants::random_growth(&state, at, id, seed))
            .expect("random growth");
        assert!(plan.writes.iter().any(|(at, _)| *at == target));
        assert!(
            plan.writes
                .iter()
                .any(|(cell, id)| *cell == at && is_block(&state.registry.blocks, *id, body))
        );
        let terminal = with_prop(&state.registry.blocks, id, "age", "25").unwrap();
        put(&mut state, at.x, at.y, at.z, terminal);
        for seed in 0..32 {
            assert!(crate::plants::random_growth(&state, at, terminal, seed).is_none());
        }
    }
}

#[test]
fn gap_bone_meal_berries_tall_grass_and_roots_are_atomic_plans() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 2, 2);
    for (name, props, result, result_y) in [
        (
            "cave_vines",
            vec![("age", "0"), ("berries", "false")],
            "cave_vines",
            Y + 2,
        ),
        ("short_grass", vec![], "tall_grass", Y + 3),
        ("rooted_dirt", vec![], "hanging_roots", Y + 1),
    ] {
        let id = block(&state, name, &props);
        put(&mut state, at.x, at.y, at.z, id);
        let plan = crate::plants::bonemeal(&state, at, 0).expect("applicable bone meal");
        assert!(
            plan.writes
                .iter()
                .any(|(cell, id)| cell.y == result_y
                    && is_block(&state.registry.blocks, *id, result))
        );
        assert!(plan.expects.contains(&(at, id)));
    }
}

#[test]
fn gap_mushrooms_spread_on_mycelium_and_bone_meal_plans_huge_caps() {
    let (mut state, _) = setup();
    let at = pos(2, Y + 1, 2);
    let soil = block(&state, "mycelium", &[("snowy", "false")]);
    for x in 0..=4 {
        for z in 0..=4 {
            state.world.set_block(x, Y, z, soil);
        }
    }
    state.world.fold_overlay();
    for name in ["brown_mushroom", "red_mushroom"] {
        let id = block(&state, name, &[]);
        put(&mut state, at.x, at.y, at.z, id);
        assert!((0..1000).any(|seed| crate::plants::random_growth(&state, at, id, seed).is_some()));
        let plan = (0..100)
            .find_map(|seed| crate::plants::bonemeal(&state, at, seed))
            .expect("huge mushroom");
        assert!(plan.writes.iter().any(|(_, id)| is_block(
            &state.registry.blocks,
            *id,
            "mushroom_stem"
        )));
        assert!(plan.writes.len() < 100);
        let stone = state.registry.states.stone;
        put(&mut state, at.x, at.y + 1, at.z, stone);
        for seed in 0..20 {
            assert!(crate::plants::bonemeal(&state, at, seed).is_none());
        }
        let air = state.registry.states.air;
        put(&mut state, at.x, at.y + 1, at.z, air);
    }
}

#[test]
fn gap_trample_intent_respects_distance_and_stale_farmland() {
    let (mut state, player) = setup();
    let at = pos(2, Y, 2);
    let farm = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, at.x, at.y, at.z, farm);
    let landing = [2.5, f64::from(Y) + 0.9375, 2.5];
    assert!(trample_intent(&state, player, landing, 0.5).is_none());
    let intent = trample_intent(&state, player, landing, 2.0).expect("fall tramples");
    let stale = intent.clone();
    assert_eq!(state.merge(&mut vec![intent]), 1);
    assert!(
        state
            .merge_block_writes
            .contains(&(at, state.registry.states.dirt))
    );
    assert_eq!(state.merge(&mut vec![stale]), 0);
}

#[test]
fn gap_cultivation_awards_progress_and_discovery_only_on_accepted_use() {
    use mc_data::advancements::{
        AdvancementDefinition, AdvancementRewards, AdvancementTree, Criterion,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let (mut state, player) = setup();
    let mut tree = AdvancementTree::default();
    tree.definitions.insert("test:till".into(), AdvancementDefinition {
        criteria: BTreeMap::from([("till".into(), Criterion {
            trigger: "minecraft:item_used_on_block".into(),
            conditions: serde_json::json!({"location":[{"condition":"minecraft:location_check","predicate":{"block":{"blocks":"minecraft:farmland"}}}]}),
        })]),
        requirements: vec![vec!["till".into()]],
        rewards: AdvancementRewards { recipes: vec!["minecraft:crafting_table".into()], ..Default::default() },
        ..Default::default()
    });
    tree.rebuild_trigger_index();
    Arc::make_mut(&mut state.registry).advancements = Arc::new(tree);
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:wooden_hoe").unwrap())
        .unwrap();
    let held = mc_data::ItemStack::new(item, 1);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
    let at = pos(1, Y, 0);
    let intent = plan_use_intent(&state, player, Hand::Main, at, Face::Up, &held).unwrap();
    let stale = intent.clone();
    assert_eq!(state.merge(&mut vec![intent]), 1);
    assert_eq!(
        state
            .advancement_events
            .iter()
            .filter(|event| event.trigger == "minecraft:item_used_on_block")
            .count(),
        1
    );
    assert_eq!(state.merge(&mut vec![stale]), 0);
    crate::advancements::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert_eq!(
        state.players[&player].advancement_progress["test:till"]["done"],
        true
    );
    assert!(
        state.players[&player]
            .unlocked_recipes
            .contains("minecraft:crafting_table")
    );
}
