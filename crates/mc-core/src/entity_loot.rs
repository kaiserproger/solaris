//! Native contextual mob death, retained attributes, equipment and on-hit effects.
use crate::boundary::{EntityDataValue, EntityId};
use crate::entities::{ColdRow, EntityKind, EntitySlot};
use crate::intent::DamageSource;
use crate::inventory::ItemRules;
use crate::survival::{DamageKind, Difficulty};
use crate::tick::TickState;
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootAttack, EntityLootEntity, EntityLootPlayerAttribution,
};
use mc_data::loot::{LootContextItem, LootEnchantments};
use mc_data::{Identifier, ItemStack};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityAttributeModifier {
    pub id: String,
    pub amount: f64,
    pub operation: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityAttribute {
    pub id: String,
    pub base: f64,
    #[serde(default)]
    pub modifiers: Vec<EntityAttributeModifier>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerLootCredit {
    pub id: i32,
    pub tick: u64,
    pub held: ItemStack,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EntityLootState {
    /// Equipment itself has one owner: ColdRow.appearance.equipment.
    pub attributes: Vec<EntityAttribute>,
    pub slime_size: Option<u8>,
    pub player_credit: Option<Box<PlayerLootCredit>>,
    pub death_handled: bool,
    pub can_pick_up_loot: bool,
    pub drop_chances: [f32; 6],
    pub body_armor: Option<Box<ItemStack>>,
    pub saddle: Option<Box<ItemStack>>,
    pub extra_drop_chances: [f32; 2],
    /// Derived from the blueprint at spawn/restore, not copied attribute records.
    #[serde(skip)]
    pub base_armor: f64,
    #[serde(skip)]
    pub base_knockback_resistance: f64,
}
impl Default for EntityLootState {
    fn default() -> Self {
        Self {
            attributes: Vec::new(),
            slime_size: None,
            player_credit: None,
            death_handled: false,
            can_pick_up_loot: false,
            drop_chances: [0.085; 6],
            body_armor: None,
            saddle: None,
            extra_drop_chances: [2.0; 2],
            base_armor: 0.0,
            base_knockback_resistance: 0.0,
        }
    }
}
fn id(name: &str) -> Identifier {
    Identifier::parse(name).expect("native entity identifier")
}
fn stack_entity(items: &ItemRules<'_>, entity: &mut EntityLootEntity, held: &ItemStack) {
    if held.is_empty() {
        return;
    }
    entity.active_enchantments = LootEnchantments::try_from_levels(
        held.enchantments
            .iter()
            .filter(|entry| entry.level > 0)
            .map(|entry| (entry.id.clone(), entry.level as u32)),
    )
    .expect("authoritative stack enchantments");
    entity.mainhand = items.items.name_of(held.item_id).map(|name| {
        LootContextItem::new(name.clone()).with_enchantments(entity.active_enchantments.clone())
    });
}
pub(crate) fn loot_entity(state: &TickState, entity: EntityId) -> Option<EntityLootEntity> {
    let slot = state.entities.slot_of_any(entity)?;
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let name = match body.kind {
        EntityKind::Player { .. } => "minecraft:player",
        EntityKind::Mob { type_id } => state.registry.blueprint(type_id)?.name.as_str(),
        _ => return None,
    };
    let mut result = EntityLootEntity::new(id(name));
    result.flags.is_baby = row.appearance.baby;
    result.flags.is_on_fire = row.appearance.shared_flags & 1 != 0;
    result.slime_size = row.loot.slime_size.map(i32::from);
    let held = state
        .players
        .get(&entity)
        .map(|player| &player.inventory.slots[usize::from(player.inventory.selected.min(8))])
        .or(row.appearance.equipment[0].as_ref());
    if let Some(held) = held {
        stack_entity(&state.registry.item_rules(), &mut result, held);
    }
    Some(result)
}
/// Capture player credit only on accepted damage; survives environmental finishing hits/restart.
pub(crate) fn accepted_hit(state: &mut TickState, slot: EntitySlot, source: Option<DamageSource>) {
    let attacker = match source {
        Some(
            DamageSource::Melee { attacker, .. }
            | DamageSource::Mace { attacker, .. }
            | DamageSource::Sweep { attacker, .. },
        ) => attacker,
        Some(DamageSource::Projectile { owner, .. }) => owner,
        Some(DamageSource::Command {
            cause: Some((attacker, _)),
            ..
        }) => attacker,
        Some(DamageSource::Command { cause: None, .. }) => return,
        None | Some(DamageSource::Explosion { .. }) => return,
    };
    if let Some(player) = state.players.get(&attacker) {
        let held = player.inventory.slots[usize::from(player.inventory.selected.min(8))].clone();
        crate::entities::set_boxed(
            &mut state.entities.row_mut(slot).loot.player_credit,
            PlayerLootCredit {
                id: attacker.0,
                tick: state.tick,
                held,
            },
        );
    }
    let Some(DamageSource::Melee { .. }) = source else {
        return;
    };
    let Some(actor) = state.entities.slot_of_any(attacker) else {
        return;
    };
    let EntityKind::Mob { type_id } = state.entities.body(actor).kind else {
        return;
    };
    if state
        .registry
        .blueprint(type_id)
        .is_none_or(|bp| !bp.capabilities.poison_melee)
    {
        return;
    }
    let Some(effect) = cave_spider_poison(state.difficulty) else {
        return;
    };
    let target = state.entities.body(slot).id;
    if let Some(player) = state.players.get_mut(&target) {
        let connection = player.connection;
        let changes = player.give_effect(effect);
        for change in changes {
            state.push_player(connection, change);
        }
    } else {
        state.entities.row_mut(slot).active_effects.add(effect);
    }
    crate::effects::sync_appearance(state, target);
}
pub fn cave_spider_poison(difficulty: Difficulty) -> Option<crate::effects::ActiveEffect> {
    let duration = match difficulty {
        Difficulty::Normal => 140,
        Difficulty::Hard => 300,
        Difficulty::Peaceful | Difficulty::Easy => return None,
    };
    Some(crate::effects::ActiveEffect::new(
        crate::effects::EffectId::Poison,
        duration,
        0,
    ))
}
pub(crate) fn death_cause(
    state: &TickState,
    slot: EntitySlot,
    source: Option<DamageSource>,
    kind: DamageKind,
) -> EntityDeathCause {
    let attack = match source {
        Some(
            DamageSource::Melee { attacker, .. }
            | DamageSource::Mace { attacker, .. }
            | DamageSource::Sweep { attacker, .. },
        ) => loot_entity(state, attacker).map_or(EntityLootAttack::None, EntityLootAttack::Direct),
        Some(DamageSource::Projectile {
            owner,
            direct_type_id,
            ..
        }) => {
            let direct_type =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(direct_type_id)
                    .expect("admitted projectile registry identity");
            let mut direct = EntityLootEntity::new(id(direct_type.name));
            direct.is_living = false;
            EntityLootAttack::Indirect {
                source: loot_entity(state, owner),
                direct,
            }
        }
        Some(DamageSource::Command { direct, cause, .. }) => {
            if let Some((entity, type_id)) = direct {
                let direct = loot_entity(state, entity).unwrap_or_else(|| {
                    let contract =
                        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                            .expect("admitted command entity identity");
                    EntityLootEntity::new(id(contract.name))
                });
                EntityLootAttack::Indirect {
                    source: cause.and_then(|(entity, _)| loot_entity(state, entity)),
                    direct,
                }
            } else {
                EntityLootAttack::None
            }
        }
        None | Some(DamageSource::Explosion { .. }) => EntityLootAttack::None,
    };
    let credit = state
        .entities
        .row(slot)
        .loot
        .player_credit
        .as_ref()
        .filter(|credit| state.tick.saturating_sub(credit.tick) < 100)
        .map(|credit| {
            let mut player = EntityLootEntity::new(id("minecraft:player"));
            stack_entity(&state.registry.item_rules(), &mut player, &credit.held);
            EntityLootPlayerAttribution::try_new(player).expect("native player credit")
        });
    let mut tags = std::collections::BTreeSet::new();
    if matches!(
        kind,
        DamageKind::InFire
            | DamageKind::OnFire
            | DamageKind::Lava
            | DamageKind::Fireball
            | DamageKind::Lightning
            | DamageKind::HotFloor
            | DamageKind::Campfire
    ) {
        tags.insert(id("minecraft:is_fire"));
    }
    if kind == DamageKind::Explosion {
        tags.insert(id("minecraft:is_explosion"));
    }
    if matches!(kind, DamageKind::Projectile | DamageKind::Fireball) {
        tags.insert(id("minecraft:is_projectile"));
    }
    if kind.bypasses_invulnerability() {
        tags.insert(id("minecraft:bypasses_invulnerability"));
    }
    EntityDeathCause::new(tags, attack, credit)
}
fn slot_matches(group: &str, slot: usize) -> bool {
    match group.trim_start_matches("minecraft:") {
        "any" => true,
        "hand" => slot < 2,
        "armor" => (2..7).contains(&slot),
        "mainhand" => slot == 0,
        "offhand" => slot == 1,
        "feet" => slot == 2,
        "legs" => slot == 3,
        "chest" => slot == 4,
        "head" => slot == 5,
        "body" => slot == 6,
        "saddle" => slot == 7,
        _ => false,
    }
}
fn equipment(row: &ColdRow) -> [Option<&ItemStack>; 8] {
    [
        row.appearance.equipment[0].as_ref(),
        row.appearance.equipment[1].as_ref(),
        row.appearance.equipment[2].as_ref(),
        row.appearance.equipment[3].as_ref(),
        row.appearance.equipment[4].as_ref(),
        row.appearance.equipment[5].as_ref(),
        row.loot.body_armor.as_deref(),
        row.loot.saddle.as_deref(),
    ]
}

/// Death-only equipment reward; picked-up equipment has drop chance above one.
pub(crate) fn death_experience(name: &str, row: &ColdRow, seed: u64) -> i32 {
    let mut xp = row.loot.slime_size.map_or_else(
        || crate::experience_orbs::mob_experience(name, row.appearance.baby, seed),
        i32::from,
    );
    if xp == 0 {
        return 0;
    }
    let mut rng = crate::natural_spawn::Rng(seed);
    for (index, stack) in equipment(row).into_iter().enumerate() {
        let chance = if index < 6 {
            row.loot.drop_chances[index]
        } else {
            row.loot.extra_drop_chances[index - 6]
        };
        if chance <= 1.0 && stack.is_some_and(|stack| !stack.is_empty()) {
            xp += 1 + rng.below(3) as i32;
        }
    }
    xp
}
/// Evaluate retained bases/modifiers plus slot-matched equipment without heap scratch.
pub fn effective_attribute(rules: &ItemRules<'_>, row: &ColdRow, name: &str, default: f64) -> f64 {
    effective_attribute_impl(rules, row, name, default, false)
}

fn effective_attribute_impl(
    rules: &ItemRules<'_>,
    row: &ColdRow,
    name: &str,
    default: f64,
    physical: bool,
) -> f64 {
    let attribute = row
        .loot
        .attributes
        .iter()
        .find(|entry| entry.id.trim_start_matches("minecraft:") == name);
    let default = match name {
        "armor" => row.loot.base_armor,
        "knockback_resistance" => row.loot.base_knockback_resistance,
        _ => default,
    };
    let base = attribute.map_or(default, |entry| entry.base);
    let mut sums = [0.0; 2];
    let mut product = 1.0;
    let mut apply = |amount: f64, operation: &str| match operation {
        "add_value" => sums[0] += amount,
        "add_multiplied_base" => sums[1] += amount,
        "add_multiplied_total" => product *= 1.0 + amount,
        _ => {}
    };
    if let Some(attribute) = attribute {
        for modifier in &attribute.modifiers {
            let replaced = equipment(row).into_iter().enumerate().any(|(slot, stack)| {
                stack
                    .as_ref()
                    .filter(|stack| !stack.is_empty())
                    .and_then(|stack| rules.facts.facts_for_stack(stack, rules.items))
                    .is_some_and(|facts| {
                        facts.attribute_modifiers.iter().any(|next| {
                            next.id == modifier.id
                                && next.kind.trim_start_matches("minecraft:") == name
                                && slot_matches(next.slot.as_deref().unwrap_or("any"), slot)
                        })
                    })
            });
            if !replaced {
                apply(modifier.amount, &modifier.operation);
            }
        }
    }
    for (slot, stack) in equipment(row).into_iter().enumerate() {
        let Some(facts) = stack
            .filter(|stack| !stack.is_empty())
            .and_then(|stack| rules.facts.facts_for_stack(stack, rules.items))
            .filter(|facts| {
                !physical
                    || facts
                        .physical_profile
                        .as_ref()
                        .is_none_or(|profile| profile.armor.is_none())
            })
        else {
            continue;
        };
        for modifier in &facts.attribute_modifiers {
            if modifier.kind.trim_start_matches("minecraft:") != name
                || !slot_matches(modifier.slot.as_deref().unwrap_or("any"), slot)
            {
                continue;
            }
            // AttributeInstance replaces modifier ids across equipment slots.
            let replaced =
                equipment(row)
                    .into_iter()
                    .enumerate()
                    .skip(slot + 1)
                    .any(|(later, stack)| {
                        stack
                            .as_ref()
                            .filter(|stack| !stack.is_empty())
                            .and_then(|stack| rules.facts.facts_for_stack(stack, rules.items))
                            .filter(|facts| {
                                !physical
                                    || facts
                                        .physical_profile
                                        .as_ref()
                                        .is_none_or(|profile| profile.armor.is_none())
                            })
                            .is_some_and(|facts| {
                                facts.attribute_modifiers.iter().any(|next| {
                                    next.id == modifier.id
                                        && next.kind == modifier.kind
                                        && slot_matches(
                                            next.slot.as_deref().unwrap_or("any"),
                                            later,
                                        )
                                })
                            })
                    });
            if !replaced {
                apply(modifier.amount, &modifier.operation);
            }
        }
    }
    let maximum = match name {
        "armor" => 30.0,
        "armor_toughness" => 20.0,
        "knockback_resistance" => 1.0,
        "max_health" | "movement_speed" => 1024.0,
        "attack_damage" => 2048.0,
        _ => f64::MAX,
    };
    ((base + sums[0]) * (1.0 + sums[1]) * product).clamp(0.0, maximum)
}
pub(crate) fn reduce_damage_with_breach(
    rules: &ItemRules<'_>,
    row: &ColdRow,
    amount: f32,
    kind: DamageKind,
    breach: u32,
) -> f32 {
    reduce_damage_impl(rules, row, amount, kind, breach, false)
}

pub(crate) fn reduce_damage_after_physical(
    rules: &ItemRules<'_>,
    row: &ColdRow,
    amount: f32,
    kind: DamageKind,
    breach: u32,
) -> f32 {
    reduce_damage_impl(rules, row, amount, kind, breach, true)
}

fn reduce_damage_impl(
    rules: &ItemRules<'_>,
    row: &ColdRow,
    amount: f32,
    kind: DamageKind,
    breach: u32,
    physical: bool,
) -> f32 {
    if kind.bypasses_invulnerability()
        || matches!(kind, DamageKind::Starvation | DamageKind::SonicBoom)
    {
        return amount;
    }
    let amount = if matches!(
        kind,
        DamageKind::FallingAnvil | DamageKind::FallingStalactite
    ) && row.appearance.equipment[5]
        .as_ref()
        .is_some_and(|helmet| !helmet.is_empty())
    {
        amount * 0.75
    } else {
        amount
    };
    let bypass = kind.bypasses_armor();
    let armor = effective_attribute_impl(rules, row, "armor", 0.0, physical) as f32;
    let toughness = effective_attribute_impl(rules, row, "armor_toughness", 0.0, physical) as f32;
    let reduced = if bypass {
        amount
    } else {
        crate::enchantments::breach_damage(
            amount,
            mc_data::armor::armor_reduced_damage(
                amount,
                mc_data::armor::ArmorStats { armor, toughness },
            ),
            breach,
        )
    };
    let mut protection = 0_i32;
    for stack in row.appearance.equipment[2..].iter().flatten() {
        let level = |name: &str| {
            stack
                .enchantments
                .iter()
                .filter(|entry| entry.id.as_str() == name)
                .map(|entry| entry.level.max(0))
                .max()
                .unwrap_or(0)
        };
        protection = protection.saturating_add(level("minecraft:protection"));
        let specialized = match kind {
            DamageKind::InFire
            | DamageKind::OnFire
            | DamageKind::Lava
            | DamageKind::Lightning
            | DamageKind::HotFloor
            | DamageKind::Campfire => level("minecraft:fire_protection").saturating_mul(2),
            DamageKind::Fall | DamageKind::Stalagmite => {
                level("minecraft:feather_falling").saturating_mul(3)
            }
            DamageKind::Explosion => level("minecraft:blast_protection").saturating_mul(2),
            DamageKind::Fireball => level("minecraft:fire_protection")
                .saturating_add(level("minecraft:projectile_protection"))
                .saturating_mul(2),
            DamageKind::Projectile => level("minecraft:projectile_protection").saturating_mul(2),
            _ => 0,
        };
        protection = protection.saturating_add(specialized);
    }
    mc_data::armor::protection_reduced_damage(reduced, protection)
}
/// Size uses vanilla Slime.setSize, shared by MagmaCube; NBT Size is size minus one.
pub fn set_size(state: &mut TickState, slot: EntitySlot, size: u8, heal: bool) {
    let size = size.clamp(1, 127);
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return;
    };
    let Some(bp) = state.registry.blueprint(type_id) else {
        return;
    };
    if bp.capabilities.size_profile == crate::blueprint::SizeProfile::None {
        return;
    }
    let magma = bp.capabilities.size_profile == crate::blueprint::SizeProfile::Armored;
    let (body, row) = state.entities.parts_mut(slot);
    row.loot.slime_size = Some(size);
    for (name, base) in [
        ("max_health", f64::from(size).powi(2)),
        (
            "movement_speed",
            f64::from(0.2_f32 + 0.1_f32 * f32::from(size)),
        ),
        ("attack_damage", f64::from(size)),
    ] {
        set_base(&mut row.loot, name, base);
    }
    if magma {
        set_base(&mut row.loot, "armor", f64::from(size) * 3.0);
    }
    if heal {
        row.health = f32::from(size).powi(2);
    }
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    appearance.health = row.health;
    state.appearance_changed.push(body.id);
    restore(state, slot);
}
fn set_base(state: &mut EntityLootState, name: &str, base: f64) {
    if let Some(attribute) = state
        .attributes
        .iter_mut()
        .find(|entry| entry.id.trim_start_matches("minecraft:") == name)
    {
        attribute.base = base;
    } else {
        state.attributes.push(EntityAttribute {
            id: format!("minecraft:{name}"),
            base,
            modifiers: Vec::new(),
        });
    }
}
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return;
    };
    let Some(bp) = state.registry.blueprint(type_id) else {
        return;
    };
    let size_profile = bp.capabilities.size_profile;
    let (armor, knockback_resistance) = (bp.params.armor, bp.params.knockback_resistance);
    let width = bp.half_width;
    let height = bp.height;
    let speed = f64::from(bp.params.speed) / 10.0;
    let damage = f64::from(bp.params.attack_damage);
    let sight = f64::from(bp.sensors.sight_range);
    if size_profile != crate::blueprint::SizeProfile::None {
        let size = state.entities.row(slot).loot.slime_size.unwrap_or(1);
        state.entities.set_half_width(slot, width * f64::from(size));
    }
    let (body, row) = state.entities.parts_mut(slot);
    if row.appearance.body_armor.as_ref() != row.loot.body_armor.as_deref()
        || row.appearance.saddle.as_ref() != row.loot.saddle.as_deref()
    {
        let appearance = std::sync::Arc::make_mut(&mut row.appearance);
        appearance.body_armor = row.loot.body_armor.as_deref().cloned();
        appearance.saddle = row.loot.saddle.as_deref().cloned();
    }
    row.loot.base_armor = armor;
    row.loot.base_knockback_resistance = knockback_resistance;
    if size_profile != crate::blueprint::SizeProfile::None {
        let size = row.loot.slime_size.unwrap_or(1);
        row.loot.slime_size = Some(size);
        body.height = height * f64::from(size);
        let appearance = std::sync::Arc::make_mut(&mut row.appearance);
        // 26.3 size is index 18; a slime saved before the bump still carries
        // its size as a VarInt at 16, where 26.3 expects AgeableMob's baby
        // Boolean, so that stale entry is dropped too.
        appearance.extra.retain(|(index, value)| {
            *index != 18 && !(*index == 16 && matches!(value, EntityDataValue::VarInt(_)))
        });
        appearance
            .extra
            .push((18, EntityDataValue::VarInt(i32::from(size))));
    }
    let rules = state.registry.item_rules();
    row.speed = (effective_attribute(&rules, row, "movement_speed", speed) * 10.0) as f32;
    row.attack_damage = effective_attribute(&rules, row, "attack_damage", damage) as f32;
    if size_profile == crate::blueprint::SizeProfile::Armored {
        row.attack_damage += 2.0;
    }
    if size_profile == crate::blueprint::SizeProfile::Slime && row.loot.slime_size == Some(1) {
        row.attack_damage = 0.0;
    }
    row.sight_range = effective_attribute(&rules, row, "follow_range", sight) as f32;
    if let Some(credit) = &row.loot.player_credit {
        row.last_player_hit = Some((EntityId(credit.id), credit.tick));
    }
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot) {
    restore(state, slot);
    if let EntityKind::Mob { type_id } = state.entities.body(slot).kind
        && state
            .registry
            .blueprint(type_id)
            .is_some_and(|bp| bp.capabilities.size_profile != crate::blueprint::SizeProfile::None)
    {
        let seed = state.entities.row(slot).uuid.as_u128() as u64;
        set_size(state, slot, 1 << (mix(seed) % 3), true);
    }
}
/// Called at the alive -> dead transition only, and guarded durably against re-entry.
pub(crate) fn commit_death(
    state: &mut TickState,
    slot: EntitySlot,
    cause: EntityDeathCause,
    spawned: &mut Vec<EntityId>,
) {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if body.alive || row.loot.death_handled {
        return;
    }
    if crate::vehicles::death(state, slot, spawned) {
        return;
    }
    if crate::armor_stand::death(state, slot, &cause, spawned) {
        return;
    }
    if crate::item_frame::death(state, slot, spawned) {
        return;
    }
    if crate::painting::death(state, slot, spawned) {
        return;
    }
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let registry = std::sync::Arc::clone(&state.registry);
    let Some(bp) = registry.blueprint(type_id) else {
        return;
    };
    let name = bp.name.as_str();
    let table = bp.loot_table.clone();
    let wool = bp.wool.then(|| {
        row.appearance
            .extra
            .iter()
            .find_map(|(index, value)| {
                if *index == crate::blueprint::SHEEP_WOOL_DATA_INDEX {
                    if let EntityDataValue::Byte(value) = value {
                        Some(*value as u8)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap_or(0)
    });
    let size = row.loot.slime_size.unwrap_or(1);
    let seed = body.id.0 as u64 ^ state.tick.rotate_left(23);
    let pos = state.entities.pos(slot);
    let mut entity = EntityLootEntity::new(id(name));
    entity.flags.is_baby = row.appearance.baby;
    entity.flags.is_on_fire = row.appearance.shared_flags & 1 != 0;
    entity.slime_size = row.loot.slime_size.map(i32::from);
    let mut equipment = equipment(row);
    if row.allay.is_some() {
        equipment[0] = None;
    }
    let equipment = equipment.map(|stack| stack.cloned());
    let chances: [f32; 8] = std::array::from_fn(|index| {
        if index < 6 {
            row.loot.drop_chances[index]
        } else {
            row.loot.extra_drop_chances[index - 6]
        }
    });
    state.entities.row_mut(slot).loot.death_handled = true;
    // Allay.dropEquipment releases fetched cargo unconditionally; only the held
    // main-hand stack observes PREVENT_EQUIPMENT_DROP (vanishing curse).
    let allay_cargo = state.entities.row_mut(slot).allay.as_mut().map(|value| {
        [
            std::mem::take(&mut value.inventory),
            std::mem::take(&mut value.held),
        ]
    });
    if let Some([inventory, held]) = allay_cargo {
        if !inventory.is_empty() {
            spawned.push(crate::tick::spawn_item_entity(state, pos, inventory));
        }
        if !held.is_empty()
            && !held
                .enchantments
                .iter()
                .any(|entry| entry.level > 0 && entry.id.as_str() == "minecraft:vanishing_curse")
        {
            spawned.push(crate::tick::spawn_item_entity(state, pos, held));
        }
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] = None;
    }
    // Villager carry (fetched build materials, mined/lumber yield) is taken once
    // under the death guard; Villager.dropEquipment-style loss obeys mob_drops.
    let carry = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .map(|villager| std::mem::take(&mut villager.inventory));
    if let Some(carry) = carry
        && crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_DROPS)
    {
        for stack in carry
            .into_iter()
            .flatten()
            .filter(|stack| !stack.is_empty())
        {
            spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
        }
    }
    if crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_DROPS) {
        if let Some(drop) = crate::special_mobs::death_drop(state, slot) {
            spawned.push(drop);
        }
        let player_kill = cause.attributed_player().is_some();
        let looting = cause.source_attacker().map_or(0, |entity| {
            entity.active_enchantments.level(&id("minecraft:looting"))
        });
        if (!entity.flags.is_baby || bp.category == crate::blueprint::MobCategory::Monster)
            && let Some(table) = table.as_ref()
        {
            for stack in
                crate::mob_loot::mob_drops(&state.registry, entity, table, pos, seed, wool, cause)
            {
                spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
            }
        }
        for (index, stack) in equipment.into_iter().enumerate() {
            let Some(mut stack) = stack.filter(|stack| !stack.is_empty()) else {
                continue;
            };
            if stack
                .enchantments
                .iter()
                .any(|entry| entry.level > 0 && entry.id.as_str() == "minecraft:vanishing_curse")
            {
                continue;
            }
            let roll = mix(seed ^ index as u64) as f64 / u64::MAX as f64;
            if chances[index] > 1.0
                || player_kill && roll < f64::from(chances[index] + looting as f32 * 0.01)
            {
                if chances[index] <= 1.0
                    && let Some(maximum) = state
                        .registry
                        .item_rules()
                        .facts
                        .facts_for_stack(&stack, &state.registry.items)
                        .and_then(|facts| facts.max_damage)
                {
                    let inner = mix(seed ^ index as u64 ^ 0x51)
                        % u64::from(maximum.saturating_sub(3).max(1));
                    let outer = mix(seed ^ index as u64 ^ 0x92) % (1 + inner);
                    stack.damage = Some(maximum.saturating_sub(outer as u32) as i32);
                }
                spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
            }
        }
    }
    if registry
        .blueprint(type_id)
        .is_some_and(|bp| bp.capabilities.splits_on_death)
        && size > 1
    {
        let count = 2 + mix(seed) % 3;
        let offset = state.entities.body(slot).half_width;
        let parent = state.entities.row(slot);
        let persistent = parent.persistent;
        let custom_name = parent.appearance.custom_name.clone();
        for index in 0..count {
            let child = state.spawn_mob(
                type_id,
                [
                    pos[0] + (index % 2) as f64 * offset - offset / 2.0,
                    pos[1] + 0.5,
                    pos[2] + (index / 2) as f64 * offset - offset / 2.0,
                ],
                1.0,
            );
            let child_slot = state.entities.slot_of(child).expect("new split child");
            set_size(state, child_slot, size / 2, true);
            let row = state.entities.row_mut(child_slot);
            row.persistent = persistent;
            std::sync::Arc::make_mut(&mut row.appearance)
                .custom_name
                .clone_from(&custom_name);
            spawned.push(child);
        }
    }
}
fn mix(mut seed: u64) -> u64 {
    seed ^= seed >> 30;
    seed = seed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    seed ^= seed >> 27;
    seed = seed.wrapping_mul(0x94d0_49bb_1331_11eb);
    seed ^ (seed >> 31)
}

/// 26.1.2 attribute and size NBT, plus native-only death/credit replay state.
pub(crate) fn to_fields(state: &EntityLootState) -> Vec<(String, mc_nbt::Tag)> {
    use mc_nbt::{ListTag, Tag, tag_type};
    let attributes = state
        .attributes
        .iter()
        .map(|attribute| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(attribute.id.clone())),
                ("base".into(), Tag::Double(attribute.base)),
                (
                    "modifiers".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: attribute
                            .modifiers
                            .iter()
                            .map(|modifier| {
                                Tag::Compound(vec![
                                    ("id".into(), Tag::String(modifier.id.clone())),
                                    ("amount".into(), Tag::Double(modifier.amount)),
                                    ("operation".into(), Tag::String(modifier.operation.clone())),
                                ])
                            })
                            .collect(),
                    }),
                ),
            ])
        })
        .collect();
    let mut fields = vec![
        (
            "CanPickUpLoot".into(),
            Tag::Byte(i8::from(state.can_pick_up_loot)),
        ),
        (
            "attributes".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: attributes,
            }),
        ),
        (
            "solaris:entity_loot".into(),
            Tag::String(serde_json::to_string(state).expect("finite native loot state")),
        ),
        (
            "drop_chances".into(),
            Tag::Compound(
                EQUIPMENT_SLOTS
                    .into_iter()
                    .chain(["body", "saddle"])
                    .zip(
                        state
                            .drop_chances
                            .into_iter()
                            .chain(state.extra_drop_chances),
                    )
                    .map(|(slot, chance)| (slot.into(), Tag::Float(chance)))
                    .collect(),
            ),
        ),
    ];
    if let Some(size) = state.slime_size {
        fields.push(("Size".into(), Tag::Int(i32::from(size) - 1)));
    }
    fields
}
const EQUIPMENT_SLOTS: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];
pub(crate) fn from_fields(
    fields: &[(String, mc_nbt::Tag)],
) -> Result<EntityLootState, crate::config::CoreError> {
    use mc_nbt::Tag;
    let find = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let mut state = if let Some(Tag::String(json)) = find("solaris:entity_loot") {
        serde_json::from_str(json)
            .map_err(|error| crate::config::CoreError::Storage(format!("entity loot: {error}")))?
    } else {
        EntityLootState::default()
    };
    if let Some(tag) = find("CanPickUpLoot") {
        let Tag::Byte(value @ (0 | 1)) = tag else {
            return Err(crate::config::CoreError::Storage(
                "CanPickUpLoot: expected boolean byte".into(),
            ));
        };
        state.can_pick_up_loot = *value != 0;
    }
    if let Some(Tag::Int(size)) = find("Size") {
        state.slime_size = Some(size.saturating_add(1).clamp(1, 127) as u8);
    }
    if let Some(Tag::List(list)) = find("attributes") {
        let mut attributes = Vec::new();
        for entry in &list.elements {
            let Tag::Compound(fields) = entry else {
                continue;
            };
            let field = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, tag)| tag)
            };
            let (Some(Tag::String(id)), Some(Tag::Double(base))) = (field("id"), field("base"))
            else {
                continue;
            };
            let mut modifiers = Vec::new();
            if let Some(Tag::List(list)) = field("modifiers") {
                for entry in &list.elements {
                    let Tag::Compound(fields) = entry else {
                        continue;
                    };
                    let field = |name: &str| {
                        fields
                            .iter()
                            .find(|(key, _)| key == name)
                            .map(|(_, tag)| tag)
                    };
                    if let (
                        Some(Tag::String(id)),
                        Some(Tag::Double(amount)),
                        Some(Tag::String(operation)),
                    ) = (field("id"), field("amount"), field("operation"))
                    {
                        modifiers.push(EntityAttributeModifier {
                            id: id.clone(),
                            amount: *amount,
                            operation: operation.clone(),
                        });
                    }
                }
            }
            attributes.push(EntityAttribute {
                id: id.clone(),
                base: *base,
                modifiers,
            });
        }
        state.attributes = attributes;
    }
    if let Some(Tag::Compound(fields)) = find("drop_chances") {
        for (index, slot) in EQUIPMENT_SLOTS.into_iter().enumerate() {
            if let Some((_, Tag::Float(chance))) = fields.iter().find(|(key, _)| key == slot) {
                state.drop_chances[index] = *chance;
            }
        }
        for (index, slot) in ["body", "saddle"].into_iter().enumerate() {
            if let Some((_, Tag::Float(chance))) = fields.iter().find(|(key, _)| key == slot) {
                state.extra_drop_chances[index] = *chance;
            }
        }
    }
    if state.attributes.iter().any(|attribute| {
        !attribute.base.is_finite()
            || attribute.modifiers.iter().any(|modifier| {
                !modifier.amount.is_finite()
                    || !matches!(
                        modifier.operation.as_str(),
                        "add_value" | "add_multiplied_base" | "add_multiplied_total"
                    )
            })
    }) || state
        .drop_chances
        .iter()
        .chain(state.extra_drop_chances.iter())
        .any(|chance| !chance.is_finite() || *chance < 0.0)
    {
        return Err(crate::config::CoreError::Storage(
            "invalid entity attributes/drop chances".into(),
        ));
    }
    Ok(state)
}
pub(crate) fn player_attributes_from_opaque(bytes: &[u8]) -> Result<Vec<EntityAttribute>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut cursor = std::io::Cursor::new(bytes);
    let (_, tag) = mc_nbt::read_named(&mut cursor).map_err(|error| error.to_string())?;
    if cursor.position() != bytes.len() as u64 {
        return Err("trailing player NBT".into());
    }
    let mc_nbt::Tag::Compound(fields) = tag else {
        return Err("player fields must be a compound".into());
    };
    from_fields(&fields)
        .map(|state| state.attributes)
        .map_err(|error| error.to_string())
}

pub(crate) fn player_attributes_to_opaque(
    bytes: &[u8],
    attributes: &[EntityAttribute],
) -> Result<Vec<u8>, String> {
    if attributes.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut fields = if bytes.is_empty() {
        Vec::new()
    } else {
        let mut cursor = std::io::Cursor::new(bytes);
        let (_, tag) = mc_nbt::read_named(&mut cursor).map_err(|error| error.to_string())?;
        if cursor.position() != bytes.len() as u64 {
            return Err("trailing player NBT".into());
        }
        let mc_nbt::Tag::Compound(fields) = tag else {
            return Err("player fields must be a compound".into());
        };
        fields
    };
    fields.retain(|(name, _)| name != "attributes");
    let state = EntityLootState {
        attributes: attributes.to_vec(),
        ..Default::default()
    };
    let (_, value) = to_fields(&state)
        .into_iter()
        .find(|(name, _)| name == "attributes")
        .expect("attribute field");
    fields.push(("attributes".into(), value));
    let mut output = Vec::new();
    mc_nbt::write_named(&mut output, "", &mc_nbt::Tag::Compound(fields))
        .map_err(|error| error.to_string())?;
    Ok(output)
}

pub(crate) fn equipment_from_fields(
    items: &mc_data::items::ItemRegistry,
    fields: &[(String, mc_nbt::Tag)],
) -> Result<[Option<ItemStack>; 6], crate::config::CoreError> {
    let mut equipment: [Option<ItemStack>; 6] = Default::default();
    if let Some((_, mc_nbt::Tag::Compound(slots))) =
        fields.iter().find(|(key, _)| key == "equipment")
    {
        for (index, slot) in EQUIPMENT_SLOTS.into_iter().enumerate() {
            if let Some((_, mc_nbt::Tag::Compound(fields))) =
                slots.iter().find(|(key, _)| key == slot)
            {
                equipment[index] = mc_data::item_nbt::stack_from_fields(items, fields)
                    .map_err(|error| crate::config::CoreError::Storage(error.to_string()))?;
            }
        }
    }
    Ok(equipment)
}
pub(crate) fn equipment_to_fields(
    items: &mc_data::items::ItemRegistry,
    equipment: &[Option<ItemStack>; 6],
    state: &EntityLootState,
) -> Result<Vec<(String, mc_nbt::Tag)>, crate::config::CoreError> {
    let mut slots = Vec::new();
    for (name, stack) in EQUIPMENT_SLOTS.into_iter().chain(["body", "saddle"]).zip(
        equipment
            .iter()
            .map(Option::as_ref)
            .chain([state.body_armor.as_deref(), state.saddle.as_deref()]),
    ) {
        if let Some(stack) = stack.filter(|stack| !stack.is_empty()) {
            slots.push((
                name.into(),
                mc_nbt::Tag::Compound(
                    mc_data::item_nbt::stack_to_fields(items, stack)
                        .map_err(|error| crate::config::CoreError::Storage(error.to_string()))?,
                ),
            ));
        }
    }
    Ok(vec![("equipment".into(), mc_nbt::Tag::Compound(slots))])
}

pub(crate) fn extra_equipment_from_fields(
    items: &mc_data::items::ItemRegistry,
    fields: &[(String, mc_nbt::Tag)],
    state: &mut EntityLootState,
) -> Result<(), crate::config::CoreError> {
    if let Some((_, mc_nbt::Tag::Compound(slots))) =
        fields.iter().find(|(key, _)| key == "equipment")
    {
        for (name, item) in [
            ("body", &mut state.body_armor),
            ("saddle", &mut state.saddle),
        ] {
            if let Some((_, mc_nbt::Tag::Compound(fields))) =
                slots.iter().find(|(key, _)| key == name)
            {
                *item = mc_data::item_nbt::stack_from_fields(items, fields)
                    .map_err(|error| crate::config::CoreError::Storage(error.to_string()))?
                    .map(Box::new);
            }
        }
    }
    Ok(())
}

pub(crate) fn wear_equipment(
    rules: &ItemRules<'_>,
    row: &mut ColdRow,
    amount: f32,
    kind: DamageKind,
) {
    wear_equipment_impl(rules, row, amount, kind, false);
}

pub(crate) fn wear_equipment_after_physical(
    rules: &ItemRules<'_>,
    row: &mut ColdRow,
    amount: f32,
    kind: DamageKind,
) {
    wear_equipment_impl(rules, row, amount, kind, true);
}

fn wear_equipment_impl(
    rules: &ItemRules<'_>,
    row: &mut ColdRow,
    amount: f32,
    kind: DamageKind,
    physical: bool,
) {
    let wear = crate::survival::armor_wear(amount, kind);
    if wear == 0 {
        return;
    }
    let mut seed = (row.uuid.as_u128() as u32 ^ row.age) as i32;
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    for (stack, slot) in appearance.equipment[2..]
        .iter_mut()
        .zip(["feet", "legs", "chest", "head"])
    {
        let Some(item) = stack.as_mut().filter(|item| !item.is_empty()) else {
            continue;
        };
        let Some(maximum) = rules
            .facts
            .facts_for_stack(item, rules.items)
            .filter(|facts| {
                !physical
                    || facts
                        .physical_profile
                        .as_ref()
                        .is_none_or(|profile| profile.armor.is_none())
            })
            .filter(|facts| facts.armor.as_ref().is_some_and(|armor| armor.slot == slot))
            .and_then(|facts| facts.max_damage)
        else {
            continue;
        };
        let wear = crate::enchantments::durability_damage(item, wear, true, &mut seed);
        if wear == 0 {
            continue;
        }
        let damage = item.damage.unwrap_or(0).saturating_add(wear);
        if u32::try_from(damage).is_ok_and(|damage| damage >= maximum) {
            *stack = None;
        } else {
            item.damage = Some(damage);
        }
    }
}
