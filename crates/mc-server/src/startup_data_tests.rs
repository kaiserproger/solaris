use std::collections::BTreeMap;

use mc_data::Identifier;

use super::{
    load_effective_block_light, load_effective_block_mining, load_effective_item_facts,
    load_effective_loot, load_effective_protocol_data, load_effective_recipes, load_effective_tags,
    validate_recipe_result_stacks,
};
#[test]
fn effective_protocol_data_rejects_missing_sidecar_root() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing-vanilla");

    let err = match load_effective_protocol_data(Some(&missing)) {
        Ok(_) => panic!("missing sidecar root must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("reading vanilla sidecar directory metadata")
    );
}

#[test]
fn effective_protocol_data_rejects_mismatched_sidecar_version() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":999999}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION,
        ),
    )
    .unwrap();

    let err = match load_effective_protocol_data(Some(tmp.path())) {
        Ok(_) => panic!("mismatched sidecar version must fail before registry loading"),
        Err(err) => err,
    };

    assert!(
        format!("{err:#}").contains("protocol_version 999999 does not match"),
        "{err:#}"
    );
}

#[test]
fn effective_tags_reject_empty_vanilla_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("registries.json"), "{}").unwrap();
    let data = mc_data::VanillaData::from_registries("", vec![]);
    let items = mc_data::items::ItemRegistry::default();

    let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
        Ok(_) => panic!("empty vanilla tag sidecar must fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("vanilla tags"));
    assert!(err.to_string().contains("were empty"));
}

#[test]
fn effective_tags_attach_fuel_values_to_embedded_startup_data() {
    let items = mc_data::items::solaris_required_items();
    let blocks = mc_data::blocks::solaris_required_blocks_report();

    let effective =
        load_effective_tags(None, &mc_data::solaris_required_data(), &items, &blocks).unwrap();
    let oak_stairs = items
        .id_of(&Identifier::parse("minecraft:oak_stairs").unwrap())
        .unwrap();
    let warped_stairs = items
        .id_of(&Identifier::parse("minecraft:warped_stairs").unwrap())
        .unwrap();

    assert_eq!(effective.fuel_values().burn_duration(oak_stairs), Some(300));
    assert!(!effective.fuel_values().is_fuel(warped_stairs));
}

#[test]
fn effective_tags_reject_partial_fuel_membership_with_all_required_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(
        reports.join("registries.json"),
        r#"{
                "minecraft:block":{"entries":{"minecraft:stone":{"protocol_id":0}}},
                "minecraft:item":{"entries":{"minecraft:coal":{"protocol_id":10}}},
                "minecraft:entity_type":{"entries":{"minecraft:pig":{"protocol_id":1}}}
            }"#,
    )
    .unwrap();
    for (registry, entry) in [
        ("block", "minecraft:stone"),
        ("entity_type", "minecraft:pig"),
    ] {
        let root = tmp.path().join("data/minecraft/tags").join(registry);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("sample.json"),
            format!(r#"{{"values":["{entry}"]}}"#),
        )
        .unwrap();
    }
    let item_tags = tmp.path().join("data/minecraft/tags/item");
    std::fs::create_dir_all(&item_tags).unwrap();
    for tag in [
        "logs",
        "bamboo_blocks",
        "planks",
        "wooden_stairs",
        "wooden_slabs",
        "wooden_trapdoors",
        "wooden_pressure_plates",
        "wooden_shelves",
        "wooden_fences",
        "fence_gates",
        "banners",
        "signs",
        "hanging_signs",
        "wooden_doors",
        "boats",
        "wool",
        "wooden_buttons",
        "saplings",
        "wool_carpets",
        "non_flammable_wood",
    ] {
        let values = if tag == "logs" {
            r#"["minecraft:coal"]"#
        } else {
            "[]"
        };
        std::fs::write(
            item_tags.join(format!("{tag}.json")),
            format!(r#"{{"values":{values}}}"#),
        )
        .unwrap();
    }
    let items = mc_data::items::ItemRegistry::from_report(&[mc_data::items::ItemReport {
        id: Identifier::parse("minecraft:coal").unwrap(),
        protocol_id: 10,
    }]);

    let err = match load_effective_tags(
        Some(tmp.path()),
        &mc_data::VanillaData::from_registries("", vec![]),
        &items,
        &[],
    ) {
        Ok(_) => panic!("partial canonical fuel membership must fail startup"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("canonical 26.1.2 default set"));
}

#[test]
fn effective_tags_reject_missing_required_vanilla_tag_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(
        reports.join("registries.json"),
        r#"{
                "minecraft:item": {
                    "entries": {
                        "minecraft:apple": { "protocol_id": 5 }
                    }
                }
            }"#,
    )
    .unwrap();
    let tags_item = tmp
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    std::fs::create_dir_all(&tags_item).unwrap();
    std::fs::write(
        tags_item.join("food.json"),
        r#"{ "values": [ "minecraft:apple" ] }"#,
    )
    .unwrap();
    let data = mc_data::VanillaData::from_registries("", vec![]);
    let items = mc_data::items::ItemRegistry::default();

    let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
        Ok(_) => panic!("partial vanilla tag sidecar must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("missing required resolved entries")
    );
    assert!(err.to_string().contains("minecraft:block"));
}

#[test]
fn effective_tags_reject_required_tag_registries_without_protocol_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("registries.json"), "{}").unwrap();
    for (root, entry) in [
        ("block", "minecraft:stone"),
        ("item", "minecraft:apple"),
        ("entity_type", "minecraft:pig"),
    ] {
        let tags_root = tmp
            .path()
            .join("data")
            .join("minecraft")
            .join("tags")
            .join(root);
        std::fs::create_dir_all(&tags_root).unwrap();
        std::fs::write(
            tags_root.join("sample.json"),
            format!(r#"{{ "values": [ "{entry}" ] }}"#),
        )
        .unwrap();
    }
    let data = mc_data::VanillaData::from_registries("", vec![]);
    let items = mc_data::items::ItemRegistry::default();

    let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
        Ok(_) => panic!("unresolved required tag registries must fail"),
        Err(err) => err,
    };

    let message = format!("{err:#}");
    assert!(message.contains("required registry entry"));
    assert!(
        message.contains("minecraft:stone")
            || message.contains("minecraft:apple")
            || message.contains("minecraft:pig")
    );
}

#[test]
fn effective_block_light_requires_sidecar_file_when_vanilla_dir_is_set() {
    let tmp = tempfile::tempdir().unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];

    let err = match load_effective_block_light(Some(tmp.path()), &report) {
        Ok(_) => panic!("missing block_light.json must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("loading vanilla block-light table")
    );
    assert!(err.to_string().contains("block_light.json"));
}

#[test]
fn effective_item_facts_reject_missing_sidecar_report() {
    let tmp = tempfile::tempdir().unwrap();

    let error = match load_effective_item_facts(Some(tmp.path())) {
        Ok(_) => panic!("missing item component report must fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("were empty"));
    assert!(error.to_string().contains("components/item"));
}

#[test]
fn effective_block_mining_requires_sidecar_file_when_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }];

    let error = match load_effective_block_mining(Some(tmp.path()), &report) {
        Ok(_) => panic!("missing block_mining.json must fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("block-mining table"));
    assert!(error.to_string().contains("block_mining.json"));
}

#[test]
fn effective_block_mining_loads_matching_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(
        reports.join("block_mining.json"),
        format!(
            r#"{{"version":"{}","max_state_id":1,"entries":[[0.0,0],[1.5,1]]}}"#,
            mc_protocol::TARGET_RELEASE
        ),
    )
    .unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 1,
            default: true,
            properties: BTreeMap::new(),
        }],
    }];

    let effective = load_effective_block_mining(Some(tmp.path()), &report).unwrap();

    assert_eq!(
        effective.as_ref().and_then(|table| table.facts(1)),
        Some(mc_data::block_mining::BlockMiningFacts {
            destroy_speed: 1.5,
            requires_correct_tool_for_drops: true,
        })
    );
}

#[test]
fn effective_item_facts_load_sidecar_tool_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let items = tmp
        .path()
        .join("reports")
        .join("minecraft")
        .join("components")
        .join("item");
    std::fs::create_dir_all(&items).unwrap();
    std::fs::write(
        items.join("wooden_pickaxe.json"),
        r##"{
                "components": {
                    "minecraft:tool": {
                        "rules": [{
                            "blocks": "#minecraft:mineable/pickaxe",
                            "speed": 2.0,
                            "correct_for_drops": true
                        }]
                    }
                }
            }"##,
    )
    .unwrap();

    let effective = load_effective_item_facts(Some(tmp.path())).unwrap();

    let tool = effective
        .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
        .and_then(|facts| facts.tool.as_ref())
        .expect("tool facts");
    assert_eq!(tool.rules.len(), 1);
    assert_eq!(tool.rules[0].speed, Some(2.0));
}

#[test]
fn effective_block_light_rejects_sidecar_that_does_not_cover_blocks_report() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(
        reports.join("block_light.json"),
        r#"{"version":"26.1.2-test","max_state_id":0,"entries":[[0,0,1,0]]}"#,
    )
    .unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 1,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];

    let err = match load_effective_block_light(Some(tmp.path()), &report) {
        Ok(_) => panic!("stale block-light sidecar must fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("requires state id 1"));
}

#[test]
fn effective_block_light_rejects_wrong_target_version() {
    let tmp = tempfile::tempdir().unwrap();
    let reports = tmp.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(
        reports.join("block_light.json"),
        r#"{"version":"not-the-target","max_state_id":0,"entries":[[0,0,1,0]]}"#,
    )
    .unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];

    let err = match load_effective_block_light(Some(tmp.path()), &report) {
        Ok(_) => panic!("wrong block-light target version must fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("not-the-target"));
    assert!(err.to_string().contains(mc_protocol::TARGET_RELEASE));
}

#[test]
fn effective_loot_uses_embedded_fallback_without_sidecar() {
    let loot = load_effective_loot(None).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
}

#[test]
fn effective_loot_uses_simple_vanilla_sidecar_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp
        .path()
        .join("data")
        .join("minecraft")
        .join("loot_table")
        .join("blocks");
    std::fs::create_dir_all(&blocks).unwrap();
    std::fs::write(
        blocks.join("stone.json"),
        r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:diamond"
                }]
              }]
            }"#,
    )
    .unwrap();

    let loot = load_effective_loot(Some(tmp.path())).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:diamond").unwrap())
    );
    assert_eq!(
        loot.entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap())
            .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
        Some(vec![
            &Identifier::parse("minecraft:leather").unwrap(),
            &Identifier::parse("minecraft:beef").unwrap(),
        ])
    );
}

#[test]
fn effective_loot_completes_partial_entity_table_from_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let entities = tmp
        .path()
        .join("data")
        .join("minecraft")
        .join("loot_table")
        .join("entities");
    std::fs::create_dir_all(&entities).unwrap();
    std::fs::write(
        entities.join("sheep.json"),
        r#"{
              "pools": [
                {
                  "entries": [{
                    "type": "minecraft:item",
                    "functions": [{
                      "function": "minecraft:set_count",
                      "count": {
                        "type": "minecraft:uniform",
                        "min": 1.0,
                        "max": 2.0
                      }
                    }],
                    "name": "minecraft:mutton"
                  }]
                },
                {
                  "entries": [{
                    "type": "minecraft:loot_table",
                    "value": "minecraft:entities/sheep/white"
                  }]
                }
              ]
            }"#,
    )
    .unwrap();

    let loot = load_effective_loot(Some(tmp.path())).unwrap();

    assert_eq!(
        loot.entity_drop_stacks(&Identifier::parse("minecraft:sheep").unwrap())
            .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
        Some(vec![
            &Identifier::parse("minecraft:mutton").unwrap(),
            &Identifier::parse("minecraft:white_wool").unwrap(),
        ])
    );
}

#[test]
fn effective_loot_rejects_sidecar_with_no_simple_loot() {
    let tmp = tempfile::tempdir().unwrap();

    let err = match load_effective_loot(Some(tmp.path())) {
        Ok(_) => panic!("configured vanilla loot sidecar without usable drops must fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("vanilla loot tables"));
    assert!(err.to_string().contains("no supported simple drops"));
}

#[test]
fn effective_recipes_keep_embedded_display_ids_when_sidecar_is_present() {
    let tmp = tempfile::tempdir().unwrap();
    let recipes = tmp.path().join("data").join("minecraft").join("recipe");
    std::fs::create_dir_all(&recipes).unwrap();
    std::fs::write(
        recipes.join("oak_planks.json"),
        r#"{
              "type": "minecraft:crafting_shapeless",
              "category": "building",
              "ingredients": [{ "tag": "minecraft:oak_logs" }],
              "result": {
                "id": "minecraft:oak_planks",
                "count": 5
              }
            }"#,
    )
    .unwrap();
    std::fs::write(
        recipes.join("zz_sidecar_only.json"),
        r#"{
              "type": "minecraft:crafting_shapeless",
              "category": "misc",
              "ingredients": [{ "item": "minecraft:stick" }],
              "result": {
                "id": "minecraft:stick",
                "count": 1
              }
            }"#,
    )
    .unwrap();

    let recipes = load_effective_recipes(Some(tmp.path())).unwrap();
    let embedded = mc_data::recipes::solaris_required_recipes();
    let oak_planks_index = embedded
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        .unwrap();

    assert_eq!(recipes.len(), embedded.len() + 1);
    assert_eq!(
        recipes[..embedded.len()]
            .iter()
            .map(|recipe| &recipe.id)
            .collect::<Vec<_>>(),
        embedded.iter().map(|recipe| &recipe.id).collect::<Vec<_>>()
    );
    assert_eq!(recipes[oak_planks_index].result.count, 5);
    assert_eq!(
        recipes.last().unwrap().id.as_str(),
        "minecraft:zz_sidecar_only"
    );
}

#[test]
fn recipe_result_stack_validation_rejects_known_item_overflow() {
    let item = Identifier::parse("minecraft:test_item").unwrap();
    let facts = mc_data::item_components::ItemFactsTable::from_entries([(
        item.clone(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(16),
            ..Default::default()
        },
    )]);
    let recipe = mc_data::recipes::Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: mc_data::recipes::RecipeKind::Shapeless(mc_data::recipes::ShapelessRecipe {
            ingredients: vec![mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(item.clone())],
            }],
        }),
        result: mc_data::recipes::RecipeResult { item, count: 17 },
    };

    let error = validate_recipe_result_stacks(&[recipe], &facts).unwrap_err();
    assert!(error.to_string().contains("max stack size 16"));
}

#[test]
fn effective_recipes_reject_configured_sidecar_without_supported_recipes() {
    let tmp = tempfile::tempdir().unwrap();

    let err = match load_effective_recipes(Some(tmp.path())) {
        Ok(_) => panic!("configured vanilla recipe sidecar without recipes must fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("vanilla recipes"));
    assert!(err.to_string().contains("no supported recipes"));
}

#[test]
fn effective_recipes_use_embedded_fallback_without_sidecar() {
    let recipes = load_effective_recipes(None).unwrap();

    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
    );
}
