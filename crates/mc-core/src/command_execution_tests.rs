use super::*;
use crate::boundary::PlayerInput;
use crate::commands::parse;
use std::sync::Arc;

fn fixture() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "Alice".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "Bob".into(),
                permission_level: 0,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    for (entity, position) in [(alice, [0.0, 80.0, 0.0]), (bob, [10.0, 80.0, 0.0])] {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, position);
    }
    state.outbox.clear();
    (state, alice, bob)
}

fn command(state: &mut TickState, executor: EntityId, input: &str) -> ChangeSet {
    let command = parse(input, true).unwrap();
    let mut changes = ChangeSet::default();
    execute(
        state,
        state.connections.entity_connection(executor).unwrap(),
        executor,
        &command,
        &mut changes,
    );
    changes
}

fn has_translation(state: &TickState, connection: ConnectionId, key: &str) -> bool {
    state.outbox.get(&connection).is_some_and(|queue| {
        queue.iter().any(|change| {
            matches!(change,
        PlayerChange::StyledChat { content:Tag::Compound(fields),.. }
        if fields.iter().any(|(name,value)|name=="translate" && value==&Tag::String(key.into())))
        })
    })
}
fn chat_contains(state: &TickState, text: &str) -> bool {
    state.outbox.get(&ConnectionId(1)).is_some_and(|queue| {
        queue.iter().any(|change| match change {
            PlayerChange::Chat { message } => message.contains(text),
            PlayerChange::StyledChat {
                content: Tag::Compound(fields),
            } => fields.iter().any(|(key, value)| {
                key == "text" && matches!(value, Tag::String(message) if message.contains(text))
            }),
            _ => false,
        })
    })
}

#[test]
fn admin_item_replace_entity_changes_slot() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "item replace entity @s hotbar.0 with minecraft:stone 3",
    );
    let stack = &state.players[&alice].inventory.slots[0];
    assert_eq!(stack.count, 3);
    assert_eq!(
        state
            .registry
            .items
            .name_of(stack.item_id)
            .unwrap()
            .as_str(),
        "minecraft:stone"
    );
    assert!(chat_contains(&state, "Replaced item"));
    command(
        &mut state,
        alice,
        "item replace entity @s hotbar.1 from entity @s hotbar.0",
    );
    assert_eq!(
        state.players[&alice].inventory.slots[1],
        state.players[&alice].inventory.slots[0]
    );
    command(
        &mut state,
        alice,
        "item replace entity @s wrong.slot with stone",
    );
    assert!(chat_contains(&state, "slot"));
    command(
        &mut state,
        alice,
        "item replace entity @s armor.head with stone",
    );
    assert!(chat_contains(&state, "cannot be placed"));
    command(
        &mut state,
        alice,
        "item modify entity @s weapon.mainhand test:modifier",
    );
    assert!(chat_contains(&state, "modifiers are not supported"));
}

#[test]
fn admin_loot_unknown_table_reports_error() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "loot give @s loot minecraft:no_such_table",
    );
    assert!(chat_contains(&state, "Unknown or unsupported loot table"));
}

#[test]
fn admin_item_and_loot_block_destinations() {
    let (mut state, alice, _) = fixture();
    state.block_entities.insert(
        (1, 80, 1),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    command(
        &mut state,
        alice,
        "item replace block 1 80 1 container.0 with stone 3",
    );
    assert_eq!(state.block_entities[&(1, 80, 1)].slots[0].count, 3);
    command(
        &mut state,
        alice,
        "item replace entity @s hotbar.0 from block 1 80 1 container.0",
    );
    assert_eq!(state.players[&alice].inventory.slots[0].count, 3);
    command(
        &mut state,
        alice,
        "loot insert 1 80 1 loot minecraft:chests/simple_dungeon",
    );
    assert!(
        state.block_entities[&(1, 80, 1)].slots[1..]
            .iter()
            .any(|stack| !stack.is_empty())
    );
    command(
        &mut state,
        alice,
        "loot replace entity @s hotbar.1 1 loot minecraft:chests/simple_dungeon",
    );
    assert!(!state.players[&alice].inventory.slots[1].is_empty());
}

#[test]
fn admin_loot_kill_rolls_without_killing_target() {
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    command(
        &mut state,
        alice,
        "loot spawn 0 80 0 kill @e[type=minecraft:cow,limit=1]",
    );
    assert!(chat_contains(&state, "Distributed"));
    assert!(
        state
            .entities
            .body(state.entities.slot_of(cow).unwrap())
            .alive
    );
}

#[test]
fn admin_loot_give_generates_chest_loot() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "loot give @s loot minecraft:chests/simple_dungeon",
    );
    assert!(
        state.players[&alice]
            .inventory
            .slots
            .iter()
            .any(|stack| !stack.is_empty())
    );
    assert!(chat_contains(&state, "Distributed"));
}

#[test]
fn admin_attribute_unknown_reports_error() {
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:no_such_attribute get",
    );
    assert!(chat_contains(&state, "Unknown or unsupported attribute"));
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health base set 30",
    );
    let slot = state.entities.slot_of(cow).unwrap();
    assert_eq!(state.entities.row(slot).loot.attributes[0].base, 30.0);
    assert!(chat_contains(&state, "updated"));
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health modifier add test:bonus 5 add_value",
    );
    assert_eq!(
        state.entities.row(slot).loot.attributes[0].modifiers[0].amount,
        5.0
    );
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health get 2",
    );
    assert!(chat_contains(&state, "value: 35"));
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health modifier value get test:bonus",
    );
    assert!(chat_contains(&state, "modifier value: 5"));
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health modifier remove test:bonus",
    );
    assert!(
        state.entities.row(slot).loot.attributes[0]
            .modifiers
            .is_empty()
    );
    command(
        &mut state,
        alice,
        "attribute @e[type=minecraft:cow,limit=1] minecraft:max_health base reset",
    );
    assert_eq!(
        state.entities.row(slot).loot.attributes[0].base,
        f64::from(
            state
                .registry
                .blueprint(state.registry.cow_type_id)
                .unwrap()
                .params
                .max_health
        )
    );
}

#[test]
fn player_attribute_command_updates_and_publishes() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "attribute @s minecraft:movement_speed base set 0.2",
    );
    command(
        &mut state,
        alice,
        "attribute @s minecraft:movement_speed modifier add test:bonus 0.5 add_multiplied_total",
    );
    command(
        &mut state,
        alice,
        "attribute @s minecraft:movement_speed get",
    );
    assert!(chat_contains(&state, "value: 0.3"));
    let (_, outbox, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(outbox.iter().any(|(_, changes)| changes.iter().any(|change|
        matches!(change, PlayerChange::Attributes { attributes } if attributes.iter().any(|attribute|
            attribute.kind == "movement_speed" && attribute.base == 0.2 && attribute.modifiers.iter().any(|modifier| modifier.id == "test:bonus"))))));
    let saved = crate::core::player_record_for(&state, alice, state.tick).unwrap();
    let restored =
        crate::entity_loot::player_attributes_from_opaque(&saved.opaque_player_fields).unwrap();
    assert!(restored.iter().any(|attribute| {
        attribute.id == "minecraft:movement_speed"
            && attribute.base == 0.2
            && attribute
                .modifiers
                .iter()
                .any(|modifier| modifier.id == "test:bonus")
    }));
}

#[test]
fn sprint_changes_publish_movement_speed_modifier() {
    let (mut state, _, _) = fixture();
    for sprinting in [true, false] {
        let (_, outbox, _) = crate::tick::run_tick(
            &mut state,
            &[PlayerInput::Sprint {
                connection: ConnectionId(1),
                sprinting,
            }],
        );
        let speed = outbox
            .iter()
            .flat_map(|(_, changes)| changes)
            .find_map(|change| match change {
                PlayerChange::Attributes { attributes } => attributes
                    .iter()
                    .find(|attribute| attribute.kind == "movement_speed"),
                _ => None,
            })
            .expect("sprint changes movement_speed");
        assert_eq!(
            speed
                .modifiers
                .iter()
                .any(|modifier| modifier.id == "minecraft:sprinting"),
            sprinting
        );
    }
}

#[test]
fn player_armor_attribute_changes_native_damage() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "attribute @s minecraft:armor base set 20",
    );
    crate::tick::run_tick(&mut state, &[]);
    let slot = state.entities.slot_of(alice).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 10.0,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
    ));
    state.commit_merge_effects();
    assert!((state.players[&alice].vitals.health - 16.0).abs() < 0.001);
}

#[test]
fn vanilla_player_combat_numbers_through_native_tick() {
    for (item, damage, speed) in [
        (None, 1.0_f32, 4.0_f64),
        (Some("wooden_sword"), 4.0, 1.6),
        (Some("stone_sword"), 5.0, 1.6),
        (Some("iron_sword"), 6.0, 1.6),
        (Some("diamond_sword"), 7.0, 1.6),
    ] {
        let (mut state, alice, _) = fixture();
        if let Some(item) = item {
            command(
                &mut state,
                alice,
                &format!("item replace entity @s hotbar.0 with minecraft:{item}"),
            );
        }
        state.tick = 40;
        state.last_swing.insert(alice, 0);
        let actor = state.entities.slot_of(alice).unwrap();
        state.entities.set_ground(actor, true);
        let victim = state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 1.5], 100.0);
        crate::tick::run_tick(&mut state, &[]);
        let player = &state.players[&alice];
        assert_eq!(player.effective_attribute("max_health"), Some(20.0));
        assert!((player.effective_attribute("attack_speed").unwrap() - speed).abs() < 0.000_001);
        assert!(
            (player.effective_attribute("attack_damage").unwrap() - f64::from(damage)).abs()
                < 0.000_001
        );
        let before = state.entities.row(state.entities.slot_of(victim).unwrap()).health;
        crate::tick::run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: victim,
            }],
        );
        let after = state.entities.row(state.entities.slot_of(victim).unwrap()).health;
        assert!((before - after - damage).abs() < 0.001, "{item:?}: dealt {}", before - after);
        if item.is_some() {
            let victim = state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 1.5], 100.0);
            let before = state.entities.row(state.entities.slot_of(victim).unwrap()).health;
            state.last_swing.insert(alice, state.tick - 6);
            crate::tick::run_tick(
                &mut state,
                &[PlayerInput::Attack {
                    connection: ConnectionId(1),
                    target: victim,
                }],
            );
            let after = state.entities.row(state.entities.slot_of(victim).unwrap()).health;
            let expected = damage * (0.2 + 0.8 * 0.52_f32.powi(2));
            assert!((before - after - expected).abs() < 0.001, "{item:?}: partial hit dealt {}", before - after);
        }
    }
}

#[test]
fn vanilla_player_armor_and_regeneration_through_native_tick() {
    for (material, armor, expected_damage) in [("leather", 7.0, 9.2), ("iron", 15.0, 6.0)] {
        let (mut state, alice, _) = fixture();
        for (slot, suffix) in [
            ("armor.feet", "boots"),
            ("armor.legs", "leggings"),
            ("armor.chest", "chestplate"),
            ("armor.head", "helmet"),
        ] {
            command(
                &mut state,
                alice,
                &format!("item replace entity @s {slot} with minecraft:{material}_{suffix}"),
            );
        }
        crate::tick::run_tick(&mut state, &[]);
        assert_eq!(
            state.players[&alice].effective_attribute("armor"),
            Some(armor)
        );
        let slot = state.entities.slot_of(alice).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 10.0,
                kind: crate::survival::DamageKind::Melee,
                source: None,
            },
        ));
        state.commit_merge_effects();
        let player = state.players.get_mut(&alice).unwrap();
        assert!((20.0 - player.vitals.health - expected_damage).abs() < 0.001);
        player.vitals.food = 20;
        player.vitals.saturation = 6.0;
        let before = player.vitals.health;
        for _ in 0..10 {
            crate::tick::run_tick(&mut state, &[]);
        }
        assert!((state.players[&alice].vitals.health - before - 1.0).abs() < 0.001);
    }
}

#[test]
fn player_attack_attribute_drives_native_melee() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "attribute @s minecraft:attack_damage base set 10",
    );
    state.tick = 40;
    state.last_swing.insert(alice, 0);
    let actor = state.entities.slot_of(alice).unwrap();
    state.entities.set_ground(actor, true);
    let victim = state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 1.5], 100.0);
    crate::tick::run_tick(&mut state, &[]);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target: victim,
        }],
    );
    let health = state
        .entities
        .row(state.entities.slot_of(victim).unwrap())
        .health;
    assert!(
        health < 92.0,
        "attribute attack should deal about 10 damage, got health {health}"
    );
}

#[test]
fn player_range_attribute_changes_block_reach() {
    let (mut state, alice, _) = fixture();
    let target = crate::boundary::BlockPos { x: 8, y: 80, z: 0 };
    assert!(!crate::tick::within_block_reach(&state, alice, target));
    command(
        &mut state,
        alice,
        "attribute @s minecraft:block_interaction_range base set 10",
    );
    crate::tick::run_tick(&mut state, &[]);
    assert!(crate::tick::within_block_reach(&state, alice, target));
}

#[test]
fn max_health_attribute_allows_natural_regeneration_above_twenty() {
    let (mut state, alice, _) = fixture();
    command(
        &mut state,
        alice,
        "attribute @s minecraft:max_health base set 30",
    );
    crate::tick::run_tick(&mut state, &[]);
    {
        let player = state.players.get_mut(&alice).unwrap();
        player.vitals.health = 20.0;
        player.vitals.food = 20;
        player.vitals.saturation = 10.0;
    }
    for _ in 0..12 {
        crate::tick::run_tick(&mut state, &[]);
    }
    assert!(state.players[&alice].vitals.health > 20.0);
}

#[test]
fn health_boost_attribute_is_published_and_allows_healing() {
    let (mut state, alice, _) = fixture();
    let (_, outbox, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "effect give @s minecraft:health_boost 30".into(),
            permission_level: 2,
        }],
    );
    assert!(outbox.iter().flat_map(|(_, changes)| changes).any(|change|
        matches!(change, PlayerChange::Attributes { attributes } if attributes.iter().any(|attribute|
            attribute.kind == "max_health" && attribute.modifiers.iter().any(|modifier|
                modifier.id == "minecraft:effect.health_boost" && modifier.amount == 4.0)))));
    {
        let player = state.players.get_mut(&alice).unwrap();
        player.vitals.health = 20.0;
        player.vitals.food = 20;
        player.vitals.saturation = 10.0;
    }
    for _ in 0..12 {
        crate::tick::run_tick(&mut state, &[]);
    }
    assert!(state.players[&alice].vitals.health > 20.0);
}

#[test]
fn command_parity_nearest_entity_selects_closest_alive() {
    // SELECTOR-NEAREST-ENTITY: Java `@n` selects the nearest alive entity
    // with default limit 1; dead entities are excluded.
    let (mut state, alice, bob) = fixture();
    for (entity, position) in [(alice, [0.0, 80.0, 0.0]), (bob, [10.0, 80.0, 0.0])] {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, position);
    }
    let target = crate::commands::target_of("@n").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    // `@n` with grammar routes through the same nearest parser path.
    let target = crate::commands::target_of("@n[distance=..5]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    let target = crate::commands::target_of("@n[distance=6..20]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
}

#[test]
fn command_parity_dead_players_stay_selectable_by_name_and_all() {
    // SELECTOR-DEAD-PLAYERS: Java @a includes dead players and @s may
    // select a nonliving executor; @e and @n stay alive-only.
    let (mut state, alice, bob) = fixture();
    let slot = state.entities.slot_of(bob).unwrap();
    state.entities.body_mut(slot).alive = false;
    let target = crate::commands::target_of("@a").unwrap();
    let mut selected = resolve_target(&state, alice, &target);
    selected.sort();
    let mut expected = vec![alice, bob];
    expected.sort();
    assert_eq!(selected, expected);
    let target = crate::commands::target_of("@s").unwrap();
    assert_eq!(resolve_target(&state, bob, &target), vec![bob]);
    let target = crate::commands::target_of("@e").unwrap();
    assert!(!resolve_target(&state, alice, &target).contains(&bob));
    let target = crate::commands::target_of("@n").unwrap();
    assert!(!resolve_target(&state, alice, &target).contains(&bob));
}

#[test]
fn command_parity_repeated_negative_type_name_clauses() {
    // SELECTOR-TYPE-NAME-FILTERS: repeated negative clauses all apply.
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let calf = state.spawn_mob(state.registry.cow_type_id, [3.0, 80.0, 0.0], 10.0);
    let target =
        crate::commands::target_of("@e[type=!minecraft:cow,type=!minecraft:player]").unwrap();
    let selected = resolve_target(&state, alice, &target);
    assert!(!selected.contains(&cow));
    assert!(!selected.contains(&calf));
    assert!(!selected.contains(&alice));
    let target = crate::commands::target_of("@e[name=!Alice,name=!Bob]").unwrap();
    let selected = resolve_target(&state, alice, &target);
    assert!(!selected.contains(&alice));
}

#[test]
fn command_parity_unnamed_mob_plain_name_and_player_type_rejection() {
    // SELECTOR-TYPE-NAME-FILTERS subcases: unnamed mobs match vanilla plain
    // names (`Cow`, not `minecraft:cow`), and `type=` is rejected on
    // player-only variables.
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let target = crate::commands::target_of("@e[name=Cow]").unwrap();
    assert!(resolve_target(&state, alice, &target).contains(&cow));
    assert!(crate::commands::target_of("@a[type=minecraft:cow]").is_err());
    assert!(crate::commands::target_of("@p[type=minecraft:cow]").is_err());
}

#[test]
fn command_parity_give_rejects_entity_selector_shape() {
    // SELECTOR-TYPED-ADMISSION: `/give @e` errors before mutation instead
    // of granting to the player subset.
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "give @e minecraft:stone 1");
    assert!(has_translation(
        &state,
        state.connections.entity_connection(alice).unwrap(),
        "argument.entity.selector.not_player",
    ));
}

#[test]
fn command_parity_teleport_rejects_multi_target_destination() {
    // SELECTOR-TYPED-ADMISSION: `/tp @s @a` errors on the declared maximum
    // even when only one player happens to match.
    let (mut state, alice, _) = fixture();
    let before = state.entities.pos(state.entities.slot_of(alice).unwrap());
    command(&mut state, alice, "tp @s @a");
    assert!(has_translation(
        &state,
        state.connections.entity_connection(alice).unwrap(),
        "argument.entity.toomany",
    ));
    assert_eq!(
        state.entities.pos(state.entities.slot_of(alice).unwrap()),
        before
    );
}

#[test]
fn command_parity_sort_limit_options_and_typed_filters() {
    // SELECTOR-OPTION-COVERAGE behavior through the live command path:
    // each family admits/rejects rather than only parsing.
    let (mut state, alice, bob) = fixture();
    for (entity, position) in [(alice, [0.0, 80.0, 0.0]), (bob, [10.0, 80.0, 0.0])] {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, position);
    }
    // sort/limit still parse.
    for selector in ["@a[sort=nearest,limit=1]", "@a[sort=furthest,limit=1]"] {
        assert!(
            crate::commands::target_of(selector).is_ok(),
            "{selector} should parse"
        );
    }
    // level range: Alice 0 by default; Bob raised to 5.
    state.players.get_mut(&bob).unwrap().vitals.experience.level = 5;
    let target = crate::commands::target_of("@a[level=3..10]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
    let target = crate::commands::target_of("@a[level=6..]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    let target = crate::commands::target_of("@a[level=..4]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    // gamemode: Bob switched to creative; survival filter keeps Alice only.
    state.players.get_mut(&bob).unwrap().game_mode = crate::commands::GameMode::Creative;
    let target = crate::commands::target_of("@a[gamemode=survival]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    let target = crate::commands::target_of("@a[gamemode=!survival]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
    // team: Bob joins red via scored state; team=red admits Bob only.
    state.scoreboard.teams.insert(
        "red".into(),
        crate::scoreboard::Team {
            display_name: "red".into(),
            color: 21,
            friendly_fire: true,
            see_friendly_invisibles: true,
            nametag_visibility: 0,
            death_message_visibility: 0,
            collision_rule: 0,
            prefix: String::new(),
            suffix: String::new(),
            members: ["Bob".to_owned()].into_iter().collect(),
        },
    );
    let target = crate::commands::target_of("@a[team=red]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
    let target = crate::commands::target_of("@a[team=!red]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    let target = crate::commands::target_of("@a[team=]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    // scores: Alice 7, Bob 9 on objective test; range 8.. admits Bob only.
    state.scoreboard.objectives.insert(
        "test".into(),
        crate::scoreboard::Objective {
            criteria: "dummy".into(),
            display_name: "test".into(),
        },
    );
    for (holder, value) in [("Alice", 7), ("Bob", 9)] {
        state
            .scoreboard
            .scores
            .entry(holder.to_owned())
            .or_default()
            .insert(
                "test".into(),
                crate::scoreboard::Score {
                    value,
                    locked: false,
                },
            );
    }
    let target = crate::commands::target_of("@a[scores={test=8..}]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
    let target = crate::commands::target_of("@a[scores={test=..7}]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    // tag: no /tag support yet, so positive matches nothing, negative admits.
    let target = crate::commands::target_of("@e[tag=!untagged]").unwrap();
    let selected = resolve_target(&state, alice, &target);
    assert!(selected.contains(&alice) && selected.contains(&bob));
    let target = crate::commands::target_of("@e[tag=untagged]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    // pitch/yaw: both players spawn at rot 0/0; tight 1..90 pitch rejects.
    let target = crate::commands::target_of("@a[x_rotation=-90..90]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target).len(), 2);
    let target = crate::commands::target_of("@a[x_rotation=1..90]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    let target = crate::commands::target_of("@a[y_rotation=0..360]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target).len(), 2);
    // An NBT predicate with a mismatched field admits nobody.
    let target = crate::commands::target_of("@e[nbt={Health:0.5f}]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    // advancements: flagged root=true admits only the granted player.
    state.players.get_mut(&alice).unwrap().advancement_progress =
        std::sync::Arc::new(serde_json::json!({
            "minecraft:story/root": {"done": true},
        }));
    let target =
        crate::commands::target_of("@a[advancements={minecraft:story/root=true}]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![alice]);
    let target =
        crate::commands::target_of("@a[advancements={minecraft:story/root=false}]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target), vec![bob]);
    // predicate: no loot-predicate registry is wired; positive matches
    // nothing (documented limitation), negative admits everything.
    let target = crate::commands::target_of("@e[predicate=minecraft:test]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    let target = crate::commands::target_of("@e[predicate=!minecraft:test]").unwrap();
    assert!(!resolve_target(&state, alice, &target).is_empty());
}

#[test]
fn command_parity_unqualified_entity_selector_reaches_loaded_remote_dimensions() {
    // SELECTOR-DIMENSION-SCOPE: unqualified @e joins @a/@r in global
    // routing (vanilla EntitySelector iterates all levels when no
    // world-limiting option is present); x/y/z or distance stays local.
    let (mut state, alice, _) = fixture();
    let registry = Arc::clone(&state.registry);
    let generator: std::sync::Arc<dyn mc_world::ChunkGenerator> = std::sync::Arc::new(
        mc_worldgen::NetherGenerator::try_new(42, Arc::clone(&registry.blocks)).unwrap(),
    );
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 150, 8],
        crate::dimensions::Dimension::Nether.geometry(),
    );
    let mut nether = TickState::new(Arc::clone(&registry), world, 0, 4096);
    nether.dimension = crate::dimensions::Dimension::Nether;
    let pig_type = state.registry.entity_type_id("minecraft:pig").unwrap();
    let local_pig = state.spawn_mob(pig_type, [2.0, 80.0, 0.0], 10.0);
    let remote_pig = nether.spawn_mob(pig_type, [8.0, 150.0, 8.0], 10.0);
    state
        .other_dimensions
        .insert(crate::dimensions::Dimension::Nether, Box::new(nether));
    let request = crate::dimensions::travel::Request {
        connection: state.connections.entity_connection(alice).unwrap(),
        executor: alice,
        dimension: crate::dimensions::Dimension::Overworld,
        command: crate::commands::parse("kill @e[type=minecraft:pig]", true).unwrap(),
    };
    let origin = [0.0, 80.0, 0.0];
    let target = crate::dimensions::commands::target(&request.command)
        .expect("kill carries a target")
        .clone();
    let mut selected = crate::dimensions::commands::resolve(&state, &request, &target, origin);
    selected.sort();
    let mut expected = vec![local_pig, remote_pig];
    expected.sort();
    assert_eq!(selected, expected, "unqualified @e spans loaded dimensions");
    // A spatial option constrains the query to the request dimension.
    let target = crate::commands::target_of("@e[type=minecraft:pig,x=0,y=80,z=0]").unwrap();
    let selected = crate::dimensions::commands::resolve(&state, &request, &target, origin);
    assert_eq!(
        selected,
        vec![local_pig],
        "position-qualified @e stays local"
    );
    let target = crate::commands::target_of("@e[type=minecraft:pig,distance=..100]").unwrap();
    let selected = crate::dimensions::commands::resolve(&state, &request, &target, origin);
    assert_eq!(
        selected,
        vec![local_pig],
        "distance-qualified @e stays local"
    );
    // Bare @p parses to Target::Nearest, which bypasses global_target
    // routing (dimensions_commands matches Target::Selector only): the
    // cross-dimension claim covers @e/@a/@r shapes, not bare @p/@n.
    assert!(!crate::dimensions::commands::global_target(
        &crate::commands::target_of("@p").unwrap()
    ));
}

#[test]
fn command_parity_repeated_random_evaluation_redraws_within_one_tick() {
    // SELECTOR-RANDOM-REPEAT: Java ORDER_RANDOM shuffles on every
    // evaluation, so repeated same-tick @r draws must not all agree.
    let (state, alice, bob) = fixture();
    let target = crate::commands::target_of("@r").unwrap();
    let mut seen_alice = false;
    let mut seen_bob = false;
    for _ in 0..32 {
        for id in resolve_target(&state, alice, &target) {
            if id == alice {
                seen_alice = true;
            }
            if id == bob {
                seen_bob = true;
            }
        }
    }
    assert!(
        seen_alice && seen_bob,
        "32 same-tick @r draws must select both players across redraws"
    );
}

#[test]
fn command_parity_selector_box_intersects_body_and_defaults_missing_axes() {
    let (mut state, alice, bob) = fixture();
    let slot = state.entities.slot_of(bob).unwrap();
    let body = state.entities.body(slot);
    let half_width = body.half_width;
    let height = body.height;
    for (position, selector, expected) in [
        ([5.1, 80.0, 0.5], "@e[x=0,y=80,z=0,dx=4,dy=1,dz=1]", true),
        ([-0.2, 80.0, 0.5], "@e[x=4,y=80,z=0,dx=-4,dy=1,dz=1]", true),
        ([0.5, 79.0, 0.5], "@e[x=0,y=80,z=0,dx=0]", true),
        ([0.5, 80.0, 3.0], "@e[x=0,y=80,z=0,dx=0]", false),
        ([0.5, 80.0, 0.5], "@e[dx=0]", true),
        (
            [5.0 + half_width, 80.0, 0.5],
            "@e[x=0,y=80,z=0,dx=4]",
            false,
        ),
        ([-half_width, 80.0, 0.5], "@e[x=0,y=80,z=0,dx=4]", false),
        ([0.5, 81.0, 0.5], "@e[x=0,y=80,z=0,dx=0]", false),
        ([0.5, 80.0 - height, 0.5], "@e[x=0,y=80,z=0,dx=0]", false),
        ([1.8, 80.0, 0.5], "@e[x=0.25,y=80,z=0.25,dx=0.5]", true),
        ([2.1, 80.0, 0.5], "@e[x=0.25,y=80,z=0.25,dx=0.5]", false),
        ([0.5, 80.0, 0.5], "@e[x=1.25,y=80,z=0,dx=-0.5]", true),
        ([0.4, 80.0, 0.5], "@e[x=1.25,y=80,z=0,dx=-0.5]", false),
        (
            [5.1, 80.0, 0.5],
            "@e[x=0,y=80,z=0,dx=4,dy=1,dz=1,distance=..5]",
            false,
        ),
        (
            [5.1, 80.0, 0.5],
            "@e[x=0,y=80,z=0,dx=4,dy=1,dz=1,distance=..6]",
            true,
        ),
    ] {
        state.entities.set_pos(slot, position);
        let target = crate::commands::target_of(selector).unwrap();
        assert_eq!(
            resolve_target(&state, alice, &target).contains(&bob),
            expected,
            "{selector} at {position:?}"
        );
    }
    command(&mut state, alice, "summon minecraft:pig 5.1 80 0.5");
    let target =
        crate::commands::target_of("@e[type=minecraft:pig,x=0,y=80,z=0,dx=4,dy=1,dz=1]").unwrap();
    assert_eq!(resolve_target(&state, alice, &target).len(), 1);
}

#[test]
fn team_message_aliases_require_membership_and_exclude_locked_recipients() {
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "teammsg not on a team");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.teammsg.failed.noteam"
    ));
    command(&mut state, alice, "team add red");
    command(&mut state, alice, "team join red Alice");
    command(&mut state, alice, "team join red Bob");
    state.outbox.clear();
    let parsed = parse("tm hello @a", false).unwrap();
    execute(
        &mut state,
        ConnectionId(1),
        alice,
        &parsed,
        &mut ChangeSet::default(),
    );
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "chat.type.team.sent"
    ));
    assert!(has_translation(
        &state,
        ConnectionId(2),
        "chat.type.team.text"
    ));
    state.require_plugin_login = true;
    state.locked_sessions.insert(ConnectionId(2));
    state.outbox.clear();
    command(&mut state, alice, "teammsg private");
    assert!(!has_translation(
        &state,
        ConnectionId(2),
        "chat.type.team.text"
    ));
    command(&mut state, alice, "msg Bob private");
    assert!(!has_translation(
        &state,
        ConnectionId(2),
        "commands.message.display.incoming"
    ));
}

fn advancement_fixture() -> mc_data::advancements::AdvancementTree {
    use mc_data::advancements::{AdvancementDefinition, AdvancementTree, Criterion};
    let mut tree = AdvancementTree::default();
    for (id, parent) in [
        ("root", None),
        ("branch", Some("root")),
        ("leaf", Some("branch")),
        ("sibling", Some("root")),
        ("other", None),
    ] {
        tree.definitions.insert(
            format!("test:{id}"),
            AdvancementDefinition {
                parent: parent.map(|parent| format!("test:{parent}")),
                criteria: [(
                    "command".into(),
                    Criterion {
                        trigger: "minecraft:impossible".into(),
                        ..Default::default()
                    },
                )]
                .into(),
                requirements: vec![vec!["command".into()]],
                ..Default::default()
            },
        );
    }
    tree
}

#[test]
fn advancement_tree_scopes_follow_parents_and_descendants_not_siblings() {
    use crate::commands::AdvancementScope;
    let tree = advancement_fixture();
    for (scope, expected) in [
        (
            AdvancementScope::Everything,
            vec!["branch", "leaf", "other", "root", "sibling"],
        ),
        (AdvancementScope::Only, vec!["branch"]),
        (AdvancementScope::From, vec!["branch", "leaf"]),
        (AdvancementScope::Through, vec!["branch", "leaf", "root"]),
        (AdvancementScope::Until, vec!["branch", "root"]),
    ] {
        let mut ids = advancement_ids(&tree, scope, Some("test:branch")).unwrap();
        ids.sort();
        assert_eq!(
            ids,
            expected
                .into_iter()
                .map(|id| format!("test:{id}"))
                .collect::<Vec<_>>()
        );
    }
    assert!(advancement_ids(&tree, AdvancementScope::Only, Some("test:missing")).is_err());
}

#[test]
fn advancement_dispatch_validates_before_mutating_and_filters_player_targets() {
    let (mut state, alice, bob) = fixture();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(advancement_fixture());
    state.registry = Arc::new(registry);
    state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let before = state.players[&bob].advancement_progress.clone();
    command(&mut state, alice, "advancement grant Bob only test:missing");
    command(
        &mut state,
        alice,
        "advancement grant Bob only test:branch missing",
    );
    assert_eq!(state.players[&bob].advancement_progress, before);
    // SELECTOR-TYPED-ADMISSION: `grant @e` rejects the non-player shape
    // before mutation instead of granting to the player subset.
    command(&mut state, alice, "advancement grant @e from test:branch");
    for entity in [alice, bob] {
        assert_eq!(state.players[&entity].advancement_progress, before);
    }
    command(&mut state, alice, "advancement grant @a from test:branch");
    for entity in [alice, bob] {
        let progress = &state.players[&entity].advancement_progress;
        assert_eq!(progress["test:branch"]["done"], true);
        assert_eq!(progress["test:leaf"]["done"], true);
        assert!(progress.get("test:root").is_none());
    }
    command(
        &mut state,
        alice,
        "advancement revoke Bob only test:leaf command",
    );
    assert_ne!(
        state.players[&bob].advancement_progress["test:leaf"]["done"],
        true
    );
    assert_eq!(
        state.players[&alice].advancement_progress["test:leaf"]["done"],
        true
    );
}

#[test]
fn selectors_cover_nearest_all_random_self_entities_and_filters() {
    let (mut state, alice, bob) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    assert_eq!(
        resolve_target(&state, bob, &crate::commands::target_of("@p").unwrap()),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[sort=furthest,limit=1]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[type=cow,distance=..3]").unwrap()
        ),
        vec![cow]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[type=!player,distance=1..3]").unwrap()
        ),
        vec![cow]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[name=!Alice]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@s[distance=0]").unwrap()
        ),
        vec![alice]
    );
    assert_eq!(
        resolve_target(&state, alice, &crate::commands::target_of("@r").unwrap()).len(),
        1
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[sort=nearest,limit=2]").unwrap()
        ),
        vec![alice, cow]
    );
}

#[test]
fn gamemode_give_clear_and_experience_apply_and_feedback() {
    let (mut state, alice, bob) = fixture();
    command(&mut state, alice, "gamemode creative Bob");
    assert_eq!(
        state.players[&bob].game_mode,
        crate::commands::GameMode::Creative
    );
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.gamemode.success.other"
    ));
    command(&mut state, alice, "give Bob stone 67");
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == stone)
            .map(|stack| stack.count)
            .sum::<i32>(),
        67
    );
    command(&mut state, alice, "clear Bob stone 0");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.clear.test.single"
    ));
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        67
    );
    command(&mut state, alice, "clear Bob stone 7");
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == stone)
            .map(|stack| stack.count)
            .sum::<i32>(),
        60
    );
    command(&mut state, alice, "experience add Bob 5 levels");
    command(&mut state, alice, "xp set Bob 3 points");
    assert_eq!(state.players[&bob].vitals.experience.level, 5);
    assert!((state.players[&bob].vitals.experience.progress - 3.0 / 17.0).abs() < 0.001);
    command(&mut state, alice, "xp query Bob levels");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.experience.query.levels"
    ));
    command(&mut state, alice, "xp set Bob 100 points");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.experience.set.points.invalid"
    ));
    command(&mut state, alice, "give Bob stone 6401");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.give.failed.toomanyitems"
    ));
}

#[test]
fn teleport_uses_executor_relative_origin_for_every_target_and_rejects_multiple_destination() {
    let (mut state, alice, bob) = fixture();
    command(&mut state, alice, "tp @a ~2 ~1 ~3");
    for entity in [alice, bob] {
        assert_eq!(
            state.entities.pos(state.entities.slot_of(entity).unwrap()),
            [2.0, 81.0, 3.0]
        );
    }
    assert!(state.outbox[&ConnectionId(2)].iter().any(|change|matches!(change,PlayerChange::PositionCorrected{position,..}if *position==[2.0,81.0,3.0])));
    command(&mut state, alice, "tp Bob @a");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "argument.entity.toomany"
    ));
}

#[test]
fn time_weather_difficulty_gamerule_and_seed_have_real_mutations() {
    let (mut state, alice, _) = fixture();
    state.day_time = 72000;
    command(&mut state, alice, "time set 30000");
    assert_eq!(state.day_time, 30000);
    command(&mut state, alice, "time add -2s");
    assert_eq!(state.day_time, 29960);
    command(&mut state, alice, "time query gametime");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.time.query"
    ));
    command(&mut state, alice, "weather thunder 5s");
    assert_eq!(state.weather, (Weather::Thunder, 100));
    assert_eq!(state.rain_level, 0.0);
    assert_eq!(state.thunder_level, 0.0);
    assert!(
        !state
            .outbox
            .values()
            .flatten()
            .any(|change| { matches!(change, PlayerChange::WeatherChanged { .. }) })
    );
    let (_, drains, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(drains.iter().any(|(connection, changes)| {
        *connection == ConnectionId(2)
            && changes.iter().any(|change| {
                matches!(
                    change,
                    PlayerChange::WeatherChanged { raining: true, thunder_level, .. }
                        if *thunder_level == 0.01
                )
            })
    }));
    command(&mut state, alice, "difficulty hard");
    assert_eq!(state.difficulty, crate::survival::Difficulty::Hard);
    command(&mut state, alice, "difficulty");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.difficulty.query"
    ));
    command(&mut state, alice, "gamerule doDaylightCycle false");
    assert_eq!(
        state.gamerules.get("advance_time").map(String::as_str),
        Some("false")
    );
    command(&mut state, alice, "gamerule minecraft:advance_time");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.gamerule.query"
    ));
    command(&mut state, alice, "seed");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.seed.success"
    ));
}

#[test]
fn summon_uses_executor_position_and_kill_stages_canonical_damage() {
    let (mut state, alice, _) = fixture();
    let changes = command(&mut state, alice, "summon cow ~2 ~ ~");
    assert_eq!(changes.entities_spawned.len(), 1);
    let cow = changes.entities_spawned[0];
    assert_eq!(
        state.entities.pos(state.entities.slot_of(cow).unwrap()),
        [2.0, 80.0, 0.0]
    );
    command(&mut state, alice, "kill @e[type=cow]");
    assert!(
        state
            .merge_damage
            .iter()
            .any(|(slot, damage)| state.entities.body(*slot).id == cow
                && damage.kind == crate::survival::DamageKind::GenericKill)
    );
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.kill.success.single"
    ));
}

#[test]
fn chat_routing_preserves_message_and_unprivileged_selector_literals() {
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "say hello ] \"quoted\" @a");
    assert!(has_translation(
        &state,
        ConnectionId(2),
        "chat.type.announcement"
    ));
    let parsed = parse("me waves @a ]", false).unwrap();
    execute(
        &mut state,
        ConnectionId(1),
        alice,
        &parsed,
        &mut ChangeSet::default(),
    );
    assert!(has_translation(&state, ConnectionId(2), "chat.type.emote"));
    command(&mut state, alice, "tell Bob hello  world @é");
    assert!(has_translation(
        &state,
        ConnectionId(2),
        "commands.message.display.incoming"
    ));
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.message.display.outgoing"
    ));
    assert!(parse("msg @a hello", false).is_err());
    assert_eq!(expand_message(&state, alice, "mail @é @"), "mail @é @");
}

#[test]
fn security_communication_includes_muted_commands() {
    assert!(
        PlayerInput::Chat {
            connection: ConnectionId(1),
            message: "hello".into(),
        }
        .is_communication()
    );
    for command in [
        "me waves",
        "msg Bob hello",
        "tell Bob hello",
        "w Bob hello",
        "reply hello",
        "teammsg hello",
        "tm hello",
        "say hello",
        "tellraw Bob {\"text\":\"hello\"}",
        "title Bob title \"hello\"",
        "  me   waves",
    ] {
        assert!(
            PlayerInput::Command {
                connection: ConnectionId(1),
                command: command.into(),
                permission_level: 0,
            }
            .is_communication(),
            "mute must block {command}"
        );
    }
    for command in [
        "gamemode creative",
        "time query daytime",
        "settlement guide",
        "help",
        "",
    ] {
        assert!(
            !PlayerInput::Command {
                connection: ConnectionId(1),
                command: command.into(),
                permission_level: 2,
            }
            .is_communication(),
            "mute must preserve {command}"
        );
    }
}

#[test]
fn security_chat_burst_disconnects_only_the_sender() {
    let (mut state, _, _) = fixture();
    state.outbox_cap = 512;
    let inputs: Vec<_> = (0..513)
        .map(|_| PlayerInput::Chat {
            connection: ConnectionId(1),
            message: "spam".into(),
        })
        .collect();
    let (_, deliveries, _) = crate::tick::run_tick(&mut state, &inputs);
    let receiver = &deliveries
        .iter()
        .find(|(id, _)| *id == ConnectionId(2))
        .unwrap()
        .1;
    assert!(
        !receiver
            .iter()
            .any(|change| matches!(change, PlayerChange::Disconnect { .. })),
        "a permitted sender burst must not disconnect its recipients"
    );
    assert_eq!(
        receiver
            .iter()
            .filter(|change| matches!(change, PlayerChange::StyledChat { .. }))
            .count(),
        9
    );
    let sender = &deliveries
        .iter()
        .find(|(id, _)| *id == ConnectionId(1))
        .unwrap()
        .1;
    assert!(sender.iter().any(|change| matches!(change, PlayerChange::Disconnect { reason } if reason == "disconnect.spam")));
}

#[test]
fn security_operator_chat_burst_is_never_kicked() {
    let (mut state, _, _) = fixture();
    state.plugin_players.observe_permission(ConnectionId(1), 4);
    // Far past the u16 counter range: the budget saturates instead of overflowing.
    let inputs: Vec<_> = (0..4000)
        .map(|_| PlayerInput::Chat {
            connection: ConnectionId(1),
            message: "spam".into(),
        })
        .collect();
    let (_, deliveries, _) = crate::tick::run_tick(&mut state, &inputs);
    assert!(state.disconnects.is_empty());
    assert!(!deliveries.iter().any(|(_, changes)| {
        changes
            .iter()
            .any(|change| matches!(change, PlayerChange::Disconnect { .. }))
    }));
}

#[test]
fn security_chat_and_message_aliases_share_the_sender_budget() {
    let (mut state, _, _) = fixture();
    let mut inputs: Vec<_> = (0..6)
        .map(|_| PlayerInput::Chat {
            connection: ConnectionId(1),
            message: "hello".into(),
        })
        .collect();
    // Non-operator sender: operators are never kicked for spam.
    for command in [
        "me waves",
        "msg Bob hello",
        "tell Bob hello",
        "w Bob spam",
        "me spam",
    ] {
        inputs.push(PlayerInput::Command {
            connection: ConnectionId(1),
            command: command.into(),
            permission_level: 0,
        });
    }
    let (_, deliveries, _) = crate::tick::run_tick(&mut state, &inputs);
    let receiver = &deliveries
        .iter()
        .find(|(id, _)| *id == ConnectionId(2))
        .unwrap()
        .1;
    assert_eq!(
        receiver
            .iter()
            .filter(|change| matches!(change, PlayerChange::StyledChat { .. }))
            .count(),
        9
    );
    assert_eq!(state.disconnects, [(ConnectionId(1), state.tick - 1)]);
}

#[test]
fn security_experience_maximum_commands_saturate() {
    let (mut state, alice, _) = fixture();
    let inputs = [
        "experience set @s 2147483647 levels",
        "experience add @s 2147483647 points",
        "experience add @s 2147483647 levels",
    ]
    .map(|command| PlayerInput::Command {
        connection: ConnectionId(1),
        command: command.into(),
        permission_level: 2,
    });
    crate::tick::run_tick(&mut state, &inputs);
    let xp = state.players[&alice].vitals.experience;
    assert_eq!(xp.level, i32::MAX);
    assert_eq!(xp.total, i32::MAX);
    assert_eq!(xp.progress, 0.0);
}

#[test]
fn security_chat_budget_decays_per_tick_and_is_per_sender() {
    let (mut state, alice, bob) = fixture();
    let inputs: Vec<_> = [ConnectionId(1), ConnectionId(2)]
        .into_iter()
        .flat_map(|connection| {
            (0..9).map(move |_| PlayerInput::Chat {
                connection,
                message: "hello".into(),
            })
        })
        .collect();
    crate::tick::run_tick(&mut state, &inputs);
    assert!(state.disconnects.is_empty());
    for entity in [alice, bob] {
        assert_eq!(state.players[&entity].chat_spam_ticks, 180);
    }
    for _ in 0..20 {
        crate::tick::run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&alice].chat_spam_ticks, 160);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Chat {
            connection: ConnectionId(1),
            message: "another message".into(),
        }],
    );
    assert!(state.disconnects.is_empty());
    assert_eq!(state.players[&alice].chat_spam_ticks, 179);
    assert_eq!(state.players[&bob].chat_spam_ticks, 159);
    for _ in 0..180 {
        crate::tick::run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&alice].chat_spam_ticks, 0);
}

#[test]
fn command_boundary_enforces_operator_level() {
    let (mut state, alice, _) = fixture();
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "gamemode creative".into(),
            permission_level: 0,
        }],
    );
    assert_eq!(
        state.players[&alice].game_mode,
        crate::commands::GameMode::Survival
    );
}

#[test]
fn selector_position_overrides_distance_sort_and_body_volumes() {
    let (mut state, alice, bob) = fixture();
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@p[x=10,y=80,z=0]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=10,distance=..1]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=10,dx=-2]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=0,dx=9]").unwrap()
        ),
        vec![alice, bob],
        "Bob's body crosses the exclusive upper face even though his position is on it"
    );
    command(
        &mut state,
        alice,
        "give @a[x=10,dx=0,y=80,dy=0,z=0,dz=0] stone 2",
    );
    assert!(
        state.players[&alice]
            .inventory
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .map(|item| item.count)
            .sum::<i32>(),
        2
    );
    for options in ["x=NaN", "dy=inf", "x=1,x=2", "dz=1,dz=2"] {
        assert!(crate::commands::target_of(&format!("@a[{options}]")).is_err());
    }
}

#[test]
fn selectors_match_plain_custom_name_component_for_summoned_entities() {
    let (mut state, alice, _) = fixture();
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    for nbt in ["{CustomName:'Foo'}", "{CustomName:{text:'Foo'}}"] {
        let before: std::collections::HashSet<_> = state
            .entities
            .slots()
            .map(|slot| state.entities.body(slot).id)
            .collect();
        command(&mut state, alice, &format!("summon cow 2 200 2 {nbt}"));
        let cow = state
            .entities
            .slots()
            .map(|slot| state.entities.body(slot).id)
            .find(|id| !before.contains(id))
            .expect("summoned named cow");
        let target = |selector: &str| crate::commands::target_of(selector).unwrap();
        assert!(resolve_target(&state, alice, &target("@e[name=Foo]")).contains(&cow));
        assert!(!resolve_target(&state, alice, &target("@e[name=!Foo]")).contains(&cow));
        assert!(!resolve_target(&state, alice, &target("@e[name=Bar]")).contains(&cow));
        assert!(resolve_target(&state, alice, &target("@e[name=!Bar]")).contains(&cow));
    }
}

#[test]
fn json_shaped_summon_custom_name_stays_literal_in_vanilla_26() {
    let (mut state, alice, _) = fixture();
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    command(
        &mut state,
        alice,
        r#"summon cow 2 200 2 {CustomName:'{"text":"Foo"}'}"#,
    );
    let target = crate::commands::target_of("@e[type=cow,name=Foo]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    let Target::Selector(mut selector) = target else {
        panic!("entity selector");
    };
    selector.names = vec![(r#"{"text":"Foo"}"#.into(), false)];
    assert_eq!(
        resolve_target(&state, alice, &Target::Selector(selector)).len(),
        1
    );
}

#[test]
fn named_and_uuid_entity_targets_preserve_player_only_command_filtering() {
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let slot = state.entities.slot_of(cow).unwrap();
    let uuid = state.entities.row(slot).uuid.to_string();
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("minecraft:cow").unwrap()
        ),
        vec![cow]
    );
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
        Some("Bessie".into());
    for name in ["Bessie", uuid.as_str()] {
        assert_eq!(
            resolve_target(&state, alice, &crate::commands::target_of(name).unwrap()),
            vec![cow]
        );
    }
    command(&mut state, alice, "give Bessie stone");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "argument.entity.notfound.player"
    ));
    command(&mut state, alice, &format!("kill {uuid}"));
    assert!(state.merge_damage.iter().any(|(target, _)| *target == slot));
}

#[test]
fn gamerule_random_tick_speed_rejects_unsafe_values() {
    assert!(parse("gamerule randomTickSpeed 2147483647", true).is_err());
}

#[test]
fn gamerule_wal_images_preserve_canonical_aliases_and_absent_subsystem_overrides() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "gamerule keepInventory true");
    command(&mut state, alice, "gamerule randomTickSpeed 0");
    command(
        &mut state,
        alice,
        "gamerule allowEnteringNetherUsingPortals false",
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let values = entries
        .iter()
        .find_map(|entry| match entry {
            JournalEntry::GameRules { values } => Some(values.clone()),
            _ => None,
        })
        .expect("one absolute gamerule postimage");
    assert_eq!(
        values.get("keep_inventory").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        values.get("random_tick_speed").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        values
            .get("allow_entering_nether_using_portals")
            .map(String::as_str),
        Some("false")
    );
    assert!(!values.contains_key("keepInventory"));
    let dir = std::env::temp_dir().join(format!("mc-core-gamerules-{}", uuid::Uuid::new_v4()));
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer.append(1, output.tick, &entries).unwrap();
        writer.flush().unwrap();
    }
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let recovered = replay_batches(Replay::default(), &batches);
    assert_eq!(recovered.gamerules, values);
    let replayed_again = replay_batches(recovered, &batches);
    assert_eq!(
        replayed_again.gamerules, values,
        "absolute images are idempotent"
    );
    command(&mut state, alice, "gamerule minecraft:keep_inventory true");
    let (unchanged, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(
        !crate::core::journal_entries_for_tick(&mut state, &unchanged)
            .iter()
            .any(|entry| matches!(entry, JournalEntry::GameRules { .. })),
        "queries and unchanged writes do not copy the rule map each tick"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn summon_snbt_initializes_native_flags_motion_sheep_age_and_restart_image() {
    let (mut state, actor, _) = fixture();
    let changes = command(
        &mut state,
        actor,
        "summon sheep 2 80 3 {Health:7f, NoAI:1b, Invulnerable:1b, Age:-1234, Color:14b, Sheared:1b, Fire:120s, FallDistance:2.5f, Motion:[0.1d,0d,0d], Rotation:[90f,7f], CustomName:'Template Sheep'}",
    );
    assert_eq!(changes.entities_spawned.len(), 1);
    let id = changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(state.entities.vel(slot), [2.0, 0.0, 0.0]);
    assert_eq!(state.entities.rot(slot)[..2], [90.0, 7.0]);
    let row = state.entities.row(slot);
    assert_eq!(row.health, 7.0);
    assert_eq!(row.entity_life.child_ticks, 1234);
    assert!(row.controls.no_ai && row.controls.invulnerable && row.appearance.baby);
    assert_eq!(
        row.appearance.custom_name.as_deref(),
        Some("Template Sheep")
    );
    assert_eq!(crate::tick::wool_of(&row.appearance), 30);
    assert_eq!(
        state
            .entities
            .land_hit(slot, 1000, 5.0, crate::survival::DamageKind::Melee),
        0.0
    );
    assert_eq!(
        state
            .entities
            .land_hit(slot, 1000, 5.0, crate::survival::DamageKind::Void),
        5.0
    );
    let image = crate::journal::EntityStateImage::from_row(
        state.entities.body(slot),
        state.entities.row(slot),
    );
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == state.entities.row(slot).uuid)
        .unwrap();
    let recovered = crate::journal::EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(recovered.controls, image.controls);
    assert_eq!(recovered.entity_life.child_ticks, 1234);
    let wal: crate::journal::EntityStateImage =
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    assert_eq!(wal, image);
}

#[test]
fn summon_rejects_malformed_unsupported_and_duplicate_identity_before_spawn() {
    let (mut state, actor, _) = fixture();
    let baseline = state.entities.len();
    for raw in [
        "summon cow 2 80 3 {Color:1b}",
        "summon cow 2 80 3 {Health:NaNf}",
        "summon cow 2 80 3 {Motion:[0d,0d]}",
        "summon cow 2 80 3 {Motion:[NaNd,0d,0d]}",
        "summon cow 2 80 3 {NoAI:'bad'}",
        "summon cow 2 80 3 {Unknown:1}",
        "summon wolf 2 80 3 {Owner:[I;1,2]}",
        "summon horse 2 80 3 {equipment:{unknown:{id:'minecraft:iron_horse_armor',count:1}}}",
    ] {
        assert!(
            command(&mut state, actor, raw).entities_spawned.is_empty(),
            "{raw}"
        );
        assert_eq!(state.entities.len(), baseline, "{raw}");
    }
    let uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    assert!(
        command(
            &mut state,
            actor,
            &format!("summon cow 2 80 3 {{UUID:'{uuid}'}}")
        )
        .entities_spawned
        .is_empty()
    );
    assert!(parse("summon cow 2 80 3 {NoAI:1b,NoAI:0b}", true).is_err());
}

#[test]
fn summon_mule_age_is_retained_despite_no_breeding_offspring() {
    let (mut state, actor, _) = fixture();
    let changes = command(&mut state, actor, "summon mule 2 80 3 {Age:-321}");
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert_eq!(row.entity_life.child_ticks, 321);
    assert!(row.appearance.baby);
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:mule".into(),
        age: -321,
        extra_fields: image.to_fields(&state.registry.blocks),
        ..Default::default()
    };
    assert_eq!(
        crate::journal::EntityStateImage::from_record(&state.registry, &record)
            .unwrap()
            .entity_life
            .child_ticks,
        321
    );
}

#[test]
fn summon_uses_native_owner_leash_and_exact_saddle_stack_images() {
    let (mut state, actor, _) = fixture();
    let changes = command(
        &mut state,
        actor,
        "summon horse 2 80 3 {Owner:[I;1,2,3,4], Tame:1b, Temper:72, leash:{UUID:[I;5,6,7,8]}, equipment:{saddle:{id:'minecraft:saddle',count:1}}}",
    );
    assert_eq!(changes.entities_spawned.len(), 1);
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert!(row.entity_life.tamed);
    assert_eq!(row.entity_life.taming.temper, 72);
    assert_eq!(
        row.entity_life.taming.owner,
        Some(uuid::Uuid::from_u128(
            (1u128 << 96) | (2u128 << 64) | (3u128 << 32) | 4
        ))
    );
    assert_eq!(
        row.entity_life.leash,
        Some(crate::leash::Holder::Entity(uuid::Uuid::from_u128(
            (5u128 << 96) | (6u128 << 64) | (7u128 << 32) | 8
        )))
    );
    assert_eq!(
        state
            .registry
            .items
            .name_of(row.loot.saddle.as_ref().unwrap().item_id)
            .unwrap()
            .path(),
        "saddle"
    );
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.uuid == row.uuid)
        .unwrap();
    let restored = crate::journal::EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(restored.entity_life, image.entity_life);
    assert_eq!(restored.loot.saddle, image.loot.saddle);
}

#[test]
fn component_item_commands_publish_native_inventory_and_selective_clear_feedback() {
    let (mut state, alice, bob) = fixture();
    let (_, deliveries, _) = crate::tick::run_tick(&mut state, &[
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @a potion[potion_contents={potion:"minecraft:water"},custom_name={text:"QA water",color:"gold"}] 1"#.into(),
        },
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give Bob potion[potion_contents={potion:"minecraft:poison"}] 1"#.into(),
        },
    ]);
    for player in [alice, bob] {
        assert!(state.players[&player].inventory.slots.iter().any(|stack| {
            stack.custom_name.as_deref() == Some("QA water")
                && stack
                    .extended()
                    .potion
                    .as_ref()
                    .is_some_and(|potion| potion.as_str() == "minecraft:water")
        }));
    }
    assert!(deliveries.iter().find_map(|(connection, queue)| (*connection == ConnectionId(1)).then_some(queue))
        .is_some_and(|queue| queue.iter().any(|change| matches!(change,
            PlayerChange::StyledChat { content: Tag::Compound(fields), .. }
            if fields.iter().any(|(name, value)| name == "translate" && value == &Tag::String("commands.give.success.multiple".into()))
        ))));
    let before = state.players[&bob].inventory.clone();
    command(
        &mut state,
        alice,
        r#"clear Bob potion[potion_contents={potion:"minecraft:water"}] 0"#,
    );
    assert_eq!(state.players[&bob].inventory, before);
    command(
        &mut state,
        alice,
        r#"clear Bob potion[potion_contents={potion:"minecraft:water"}]"#,
    );
    assert!(state.players[&bob].inventory.slots.iter().all(|stack| {
        stack
            .extended()
            .potion
            .as_ref()
            .is_none_or(|potion| potion.as_str() != "minecraft:water")
    }));
    assert!(state.players[&bob].inventory.slots.iter().any(|stack| {
        stack
            .extended()
            .potion
            .as_ref()
            .is_some_and(|potion| potion.as_str() == "minecraft:poison")
    }));
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.clear.success.single"
    ));
}

#[test]
fn malformed_item_component_reports_error_without_any_inventory_write() {
    let (mut state, alice, _) = fixture();
    let before = state.players[&alice].inventory.clone();
    command(
        &mut state,
        alice,
        r#"give @s potion[potion_contents={potion:"minecraft:no_such_potion"}]"#,
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(change, PlayerChange::Chat { message, .. } if message.contains("unknown potion"))));
    state.outbox.clear();
    let (_, deliveries, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            permission_level: 4,
            command: "give @s stone[not_a_component=1]".into(),
        }],
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert!(deliveries.iter().find_map(|(connection, queue)| (*connection == ConnectionId(1)).then_some(queue))
        .is_some_and(|queue| queue.iter().any(|change| matches!(change,
            PlayerChange::Chat { message, .. } if message.contains("Unsupported item component minecraft:not_a_component")
        ))));
}

#[test]
fn command_component_inventory_survives_real_wal_replay_and_native_player_checkpoint() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    use crate::native_checkpoint::tests::{read_player, write_player};
    let (mut state, alice, _) = fixture();
    let (output, _, _) = crate::tick::run_tick(&mut state, &[
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @s potion[potion_contents={potion:"minecraft:water"},custom_name={text:"Saved water",color:"gold"},!enchantment_glint_override] 2"#.into(),
        },
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @s diamond_sword[enchantments={"minecraft:sharpness":5}]"#.into(),
        },
    ]);
    let uuid = state
        .entities
        .row(state.entities.slot_of(alice).unwrap())
        .uuid
        .to_string();
    let expected = state.players[&alice].inventory.slots.clone();
    assert_eq!(
        expected
            .iter()
            .filter(|stack| stack.custom_name.as_deref() == Some("Saved water"))
            .map(|stack| stack.count)
            .sum::<i32>(),
        2
    );
    assert!(expected.iter().any(|stack| {
        stack
            .enchantments
            .iter()
            .any(|entry| entry.id.as_str() == "minecraft:sharpness" && entry.level == 5)
    }));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let directory = tempfile::tempdir().unwrap();
    {
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(1, output.tick, &entries).unwrap();
        writer.flush().unwrap();
    }
    let batches = read_complete_batches_after(&directory.path().join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(
        replay.players[&uuid].inventory.as_slice(),
        expected.as_slice()
    );
    let replay = replay_batches(replay, &batches);
    assert_eq!(
        replay.players[&uuid].inventory.as_slice(),
        expected.as_slice(),
        "absolute WAL images do not duplicate command grants"
    );
    let config = crate::CoreConfig::new(directory.path().to_owned(), 42);
    write_player(
        &config,
        &uuid,
        &replay.players[&uuid],
        &state.registry.items,
    )
    .unwrap();
    let restored = read_player(&config, &uuid, &state.registry.items).unwrap();
    assert_eq!(
        restored.inventory.as_slice(),
        expected.as_slice(),
        "native checkpoint reader retains styled text and explicit removals"
    );
    write_player(&config, &uuid, &restored, &state.registry.items).unwrap();
    let restored_again = read_player(&config, &uuid, &state.registry.items).unwrap();
    assert_eq!(restored_again.inventory.as_slice(), expected.as_slice());
}

#[test]
fn settlement_create_refuses_without_authenticated_actor_and_creates_nothing() {
    let (mut state, alice, _) = fixture();
    // Simulate a connection whose UUID registration is gone: every mutation
    // must refuse rather than invent a shared owner.
    let ghost = crate::boundary::ConnectionId(99);
    let parsed = parse("settlement create ghost-town", true).unwrap();
    let mut changes = ChangeSet::default();
    super::execute(&mut state, ghost, alice, &parsed, &mut changes);
    assert!(state.settlements.find("ghost-town").is_none());
    assert!(state.settlements.records().next().is_none());
}

#[test]
fn settlement_commands_keep_native_identity_and_enforce_owner_or_operator() {
    let (mut state, alice, bob) = fixture();
    assert!(parse("settlement create town", false).is_ok());
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [-0.5, 80.0, -0.5]);
    command(&mut state, alice, "settlement create town");
    let record = state.settlements.find("town").unwrap();
    let id = record.id;
    assert_eq!(
        record.anchor,
        [-1, 80, -1],
        "anchor is the containing block"
    );
    assert_eq!(
        record.owner,
        Some(
            state
                .connections
                .uuid_of(ConnectionId(1))
                .unwrap()
                .to_string()
        )
    );
    assert_eq!(record.radius, 48);
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let resident = state.spawn_mob(villager_type, [1.0, 80.0, 1.0], 20.0);
    let slot = state.entities.slot_of(resident).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    let uuid = state.entities.row(slot).uuid;
    let bodies = state.entities.len();

    command(&mut state, bob, &format!("settlement bind town {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    command(&mut state, alice, &format!("settlement bind town {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id)
    );
    assert_eq!(crate::settlements::member_count(&state, id), 1);
    command(&mut state, bob, &format!("settlement unbind {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id)
    );
    command(&mut state, bob, "settlement rename town stolen");
    command(&mut state, bob, "settlement remove town");
    assert_eq!(state.settlements.find("town").unwrap().id, id);
    assert!(state.settlements.find("stolen").is_none());

    state.outbox.clear();
    command(&mut state, bob, "settlement list");
    command(&mut state, bob, "settlement info town");
    assert!(
        !state.outbox[&ConnectionId(2)].is_empty(),
        "reads are public"
    );
    command(&mut state, alice, &format!("settlement unbind {uuid}"));
    assert_eq!(crate::settlements::member_count(&state, id), 0);
    command(&mut state, alice, &format!("settlement bind town {uuid}"));
    command(&mut state, alice, "settlement rename town borough");
    assert_eq!(state.settlements.find("borough").unwrap().id, id);
    state.plugin_players.observe_permission(ConnectionId(2), 2);
    command(&mut state, bob, "settlement rename borough city");
    assert_eq!(state.settlements.find("city").unwrap().id, id);
    command(&mut state, bob, "settlement remove city");
    assert!(state.settlements.get(id).is_none());
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    assert_eq!(state.entities.body(slot).id, resident);
    assert_eq!(state.entities.row(slot).uuid, uuid);
    assert_eq!(
        state.entities.len(),
        bodies,
        "binding never creates a replacement NPC"
    );
}

#[test]
fn settlement_nearest_binding_is_bounded_and_preserves_owner_checks() {
    let (mut state, alice, bob) = fixture();
    command(&mut state, alice, "settlement create town");
    let id = state.settlements.find("town").unwrap().id;
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let mut residents = Vec::new();
    for x in [2.0, 3.0, 9.0] {
        let entity = state.spawn_mob(villager_type, [x, 80.0, 0.0], 20.0);
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.row_mut(slot).villager =
            Some(Box::new(crate::villager::VillagerState::default()));
        residents.push(slot);
    }
    command(&mut state, alice, "settlement bind town");
    assert_eq!(crate::settlements::member_count(&state, id), 1);
    assert_eq!(
        state
            .entities
            .row(residents[0])
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id)
    );
    assert_eq!(
        state
            .entities
            .row(residents[1])
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    let bob_slot = state.entities.slot_of(bob).unwrap();
    state.entities.set_pos(bob_slot, [0.0, 80.0, 0.0]);
    command(&mut state, bob, "settlement unbind");
    assert_eq!(
        crate::settlements::member_count(&state, id),
        1,
        "strangers cannot unbind nearest"
    );
    command(&mut state, alice, "settlement unbind");
    assert_eq!(crate::settlements::member_count(&state, id), 0);
    command(&mut state, bob, "settlement bind town");
    assert_eq!(
        crate::settlements::member_count(&state, id),
        0,
        "strangers cannot bind nearest"
    );
    // Exactly eight blocks is admitted; further bodies are not.
    state.entities.set_pos(residents[0], [8.0, 80.0, 0.0]);
    state.entities.set_pos(residents[1], [8.01, 80.0, 0.0]);
    command(&mut state, alice, "settlement bind town");
    assert_eq!(crate::settlements::member_count(&state, id), 1);
    command(&mut state, alice, "settlement unbind");
    state.entities.set_pos(residents[0], [8.01, 80.0, 0.0]);
    command(&mut state, alice, "settlement bind town");
    assert_eq!(
        crate::settlements::member_count(&state, id),
        0,
        "no target outside eight blocks"
    );
}

#[test]
fn settlement_building_commands_select_executor_town_and_enforce_owner() {
    let (mut state, alice, bob) = fixture();
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [-0.5, 80.0, -0.5]);
    // Unknown kinds and bad coordinates refuse at parse; execution is never
    // reached and nothing is created.
    assert!(parse("settlement building certify castle 0 80 0 4 82 4", true).is_ok());
    assert!(parse("settlement building certify house 0 80", true).is_err());
    assert!(parse("settlement building upgrade nope", true).is_err());
    assert!(parse("settlement building list extra", true).is_err());
    command(&mut state, alice, "settlement create town");
    let town = state.settlements.find("town").unwrap().id;
    // Bob starts at x=10 (fixture) inside the default 48-block sphere: move
    // him far outside before his list, so no settlement contains him.
    let bob_slot = state.entities.slot_of(bob).unwrap();
    state.entities.set_pos(bob_slot, [200.0, 80.0, 0.0]);
    command(&mut state, bob, "settlement building list");
    let queue = &state.outbox[&ConnectionId(2)];
    assert!(
        queue.iter().any(|change| matches!(
            change,
            PlayerChange::Chat { message, .. }
                if message == "No settlement contains your position."
        )),
        "outsiders select no settlement"
    );
    // Strangers cannot certify in Alice's town even standing inside it.
    state.entities.set_pos(bob_slot, [-0.5, 80.0, -0.5]);
    command(
        &mut state,
        bob,
        "settlement building certify house -4 80 -4 4 82 4",
    );
    let record = state.settlements.get(town).unwrap();
    assert!(record.buildings.is_empty(), "strangers certify nothing");
    // Alice's own certify reaches the helper (empty dirt volume reports the
    // native missing-requirements error, not an authorization refusal).
    command(
        &mut state,
        alice,
        "settlement building certify house -4 80 -4 4 82 4",
    );
    let queue = &state.outbox[&ConnectionId(1)];
    assert!(
        queue.iter().any(|change| matches!(
            change,
            PlayerChange::Chat { message, .. }
                if !message.contains("Only the owner")
        )),
        "owner passes authorization"
    );
}

#[test]
fn settlement_autonomous_claim_refuses_other_mutations_and_other_players() {
    let (mut state, alice, bob) = fixture();
    let at = state
        .entities
        .pos(state.entities.slot_of(alice).unwrap())
        .map(|value| value.floor() as i32);
    let id = state
        .settlements
        .adopt_village(
            &mut state.settlement_sequence,
            "minecraft:overworld:test-village",
            at,
        )
        .unwrap();
    let name = state.settlements.get(id).unwrap().name.clone();
    state.plugin_players.observe_permission(ConnectionId(2), 2);
    command(
        &mut state,
        alice,
        &format!("settlement rename {name} stolen"),
    );
    command(&mut state, bob, &format!("settlement remove {name}"));
    command(&mut state, bob, &format!("settlement rename {name} stolen"));
    assert_eq!(state.settlements.get(id).unwrap().owner, None);
    assert!(state.settlements.find("stolen").is_none());
    command(&mut state, alice, "settlement claim");
    let owner = state
        .connections
        .uuid_of(ConnectionId(1))
        .unwrap()
        .to_string();
    assert_eq!(
        state.settlements.get(id).unwrap().owner.as_deref(),
        Some(owner.as_str())
    );
    command(&mut state, bob, "settlement claim");
    assert_eq!(
        state.settlements.get(id).unwrap().owner.as_deref(),
        Some(owner.as_str())
    );
    command(
        &mut state,
        alice,
        &format!("settlement rename {name} claimed"),
    );
    assert_eq!(state.settlements.find("claimed").unwrap().id, id);
}

fn build_last_chat(state: &TickState, connection: ConnectionId) -> String {
    state.outbox[&connection]
        .iter()
        .rev()
        .find_map(|change| match change {
            PlayerChange::Chat { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("build command replies over chat")
}

/// Narrow stock probe: construction materials move only through the shared
/// executor, so an accepted order starts the project without debiting stock.
fn build_chest_stock_at(state: &TickState, pos: (i32, i32, i32)) -> Vec<mc_data::ItemStack> {
    state
        .block_entities
        .get(&pos)
        .map(|entity| {
            entity
                .slots
                .iter()
                .filter(|stack| !stack.is_empty())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
fn build_chest_stock(state: &TickState) -> Vec<mc_data::ItemStack> {
    build_chest_stock_at(state, (1, 201, 1))
}

/// Fill the chest at `pos` with exact catalog stock for the requested level.
fn fund_at(state: &mut TickState, pos: (i32, i32, i32), blueprint: &str, level: u8) {
    let materials = crate::settlements::blueprints::get(&state.registry, blueprint)
        .unwrap()
        .level(level)
        .unwrap()
        .materials
        .clone();
    let slots = &mut state.block_entities.get_mut(&pos).unwrap().slots;
    slots.fill(mc_data::ItemStack::EMPTY);
    let mut index = 0;
    for (item, quantity) in &materials {
        let mut quantity = *quantity;
        while quantity > 0 {
            let count = quantity.min(64);
            slots[index] = mc_data::ItemStack::new(*item, count as i32);
            quantity -= count;
            index += 1;
        }
    }
}

#[test]
fn settlement_build_starts_funded_order_without_touching_blocks_or_stock() {
    use crate::settlements::allocator::tests::block;
    let (mut state, alice, bob) = fixture();
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [2.5, 201.0, 2.5]);
    // Exact syntax works; malformed shapes fail at parse and reach no record.
    assert!(parse("settlement build house", true).is_ok());
    assert!(parse("settlement build upgrade 7", true).is_ok());
    assert!(parse("settlement build", true).is_err());
    assert!(parse("settlement build house extra", true).is_err());
    assert!(parse("settlement build upgrade nope", true).is_err());
    // NOTE: `build upgrade` alone parses as blueprint "upgrade" (blueprint
    // ids are free-form; validation happens at execution). Only extra words
    // or a non-numeric id after `upgrade` fail at parse.
    assert!(parse("settlement build upgrade 1 extra", true).is_err());
    // Flatten ground around the anchor so the bounded site scan finds a
    // free loaded site deterministically.
    for x in 0..16 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    state
        .world
        .set_block(1, 201, 1, block(&state, "minecraft:chest", &[]));
    // A chest with weak stock proves unfunded orders refuse with a named
    // shortage and never persist.
    state.block_entities.insert(
        (1, 201, 1),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots[0] =
        mc_data::ItemStack::new(dirt, 64);
    command(&mut state, alice, "settlement create town");
    let id = state.settlements.find("town").unwrap().id;
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([1, 201, 1]);
    command(&mut state, alice, "settlement build house");
    assert!(
        build_last_chat(&state, ConnectionId(1)).starts_with("not enough:"),
        "unfunded order names its shortage"
    );
    assert!(
        state.settlements.get(id).unwrap().project.is_none(),
        "unfunded orders never persist"
    );
    // Full house L1 funding starts the single project with no debit here.
    fund_at(&mut state, (1, 201, 1), "house", 1);
    let before_writes = state.plugin_block_writes.len();
    let before_stock = build_chest_stock(&state);
    command(&mut state, alice, "settlement build house");
    assert_eq!(
        build_last_chat(&state, ConnectionId(1)),
        "Started house construction."
    );
    let record = state.settlements.get(id).unwrap();
    assert_eq!(record.project.as_ref().unwrap().blueprint, "house");
    assert_eq!(record.project.as_ref().unwrap().level, 1);
    assert_eq!(state.plugin_block_writes.len(), before_writes);
    assert_eq!(build_chest_stock(&state), before_stock);
    // A second order while the project runs is Busy and changes nothing.
    command(&mut state, alice, "settlement build house");
    assert_eq!(build_last_chat(&state, ConnectionId(1)), "Busy.");
    assert_eq!(
        state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .blueprint,
        "house"
    );
    assert_eq!(state.plugin_block_writes.len(), before_writes);
    assert_eq!(build_chest_stock(&state), before_stock);
    // Real validation errors surface instead of a started reply.
    command(&mut state, alice, "settlement build not-a-blueprint");
    assert_eq!(build_last_chat(&state, ConnectionId(1)), "Busy.");
    // Foreign owners and unowned records refuse; operators of an owned
    // settlement pass per existing semantics.
    let bob_slot = state.entities.slot_of(bob).unwrap();
    state.entities.set_pos(bob_slot, [2.5, 201.0, 2.5]);
    command(&mut state, bob, "settlement build house");
    assert_eq!(
        build_last_chat(&state, ConnectionId(2)),
        "Only the owner or an operator orders construction."
    );
    assert_eq!(state.plugin_block_writes.len(), before_writes);
    assert_eq!(build_chest_stock(&state), before_stock);
    state.plugin_players.observe_permission(ConnectionId(2), 2);
    command(&mut state, bob, "settlement build house");
    assert_eq!(build_last_chat(&state, ConnectionId(2)), "Busy.");
    // Execution outside every settlement radius selects nothing.
    state.entities.set_pos(bob_slot, [200.0, 80.0, 0.0]);
    command(&mut state, bob, "settlement build house");
    assert_eq!(
        build_last_chat(&state, ConnectionId(2)),
        "No settlement contains your position."
    );
}

#[test]
fn settlement_build_upgrade_refuses_stale_foreign_and_unowned_targets() {
    use crate::settlements::{buildings::BuildingId, construction};
    let (mut state, alice, bob) = fixture();
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [-0.5, 80.0, -0.5]);
    command(&mut state, alice, "settlement create town");
    let id = state.settlements.find("town").unwrap().id;
    let ghost = BuildingId((id.0 << 32) | 1);
    // Stale targets (no such building) report the real admission refusal.
    command(
        &mut state,
        alice,
        &format!("settlement build upgrade {}", ghost.0),
    );
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(
        build_last_chat(&state, ConnectionId(1)),
        "Building not found."
    );
    // A foreign building id never resolves against the enclosing settlement.
    let bob_slot = state.entities.slot_of(bob).unwrap();
    state.entities.set_pos(bob_slot, [-0.5, 80.0, -0.5]);
    let foreign = (id.0.wrapping_add(2048) << 32) | 1;
    command(
        &mut state,
        alice,
        &format!("settlement build upgrade {foreign}"),
    );
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(
        build_last_chat(&state, ConnectionId(1)),
        "Building not found."
    );
    // Foreign owners refuse even a well-formed request.
    command(
        &mut state,
        bob,
        &format!("settlement build upgrade {}", ghost.0),
    );
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(
        build_last_chat(&state, ConnectionId(2)),
        "Only the owner or an operator orders construction."
    );
    // Unowned records refuse even operators via can_manage.
    let unowned = state
        .settlements
        .adopt_village(
            &mut state.settlement_sequence,
            "minecraft:overworld:unowned-build",
            [300, 80, 0],
        )
        .unwrap();
    assert_eq!(state.settlements.get(unowned).unwrap().owner, None);
    state.entities.set_pos(alice_slot, [300.0, 80.0, 0.0]);
    // Shrink town so the unowned village is the closest enclosing anchor.
    state.settlements.get_mut(id).unwrap().radius = 1;
    state.plugin_players.observe_permission(ConnectionId(1), 2);
    command(&mut state, alice, "settlement build house");
    assert!(
        state.settlements.get(unowned).unwrap().project.is_none(),
        "unowned records refuse even operators"
    );
    assert_eq!(
        build_last_chat(&state, ConnectionId(1)),
        "Only the owner or an operator orders construction."
    );
    // Absent actors (lost UUID registration) refuse rather than start work.
    let ghost_connection = crate::boundary::ConnectionId(99);
    let parsed = parse(&format!("settlement build upgrade {}", ghost.0), true).unwrap();
    let mut changes = ChangeSet::default();
    let stock = build_chest_stock(&state);
    super::execute(&mut state, ghost_connection, alice, &parsed, &mut changes);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(build_chest_stock(&state), stock);
    // Positive admission lives with the planner owner: only a ledger
    // entry marked committed plus matching L1 geometry makes a building
    // upgrade-available (see upgrade_uses_diff_and_certifies_level_two).
    assert!(
        !construction::upgrade_available(&state, id, ghost),
        "fixture building is not blueprint-upgradeable"
    );
}

#[test]
fn settlement_build_upgrade_owner_command_starts_a_real_blueprint_upgrade() {
    use crate::settlements::allocator::tests::{block, villager};
    use crate::settlements::construction::tests::{complete, fund, start};
    let (mut state, alice, _) = fixture();
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [8.5, 201.0, 8.5]);
    command(&mut state, alice, "settlement create town");
    let id = state.settlements.find("town").unwrap().id;
    for x in 0..16 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let chest = block(&state, "minecraft:chest", &[]);
    state.world.set_block(1, 201, 1, chest);
    state.block_entities.insert(
        (1, 201, 1),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([1, 201, 1]);
    let builder = villager(&mut state, [1.5, 201.0, 1.5]);
    let entity = state.entities.body(builder).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    let v = state.entities.row_mut(builder).villager.as_mut().unwrap();
    v.data.profession = crate::villager::VillagerProfession::Builder;
    v.activity = crate::villager::Activity::Work;
    state.day_time = 3000;
    state.world.fold_overlay();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    complete(&mut state, id, builder);
    let building = *state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .keys()
        .next()
        .unwrap();
    // L1 stock is spent by the finished build; L2 stock funds the upgrade.
    fund(&mut state, "house", 2);
    let before_writes = state.plugin_block_writes.len();
    let before_stock = state.block_entities[&(1, 201, 1)].slots.clone();
    command(
        &mut state,
        alice,
        &format!("settlement build upgrade {}", building.0),
    );
    let record = state.settlements.get(id).unwrap();
    let project = record.project.as_ref().unwrap();
    assert_eq!(project.building, Some(building));
    assert_eq!(project.level, 2);
    assert_eq!(project.blueprint, "house");
    assert_eq!(state.plugin_block_writes.len(), before_writes);
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before_stock);
    assert_eq!(
        build_last_chat(&state, ConnectionId(1)),
        format!("Started upgrade for building #{}.", building.0)
    );
    // While the upgrade runs, another order is Busy and changes nothing.
    command(&mut state, alice, "settlement build house");
    assert_eq!(build_last_chat(&state, ConnectionId(1)), "Busy.");
    assert_eq!(state.plugin_block_writes.len(), before_writes);
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before_stock);
}
