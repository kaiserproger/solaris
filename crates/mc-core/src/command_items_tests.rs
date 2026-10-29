use super::*;
use crate::{
    commands::{self, Command},
    inventory::PlayerInventory,
    registry::CoreRegistry,
};
use std::sync::LazyLock;

fn registry() -> &'static CoreRegistry {
    static REGISTRY: LazyLock<CoreRegistry> = LazyLock::new(|| CoreRegistry::load(None).unwrap());
    &REGISTRY
}
fn argument(command: &str) -> ItemArgument {
    match commands::parse(command, true).unwrap() {
        Command::Give { item, .. } | Command::DebugGive { item, .. } => item,
        other => panic!("unexpected command {other:?}"),
    }
}
#[test]
fn give_water_potion_component_is_typed_and_not_plain_empty_potion() {
    // Serverbound command strings omit the chat UI's leading slash.
    let item =
        argument(r#"give @s minecraft:potion[potion_contents={potion:"minecraft:water"}] 1"#);
    let stack = item.stack(&registry().items).unwrap();
    assert_eq!(
        stack.extended().potion.as_ref().unwrap().as_str(),
        "minecraft:water"
    );
    let mut inventory = PlayerInventory::default();
    commands::give(&registry().item_rules(), &mut inventory, &stack, 2);
    assert_eq!(
        inventory.slots.iter().map(|stack| stack.count).sum::<i32>(),
        2
    );
    assert!(
        inventory
            .slots
            .iter()
            .filter(|stack| !stack.is_empty())
            .all(|stack| stack.extended().potion.as_ref().unwrap().as_str() == "minecraft:water")
    );
}

#[test]
fn give_unbreakable_sword_preserves_component_in_inventory() {
    let item = argument("give @s diamond_sword[unbreakable={}]");
    let stack = item.stack(&registry().items).unwrap();
    let mut inventory = PlayerInventory::default();
    commands::give(&registry().item_rules(), &mut inventory, &stack, 1);
    let given = inventory
        .slots
        .iter()
        .find(|stack| !stack.is_empty())
        .unwrap();
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, given).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components") else {
        panic!("missing components");
    };
    assert_eq!(
        mc_data::item_nbt::field(components, "minecraft:unbreakable"),
        Some(&Tag::Compound(Vec::new()))
    );
    assert!(
        argument("give @s diamond_sword[unbreakable=1]")
            .stack(&registry().items)
            .is_err()
    );
}

#[test]
fn give_dyed_color_is_admitted_and_survives_inventory_and_durable_storage() {
    let item = argument("give @s minecraft:leather_boots[minecraft:dyed_color=16711935]");
    let stack = item.stack(&registry().items).unwrap();
    let mut inventory = PlayerInventory::default();
    commands::give(&registry().item_rules(), &mut inventory, &stack, 1);
    let given = inventory
        .slots
        .iter()
        .find(|stack| !stack.is_empty())
        .unwrap();
    let bytes = serde_json::to_vec(given).unwrap();
    let restored: ItemStack = serde_json::from_slice(&bytes).unwrap();
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &restored).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components") else {
        panic!("missing dyed component");
    };
    assert_eq!(
        mc_data::item_nbt::field(components, "minecraft:dyed_color"),
        Some(&Tag::Int(16_711_935))
    );
    for invalid in ["{}", "'magenta'", "[1.0f,0.0f]"] {
        assert!(
            argument(&format!("give @s leather_boots[dyed_color={invalid}]"))
                .stack(&registry().items)
                .is_err()
        );
    }
}

#[test]
fn give_all_new_26_3_components_survive_saved_inventory() {
    for (name, value) in [
        ("attack_animation", "{type:'stab',duration:9}"),
        ("interact_animation", "{}"),
        ("block_transformer", "'minecraft:axe'"),
        ("villager_food", "{nutrition:12}"),
        ("sulfur_cube_content", "'minecraft:stone'"),
        ("compostable", "{layers:1}"),
        ("cooking_fuel", "{burn_time:200,speed_multiplier:1.0f}"),
        ("brewing_fuel", "{uses:20,speed_multiplier:1.0f}"),
        (
            "mob_visibility",
            "{targeting_entity_types:'#minecraft:skeletons',visibility:0.5f}",
        ),
        ("provides_pottery_pattern", "'minecraft:angler'"),
        ("sign_text_front", "{messages:['A','B','C','D']}"),
        (
            "sign_text_back",
            "{messages:['a','b','c','d'],color:'red',has_glowing_text:true}",
        ),
        ("waxed", "{}"),
        ("cushion/color", "'red'"),
    ] {
        let item = argument(&format!("give @s stone[minecraft:{name}={value}]"));
        let stack = item.stack(&registry().items).unwrap();
        let mut inventory = PlayerInventory::default();
        commands::give(&registry().item_rules(), &mut inventory, &stack, 1);
        let given = inventory
            .slots
            .iter()
            .find(|stack| !stack.is_empty())
            .unwrap();
        let saved = serde_json::to_vec(given).unwrap();
        let restored: ItemStack = serde_json::from_slice(&saved).unwrap();
        let nbt = mc_data::item_nbt::stack_to_fields(&registry().items, &restored).unwrap();
        let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&nbt, "components") else {
            panic!("missing {name}")
        };
        assert_eq!(
            mc_data::item_nbt::field(components, &format!("minecraft:{name}")),
            Some(&item.components[0].1),
            "{name}"
        );
    }
}

#[test]
fn give_death_protection_admits_empty_actions_and_rejects_custom_actions() {
    for value in ["{}", "{death_effects:[]}"] {
        let stack = argument(&format!("give @s stick[death_protection={value}]"))
            .stack(&registry().items)
            .unwrap();
        let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &stack).unwrap();
        let restored = mc_data::item_nbt::stack_from_fields(&registry().items, &fields)
            .unwrap()
            .unwrap();
        assert_eq!(restored, stack);
    }
    let error = argument(
        "give @s stick[death_protection={death_effects:[{type:'minecraft:clear_all_effects'}]}]",
    )
    .stack(&registry().items)
    .unwrap_err();
    assert!(
        error.contains("nonempty custom death_protection"),
        "{error}"
    );
    for value in ["1", "{death_effects:1}"] {
        assert!(
            argument(&format!("give @s stick[death_protection={value}]"))
                .stack(&registry().items)
                .is_err()
        );
    }
}
#[test]
fn give_custom_name_accepts_snbt_text_and_spaces_with_other_components() {
    let item = argument(
        r#"give @s stone[custom_name={text:"A named stone",color:"gold",italic:false},damage=7] 1"#,
    );
    let stack = item.stack(&registry().items).unwrap();
    assert_eq!(stack.custom_name.as_deref(), Some("A named stone"));
    assert_eq!(stack.damage, Some(7));
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &stack).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components") else {
        panic!("missing components")
    };
    assert_eq!(
        mc_data::item_nbt::field(components, "minecraft:custom_name"),
        Some(&item.components[0].1)
    );
}
#[test]
fn give_enchantments_uses_native_map_and_registry_validation() {
    let stack = argument(
        r#"give @s diamond_sword[enchantments={"minecraft:sharpness":5,"minecraft:unbreaking":3}]"#,
    )
    .stack(&registry().items)
    .unwrap();
    assert_eq!(stack.enchantments.len(), 2);
    assert_eq!(stack.enchantments[0].id.as_str(), "minecraft:sharpness");
    assert_eq!(stack.enchantments[0].level, 5);
    assert!(
        argument(r#"give @s stone[enchantments={"minecraft:not_an_enchantment":1}]"#)
            .stack(&registry().items)
            .unwrap_err()
            .contains("registry entry")
    );
}
#[test]
fn unsupported_and_repeated_components_are_clear_command_errors() {
    for input in [
        "give @s stone[not_a_component=1]",
        "give @s stone[attribute_modifiers={}]",
        "clear @s *[not_a_component=1]",
    ] {
        assert!(
            matches!(commands::parse(input, true), Err(CommandError::Message(message)) if message.contains("Unsupported item component")),
            "{input}"
        );
    }
    for input in [
        "give @s stone[damage=1,!minecraft:damage]",
        "give @s stone[damage=1,damage=2]",
        "give @s stone[!damage=2]",
        "give @s stone[custom_name='unfinished]",
        "give @s stone[damage=1]extra",
    ] {
        assert!(commands::parse(input, true).is_err(), "{input}");
    }
}
#[test]
fn malformed_supported_components_fail_before_inventory_mutation() {
    for item in [
        r#"potion[potion_contents={potion:"minecraft:no_such_potion"}]"#,
        "stone[custom_name=7]",
        "stone[custom_name={unexpected:1}]",
        "stone[damage='wrong']",
        "stone[profile=7]",
        "stone[container=1]",
        "stone[rarity='wrong']",
    ] {
        let item = ItemArgument::parse(item).unwrap();
        assert!(item.stack(&registry().items).is_err(), "{item:?}");
    }
}
#[test]
fn debug_give_retains_components_when_setting_fixture_slot() {
    let item = argument(
        r#"debug give potion[potion_contents={potion:"minecraft:water"},custom_name='Fixture water'] 2 1"#,
    );
    let mut inventory = PlayerInventory::default();
    commands::debug_give(&mut inventory, item.stack(&registry().items).unwrap(), 2, 1);
    assert_eq!(inventory.slots[1].count, 2);
    assert_eq!(
        inventory.slots[1].custom_name.as_deref(),
        Some("Fixture water")
    );
    assert_eq!(
        inventory.slots[1]
            .extended()
            .potion
            .as_ref()
            .unwrap()
            .as_str(),
        "minecraft:water"
    );
}
#[test]
fn component_removals_survive_native_nbt_and_typed_edits_retire_old_removal() {
    let mut stack = argument("give @s potion[!potion_contents,!custom_name]")
        .stack(&registry().items)
        .unwrap();
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &stack).unwrap();
    let recovered = mc_data::item_nbt::stack_from_fields(&registry().items, &fields)
        .unwrap()
        .unwrap();
    assert_eq!(stack, recovered);
    assert_eq!(stack.extended().preserved_components.len(), 2);
    stack.custom_name = Some("A new name".into());
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &stack).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components") else {
        panic!("missing components")
    };
    assert!(mc_data::item_nbt::field(components, "!minecraft:custom_name").is_none());
    assert!(mc_data::item_nbt::field(components, "!minecraft:potion_contents").is_some());
}
#[test]
fn clear_component_predicate_counts_and_only_debits_matching_stack() {
    let rules = registry().item_rules();
    let water = argument(r#"give @s potion[potion_contents={potion:"minecraft:water"}]"#)
        .stack(rules.items)
        .unwrap();
    let poison = argument(r#"give @s potion[potion_contents={potion:"minecraft:poison"}]"#)
        .stack(rules.items)
        .unwrap();
    let mut inventory = PlayerInventory::default();
    inventory.slots[0] = water;
    inventory.slots[1] = poison.clone();
    let Command::Clear {
        item: Some(predicate),
        ..
    } = commands::parse(
        r#"clear @s potion[potion_contents={potion:"minecraft:water"}] 0"#,
        true,
    )
    .unwrap()
    else {
        panic!("missing predicate")
    };
    predicate.validate(&rules).unwrap();
    assert_eq!(
        commands::clear(
            &mut inventory,
            |stack| predicate.matches(&rules, stack),
            Some(0)
        ),
        1
    );
    assert!(!inventory.slots[0].is_empty());
    assert_eq!(
        commands::clear(
            &mut inventory,
            |stack| predicate.matches(&rules, stack),
            None
        ),
        1
    );
    assert!(inventory.slots[0].is_empty());
    assert_eq!(inventory.slots[1], poison);
}
#[test]
fn clear_wildcard_presence_negation_and_literal_text_equality() {
    let rules = registry().item_rules();
    let named = argument("give @s stone[custom_name='Named']")
        .stack(rules.items)
        .unwrap();
    for (input, expected) in [
        ("*[custom_name]", true),
        ("*[!custom_name]", false),
        ("stone[custom_name={text:'Named'}]", true),
        ("stone[!custom_name='Other']", true),
    ] {
        let predicate = ItemPredicate::parse(input).unwrap();
        predicate.validate(&rules).unwrap();
        assert_eq!(predicate.matches(&rules, &named), expected, "{input}");
    }
}

#[test]
fn clear_component_alternatives_are_or_with_comma_conditions_and() {
    let rules = registry().item_rules();
    let named = argument("give @s stone[custom_name='Named']")
        .stack(rules.items)
        .unwrap();
    let predicate =
        ItemPredicate::parse("*[damage=9|custom_name='Named',!potion_contents]").unwrap();
    predicate.validate(&rules).unwrap();
    assert!(predicate.matches(&rules, &named));
    assert!(!predicate.matches(&rules, &ItemStack::new(named.item_id, 1)));
    assert!(ItemPredicate::parse("*[custom_name,]").is_err());
    assert!(ItemArgument::parse("stone[damage=7,]").is_ok());
}

#[test]
fn give_lore_accepts_native_heterogeneous_snbt_text_list() {
    let item = argument("give @s stone[lore=['Literal line',{text:'Styled line',color:'gold'}]]");
    let stack = item.stack(&registry().items).unwrap();
    let fields = mc_data::item_nbt::stack_to_fields(&registry().items, &stack).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&fields, "components") else {
        panic!()
    };
    let Some(Tag::List(lines)) = mc_data::item_nbt::field(components, "minecraft:lore") else {
        panic!()
    };
    assert_eq!(lines.element_type, mc_nbt::tag_type::COMPOUND);
    assert_eq!(lines.elements.len(), 2);
    assert!(
        matches!(&lines.elements[0], Tag::Compound(fields) if fields == &vec![(String::new(), Tag::String("Literal line".into()))])
    );
    assert!(
        matches!(&lines.elements[1], Tag::Compound(fields) if mc_data::item_nbt::field(fields, "color") == Some(&Tag::String("gold".into())))
    );
}

#[test]
fn clear_item_tags_use_authoritative_registry_ids_and_unknown_tags_error() {
    let rules = registry().item_rules();
    let registry_id = Identifier::parse("minecraft:item").unwrap();
    let (tag, item_id) = rules.tags.registries[&registry_id]
        .iter()
        .find_map(|(tag, ids)| ids.iter().find(|id| **id > 0).map(|id| (tag, *id as u32)))
        .unwrap();
    let predicate = ItemPredicate::parse(&format!("#{tag}")).unwrap();
    predicate.validate(&rules).unwrap();
    assert!(predicate.matches(&rules, &ItemStack::new(item_id, 1)));
    assert!(
        ItemPredicate::parse("#minecraft:no_such_tag")
            .unwrap()
            .validate(&rules)
            .unwrap_err()
            .contains("Unknown item tag")
    );
}
