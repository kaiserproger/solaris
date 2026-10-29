use super::super::ItemStackWireCodec;
use super::*;
use mc_data::item_stack::PreservedItemComponent;

#[path = "copied_components_26_3_tests.rs"]
mod v263;

fn text(value: &str) -> Tag {
    Tag::String(value.into())
}
fn fields(values: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        values
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}
fn stack(name: &str, value: Tag) -> ItemStack {
    mc_data::item_nbt::stack_from_fields(
        items(),
        &[
            ("id".into(), text("minecraft:stone")),
            ("count".into(), Tag::Int(1)),
            ("components".into(), fields(vec![(name, value)])),
        ],
    )
    .unwrap()
    .unwrap()
}
fn fixture(name: &str, value: Tag, payload: Vec<u8>) {
    let stack = stack(name, value);
    let mut expected = Vec::new();
    expected.write_varint(1);
    expected.write_varint(registry_id("item", "minecraft:stone").unwrap());
    expected.write_varint(1);
    expected.write_varint(0);
    expected.write_varint(
        mc_data::data_component_types_26_1_2::id(&Identifier::parse(name).unwrap()).unwrap() as i32,
    );
    expected.extend(payload);
    let stored = Tag::Compound(mc_data::item_nbt::stack_to_fields(items(), &stack).unwrap());
    let bytes = nbt(&stored);
    let Tag::Compound(stored) = mc_nbt::read_network_value(&mut bytes.as_slice()).unwrap() else {
        panic!()
    };
    let durable = mc_data::item_nbt::stack_from_fields(items(), &stored)
        .unwrap()
        .unwrap();
    let mut actual = Vec::new();
    durable.encode(&mut actual).unwrap();
    assert_eq!(actual, expected, "{name} exact vanilla bytes");
    let mut cursor = actual.as_slice();
    let decoded = ItemStack::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty());
    // Native inventory snapshots serialize ItemStack; recreate it before the
    // second publication to catch loss after save/restart, not just on the wire.
    let saved = serde_json::to_vec(&decoded).unwrap();
    let restored: ItemStack = serde_json::from_slice(&saved).unwrap();
    let durable = mc_data::item_nbt::stack_to_fields(items(), &restored).unwrap();
    let restored = mc_data::item_nbt::stack_from_fields(items(), &durable)
        .unwrap()
        .unwrap();
    let mut again = Vec::new();
    restored.encode(&mut again).unwrap();
    assert_eq!(again, expected, "{name} decode/save/restart/re-encode");
}
fn nbt(value: &Tag) -> Vec<u8> {
    let mut bytes = Vec::new();
    mc_nbt::write_network_value(&mut bytes, value).unwrap();
    bytes
}

#[test]
fn copied_shield_base_color_roundtrips_all_dyes() {
    for (id, color) in mc_data::stations::DYE_COLORS.iter().enumerate() {
        fixture("minecraft:base_color", text(color), vec![id as u8]);
    }
}

#[test]
fn copied_death_protection_empty_actions_use_vanilla_list_count() {
    for value in [
        fields(Vec::new()),
        fields(vec![("death_effects", make_list(Vec::new()))]),
    ] {
        fixture("minecraft:death_protection", value, vec![0]);
    }
}

#[test]
fn copied_death_protection_rejects_nonempty_custom_actions() {
    let value = fields(vec![(
        "death_effects",
        make_list(vec![fields(vec![(
            "type",
            text("minecraft:clear_all_effects"),
        )])]),
    )]);
    let error = stack("minecraft:death_protection", value)
        .encode(&mut Vec::new())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("nonempty custom death_protection")
    );
    let error = decode(&mut [1_u8].as_slice(), 36, 0).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("nonempty custom death_protection")
    );
}

#[test]
fn copied_custom_name_preserves_styled_text() {
    let value = fields(vec![("text", text("Custody")), ("color", text("gold"))]);
    fixture("minecraft:custom_name", value.clone(), nbt(&value));
}
#[test]
fn copied_item_name_uses_compact_literal_nbt() {
    fixture(
        "minecraft:item_name",
        text("Named"),
        vec![8, 0, 5, b'N', b'a', b'm', b'e', b'd'],
    );
}
#[test]
fn copied_dyed_color_uses_fixed_big_endian_int_and_survives_durable_round_trip() {
    for color in [0x00ff_00ff_i32, 0, -1, i32::MIN, i32::MAX] {
        fixture(
            "minecraft:dyed_color",
            Tag::Int(color),
            color.to_be_bytes().to_vec(),
        );
    }
}

#[test]
fn copied_dyed_color_float_rgb_is_normalized_for_the_client() {
    fixture(
        "minecraft:dyed_color",
        Tag::List(ListTag {
            element_type: tag_type::FLOAT,
            elements: vec![Tag::Float(1.0), Tag::Float(0.5), Tag::Float(0.0)],
        }),
        vec![255, 255, 127, 0],
    );
}
#[test]
fn copied_lore_preserves_mixed_literal_and_styled_lines() {
    let styled = fields(vec![("text", text("B")), ("bold", Tag::Byte(1))]);
    let mut expected = vec![2, 8, 0, 1, b'A'];
    expected.extend(nbt(&styled));
    fixture(
        "minecraft:lore",
        make_list(vec![text("A"), styled]),
        expected,
    );
}

#[test]
fn mixed_lore_durable_binary_matches_vanilla_list_tag_wrappers() {
    let value = make_list(vec![
        text("A"),
        fields(vec![("text", text("B")), ("bold", Tag::Byte(1))]),
    ]);
    // ListTag.write selects COMPOUND for mixed types and wraps the scalar
    // as a singleton empty-key compound, leaving the styled compound intact.
    let expected = vec![
        9, 10, 0, 0, 0, 2, 8, 0, 0, 0, 1, b'A', 0, 8, 0, 4, b't', b'e', b'x', b't', 0, 1, b'B', 1,
        0, 4, b'b', b'o', b'l', b'd', 1, 0,
    ];
    assert_eq!(nbt(&value), expected);
    let restored = mc_nbt::read_network_value(&mut expected.as_slice()).unwrap();
    assert_eq!(restored, value);
    let restored_stack = stack("minecraft:lore", restored);
    let durable = nbt(&Tag::Compound(
        mc_data::item_nbt::stack_to_fields(items(), &restored_stack).unwrap(),
    ));
    let Tag::Compound(saved) = mc_nbt::read_network_value(&mut durable.as_slice()).unwrap() else {
        panic!("saved item is a compound")
    };
    let restored_stack = mc_data::item_nbt::stack_from_fields(items(), &saved)
        .unwrap()
        .unwrap();
    let mut wire = Vec::new();
    restored_stack.encode(&mut wire).unwrap();
    let decoded = ItemStack::decode(&mut wire.as_slice()).unwrap();
    let mut again = Vec::new();
    decoded.encode(&mut again).unwrap();
    assert_eq!(wire, again);
}
#[test]
fn copied_rarity_uses_enum_varint() {
    fixture("minecraft:rarity", text("epic"), vec![3]);
}
#[test]
fn copied_tooltip_display_is_boolean_then_component_registry_list() {
    let value = fields(vec![
        ("hide_tooltip", Tag::Byte(1)),
        (
            "hidden_components",
            make_list(vec![text("minecraft:custom_name"), text("minecraft:lore")]),
        ),
    ]);
    fixture("minecraft:tooltip_display", value, vec![1, 2, 6, 11]);
}
#[test]
fn copied_glint_override_is_boolean() {
    fixture(
        "minecraft:enchantment_glint_override",
        Tag::Byte(1),
        vec![1],
    );
}
#[test]
fn copied_profile_full_properties_and_skin_patch_have_exact_stream_layout() {
    let value = fields(vec![
        ("id", Tag::IntArray(vec![1, 2, 3, 4])),
        ("name", text("Alex")),
        (
            "properties",
            make_list(vec![fields(vec![
                ("name", text("textures")),
                ("value", text("value")),
                ("signature", text("sig")),
            ])]),
        ),
        ("texture", text("minecraft:skin")),
        ("cape", text("minecraft:cape")),
        ("elytra", text("minecraft:elytra")),
        ("model", text("slim")),
    ]);
    let mut expected = vec![1];
    for part in [1_i32, 2, 3, 4] {
        expected.extend(part.to_be_bytes());
    }
    expected.write_string("Alex", 16).unwrap();
    expected.write_varint(1);
    expected.write_string("textures", 64).unwrap();
    expected.write_string("value", 32767).unwrap();
    expected.write_bool(true);
    expected.write_string("sig", 1024).unwrap();
    for resource in ["minecraft:skin", "minecraft:cape", "minecraft:elytra"] {
        expected.write_bool(true);
        expected.write_string(resource, 32767).unwrap();
    }
    expected.extend([1, 1]);
    fixture("minecraft:profile", value, expected);
}
#[test]
fn copied_profile_partial_name_uses_optional_fields_and_boolean_model() {
    let value = fields(vec![("name", text("Alex")), ("model", text("wide"))]);
    let mut expected = vec![0, 1];
    expected.write_string("Alex", 16).unwrap();
    expected.extend([0, 0, 0, 0, 0, 1, 0]);
    fixture("minecraft:profile", value, expected);
}
#[test]
fn copied_note_block_sound_is_identifier() {
    let mut expected = Vec::new();
    expected
        .write_string("minecraft:block.note_block.harp", 32767)
        .unwrap();
    fixture(
        "minecraft:note_block_sound",
        text("minecraft:block.note_block.harp"),
        expected,
    );
}
#[test]
fn copied_banner_pattern_is_reference_holder_plus_color() {
    let mut expected = vec![1];
    expected.write_varint(registry_id("banner_pattern", "minecraft:base").unwrap() + 1);
    expected.write_varint(13);
    fixture(
        "minecraft:banner_patterns",
        make_list(vec![fields(vec![
            ("pattern", text("minecraft:base")),
            ("color", text("green")),
        ])]),
        expected,
    );
}
#[test]
fn copied_pot_decorations_are_four_optional_item_templates() {
    // 26.3 PotDecorations: back, left, right, front, each an optional
    // ItemStackTemplate (item, count, empty patch).
    let mut expected = Vec::new();
    for name in [
        "minecraft:angler_pottery_sherd",
        "minecraft:brick",
        "minecraft:brick",
        "minecraft:brick",
    ] {
        expected.push(1);
        expected.write_varint(registry_id("item", name).unwrap());
        expected.extend([1, 0, 0]);
    }
    fixture(
        "minecraft:pot_decorations",
        fields(vec![
            ("back", text("minecraft:angler_pottery_sherd")),
            ("left", text("minecraft:brick")),
            ("right", text("minecraft:brick")),
            ("front", text("minecraft:brick")),
        ]),
        expected,
    );
}
#[test]
fn copied_container_sparse_slots_use_optional_item_templates_not_slot_stacks() {
    let value = make_list(vec![fields(vec![
        ("slot", Tag::Int(3)),
        (
            "item",
            fields(vec![
                ("id", text("minecraft:diamond")),
                ("count", Tag::Int(2)),
            ]),
        ),
    ])]);
    let mut expected = vec![4, 0, 0, 0, 1];
    expected.write_varint(registry_id("item", "minecraft:diamond").unwrap());
    expected.extend([2, 0, 0]);
    fixture("minecraft:container", value, expected);
}
#[test]
fn copied_block_state_is_string_map() {
    let value = fields(vec![("honey_level", text("3"))]);
    let mut expected = vec![1];
    expected.write_string("honey_level", 32767).unwrap();
    expected.write_string("3", 32767).unwrap();
    fixture("minecraft:block_state", value, expected);
}
#[test]
fn copied_bees_use_entity_registry_id_compound_without_id_and_two_varints() {
    let value = make_list(vec![fields(vec![
        (
            "entity_data",
            fields(vec![("id", text("minecraft:bee")), ("Age", Tag::Int(4))]),
        ),
        ("ticks_in_hive", Tag::Int(7)),
        ("min_ticks_in_hive", Tag::Int(2400)),
    ])]);
    let mut expected = vec![1];
    expected.write_varint(registry_id("entity_type", "minecraft:bee").unwrap());
    // Occupant -> TypedEntityData -> COMPOUND_TAG -> NbtIo.writeAnyTag:
    // the type byte is present, the root-name length is not.
    expected.extend([10, 3, 0, 3, b'A', b'g', b'e', 0, 0, 0, 4, 0]);
    expected.write_varint(7);
    expected.write_varint(2400);
    fixture("minecraft:bees", value, expected);
}
#[test]
fn copied_lock_is_registry_codec_nbt_not_a_string() {
    let value = fields(vec![("items", text("minecraft:tripwire_hook"))]);
    fixture("minecraft:lock", value.clone(), nbt(&value));
}
#[test]
fn copied_container_loot_uses_generic_nbt_and_omits_default_seed() {
    let value = fields(vec![
        ("loot_table", text("minecraft:chests/simple_dungeon")),
        ("seed", Tag::Long(0)),
    ]);
    let expected = nbt(&fields(vec![(
        "loot_table",
        text("minecraft:chests/simple_dungeon"),
    )]));
    fixture("minecraft:container_loot", value, expected);
}
#[test]
fn edited_typed_name_never_resurrects_old_preserved_style() {
    let mut value = stack(
        "minecraft:custom_name",
        fields(vec![("text", text("Old")), ("color", text("red"))]),
    );
    value.custom_name = Some("New".into());
    let mut bytes = Vec::new();
    value.encode(&mut bytes).unwrap();
    let decoded = ItemStack::decode(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded.custom_name.as_deref(), Some("New"));
    assert!(decoded.extended().preserved_components.is_empty());
}
#[test]
fn duplicate_preserved_component_fails_loud_even_when_typed_projection_collapses_it() {
    let mut value = stack(
        "minecraft:custom_name",
        fields(vec![("text", text("A")), ("color", text("red"))]),
    );
    let duplicate = value.extended().preserved_components[0].clone();
    value.extended_mut().preserved_components.push(duplicate);
    assert!(matches!(
        value.encode(&mut Vec::new()),
        Err(CodecError::NotSupported("duplicate item component"))
    ));
}

// Every other 26.3 registered component is deliberately unsupported on the
// preserved-component wire path. It is retained durably, but sending it must fail.
const UNSUPPORTED: &[&str] = &[
    "minecraft:additional_trade_cost",
    "minecraft:attack_range",
    "minecraft:attribute_modifiers",
    "minecraft:axolotl/variant",
    "minecraft:block_entity_data",
    "minecraft:blocks_attacks",
    "minecraft:break_sound",
    "minecraft:bucket_entity_data",
    "minecraft:can_break",
    "minecraft:can_place_on",
    "minecraft:cat/collar",
    "minecraft:cat/sound_variant",
    "minecraft:cat/variant",
    "minecraft:chicken/sound_variant",
    "minecraft:chicken/variant",
    "minecraft:consumable",
    "minecraft:cow/sound_variant",
    "minecraft:cow/variant",
    "minecraft:creative_slot_lock",
    "minecraft:custom_model_data",
    "minecraft:damage_resistant",
    "minecraft:damage_type",
    "minecraft:debug_stick_state",
    "minecraft:dye",
    "minecraft:enchantable",
    "minecraft:entity_data",
    "minecraft:equippable",
    "minecraft:firework_explosion",
    "minecraft:food",
    "minecraft:fox/variant",
    "minecraft:frog/variant",
    "minecraft:glider",
    "minecraft:horse/variant",
    "minecraft:instrument",
    "minecraft:intangible_projectile",
    "minecraft:jukebox_playable",
    "minecraft:kinetic_weapon",
    "minecraft:llama/variant",
    "minecraft:lodestone_tracker",
    "minecraft:map_decorations",
    "minecraft:map_post_processing",
    "minecraft:max_stack_size",
    "minecraft:minimum_attack_charge",
    "minecraft:mooshroom/variant",
    "minecraft:painting/variant",
    "minecraft:parrot/variant",
    "minecraft:piercing_weapon",
    "minecraft:pig/sound_variant",
    "minecraft:pig/variant",
    "minecraft:potion_duration_scale",
    "minecraft:provides_banner_patterns",
    "minecraft:provides_trim_material",
    "minecraft:rabbit/variant",
    "minecraft:recipes",
    "minecraft:repairable",
    "minecraft:salmon/size",
    "minecraft:sheep/color",
    "minecraft:shulker/color",
    "minecraft:tool",
    "minecraft:tooltip_style",
    "minecraft:tropical_fish/base_color",
    "minecraft:tropical_fish/pattern",
    "minecraft:tropical_fish/pattern_color",
    "minecraft:use_cooldown",
    "minecraft:use_effects",
    "minecraft:villager/variant",
    "minecraft:weapon",
    "minecraft:wolf/collar",
    "minecraft:wolf/sound_variant",
    "minecraft:wolf/variant",
    "minecraft:zombie_nautilus/variant",
];
#[test]
fn all_explicitly_unsupported_preserved_components_fail_loud_instead_of_disappearing() {
    assert_eq!(
        ITEM_COMPONENT_CODECS.len() + UNSUPPORTED.len(),
        mc_data::data_component_types_26_1_2::COUNT
    );
    for name in UNSUPPORTED {
        assert!(
            !ITEM_COMPONENT_CODECS
                .iter()
                .any(|(supported, _)| supported == name)
        );
        assert!(registry_id("data_component_type", name).is_ok(), "{name}");
        let mut value = ItemStack::new(registry_id("item", "minecraft:stone").unwrap() as u32, 1);
        value
            .extended_mut()
            .preserved_components
            .push(PreservedItemComponent {
                name: (*name).into(),
                value: nbt(&Tag::Compound(Vec::new())),
                baseline: None,
            });
        assert!(
            matches!(
                value.encode(&mut Vec::new()),
                Err(CodecError::NotSupported(
                    "unsupported preserved item component"
                ))
            ),
            "{name}"
        );
    }
}

#[test]
fn explicit_component_removals_use_native_added_then_removed_patch_bytes() {
    // DataComponentPatch$3: added count, removed count, added payloads, then
    // only the registered IDs of removed components (no NBT payload).
    let value = mc_data::item_nbt::stack_from_fields(
        items(),
        &[
            ("id".into(), text("minecraft:stone")),
            ("count".into(), Tag::Int(1)),
            (
                "components".into(),
                fields(vec![
                    ("minecraft:damage", Tag::Int(7)),
                    ("!minecraft:custom_name", Tag::Compound(Vec::new())),
                    ("!minecraft:potion_contents", Tag::Compound(Vec::new())),
                ]),
            ),
        ],
    )
    .unwrap()
    .unwrap();
    let mut expected = vec![1];
    expected.write_varint(registry_id("item", "minecraft:stone").unwrap());
    expected.extend([
        1,
        2,
        mc_data::data_component_types_26_1_2::DAMAGE as u8,
        7,
        mc_data::data_component_types_26_1_2::CUSTOM_NAME as u8,
        mc_data::data_component_types_26_1_2::POTION_CONTENTS as u8,
    ]);
    let mut bytes = Vec::new();
    value.encode(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
    let mut input = bytes.as_slice();
    let decoded = ItemStack::decode(&mut input).unwrap();
    assert!(input.is_empty());
    let mut again = Vec::new();
    decoded.encode(&mut again).unwrap();
    assert_eq!(again, expected);
    let durable = mc_data::item_nbt::stack_to_fields(items(), &decoded).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&durable, "components") else {
        panic!()
    };
    assert!(mc_data::item_nbt::field(components, "!minecraft:custom_name").is_some());
    assert!(mc_data::item_nbt::field(components, "!minecraft:potion_contents").is_some());
}

#[test]
fn removed_default_components_need_no_unsupported_added_value_codec() {
    let value = stack("!minecraft:attribute_modifiers", Tag::Compound(Vec::new()));
    let mut expected = vec![1];
    expected.write_varint(registry_id("item", "minecraft:stone").unwrap());
    expected.extend([0, 1, 16]);
    let mut bytes = Vec::new();
    value.encode(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
    let decoded = ItemStack::decode(&mut bytes.as_slice()).unwrap();
    let mut again = Vec::new();
    decoded.encode(&mut again).unwrap();
    assert_eq!(again, expected);
}

#[test]
fn duplicate_and_unknown_removed_components_fail_closed() {
    for (patch, expected) in [
        (vec![0, 2, 6, 6], "duplicate item component"),
        (vec![1, 1, 3, 7, 3], "duplicate item component"),
        (
            vec![0, 1, mc_data::data_component_types_26_1_2::COUNT as u8],
            "unknown removed item component",
        ),
    ] {
        let mut bytes = vec![1];
        bytes.write_varint(registry_id("item", "minecraft:stone").unwrap());
        bytes.extend(patch);
        assert_eq!(
            ItemStack::decode(&mut bytes.as_slice()).unwrap_err(),
            CodecError::NotSupported(expected)
        );
    }
}

#[test]
fn typed_name_edit_retires_preserved_removal_before_wire_publication() {
    let mut value = stack("!minecraft:custom_name", Tag::Compound(Vec::new()));
    value.custom_name = Some("Restored name".into());
    let mut bytes = Vec::new();
    value.encode(&mut bytes).unwrap();
    let decoded = ItemStack::decode(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded.custom_name.as_deref(), Some("Restored name"));
    assert!(
        decoded
            .extended()
            .preserved_components
            .iter()
            .all(|component| component.name != "!minecraft:custom_name")
    );
}

#[test]
fn crafted_dyed_leather_boots_reach_the_client_with_the_blended_component() {
    let item = |name: &str| {
        ItemStack::new(
            items()
                .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
                .unwrap(),
            1,
        )
    };
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = mc_data::item_nbt::stack_from_component_patch(
        items(),
        &Identifier::parse("minecraft:leather_boots").unwrap(),
        &[("minecraft:dyed_color".into(), Tag::Int(0xff00ff))],
    )
    .unwrap();
    input[1] = item("blue_dye");
    let output = mc_data::recipes::armor_dye_crafting_result(items(), &input).unwrap();
    let mut bytes = Vec::new();
    output.encode(&mut bytes).unwrap();
    let mut cursor = bytes.as_slice();
    assert_eq!(cursor.read_varint().unwrap(), 1);
    assert_eq!(cursor.read_varint().unwrap(), output.item_id as i32);
    assert_eq!(cursor.read_varint().unwrap(), 1); // Added component count.
    assert_eq!(cursor.read_varint().unwrap(), 0); // Removed component count.
    assert_eq!(
        cursor.read_varint().unwrap(),
        mc_data::data_component_types_26_1_2::DYED_COLOR
    );
    assert_eq!(cursor.read_i32().unwrap(), 0x9d22d4);
    assert!(cursor.is_empty());
    let decoded = ItemStack::decode(&mut bytes.as_slice()).unwrap();
    let durable = mc_data::item_nbt::stack_to_fields(items(), &decoded).unwrap();
    let Some(Tag::Compound(components)) = mc_data::item_nbt::field(&durable, "components") else {
        panic!("missing client color");
    };
    assert_eq!(
        mc_data::item_nbt::field(components, "minecraft:dyed_color"),
        Some(&Tag::Int(0x9d22d4))
    );
}

#[test]
fn map_id_is_a_varint_so_a_filled_map_can_reach_the_client() {
    // MapId.STREAM_CODEC is a VarInt; 300 needs two bytes.
    fixture("minecraft:map_id", Tag::Int(300), vec![0xac, 0x02]);
    fixture("minecraft:map_id", Tag::Int(0), vec![0]);
}

#[test]
fn bundle_nesting_encodes_up_to_eight_non_empty_levels_and_no_deeper() {
    // core's inventory::bundle::MAX_NESTING mirrors this bound.
    let bundle = |contents: Vec<Tag>| {
        let mut components = Vec::new();
        if !contents.is_empty() {
            components.push((
                "minecraft:bundle_contents",
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: contents,
                }),
            ));
        }
        let fields = vec![
            ("id".to_string(), text("minecraft:bundle")),
            ("count".to_string(), Tag::Int(1)),
            ("components".to_string(), fields(components)),
        ];
        fields
    };
    let encodes = |levels: usize| {
        let mut inner = Vec::new();
        for _ in 0..levels {
            let wrapped = if inner.is_empty() {
                bundle(Vec::new())
            } else {
                inner
            };
            inner = bundle(vec![Tag::Compound(wrapped)]);
        }
        let Ok(Some(stack)) = mc_data::item_nbt::stack_from_fields(items(), &inner) else {
            return false;
        };
        let mut out = Vec::new();
        stack.encode(&mut out).is_ok()
    };
    assert!(encodes(8));
    assert!(!encodes(9));
}
