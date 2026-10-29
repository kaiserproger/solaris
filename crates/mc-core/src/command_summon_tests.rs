use super::*;
use crate::boundary::{ChangeSet, ConnectionId, PlayerChange, PlayerInput};

fn summon(kind: &str, nbt: &str) -> (TickState, EntityId) {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    let parsed = crate::commands::parse(&format!("summon {kind} ~ ~ ~ {nbt}"), true).unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(&mut state, ConnectionId(1), actor, &parsed, &mut changes);
    assert_eq!(
        changes.entities_spawned.len(),
        1,
        "{kind} {nbt}: {:?}",
        state.outbox
    );
    let id = changes.entities_spawned[0];
    (state, id)
}

#[test]
fn persistence_required_iron_golem_template_and_checkpoint_are_native() {
    let (state, id) = summon("iron_golem", "{NoAI:1b,PersistenceRequired:1b}");
    let slot = state.entities.slot_of(id).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert!(row.persistent && row.controls.no_ai);
    let image = EntityStateImage::from_row(body, row);
    let wal: EntityStateImage =
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    assert!(wal.persistent && wal.controls.no_ai);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = chunks
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == row.uuid)
        .unwrap();
    let restored = EntityStateImage::from_record(&state.registry, native).unwrap();
    assert!(restored.persistent && restored.controls.no_ai);
    assert!(
        native
            .extra_fields
            .contains(&("PersistenceRequired".into(), Tag::Byte(1)))
    );
}

#[test]
fn no_ai_is_applied() {
    let (state, id) = summon("cow", "{NoAI:1b}");
    assert!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .controls
            .no_ai
    );
}

#[test]
fn invulnerable_is_applied() {
    let (mut state, id) = summon("cow", "{Invulnerable:1b}");
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    assert!(row.controls.invulnerable);
    assert_eq!(
        state
            .entities
            .land_hit(slot, 1000, 4.0, crate::survival::DamageKind::Melee),
        0.0
    );
}

#[test]
fn health_is_applied() {
    let (state, id) = summon("cow", "{Health:7f}");
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .health,
        7.0
    );
}

#[test]
fn motion_is_applied_in_core_units() {
    let (state, id) = summon("cow", "{Motion:[0.1d,0.2d,-0.3d]}");
    assert_eq!(
        state.entities.vel(state.entities.slot_of(id).unwrap()),
        [2.0, 4.0, -6.0]
    );
}

#[test]
fn rotation_is_applied() {
    let (state, id) = summon("cow", "{Rotation:[90f,7f]}");
    assert_eq!(
        state.entities.rot(state.entities.slot_of(id).unwrap())[..2],
        [90.0, 7.0]
    );
}

#[test]
fn age_is_applied_to_an_ageable_species() {
    let (state, id) = summon("cow", "{Age:-1234}");
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.entity_life.child_ticks, 1234);
    assert!(row.appearance.baby);
}

#[test]
fn custom_name_literal_component_is_applied() {
    for nbt in [
        "{CustomName:'Template Cow'}",
        "{CustomName:{text:'Template Cow'}}",
    ] {
        let (state, id) = summon("cow", nbt);
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        assert_eq!(row.appearance.custom_name.as_deref(), Some("Template Cow"));
        let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let native = chunks
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.uuid == row.uuid)
            .unwrap();
        assert!(
            native
                .extra_fields
                .contains(&("CustomName".into(), Tag::String("Template Cow".into())))
        );
    }
}

fn rejected_tag_reaches_sender(kind: &str, key: &str, value: &str) {
    let (mut state, _, _) = crate::hostile_tests::setup(8.0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "Other".into(),
            permission_level: 0,
        }],
    );
    state.outbox.clear();
    let before = state.entities.len();
    let (_, drains, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: format!("summon {kind} ~ ~ ~ {{{key}:{value}}}"),
            permission_level: 2,
        }],
    );
    assert_eq!(
        state.entities.len(),
        before,
        "rejected {key} must not allocate an entity"
    );
    let has_error = |connection| {
        drains.iter().any(|(recipient, queue)| {
            *recipient == connection
                && queue.iter().any(|change| {
                    matches!(
                        change, PlayerChange::Chat { message } if message.contains(key)
                    )
                })
        })
    };
    assert!(
        has_error(ConnectionId(1)),
        "{key} error did not reach sender chat: {:?}",
        drains
    );
    assert!(
        !has_error(ConnectionId(2)),
        "{key} error leaked to another player"
    );
}

macro_rules! unsupported_tag {
    ($test:ident, $key:literal, $value:literal) => {
        #[test]
        fn $test() {
            rejected_tag_reaches_sender("cow", $key, $value);
        }
    };
}
#[test]
fn custom_name_visible_is_authoritative_and_can_clear() {
    for visible in [false, true] {
        let (mut state, id) = summon(
            "cow",
            &format!(
                "{{CustomName:'Visible Cow',CustomNameVisible:{}b}}",
                i8::from(visible)
            ),
        );
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row(slot);
        assert_eq!(row.appearance.custom_name_visible, visible);
        assert!(row.opaque_entity_fields.is_empty());
        let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let native = chunks
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.uuid == row.uuid)
            .unwrap();
        assert_eq!(
            native
                .extra_fields
                .iter()
                .filter(|(key, _)| key == "CustomNameVisible")
                .count(),
            1
        );
        assert!(
            native
                .extra_fields
                .contains(&("CustomNameVisible".into(), Tag::Byte(i8::from(visible))))
        );
        assert_eq!(
            crate::core::appearance_from_vanilla_entity(native).custom_name_visible,
            visible
        );
        let image = EntityStateImage::from_record(&state.registry, native).unwrap();
        assert!(image.opaque_entity_fields.is_empty());
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name_visible = false;
        let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let native = chunks
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.wire_id == id.0)
            .unwrap();
        assert!(
            native
                .extra_fields
                .contains(&("CustomNameVisible".into(), Tag::Byte(0)))
        );
        let mut invalid = native.clone();
        invalid
            .extra_fields
            .iter_mut()
            .find(|(key, _)| key == "CustomNameVisible")
            .unwrap()
            .1 = Tag::String("true".into());
        assert!(EntityStateImage::from_record(&state.registry, &invalid).is_err());
    }
}
unsupported_tag!(silent_is_explicitly_unsupported, "Silent", "1b");
unsupported_tag!(no_gravity_is_explicitly_unsupported, "NoGravity", "1b");
unsupported_tag!(glowing_is_explicitly_unsupported, "Glowing", "1b");
unsupported_tag!(tags_are_explicitly_unsupported, "Tags", "['probe']");
unsupported_tag!(left_handed_is_explicitly_unsupported, "LeftHanded", "1b");
unsupported_tag!(unknown_tag_names_reach_chat, "UnsupportedExample", "1b");

#[test]
fn is_baby_is_rejected_when_species_has_no_retained_baby_model() {
    rejected_tag_reaches_sender("iron_golem", "IsBaby", "1b");
}

#[test]
fn age_is_rejected_when_species_is_not_ageable() {
    rejected_tag_reaches_sender("iron_golem", "Age", "-100");
}

#[test]
fn unsupported_styled_custom_name_is_not_silently_flattened() {
    rejected_tag_reaches_sender("cow", "CustomName", "{text:'Cow',color:'red'}");
}

#[test]
fn villager_age_uses_existing_villager_authority_and_checkpoint() {
    let (state, id) = summon("villager", "{Age:-321}");
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.villager.as_ref().unwrap().age, -321);
    assert!(row.appearance.baby);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = chunks
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == row.uuid)
        .unwrap();
    let restored = crate::villager::persistence::villager_from_fields(
        &state.registry.items,
        &native.extra_fields,
        state.tick,
    )
    .unwrap();
    assert_eq!(restored.age, -321);
}

#[test]
fn zombie_villager_is_baby_uses_existing_conversion_authority() {
    for baby in [false, true] {
        let (state, id) = summon(
            "zombie_villager",
            &format!("{{IsBaby:{}b}}", i8::from(baby)),
        );
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        let villager = row.villager.as_ref().unwrap();
        assert_eq!(villager.age < 0, baby);
        assert_eq!(villager.conversion_time, Some(-1));
        assert_eq!(row.appearance.baby, baby);
        let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let native = chunks
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.uuid == row.uuid)
            .unwrap();
        let restored = crate::villager::persistence::villager_from_fields(
            &state.registry.items,
            &native.extra_fields,
            state.tick,
        )
        .unwrap();
        assert_eq!(restored.age < 0, baby);
    }
}

#[test]
fn tadpole_age_uses_existing_native_growth_clock() {
    let (state, id) = summon("tadpole", "{Age:1234}");
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.entity_life.age_lock.tadpole_age, 1234);
    assert_eq!(row.age, 1234);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = chunks
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == row.uuid)
        .unwrap();
    assert_eq!(native.age, 1234);
}

#[test]
fn invalid_supported_tags_reach_sender_chat() {
    for (key, value) in [
        ("PersistenceRequired", "'bad'"),
        ("NoAI", "'bad'"),
        ("Invulnerable", "'bad'"),
        ("Health", "-1f"),
        ("Rotation", "[90f]"),
        ("Motion", "[0d,0d]"),
        ("Age", "'bad'"),
    ] {
        rejected_tag_reaches_sender("cow", key, value);
    }
    rejected_tag_reaches_sender("zombie_villager", "IsBaby", "'bad'");
    rejected_tag_reaches_sender("tadpole", "Age", "-1");
}

#[test]
fn summon_common_state_survives_real_wal_replay_and_native_region_restart() {
    let (mut state, _, _) = crate::hostile_tests::setup(8.0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let (output, _, _) = crate::tick::run_tick(&mut state, &[PlayerInput::Command {
        connection: ConnectionId(1),
        command: "summon sheep ~ ~ ~ {PersistenceRequired:1b,NoAI:1b,Invulnerable:1b,Age:-1234,Health:7f,CustomName:{text:'Durable Sheep'},CustomNameVisible:1b,CanPickUpLoot:1b,equipment:{mainhand:{id:'minecraft:iron_sword',components:{'minecraft:damage':4}},body:{id:'minecraft:leather_horse_armor'}},drop_chances:{mainhand:2f,body:0.5f}}".into(),
        permission_level: 2,
    }]);
    assert_eq!(output.changes.entities_spawned.len(), 1);
    let id = output.changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    let uuid = state.entities.row(slot).uuid;
    let expected_equipment = state.entities.row(slot).appearance.equipment.clone();
    let expected_body = state.entities.row(slot).loot.body_armor.clone();
    let root = std::env::temp_dir().join(format!("solaris-summon-common-{}", uuid::Uuid::new_v4()));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = crate::journal::JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &crate::journal::read_complete_batches_after(&root.join("journal"), 0).unwrap(),
    );
    assert_eq!(replay.entities.len(), 1);
    let record = &replay.entities[&id.0];
    assert_eq!(record.appearance.equipment, expected_equipment);
    assert_eq!(record.state.loot.body_armor, expected_body);
    assert_eq!(record.state.loot.drop_chances[0], 2.0);
    assert_eq!(record.state.loot.extra_drop_chances[0], 0.5);
    assert!(record.state.loot.can_pick_up_loot);
    assert!(
        record.state.persistent
            && record.state.controls.no_ai
            && record.state.controls.invulnerable
    );
    assert_eq!(
        record.appearance.custom_name.as_deref(),
        Some("Durable Sheep")
    );
    assert!(record.appearance.custom_name_visible);
    state.remove_entity(slot);
    crate::core::restore_entity(&mut state, id.0, record).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row(slot);
    assert!(row.persistent && row.controls.no_ai && row.controls.invulnerable);
    assert_eq!(
        row.entity_life.child_ticks,
        record.state.entity_life.child_ticks
    );
    assert_eq!(row.appearance.custom_name.as_deref(), Some("Durable Sheep"));
    assert!(row.appearance.custom_name_visible);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let entities_dir = root.join("entities");
    mc_world::anvil::write_entity_regions(&entities_dir, &state.registry.items, &chunks, 0)
        .unwrap();
    let reopened =
        mc_world::anvil::read_entity_regions(&entities_dir, &state.registry.items).unwrap();
    let native = reopened
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == uuid)
        .unwrap();
    let image = EntityStateImage::from_record(&state.registry, native).unwrap();
    assert!(image.persistent && image.controls.no_ai && image.controls.invulnerable);
    assert_eq!(image.entity_life.child_ticks, row.entity_life.child_ticks);
    assert_eq!(native.wire_id, id.0);
    assert!(
        native
            .extra_fields
            .contains(&("CustomName".into(), Tag::String("Durable Sheep".into())))
    );
    assert!(
        native
            .extra_fields
            .contains(&("CustomNameVisible".into(), Tag::Byte(1)))
    );
    assert!(crate::core::appearance_from_vanilla_entity(native).custom_name_visible);
    assert_eq!(
        crate::entity_loot::equipment_from_fields(&state.registry.items, &native.extra_fields)
            .unwrap(),
        expected_equipment
    );
    assert_eq!(image.loot.body_armor, expected_body);
    assert_eq!(image.loot.drop_chances[0], 2.0);
    assert_eq!(image.loot.extra_drop_chances[0], 0.5);
    assert!(image.loot.can_pick_up_loot);
    assert!(image.opaque_entity_fields.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn slime_size_is_applied_and_clamped() {
    for (kind, nbt, size) in [
        ("slime", "{Size:1}", 2),
        ("magma_cube", "{Size:3}", 4),
        ("slime", "{Size:900}", 127),
    ] {
        let (state, id) = summon(kind, nbt);
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        assert_eq!(row.loot.slime_size, Some(size), "{kind} {nbt}");
    }
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    let parsed = crate::commands::parse("summon cow ~ ~ ~ {Size:1}", true).unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(&mut state, ConnectionId(1), actor, &parsed, &mut changes);
    assert!(
        changes.entities_spawned.is_empty(),
        "Size is slime/magma-only"
    );
}

#[test]
fn summon_equipment_all_living_slots_and_saddle_reach_authoritative_state() {
    for kind in [
        "cow",
        "zombie",
        "skeleton",
        "villager",
        "wolf",
        "armor_stand",
    ] {
        let (state, id) = summon(
            kind,
            "{equipment:{mainhand:{id:'minecraft:stick',count:2},offhand:{id:'minecraft:shield'},feet:{id:'minecraft:iron_boots'},legs:{id:'minecraft:iron_leggings'},chest:{id:'minecraft:iron_chestplate'},head:{id:'minecraft:iron_helmet'},body:{id:'minecraft:leather_horse_armor'}}}",
        );
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        for (stack, name) in row.appearance.equipment.iter().zip([
            "stick",
            "shield",
            "iron_boots",
            "iron_leggings",
            "iron_chestplate",
            "iron_helmet",
        ]) {
            let stack = stack.as_ref().unwrap();
            assert_eq!(
                state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .unwrap()
                    .as_str(),
                format!("minecraft:{name}"),
                "{kind}"
            );
        }
        assert_eq!(row.appearance.equipment[0].as_ref().unwrap().count, 2);
        assert!(row.loot.body_armor.is_some());
        assert_eq!(
            row.appearance.body_armor.as_ref(),
            row.loot.body_armor.as_deref()
        );
    }
    for kind in ["horse", "pig", "strider"] {
        let (state, id) = summon(kind, "{equipment:{saddle:{id:'minecraft:saddle'}}}");
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        assert!(row.loot.saddle.is_some(), "{kind}");
        assert_eq!(row.appearance.saddle.as_ref(), row.loot.saddle.as_deref());
    }
}

#[test]
fn summon_preserves_spawn_defaults_unless_equipment_is_authored() {
    let (mut state, id) = summon("skeleton", "{}");
    let slot = state.entities.slot_of(id).unwrap();
    let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        panic!("expected mob");
    };
    // Distinct initialized values prove omission preserves, rather than reconstructs, defaults.
    let stack = mc_data::item_nbt::stack_from_component_patch(
        &state.registry.items,
        &mc_data::Identifier::parse("minecraft:bow").unwrap(),
        &[],
    )
    .unwrap();
    {
        let row = state.entities.row_mut(slot);
        Arc::make_mut(&mut row.appearance).equipment[0] = Some(stack.clone());
        row.loot.body_armor = Some(Box::new(stack.clone()));
        row.loot.saddle = Some(Box::new(stack.clone()));
        row.loot.drop_chances = [0.3; 6];
        row.loot.extra_drop_chances = [0.4; 2];
        row.loot.can_pick_up_loot = true;
    }
    let image = prepare(
        &state,
        type_id,
        "minecraft:skeleton",
        [0.0; 3],
        &Tag::Compound(vec![("NoAI".into(), Tag::Byte(1))]),
    )
    .unwrap();
    apply(&mut state, id, image);
    let row = state.entities.row(slot);
    assert_eq!(row.appearance.equipment[0], Some(stack.clone()));
    assert_eq!(row.loot.body_armor.as_deref(), Some(&stack));
    assert_eq!(row.loot.saddle.as_deref(), Some(&stack));
    assert_eq!(row.loot.drop_chances, [0.3; 6]);
    assert_eq!(row.loot.extra_drop_chances, [0.4; 2]);
    assert!(row.loot.can_pick_up_loot);
    let image = prepare(
        &state,
        type_id,
        "minecraft:skeleton",
        [0.0; 3],
        &Tag::Compound(vec![
            ("equipment".into(), Tag::Compound(vec![])),
            (
                "drop_chances".into(),
                Tag::Compound(vec![("head".into(), Tag::Int(2))]),
            ),
            ("CanPickUpLoot".into(), Tag::Byte(0)),
        ]),
    )
    .unwrap();
    apply(&mut state, id, image);
    let row = state.entities.row(slot);
    assert!(row.appearance.equipment.iter().all(Option::is_none));
    assert!(row.loot.body_armor.is_none() && row.loot.saddle.is_none());
    assert_eq!(row.loot.drop_chances, [0.3, 0.3, 0.3, 0.3, 0.3, 2.0]);
    assert_eq!(row.loot.extra_drop_chances, [0.4; 2]);
    assert!(!row.loot.can_pick_up_loot);
}

#[test]
fn summon_equipment_rejects_invalid_stacks_components_and_chances_before_allocation() {
    for value in [
        "1b",
        "{unknown:{}}",
        "{mainhand:1b}",
        "{mainhand:{id:'minecraft:not_an_item'}}",
        "{mainhand:{id:1b}}",
        "{mainhand:{count:1}}",
        "{mainhand:{id:'minecraft:stick',count:'bad'}}",
        "{mainhand:{id:'minecraft:stick',count:-1}}",
        "{mainhand:{id:'minecraft:stick',unknown:1}}",
        "{mainhand:{id:'minecraft:stick',components:1b}}",
        "{mainhand:{id:'minecraft:stick',components:{'minecraft:damage':'bad'}}}",
        "{mainhand:{id:'minecraft:stick',components:{'minecraft:enchantments':{'minecraft:not_an_enchantment':1}}}}",
        "{mainhand:{id:'minecraft:stick',components:{'minecraft:not_a_component':1}}}",
        "{saddle:{id:'minecraft:saddle'}}",
    ] {
        rejected_tag_reaches_sender("cow", "equipment", value);
    }
    for value in ["1b", "{unknown:1f}", "{head:-1f}", "{head:'bad'}"] {
        rejected_tag_reaches_sender("cow", "drop_chances", value);
    }
    rejected_tag_reaches_sender("cow", "CanPickUpLoot", "'bad'");
    for key in ["equipment", "drop_chances", "CanPickUpLoot"] {
        rejected_tag_reaches_sender("oak_boat", key, "{}");
    }
}

#[test]
fn summon_equipment_components_match_give_and_survive_native_anvil() {
    let (state, id) = summon(
        "cow",
        "{CanPickUpLoot:1b,equipment:{mainhand:{id:'minecraft:diamond_sword',components:{'minecraft:damage':7,'minecraft:custom_name':'Summoned blade','minecraft:enchantments':{'minecraft:sharpness':3},'minecraft:custom_data':{source:'summon'}}},body:{id:'minecraft:leather_horse_armor',components:{'minecraft:custom_name':'Summoned armor'}}},drop_chances:{mainhand:2,offhand:0,feet:0.1f,legs:0.2f,chest:0.3f,head:0.4f,body:0.5f,saddle:0.6f}}",
    );
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    let held = row.appearance.equipment[0].as_ref().unwrap();
    let given = crate::command_items::ItemArgument::parse(
        "minecraft:diamond_sword[minecraft:damage=7,minecraft:custom_name='Summoned blade',minecraft:enchantments={'minecraft:sharpness':3},minecraft:custom_data={source:'summon'}]",
    )
    .unwrap()
    .stack(&state.registry.items)
    .unwrap();
    assert_eq!(held, &given);
    assert_eq!(held.damage, Some(7));
    assert_eq!(held.custom_name.as_deref(), Some("Summoned blade"));
    assert_eq!(held.enchantments[0].level, 3);
    assert!(row.loot.can_pick_up_loot);
    assert_eq!(row.loot.drop_chances, [2.0, 0.0, 0.1, 0.2, 0.3, 0.4]);
    assert_eq!(row.loot.extra_drop_chances, [0.5, 0.6]);
    let wal = crate::journal::EntityAppearanceImage::from(&*row.appearance);
    let wal: crate::journal::EntityAppearanceImage =
        serde_json::from_slice(&serde_json::to_vec(&wal).unwrap()).unwrap();
    assert_eq!(wal.equipment, row.appearance.equipment);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let root =
        std::env::temp_dir().join(format!("solaris-summon-equipment-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::write_entity_regions(&root, &state.registry.items, &chunks, 0).unwrap();
    let reopened = mc_world::anvil::read_entity_regions(&root, &state.registry.items).unwrap();
    let native = reopened
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == row.uuid)
        .unwrap();
    let restored = EntityStateImage::from_record(&state.registry, native).unwrap();
    let equipment =
        crate::entity_loot::equipment_from_fields(&state.registry.items, &native.extra_fields)
            .unwrap();
    assert_eq!(equipment, row.appearance.equipment);
    assert_eq!(restored.loot.body_armor, row.loot.body_armor);
    assert_eq!(restored.loot.drop_chances, row.loot.drop_chances);
    assert_eq!(
        restored.loot.extra_drop_chances,
        row.loot.extra_drop_chances
    );
    assert!(restored.loot.can_pick_up_loot);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn summon_equipment_rejects_the_same_unsupported_components_as_give() {
    assert!(
        crate::command_items::ItemArgument::parse(
            "minecraft:diamond_sword[minecraft:attack_range={}]"
        )
        .is_err()
    );
    rejected_tag_reaches_sender(
        "cow",
        "equipment",
        "{mainhand:{id:'minecraft:diamond_sword',components:{'minecraft:attack_range':{}}}}",
    );
}

#[test]
fn item_entity_summon_carries_stack_pickup_delay_age_and_motion() {
    let (state, id) = summon(
        "item",
        "{Item:{id:\"minecraft:carrot\",count:3},PickupDelay:25,Age:100,Motion:[0.0,0.5,0.0]}",
    );
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row(slot);
    let stack = row.item.as_ref().expect("item entity stack");
    assert_eq!(stack.count, 3);
    assert_eq!(
        state
            .registry
            .items
            .name_of(stack.item_id)
            .unwrap()
            .to_string(),
        "minecraft:carrot"
    );
    assert_eq!(state.entities.body(slot).pickup_delay, 25);
    assert_eq!(row.age, 100);
    assert!((state.entities.vel(slot)[1] - 10.0).abs() < 1e-9);
}

#[test]
fn item_entity_summon_rejects_missing_item_and_unknown_fields() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    for nbt in [
        "{}",
        "{Item:{id:\"minecraft:carrot\",count:0}}",
        "{Item:{id:\"minecraft:carrot\"},Owner:\"x\"}",
    ] {
        let parsed = crate::commands::parse(&format!("summon item ~ ~ ~ {nbt}"), true).unwrap();
        let mut changes = ChangeSet::default();
        crate::command_execution::execute(
            &mut state,
            ConnectionId(1),
            actor,
            &parsed,
            &mut changes,
        );
        assert!(changes.entities_spawned.is_empty(), "{nbt} must be refused");
    }
}

#[test]
fn villager_summon_keeps_its_authored_profession_and_rejects_bad_data() {
    let (state, id) = summon(
        "villager",
        "{VillagerData:{profession:'minecraft:farmer',type:'minecraft:plains',level:2}}",
    );
    let slot = state.entities.slot_of(id).unwrap();
    let data = &state.entities.row(slot).villager.as_ref().unwrap().data;
    assert_eq!(data.profession, crate::villager::VillagerProfession::Farmer);
    assert_eq!(data.level, 2);
    rejected_tag_reaches_sender("villager", "VillagerData", "{profession:'minecraft:bogus'}");
    rejected_tag_reaches_sender("cow", "VillagerData", "{profession:'minecraft:farmer'}");
}
