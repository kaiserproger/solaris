use super::*;
use crate::packets::Packet;
use crate::packets::play::ServerboundSetCreativeModeSlot;

fn fixture(name: &str, value: Tag, payload: Vec<u8>) {
    // Exercise the actual untrusted creative framing, not only inventory slots.
    mc_data::item_nbt::stack_from_component_patch(
        items(),
        &Identifier::parse("minecraft:stone").unwrap(),
        &[(name.into(), value.clone())],
    )
    .unwrap();
    let mut creative = Vec::new();
    creative.write_i16(36);
    creative.write_varint(1);
    creative.write_varint(registry_id("item", "minecraft:stone").unwrap());
    creative.write_varint(1);
    creative.write_varint(0);
    creative.write_varint(
        mc_data::data_component_types_26_1_2::id(&Identifier::parse(name).unwrap()).unwrap() as i32,
    );
    creative.write_varint(payload.len() as i32);
    creative.extend_from_slice(&payload);
    let mut cursor = creative.as_slice();
    let decoded = ServerboundSetCreativeModeSlot::decode(&mut cursor)
        .unwrap_or_else(|error| panic!("{name} creative ingress: {error}"));
    assert!(cursor.is_empty(), "{name} creative packet trailing bytes");
    let saved = serde_json::to_vec(&decoded.item_stack).unwrap();
    let restored: ItemStack = serde_json::from_slice(&saved).unwrap();
    let fields = mc_data::item_nbt::stack_to_fields(items(), &restored).unwrap();
    let restored = mc_data::item_nbt::stack_from_fields(items(), &fields)
        .unwrap()
        .unwrap();
    let mut again = Vec::new();
    ServerboundSetCreativeModeSlot {
        slot: 36,
        item_stack: restored,
    }
    .encode(&mut again)
    .unwrap();
    assert_eq!(again, creative, "{name} creative save/restart/publication");
    super::fixture(name, value, payload);
}

// Local 26.3 DataComponents registrations and the individual STREAM_CODEC
// declarations are the oracle, not an encode/decode self-round-trip alone.
#[test]
fn new_26_3_animation_components() {
    for name in ["minecraft:attack_animation", "minecraft:interact_animation"] {
        fixture(
            name,
            fields(vec![("type", text("stab")), ("duration", Tag::Int(130))]),
            vec![2, 130, 1],
        );
        fixture(name, fields(vec![]), vec![1, 6]);
    }
}

#[test]
fn new_26_3_registry_components() {
    for (component, registry, entry) in [
        (
            "minecraft:block_transformer",
            "block_transformer",
            "minecraft:axe",
        ),
        (
            "minecraft:provides_pottery_pattern",
            "decorated_pot_pattern",
            "minecraft:angler",
        ),
    ] {
        let mut payload = Vec::new();
        payload.write_varint(registry_id(registry, entry).unwrap());
        fixture(component, text(entry), payload);
    }
}

#[test]
fn new_26_3_resolvable_fuels_and_compost() {
    fixture(
        "minecraft:compostable",
        fields(vec![("layers", Tag::Int(2))]),
        vec![1, 0, 0, 0, 2],
    );
    let reference = "minecraft:composting/default";
    let mut payload = vec![0];
    payload
        .write_identifier(&Identifier::parse(reference).unwrap())
        .unwrap();
    fixture(
        "minecraft:compostable",
        fields(vec![("layers", text(reference))]),
        payload,
    );
    for (name, field_name) in [
        ("minecraft:cooking_fuel", "burn_time"),
        ("minecraft:brewing_fuel", "uses"),
    ] {
        let mut payload = vec![1];
        payload.extend(1600_i32.to_be_bytes());
        payload.push(1);
        payload.extend(1.5_f32.to_be_bytes());
        fixture(
            name,
            fields(vec![
                (field_name, Tag::Int(1600)),
                ("speed_multiplier", Tag::Float(1.5)),
            ]),
            payload,
        );
        let mut payload = vec![0];
        payload.write_string("minecraft:fuel/value", 32767).unwrap();
        payload.push(0);
        payload.write_string("minecraft:fuel/speed", 32767).unwrap();
        fixture(
            name,
            fields(vec![
                (field_name, text("minecraft:fuel/value")),
                ("speed_multiplier", text("minecraft:fuel/speed")),
            ]),
            payload,
        );
    }
}

#[test]
fn new_26_3_mob_visibility_named_and_direct_sets() {
    let mut payload = vec![0];
    payload.write_string("minecraft:skeletons", 32767).unwrap();
    payload.extend(0.5_f32.to_be_bytes());
    fixture(
        "minecraft:mob_visibility",
        fields(vec![
            ("targeting_entity_types", text("#minecraft:skeletons")),
            ("visibility", Tag::Float(0.5)),
        ]),
        payload,
    );
    let mut payload = vec![3];
    payload.write_varint(registry_id("entity_type", "minecraft:zombie").unwrap());
    payload.write_varint(registry_id("entity_type", "minecraft:skeleton").unwrap());
    payload.extend(2.0_f32.to_be_bytes());
    fixture(
        "minecraft:mob_visibility",
        fields(vec![
            (
                "targeting_entity_types",
                make_list(vec![text("minecraft:zombie"), text("minecraft:skeleton")]),
            ),
            ("visibility", Tag::Float(2.0)),
        ]),
        payload,
    );
}

#[test]
fn new_26_3_unit_color_and_villager_food() {
    fixture("minecraft:unbreakable", fields(vec![]), vec![]);
    fixture("minecraft:waxed", fields(vec![]), vec![]);
    fixture("minecraft:cushion/color", text("red"), vec![14]);
    fixture(
        "minecraft:villager_food",
        fields(vec![("nutrition", Tag::Int(130))]),
        vec![130, 1],
    );
}

#[test]
fn new_26_3_sign_text_fixed_four_lines_and_optional_filter() {
    for name in ["minecraft:sign_text_front", "minecraft:sign_text_back"] {
        for filtered in [false, true] {
            let messages = vec![text("A"), text("B"), text("C"), text("D")];
            let mut payload = Vec::new();
            for line in &messages {
                payload.extend(nbt(line));
            }
            payload.push(u8::from(filtered));
            let mut value = vec![
                ("messages", make_list(messages)),
                ("color", text("blue")),
                ("has_glowing_text", Tag::Byte(1)),
            ];
            if filtered {
                let lines = vec![text("a"), text("b"), text("c"), text("d")];
                for line in &lines {
                    payload.extend(nbt(line));
                }
                value.push(("filtered_messages", make_list(lines)));
            }
            payload.extend([11, 1]);
            fixture(name, fields(value), payload);
        }
    }
}

#[test]
fn new_26_3_sulfur_template_and_pot_sides_preserve_nested_patches() {
    let value = fields(vec![
        ("id", text("minecraft:brick")),
        ("count", Tag::Int(2)),
        (
            "components",
            fields(vec![("minecraft:waxed", fields(vec![]))]),
        ),
    ]);
    // ItemStackTemplate is item id, count, added count, removed count, patch.
    let mut template = Vec::new();
    template.write_varint(registry_id("item", "minecraft:brick").unwrap());
    template.extend([2, 1, 0, 120]);
    fixture(
        "minecraft:sulfur_cube_content",
        value.clone(),
        template.clone(),
    );
    let mut payload = vec![1];
    payload.extend(template);
    payload.extend([0, 0, 0]);
    fixture(
        "minecraft:pot_decorations",
        fields(vec![("back", value)]),
        payload,
    );
    fixture(
        "minecraft:pot_decorations",
        fields(vec![]),
        vec![0, 0, 0, 0],
    );
}

#[test]
fn new_26_3_nested_patch_in_mixed_bundle_templates_reaches_inventory_wire() {
    let value = make_list(vec![
        text("minecraft:stone"),
        fields(vec![
            ("id", text("minecraft:brick")),
            ("count", Tag::Int(2)),
            (
                "components",
                fields(vec![("minecraft:waxed", fields(vec![]))]),
            ),
        ]),
    ]);
    let mut payload = vec![2];
    payload.write_varint(registry_id("item", "minecraft:stone").unwrap());
    payload.extend([1, 0, 0]);
    payload.write_varint(registry_id("item", "minecraft:brick").unwrap());
    payload.extend([2, 1, 0, 120]);
    fixture("minecraft:bundle_contents", value, payload);
}

fn projectile_fixture(count: usize) -> (Tag, Vec<u8>) {
    let projectile = fields(vec![
        ("id", text("minecraft:arrow")),
        ("count", Tag::Int(1)),
    ]);
    let mut payload = Vec::new();
    payload.write_varint(count as i32);
    for _ in 0..count {
        payload.write_varint(registry_id("item", "minecraft:arrow").unwrap());
        payload.extend([1, 0, 0]);
    }
    (make_list(vec![projectile; count]), payload)
}

#[test]
fn charged_projectiles_26_3_native_limit_matches_1024_template_codec() {
    // Local ChargedProjectiles.CODEC and STREAM_CODEC both cap at 1024, not
    // the three projectiles produced by ordinary Multishot gameplay.
    for count in [4, 1024] {
        let (value, payload) = projectile_fixture(count);
        fixture("minecraft:charged_projectiles", value, payload);
    }
    let (value, _) = projectile_fixture(1025);
    assert!(
        mc_data::item_nbt::stack_from_component_patch(
            items(),
            &Identifier::parse("minecraft:stone").unwrap(),
            &[("minecraft:charged_projectiles".into(), value)],
        )
        .is_err()
    );
}

#[test]
fn charged_projectiles_26_3_wire_limit_matches_1024_template_codec() {
    for count in [1024, 1025] {
        let (_, payload) = projectile_fixture(count);
        let mut bytes = Vec::new();
        bytes.write_varint(1);
        bytes.write_varint(registry_id("item", "minecraft:stone").unwrap());
        bytes.extend([1, 0]);
        bytes.write_varint(mc_data::data_component_types_26_1_2::CHARGED_PROJECTILES);
        bytes.extend(payload);
        let mut cursor = bytes.as_slice();
        let decoded = ItemStack::decode(&mut cursor);
        if count == 1024 {
            let decoded = decoded.expect("valid 1024-template stream");
            assert!(cursor.is_empty());
            assert_eq!(decoded.extended().charged_projectiles.len(), count);
        } else {
            assert!(decoded.is_err(), "1025-template stream must be refused");
        }
    }
}

#[test]
fn native_26_3_charged_override_publishes_one_component() {
    let (_, payload) = projectile_fixture(1);
    // Omitted vanilla defaults retain an exact patch beside typed gameplay
    // fields; the preserved patch must override, not duplicate, the typed one.
    let value = make_list(vec![fields(vec![("id", text("minecraft:arrow"))])]);
    fixture("minecraft:charged_projectiles", value, payload);
}

#[test]
fn native_26_3_stew_override_publishes_one_component() {
    let value = make_list(vec![fields(vec![("id", text("minecraft:speed"))])]);
    let mut payload = vec![1];
    payload.write_varint(registry_id("mob_effect", "minecraft:speed").unwrap());
    payload.write_varint(160);
    fixture("minecraft:suspicious_stew_effects", value, payload);
}
