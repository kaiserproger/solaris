use super::*;
use crate::boundary::{ConnectionId, Hand, PlayerChange, PlayerInput};
use crate::commands::{Command, GameMode};
use crate::inventory::ItemRules;
use crate::player::PlayerState;
use mc_data::{Identifier, ItemStack};

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}
struct Data {
    items: mc_data::items::ItemRegistry,
    facts: mc_data::item_components::ItemFactsTable,
    tags: mc_data::tags::TagsData,
}
impl Data {
    fn new() -> Self {
        use mc_data::item_components::ItemFacts;
        let names = [
            "golden_apple",
            "enchanted_golden_apple",
            "milk_bucket",
            "bucket",
            "potion",
            "glass_bottle",
            "suspicious_stew",
            "bowl",
        ];
        let items = mc_data::items::ItemRegistry::from_report(
            &names
                .iter()
                .enumerate()
                .map(|(index, name)| mc_data::items::ItemReport {
                    id: id(&format!("minecraft:{name}")),
                    protocol_id: index as u32 + 1,
                })
                .collect::<Vec<_>>(),
        );
        // Golden apples take their vanilla consumable effects from the
        // embedded component catalog, as the real registry does.
        let required = mc_data::item_components::solaris_required_item_facts();
        let facts =
            mc_data::item_components::ItemFactsTable::from_entries(names.iter().map(|name| {
                let name = id(&format!("minecraft:{name}"));
                let food = matches!(
                    name.path(),
                    "golden_apple" | "enchanted_golden_apple" | "suspicious_stew"
                )
                .then_some(mc_data::food::FoodEntry {
                    food: 4,
                    saturation: 9.6,
                    can_always_eat: true,
                });
                let consume_effects = required
                    .get(&name)
                    .map(|facts| facts.consume_effects.clone())
                    .unwrap_or_default();
                (
                    name,
                    ItemFacts {
                        food,
                        use_duration_ticks: Some(32),
                        consume_effects,
                        ..Default::default()
                    },
                )
            }));
        Self {
            items,
            facts,
            tags: Default::default(),
        }
    }
    fn rules(&self) -> ItemRules<'_> {
        ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &[],
        }
    }
    fn stack(&self, name: &str) -> ItemStack {
        ItemStack::new(self.items.id_of(&id(name)).unwrap(), 1)
    }
}
fn consume(player: &mut PlayerState, data: &Data, stack: ItemStack) -> Vec<PlayerChange> {
    player.inventory.slots[0] = stack;
    player.use_item(&data.rules(), Hand::Main);
    let mut changes = Vec::new();
    for _ in 0..32 {
        changes.extend(player.tick_item_use(&data.rules()).changes);
    }
    changes
}
#[test]
fn golden_apple_gives_absorption_and_regeneration_and_heals() {
    let data = Data::new();
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.vitals.health = 10.0;
    let changes = consume(&mut player, &data, data.stack("minecraft:golden_apple"));
    assert!(player.active_effects.has(EffectId::Absorption));
    assert_eq!(
        player
            .active_effects
            .get(EffectId::Regeneration)
            .unwrap()
            .amplifier,
        1
    );
    assert_eq!(player.active_effects.absorption, 4.0);
    assert!(changes.iter().any(|change|matches!(change,PlayerChange::MobEffect {effect,..} if effect.id==EffectId::Regeneration)));
    for tick in 0..100 {
        player.active_effects.tick(&mut player.vitals, tick, 20.0);
    }
    assert_eq!(player.vitals.health, 14.0);
    assert!(!player.active_effects.has(EffectId::Regeneration));
}
#[test]
fn poison_stops_at_one_hp_and_infinite_never_expires() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Poison, -1, 8));
    let mut vitals = Vitals::default();
    vitals.health = 5.5;
    for tick in 0..100 {
        for (kind, amount) in effects.tick(&mut vitals, tick, 20.0).damage {
            vitals.hurt(kind, amount);
        }
    }
    assert_eq!(vitals.health, 1.0);
    assert_eq!(effects.get(EffectId::Poison).unwrap().duration, -1);
}
#[test]
fn milk_consumption_clears_effects_absorption_and_returns_bucket() {
    let data = Data::new();
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.give_effect(ActiveEffect::new(EffectId::Speed, 400, 1));
    player.give_effect(ActiveEffect::new(EffectId::Absorption, 400, 1));
    let changes = consume(&mut player, &data, data.stack("minecraft:milk_bucket"));
    assert!(player.active_effects.entries.is_empty());
    assert_eq!(player.active_effects.absorption, 0.0);
    assert_eq!(player.inventory.slots[0], data.stack("minecraft:bucket"));
    assert_eq!(
        changes
            .iter()
            .filter(|change| matches!(change, PlayerChange::RemoveMobEffect { .. }))
            .count(),
        2
    );
}
#[test]
fn potion_and_suspicious_stew_apply_component_effects() {
    let data = Data::new();
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    let mut potion = data.stack("minecraft:potion");
    potion.extended_mut().potion = Some(id("minecraft:strong_healing"));
    potion
        .extended_mut()
        .potion_custom_effects
        .push(mc_data::item_stack::PotionEffect {
            id: id("minecraft:speed"),
            amplifier: 2,
            duration: 120,
            ambient: true,
            show_particles: false,
            show_icon: true,
            hidden: None,
        });
    player.vitals.health = 3.0;
    consume(&mut player, &data, potion);
    player.active_effects.tick(&mut player.vitals, 0, 20.0);
    assert_eq!(player.vitals.health, 11.0);
    let speed = player.active_effects.get(EffectId::Speed).unwrap();
    assert_eq!((speed.amplifier, speed.duration), (2, 119));
    assert!(speed.ambient);
    assert!(!speed.show_particles);
    assert!(speed.show_icon);
    let mut stew = data.stack("minecraft:suspicious_stew");
    stew.stew_effects.push(mc_data::item_stack::StewEffect {
        id: id("minecraft:night_vision"),
        duration: 100,
    });
    consume(&mut player, &data, stew);
    assert_eq!(
        player
            .active_effects
            .get(EffectId::NightVision)
            .unwrap()
            .duration,
        100
    );
}
#[test]
fn damage_reduction_absorption_fire_resistance_and_wither() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Resistance, 200, 0));
    effects.add(ActiveEffect::new(EffectId::Absorption, 200, 0));
    assert!((effects.reduce_damage(DamageKind::Melee, 10.0) - 4.0).abs() < 0.001);
    assert_eq!(effects.absorption, 0.0);
    effects.add(ActiveEffect::new(EffectId::FireResistance, 200, 0));
    assert_eq!(effects.reduce_damage(DamageKind::Lava, 4.0), 0.0);
    assert_eq!(effects.reduce_damage(DamageKind::Void, 100.0), 100.0);
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Wither, 1, 8));
    let mut vitals = Vitals::default();
    vitals.health = 1.0;
    for (kind, amount) in effects.tick(&mut vitals, 0, 20.0).damage {
        vitals.hurt(kind, amount);
    }
    assert!(vitals.is_dead());
}
#[test]
fn stronger_short_effect_restores_weaker_remaining_duration() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Speed, 100, 0));
    effects.add(ActiveEffect::new(EffectId::Speed, 20, 1));
    let mut vitals = Vitals::default();
    let mut promoted = false;
    for tick in 0..20 {
        promoted |= !effects.tick(&mut vitals, tick, 20.0).updated.is_empty();
    }
    assert!(promoted);
    let speed = effects.get(EffectId::Speed).unwrap();
    assert_eq!((speed.amplifier, speed.duration), (0, 80));
}
#[test]
fn speed_changes_existing_movement_speed_attribute_path_and_clears() {
    let data = Data::new();
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.equipment_attributes(&data.rules(), &[]).unwrap();
    player.give_effect(ActiveEffect::new(EffectId::Speed, 200, 1));
    let PlayerChange::Attributes { attributes, .. } =
        player.equipment_attributes(&data.rules(), &[]).unwrap()
    else {
        panic!("attribute sync")
    };
    let speed = attributes
        .iter()
        .find(|attribute| attribute.kind == "movement_speed")
        .unwrap();
    assert_eq!(speed.base, 0.1);
    assert_eq!(speed.modifiers.len(), 1);
    assert_eq!(speed.modifiers[0].id, "minecraft:effect.speed");
    assert!((speed.modifiers[0].amount - 0.4).abs() < 1e-7);
    player.clear_effects(Some(EffectId::Speed));
    let PlayerChange::Attributes { attributes, .. } =
        player.equipment_attributes(&data.rules(), &[]).unwrap()
    else {
        panic!("attribute reset")
    };
    assert!(
        attributes
            .iter()
            .find(|attribute| attribute.kind == "movement_speed")
            .unwrap()
            .modifiers
            .is_empty()
    );
}
#[test]
fn effect_command_vanilla_syntax_permission_and_tick_execution() {
    use crate::commands::parse;
    assert!(parse("effect give @s speed", false).is_err());
    assert!(parse("effect give @s speed 0", true).is_err());
    assert!(parse("effect give @s speed 10 256", true).is_err());
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = std::sync::Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        std::sync::Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = crate::tick::TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Alice".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let command = |state: &mut crate::tick::TickState, input: &str, permission_level| {
        crate::tick::run_tick(
            state,
            &[PlayerInput::Command {
                connection: ConnectionId(1),
                command: input.into(),
                permission_level,
            }],
        )
    };
    command(&mut state, "effect give @s speed", 1);
    assert!(state.players[&entity].active_effects.entries.is_empty());
    let give = parse("effect give @s minecraft:speed infinite 1 true", true).unwrap();
    let Command::EffectGive { effect, .. } = &give else {
        panic!("parsed give")
    };
    assert_eq!(effect.duration, -1);
    assert!(!effect.show_particles);
    assert!(!effect.show_icon);
    let (_, outbox, _) = command(
        &mut state,
        "effect give @s minecraft:speed infinite 1 true",
        2,
    );
    assert!(outbox.iter().flat_map(|(_,changes)|changes).any(|change|
        matches!(change,PlayerChange::Attributes {attributes,..} if attributes.iter().any(|attribute|
            attribute.kind=="movement_speed" && attribute.modifiers.iter().any(|modifier|modifier.id=="minecraft:effect.speed")))));
    assert!(state.players[&entity].active_effects.has(EffectId::Speed));
    command(&mut state, "effect clear @s speed", 2);
    assert!(state.players[&entity].active_effects.entries.is_empty());
    command(&mut state, "effect give @e[type=player] absorption 30", 2);
    assert_eq!(state.players[&entity].active_effects.absorption, 4.0);
    command(&mut state, "effect clear", 2);
    assert_eq!(state.players[&entity].active_effects.absorption, 0.0);
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let cow = state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 0.0], 5.0);
    command(&mut state, "effect give @e[type=cow] regeneration 10", 2);
    let slot = state.entities.slot_of(cow).unwrap();
    assert_eq!(state.entities.row(slot).health, 6.0);
    assert!(
        state
            .entities
            .row(slot)
            .active_effects
            .has(EffectId::Regeneration)
    );
    command(&mut state, "effect clear @e[type=cow]", 2);
    assert!(state.entities.row(slot).active_effects.entries.is_empty());
}
#[test]
fn haste_fatigue_melee_and_safe_fall_modifiers() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Haste, 100, 1));
    assert!((effects.dig_multiplier() - 1.4).abs() < 0.001);
    effects.add(ActiveEffect::new(EffectId::MiningFatigue, 100, 0));
    assert!((effects.dig_multiplier() - 0.42).abs() < 0.001);
    effects.add(ActiveEffect::new(EffectId::Strength, 100, 1));
    effects.add(ActiveEffect::new(EffectId::Weakness, 100, 0));
    assert_eq!(effects.melee_bonus(), 2.0);
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.give_effect(ActiveEffect::new(EffectId::JumpBoost, 100, 1));
    player.fall(-6.0, false, false);
    assert_eq!(player.fall(0.0, true, false), 1.0);
    player.give_effect(ActiveEffect::new(EffectId::SlowFalling, 100, 0));
    player.fall(-20.0, false, false);
    assert_eq!(player.fall(0.0, true, false), 0.0);
}

#[test]
fn landing_move_counts_its_own_drop_toward_fall_damage() {
    // Vanilla Entity.checkFallDamage accumulates `ya < 0` before the
    // on-ground test, so a 3.5-block fall whose last 0.5 arrives on the landing
    // packet is 3.5 > 3 safe blocks: ceil(0.5) = 1 damage, not 0.
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    player.fall(-3.0, false, false);
    assert_eq!(player.fall(-0.5, true, false), 1.0);
    assert_eq!(player.fall_distance, 0.0);
}

#[test]
fn absorption_replenishes_on_consumption_and_expires_when_used() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Absorption, 200, 0));
    assert_eq!(effects.reduce_damage(DamageKind::Melee, 3.0), 0.0);
    assert_eq!(effects.absorption, 1.0);
    effects.add(ActiveEffect::new(EffectId::Absorption, 200, 0));
    assert_eq!(effects.absorption, 4.0);
    assert_eq!(effects.reduce_damage(DamageKind::Melee, 5.0), 1.0);
    let out = effects.tick(&mut Vitals::default(), 1, 20.0);
    assert_eq!(out.removed, vec![EffectId::Absorption]);
}

#[test]
fn regeneration_uses_living_entities_maximum_health_not_player_cap() {
    let mut effects = ActiveEffects::default();
    effects.add(ActiveEffect::new(EffectId::Regeneration, 50, 0));
    let mut vitals = Vitals::default();
    vitals.health = 75.0;
    effects.tick(&mut vitals, 1, 80.0);
    assert_eq!(vitals.health, 76.0);
}

#[test]
fn advancement_effect_evidence_tracks_only_accepted_transitions() {
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Survival);
    assert!(!player.advancement_effects_changed);
    assert!(
        player
            .give_effect(ActiveEffect::new(EffectId::Speed, 0, 0))
            .is_empty()
    );
    assert!(!player.advancement_effects_changed);
    let mut effect = ActiveEffect::new(EffectId::Speed, 400, 1);
    effect.ambient = true;
    effect.show_particles = false;
    assert!(!player.give_effect(effect.clone()).is_empty());
    assert!(std::mem::take(&mut player.advancement_effects_changed));
    assert_eq!(
        advancement_effect_snapshot(&player.active_effects),
        serde_json::json!({
            "minecraft:speed": {
                "amplifier": 1,
                "duration": 400,
                "ambient": true,
                "visible": false,
            },
        }),
    );
    assert!(player.give_effect(effect).is_empty());
    assert!(!player.advancement_effects_changed);
    assert!(player.clear_effects(Some(EffectId::Poison)).is_empty());
    assert!(!player.advancement_effects_changed);
    assert!(!player.clear_effects(Some(EffectId::Speed)).is_empty());
    assert!(player.advancement_effects_changed);
    assert_eq!(
        advancement_effect_snapshot(&player.active_effects),
        serde_json::json!({})
    );
}

#[test]
fn run_skips_effect_free_population_without_side_effects() {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = crate::tick::TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Alice".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    let cow = state.spawn_mob(state.registry.cow_type_id, [0.0, 80.0, 0.0], 10.0);
    let before_outbox = state.outbox.clone();
    let before_touched = state.entities.touched_ids().len();
    let before_appearance = state.appearance_changed.len();
    super::run(&mut state, &mut Vec::new());
    assert!(state.players[&entity].active_effects.entries.is_empty());
    assert!(
        state
            .entities
            .row(state.entities.slot_of(cow).unwrap())
            .active_effects
            .entries
            .is_empty()
    );
    assert_eq!(state.outbox, before_outbox);
    assert_eq!(state.entities.touched_ids().len(), before_touched);
    assert_eq!(state.appearance_changed.len(), before_appearance);
}

#[test]
fn run_clears_lingering_appearance_flags_without_effects() {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = crate::tick::TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Alice".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    // Stale client-visible flag with no active effects must still reach
    // appearance sync; the run probe keeps appearance-flagged bodies.
    let slot = state.entities.slot_of(entity).unwrap();
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 0x20;
    let before_appearance = state.appearance_changed.len();
    super::run(&mut state, &mut Vec::new());
    assert!(state.players[&entity].active_effects.entries.is_empty());
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(state.entities.row(slot).appearance.shared_flags & 0x60, 0);
    assert!(state.appearance_changed.len() > before_appearance);
}

#[test]
fn run_projects_and_expires_invisibility_appearance() {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = crate::tick::TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Alice".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    state
        .players
        .get_mut(&entity)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::Invisibility, 2, 0));
    super::run(&mut state, &mut Vec::new());
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & 0x60,
        0x20
    );
    super::run(&mut state, &mut Vec::new());
    assert!(state.players[&entity].active_effects.entries.is_empty());
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(state.entities.row(slot).appearance.shared_flags & 0x60, 0);
}

#[test]
fn run_uses_blueprint_max_health_without_equipment_or_attributes() {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = crate::tick::TickState::new(registry, world, 1, 4096);
    let cow = state.spawn_mob(state.registry.cow_type_id, [8.5, 80.0, 8.5], 5.0);
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    state.world.set_active_chunks(
        std::iter::once(crate::boundary::ChunkPos { x: 0, z: 0 }).collect(),
        state.tick,
    );
    // Admit the empty population first, then mutate into the effect lane.
    super::run(&mut state, &mut Vec::new());
    let slot = state.entities.slot_of(cow).unwrap();
    state
        .entities
        .row_mut(slot)
        .active_effects
        .add(ActiveEffect::new(EffectId::Regeneration, 50, 0));
    let max_health = state
        .registry
        .blueprint(state.registry.cow_type_id)
        .map_or(20.0, |blueprint| blueprint.params.max_health);
    super::run(&mut state, &mut Vec::new());
    assert!(state.entities.row(slot).health > 5.0);
    assert!(state.entities.row(slot).health <= max_health);
}
