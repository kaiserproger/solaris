use super::{flat_player_fixture, run_tick};
use crate::boundary::{BlockPos, ConnectionId, EntityId, PlayerInput};
use crate::enchantments;
use crate::intent::DamageSource;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};
use mc_data::{Identifier, ItemEnchantment, ItemStack};

fn stack(state: &TickState, item: &str, enchants: &[(&str, i32)]) -> ItemStack {
    let mut stack = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{item}")).unwrap())
            .unwrap(),
        1,
    );
    for &(name, level) in enchants {
        stack.enchantments.push(ItemEnchantment {
            id: Identifier::parse(format!("minecraft:{name}")).unwrap(),
            level,
        });
    }
    stack
}
fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
        .default
        .0
}
fn damage(state: &mut TickState, attacker: EntityId, target: EntityId, amount: f32) {
    let actor = state.entities.slot_of(attacker).unwrap();
    let slot = state.entities.slot_of(target).unwrap();
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount,
            kind: DamageKind::Melee,
            source: Some(DamageSource::Melee {
                attacker,
                position: state.entities.pos(actor),
                yaw: 0.0,
                bonus_knockback: 0.0,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
}

#[test]
fn everyday_equipment_attributes_use_the_correct_slots_and_reset_after_removal() {
    let (mut state, actor, _) = flat_player_fixture();
    let boots = stack(
        &state,
        "diamond_boots",
        &[("depth_strider", 3), ("soul_speed", 3)],
    );
    let legs = stack(&state, "diamond_leggings", &[("swift_sneak", 3)]);
    let head = stack(
        &state,
        "diamond_helmet",
        &[("respiration", 3), ("aqua_affinity", 1)],
    );
    let rules = state.registry.item_rules();
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[36] = boots;
    player.inventory.slots[37] = legs;
    player.inventory.slots[39] = head;
    let Some(crate::boundary::PlayerChange::Attributes { attributes }) =
        player.equipment_attributes(&rules, &[])
    else {
        panic!("equipment must publish attributes");
    };
    for (kind, expected) in [("armor", 12.0), ("armor_toughness", 6.0)] {
        let attribute = attributes
            .iter()
            .find(|attribute| attribute.kind == kind)
            .unwrap();
        let actual = attribute.base
            + attribute
                .modifiers
                .iter()
                .map(|modifier| modifier.amount)
                .sum::<f64>();
        assert_eq!(actual, expected);
    }
    for (kind, value) in [
        ("water_movement_efficiency", 1.0),
        ("sneaking_speed", 0.75),
        ("oxygen_bonus", 3.0),
        ("submerged_mining_speed", 1.0),
    ] {
        let attribute = player
            .sent_attributes
            .iter()
            .find(|attribute| attribute.kind == kind)
            .unwrap();
        let mut actual = attribute.base;
        for modifier in &attribute.modifiers {
            if modifier.operation == "add_multiplied_total" {
                actual *= 1.0 + modifier.amount;
            } else {
                actual += modifier.amount;
            }
        }
        assert!((actual - value).abs() < 1e-6, "{kind}: {actual}");
    }
    // A curse/enchantment carried in a hand does not count as head/feet equipment.
    player.inventory.slots[0] = std::mem::take(&mut player.inventory.slots[39]);
    player.equipment_attributes(&rules, &[]).unwrap();
    assert!(
        player
            .sent_attributes
            .iter()
            .find(|entry| entry.kind == "oxygen_bonus")
            .unwrap()
            .modifiers
            .is_empty()
    );
    assert!(player.equipment_attributes(&rules, &[]).is_none());
}

#[test]
fn respiration_spends_air_probabilistically_without_recovering_skipped_ticks() {
    let (mut state, actor, pos) = flat_player_fixture();
    let head = stack(&state, "diamond_helmet", &[("respiration", 3)]);
    state.players.get_mut(&actor).unwrap().inventory.slots[39] = head;
    let water = state.registry.states.water;
    for y in pos[1] as i32..=pos[1] as i32 + 2 {
        state
            .world
            .set_block(pos[0] as i32, y, pos[2] as i32, water);
    }
    let before = state.players[&actor].vitals.air;
    for _ in 0..100 {
        run_tick(&mut state, &[]);
    }
    let used = before - state.players[&actor].vitals.air;
    assert!((10..45).contains(&used), "respiration air used: {used}");
    let seed = state.players[&actor].enchantment_seed;
    let mut recovered = seed;
    let helmet = &state.players[&actor].inventory.slots[39];
    let mut live = seed;
    for _ in 0..32 {
        assert_eq!(
            enchantments::consumes_air(helmet, &mut live),
            enchantments::consumes_air(helmet, &mut recovered)
        );
    }
    assert_eq!(live, recovered);
}

#[test]
fn aqua_affinity_removes_water_mining_penalty_but_not_airborne_penalty() {
    let (mut state, actor, pos) = flat_player_fixture();
    let stone = block(&state, "stone");
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.mining = mc_data::block_mining::BlockMiningTable::from_arrays(
        "fixture",
        vec![1.5; stone as usize + 1],
        vec![true; stone as usize + 1],
    );
    state.registry = std::sync::Arc::new(registry);
    let pick = stack(&state, "diamond_pickaxe", &[]);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = pick;
    let dry = crate::effects::mining_progress(&state, actor, stone);
    assert!(
        dry > 0.0,
        "real mining progress, not an absent bootstrap hardness table"
    );
    state.world.set_block(
        pos[0] as i32,
        (pos[1] + 1.62) as i32,
        pos[2] as i32,
        state.registry.states.water,
    );
    let submerged = crate::effects::mining_progress(&state, actor, stone);
    assert!((dry - submerged * 5.0).abs() < 1e-6);
    let flowing = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:water").unwrap(),
            &[("level".into(), "1".into())],
        )
        .unwrap()
        .0;
    state.world.set_block(
        pos[0] as i32,
        (pos[1] + 1.62) as i32,
        pos[2] as i32,
        flowing,
    );
    assert!((dry - crate::effects::mining_progress(&state, actor, stone) * 5.0).abs() < 1e-6);
    let helmet = stack(&state, "diamond_helmet", &[("aqua_affinity", 1)]);
    state.players.get_mut(&actor).unwrap().inventory.slots[39] = helmet;
    assert!((dry - crate::effects::mining_progress(&state, actor, stone)).abs() < 1e-6);
    state
        .entities
        .set_ground(state.entities.slot_of(actor).unwrap(), false);
    assert!((dry - crate::effects::mining_progress(&state, actor, stone) * 5.0).abs() < 1e-6);
}

#[test]
fn soul_speed_switches_cached_attributes_and_wears_only_on_soul_ground() {
    let (mut state, actor, pos) = flat_player_fixture();
    let boots = stack(&state, "diamond_boots", &[("soul_speed", 3)]);
    state.players.get_mut(&actor).unwrap().inventory.slots[36] = boots;
    let soul = block(&state, "soul_sand");
    state
        .world
        .set_block(pos[0] as i32, pos[1] as i32 - 1, pos[2] as i32, soul);
    let mut intents = Vec::new();
    crate::enchantment_locations::run(&mut state, &mut intents);
    assert!(state.players[&actor].soul_speed_active);
    {
        let rules = state.registry.item_rules();
        let player = state.players.get_mut(&actor).unwrap();
        player.equipment_attributes(&rules, &[]).unwrap();
        let speed = player
            .sent_attributes
            .iter()
            .find(|entry| entry.kind == "movement_speed")
            .unwrap();
        assert!(
            (speed
                .modifiers
                .iter()
                .find(|entry| entry.id == "minecraft:enchantment.soul_speed")
                .unwrap()
                .amount
                - 0.0615)
                .abs()
                < 1e-6
        );
    }
    for _ in 0..200 {
        crate::enchantment_locations::run(&mut state, &mut intents);
    }
    let wear = state.players[&actor].inventory.slots[36]
        .damage
        .unwrap_or(0);
    assert!(wear > 0 && wear < 30, "soul wear {wear}");
    let stone = block(&state, "stone");
    state
        .world
        .set_block(pos[0] as i32, pos[1] as i32 - 1, pos[2] as i32, stone);
    crate::enchantment_locations::run(&mut state, &mut intents);
    assert!(!state.players[&actor].soul_speed_active);
    assert!(state.players[&actor].attribute_equipment.is_none());
    assert_eq!(
        state.players[&actor].inventory.slots[36]
            .damage
            .unwrap_or(0),
        wear
    );
}

#[test]
fn frost_walker_freezes_only_exposed_sources_and_retained_ticks_melt_them() {
    let (mut state, actor, pos) = flat_player_fixture();
    let boots = stack(&state, "diamond_boots", &[("frost_walker", 1)]);
    state.players.get_mut(&actor).unwrap().inventory.slots[36] = boots;
    let cell = BlockPos {
        x: pos[0] as i32 + 1,
        y: pos[1] as i32 - 1,
        z: pos[2] as i32,
    };
    state
        .world
        .set_block(cell.x, cell.y, cell.z, state.registry.states.water);
    let hidden = BlockPos {
        x: cell.x + 1,
        ..cell
    };
    state
        .world
        .set_block(hidden.x, hidden.y, hidden.z, state.registry.states.water);
    let stone = block(&state, "stone");
    state
        .world
        .set_block(hidden.x, hidden.y + 1, hidden.z, stone);
    run_tick(&mut state, &[]);
    let frozen = state.world.block_at(cell.x, cell.y, cell.z).unwrap();
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(frozen))
            .unwrap()
            .block
            .id
            .path(),
        "frosted_ice"
    );
    assert_eq!(
        state.world.block_at(hidden.x, hidden.y, hidden.z),
        Some(state.registry.states.water)
    );
    let mut entries = Vec::new();
    state.block_ticks.journal_entries(&mut entries);
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, crate::journal::JournalEntry::BlockTicks { .. }))
    );
    let image = state.block_ticks.image();
    assert!(
        image
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.ticks)
            .any(|tick| tick.pos == [cell.x, cell.y, cell.z])
    );
    for _ in 0..8 {
        state.tick += 1;
        crate::enchantment_locations::changed(&mut state, cell);
    }
    assert_eq!(
        state.block_ticks.image(),
        image,
        "neighbor writes cannot multiply melting work"
    );
    state.block_ticks = crate::block_ticks::BlockTicks::from_image(
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap(),
        None,
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[36] = ItemStack::EMPTY;
    for _ in 0..500 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.world.block_at(cell.x, cell.y, cell.z),
        Some(state.registry.states.water)
    );
}

#[test]
fn smite_bane_and_impaling_use_authored_target_tags_and_admitted_bane_effect() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.tick = 20;
    for (mob, enchantment) in [
        ("zombie", "smite"),
        ("spider", "bane_of_arthropods"),
        ("squid", "impaling"),
    ] {
        let type_id = state
            .registry
            .blueprints
            .iter()
            .find(|(_, blueprint)| blueprint.name == format!("minecraft:{mob}"))
            .unwrap()
            .0;
        let target = state.spawn_mob(type_id, [pos[0] + 1.0, pos[1], pos[2]], 100.0);
        state
            .entities
            .row_mut(state.entities.slot_of(target).unwrap())
            .controls
            .no_ai = true;
        let weapon = stack(&state, "wooden_sword", &[(enchantment, 2)]);
        assert_eq!(
            enchantments::target_damage(&state, target, &weapon),
            5.0,
            "{mob}"
        );
        let cow = state.spawn_mob(
            state.registry.cow_type_id,
            [pos[0] + 2.0, pos[1], pos[2]],
            100.0,
        );
        assert_eq!(enchantments::target_damage(&state, cow, &weapon), 0.0);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
        run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target,
            }],
        );
        let slot = state.entities.slot_of(target).unwrap();
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        assert!(row.health < 100.0, "admitted {mob} melee hit");
        if mob == "spider" {
            let effect = row
                .active_effects
                .get(crate::effects::EffectId::Slowness)
                .unwrap();
            assert_eq!(effect.amplifier, 3);
            assert!((30..40).contains(&effect.duration));
            let image = crate::journal::EntityStateImage::from_row(body, row);
            let restored: crate::journal::EntityStateImage =
                serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
            assert_eq!(restored, image);
        }
        state.tick += 20;
    }
}

#[test]
fn thorns_rejects_blocked_hits_and_reflects_admitted_damage_in_the_same_commit() {
    for blocked in [true, false] {
        let (mut state, actor, pos) = flat_player_fixture();
        state.tick = 20;
        state
            .entities
            .row_mut(state.entities.slot_of(actor).unwrap())
            .protected_until = 0;
        let target = state.spawn_mob(
            state.registry.zombie_type_id,
            [pos[0] + 1.0, pos[1], pos[2]],
            100.0,
        );
        let armor = stack(&state, "diamond_chestplate", &[("thorns", 10)]);
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).controls.invulnerable = blocked;
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[4] =
            Some(armor);
        damage(&mut state, actor, target, 5.0);
        state.commit_merge_effects();
        assert!(
            state.merge_damage.is_empty(),
            "reflection cannot leak to the next tick"
        );
        let player = &state.players[&actor];
        if blocked {
            assert_eq!(player.vitals.health, 20.0);
            assert_eq!(
                state.entities.row(slot).appearance.equipment[4]
                    .as_ref()
                    .unwrap()
                    .damage
                    .unwrap_or(0),
                0
            );
        } else {
            assert!(player.vitals.health < 20.0 && player.vitals.health > 15.0);
            assert!(
                state.entities.row(slot).appearance.equipment[4]
                    .as_ref()
                    .unwrap()
                    .damage
                    .unwrap_or(0)
                    >= 2
            );
            let equipment = state.entities.row(slot).appearance.equipment.clone();
            let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
            let native = regions
                .values()
                .flat_map(|chunk| &chunk.entities)
                .find(|record| record.uuid == state.entities.row(slot).uuid)
                .unwrap();
            let restored = crate::entity_loot::equipment_from_fields(
                &state.registry.items,
                &native.extra_fields,
            )
            .unwrap();
            assert_eq!(restored, equipment);
        }
    }
}

#[test]
fn no_ai_stops_native_zombie_attacks_without_removing_the_entity() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.day_time = 14000;
    state
        .entities
        .row_mut(state.entities.slot_of(actor).unwrap())
        .protected_until = 0;
    state.players.get_mut(&actor).unwrap().vitals.food = 17;
    let zombie = state.spawn_mob(
        state.registry.zombie_type_id,
        [pos[0] + 1.0, pos[1], pos[2]],
        20.0,
    );
    state
        .entities
        .row_mut(state.entities.slot_of(zombie).unwrap())
        .controls
        .no_ai = true;
    for _ in 0..40 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&actor].vitals.health, 20.0);
    assert!(state.entities.slot_of(zombie).is_some());
    state
        .entities
        .row_mut(state.entities.slot_of(zombie).unwrap())
        .controls
        .no_ai = false;
    for _ in 0..40 {
        run_tick(&mut state, &[]);
    }
    assert!(state.players[&actor].vitals.health < 20.0);
}

#[test]
fn impaling_applies_to_actual_thrown_trident_collision() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.tick = 20;
    let type_id = state
        .registry
        .blueprints
        .iter()
        .find(|(_, blueprint)| blueprint.name == "minecraft:squid")
        .unwrap()
        .0;
    let target = state.spawn_mob(type_id, [pos[0] + 1.0, pos[1], pos[2]], 100.0);
    state
        .entities
        .row_mut(state.entities.slot_of(target).unwrap())
        .controls
        .no_ai = true;
    let payload = stack(&state, "trident", &[("impaling", 2)]);
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let record = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Trident,
        Some(owner),
        payload,
    );
    crate::projectiles::launch(
        &mut state,
        record,
        [pos[0], pos[1] + 0.4, pos[2]],
        [0.5, 0.0, 0.0],
    )
    .unwrap();
    for _ in 0..4 {
        run_tick(&mut state, &[]);
    }
    let health = state
        .entities
        .row(state.entities.slot_of(target).unwrap())
        .health;
    assert_eq!(health, 87.0, "base thrown trident 8 + Impaling II 5");
}

#[test]
fn existing_enchantment_protection_families_are_kind_specific_and_void_bypasses_all() {
    let (state, _, _) = flat_player_fixture();
    let rules = state.registry.item_rules();
    for (name, kind, factor) in [
        ("protection", DamageKind::Melee, 0.84),
        ("fire_protection", DamageKind::OnFire, 0.68),
        ("blast_protection", DamageKind::Explosion, 0.68),
        ("projectile_protection", DamageKind::Projectile, 0.68),
        ("feather_falling", DamageKind::Fall, 0.52),
    ] {
        let plain = [
            stack(&state, "diamond_boots", &[]),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
        ];
        let enchanted = [
            stack(&state, "diamond_boots", &[(name, 4)]),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
        ];
        let base = crate::survival::reduce_damage(&rules, &plain, 10.0, kind);
        let reduced = crate::survival::reduce_damage(&rules, &enchanted, 10.0, kind);
        assert!(
            (reduced - base * factor).abs() < 1e-5,
            "{name}: {reduced}/{base}"
        );
        assert_eq!(
            crate::survival::reduce_damage(&rules, &enchanted, 10.0, DamageKind::Void),
            10.0
        );
        if name != "protection" {
            assert_eq!(
                crate::survival::reduce_damage(&rules, &enchanted, 10.0, DamageKind::Melee),
                crate::survival::reduce_damage(&rules, &plain, 10.0, DamageKind::Melee)
            );
        }
    }
}

#[test]
fn existing_enchantment_fortune_silk_and_efficiency_change_real_loot_and_progress() {
    let (mut state, actor, _) = flat_player_fixture();
    let ore = block(&state, "diamond_ore");
    let ordinary = stack(&state, "diamond_pickaxe", &[]);
    let fortune = stack(&state, "diamond_pickaxe", &[("fortune", 3)]);
    let silk = stack(&state, "diamond_pickaxe", &[("silk_touch", 1)]);
    {
        let drops = crate::block_drops::BlockDropRules {
            blocks: &state.registry.blocks,
            loot: &state.registry.loot,
            mining: &state.registry.mining,
        };
        let rules = state.registry.item_rules();
        let mut improved = false;
        for seed in 0..64 {
            let base = crate::block_drops::block_drops(&drops, &rules, ore, &ordinary, seed, None)
                .unwrap();
            let boosted =
                crate::block_drops::block_drops(&drops, &rules, ore, &fortune, seed, None).unwrap();
            assert_eq!(base.iter().map(|stack| stack.count).sum::<i32>(), 1);
            improved |= boosted.iter().map(|stack| stack.count).sum::<i32>() > 1;
            let silk_drops =
                crate::block_drops::block_drops(&drops, &rules, ore, &silk, seed, None).unwrap();
            assert_eq!(silk_drops.len(), 1);
            assert_eq!(
                state
                    .registry
                    .items
                    .name_of(silk_drops[0].item_id)
                    .unwrap()
                    .path(),
                "diamond_ore"
            );
            assert_eq!(
                crate::block_drops::block_experience(&drops, &rules, ore, &silk, seed),
                0
            );
        }
        assert!(
            improved,
            "Fortune must change an actual compiled ore-table roll"
        );
    }
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.mining = mc_data::block_mining::BlockMiningTable::from_arrays(
        "fixture",
        vec![3.0; ore as usize + 1],
        vec![true; ore as usize + 1],
    );
    state.registry = std::sync::Arc::new(registry);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ordinary;
    let base = crate::effects::mining_progress(&state, actor, ore);
    assert!(base > 0.0);
    let fast = stack(&state, "diamond_pickaxe", &[("efficiency", 5)]);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = fast;
    assert!(crate::effects::mining_progress(&state, actor, ore) > base * 3.0);
}

#[test]
fn enchantment_sweeps_capture_each_targets_bonus_and_gate_all_effects_on_primary_admission() {
    for blocked in [false, true] {
        let (mut state, actor, pos) = flat_player_fixture();
        state.tick = 20;
        let primary = state.spawn_mob(
            state.registry.cow_type_id,
            [pos[0] + 1.0, pos[1], pos[2]],
            100.0,
        );
        let spider = state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:spider").unwrap())
            .unwrap();
        let secondary = state.spawn_mob(spider, [pos[0] + 1.1, pos[1], pos[2] + 0.5], 100.0);
        for id in [primary, secondary] {
            state
                .entities
                .row_mut(state.entities.slot_of(id).unwrap())
                .controls
                .no_ai = true;
        }
        state
            .entities
            .row_mut(state.entities.slot_of(primary).unwrap())
            .controls
            .invulnerable = blocked;
        let weapon = stack(
            &state,
            "wooden_sword",
            &[("fire_aspect", 1), ("bane_of_arthropods", 2)],
        );
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
        run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: primary,
            }],
        );
        let slot = state.entities.slot_of(secondary).unwrap();
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if blocked {
            assert_eq!(row.health, 100.0);
            assert_eq!(body.burn_ticks, 0);
            assert!(
                row.active_effects
                    .get(crate::effects::EffectId::Slowness)
                    .is_none()
            );
        } else {
            assert_eq!(row.health, 94.0);
            assert_eq!(body.burn_ticks, 80);
            assert_eq!(
                row.active_effects
                    .get(crate::effects::EffectId::Slowness)
                    .unwrap()
                    .amplifier,
                3
            );
        }
    }
}

fn location_fixture() -> (TickState, Vec<EntityId>) {
    let (mut state, first, pos) = flat_player_fixture();
    let hole = state.spawn_mob(state.registry.cow_type_id, pos, 20.0);
    let mut players = vec![first];
    for connection in 2..=6 {
        players.push(super::join(&mut state, connection, "location"));
    }
    state.remove_entity(state.entities.slot_of(hole).unwrap());
    assert!(
        state.entities.slot_of(players[5]).unwrap().0
            < state.entities.slot_of(players[1]).unwrap().0
    );
    for (index, &actor) in players.iter().enumerate() {
        let at = [pos[0] + index as f64 - 2.0, pos[1], pos[2]];
        let slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(slot, at);
        state.entities.set_ground(slot, index != 5);
        let boots = stack(
            &state,
            "diamond_boots",
            &[("soul_speed", 3), ("frost_walker", 2)],
        );
        let player = state.players.get_mut(&actor).unwrap();
        player.inventory.slots[36] = boots;
        player.enchantment_seed = index as i32 + 1;
        player.soul_speed_active = index == 5;
        if index == 3 {
            player.game_mode = crate::commands::GameMode::Spectator;
        }
        if index == 4 {
            state.entities.body_mut(slot).alive = false;
        }
        let soul = block(&state, "soul_sand");
        state.world.set_block(
            at[0].floor() as i32,
            at[1] as i32 - 1,
            at[2].floor() as i32,
            soul,
        );
        state.world.set_block(
            at[0].floor() as i32,
            at[1] as i32 - 1,
            at[2].floor() as i32 + 1,
            state.registry.states.water,
        );
    }
    let stale = state.players[&first].clone();
    state.players.insert(EntityId(2_000_000), stale);
    state.outbox.clear();
    (state, players)
}

#[test]
fn indexed_enchantment_locations_match_full_scan_after_compaction() {
    let (mut indexed, actors) = location_fixture();
    let (mut reference, reference_actors) = location_fixture();
    assert_eq!(actors, reference_actors);
    for tick in 0..100 {
        if tick == 40 {
            for state in [&mut indexed, &mut reference] {
                let _ = std::mem::take(
                    &mut state.players.get_mut(&actors[0]).unwrap().inventory.slots[36],
                );
            }
        }
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        crate::enchantment_locations::run(&mut indexed, &mut actual);
        crate::enchantment_locations::run_all_rows(&mut reference, &mut expected);
        assert_eq!(actual, expected, "intent order at tick {tick}");
        assert!(
            !actual.is_empty(),
            "fixture must exercise frost-walker intents"
        );
        assert_eq!(
            indexed.outbox, reference.outbox,
            "outbox order at tick {tick}"
        );
        for &actor in &actors {
            let actual = &indexed.players[&actor];
            let expected = &reference.players[&actor];
            assert_eq!(actual.inventory, expected.inventory);
            assert_eq!(actual.enchantment_seed, expected.enchantment_seed);
            assert_eq!(actual.soul_speed_active, expected.soul_speed_active);
            assert_eq!(actual.attribute_equipment, expected.attribute_equipment);
        }
        indexed.outbox.clear();
        reference.outbox.clear();
    }
    assert_ne!(indexed.players[&actors[1]].enchantment_seed, 2);
    assert!(!indexed.players[&actors[0]].soul_speed_active);
    assert!(indexed.players[&actors[5]].soul_speed_active);
}

#[test]
#[ignore = "release-only stage microbenchmark; run under mem-run -p"]
fn enchantment_locations_release_micro() {
    let (mut state, _, pos) = flat_player_fixture();
    for connection in 2..=100 {
        super::join(&mut state, connection, "location-micro");
    }
    for _ in 0..105_000 {
        state.spawn_mob(state.registry.cow_type_id, pos, 20.0);
    }
    eprintln!(
        "enchantment location micro: rows={} players={}",
        state.entities.len(),
        state.players.len(),
    );
    let mut intents = Vec::new();
    for (label, run) in [
        (
            "B",
            crate::enchantment_locations::run
                as fn(&mut TickState, &mut Vec<crate::intent::Intent>),
        ),
        ("A", crate::enchantment_locations::run_all_rows),
        ("B", crate::enchantment_locations::run),
        ("A", crate::enchantment_locations::run_all_rows),
    ] {
        run(&mut state, &mut intents);
        let start = std::time::Instant::now();
        for _ in 0..128 {
            run(std::hint::black_box(&mut state), &mut intents);
        }
        eprintln!(
            "enchantment location micro {label}: {:.6} ms/pass",
            start.elapsed().as_secs_f64() * 1000.0 / 128.0
        );
    }
    assert!(intents.is_empty());
}
