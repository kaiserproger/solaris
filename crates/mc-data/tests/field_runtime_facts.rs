use mc_data::block_mining::{destroy_progress_per_tick, fallback_mining_facts};

#[test]
fn hand_breaking_leaves_finishes_at_the_vanilla_client_stop() {
    let mining = fallback_mining_facts("oak_leaves");
    let progress = destroy_progress_per_tick(
        mining.destroy_speed,
        1.0,
        !mining.requires_correct_tool_for_drops,
        true,
        false,
    );
    // 26.1.2 Blocks.leavesProperties: strength 0.2, no tool requirement.
    assert!(progress * 5.0 < 1.0);
    assert!(progress * 6.0 >= 1.0);
}

#[test]
fn squid_has_ten_health_before_the_first_hit() {
    let registry = mc_data::entity_types::solaris_required_entity_types();
    let squid = registry
        .facts_of(&mc_data::Identifier::parse("minecraft:squid").unwrap())
        .unwrap();
    // 26.1.2 Squid.createAttributes sets MAX_HEALTH to 10, not the Mob default 20.
    assert_eq!(squid.attributes.max_health, Some(10.0));
}

#[test]
fn overworld_timeline_tag_activates_the_day_cycle() {
    let items = mc_data::items::solaris_required_items();
    let blocks = mc_data::blocks::solaris_required_blocks_report();
    let tags = mc_data::tags::solaris_required_client_tags(&items, &blocks);
    let data = mc_data::solaris_required_data();
    let timeline = data.registry("timeline").unwrap();
    let id = |name| mc_data::Identifier::parse(name).unwrap();
    let timeline_tags = &tags.registries[&id("minecraft:timeline")];
    let members = &timeline_tags[&id("minecraft:in_overworld")];
    let names = members
        .iter()
        .map(|&index| timeline.entries[index as usize].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "minecraft:villager_schedule",
            "minecraft:day",
            "minecraft:moon",
            "minecraft:early_game",
        ]
    );
}

#[test]
fn report_food_eligibility_requires_consumable_and_preserves_always_eat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("golden_apple.json"),
        r#"{"components":{"minecraft:food":{"nutrition":4,"saturation":9.6,"can_always_eat":true},"minecraft:consumable":{}}}"#,
    ).unwrap();
    std::fs::write(
        dir.path().join("cod_bucket.json"),
        r#"{"components":{"minecraft:food":{"nutrition":2,"saturation":0.4}}}"#,
    )
    .unwrap();
    let facts = mc_data::item_components::load_item_facts(dir.path()).unwrap();
    let rule = |name| {
        mc_data::food::rule_for_item(
            &facts,
            &mc_data::Identifier::parse(name).unwrap(),
            mc_data::food::DEFAULT_USE_DURATION,
        )
    };
    assert!(rule("minecraft:golden_apple").unwrap().0.can_always_eat);
    assert!(rule("minecraft:cod_bucket").is_none());
}
