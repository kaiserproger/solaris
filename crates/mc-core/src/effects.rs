//! Living-entity status effects: vanilla 26.1.2 identities and pure transitions.
use crate::survival::{DamageKind, Vitals};
pub use mc_data::mob_effects_26_1_2::MobEffect as EffectId;
use mc_physics::mth;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveEffect {
    pub id: EffectId,
    pub amplifier: u8,
    /// -1 is infinite, matching MobEffectInstance.INFINITE_DURATION.
    pub duration: i32,
    pub ambient: bool,
    pub show_particles: bool,
    pub show_icon: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Box<ActiveEffect>>,
}
impl ActiveEffect {
    pub fn new(id: EffectId, duration: i32, amplifier: u8) -> Self {
        Self {
            id,
            duration,
            amplifier,
            ambient: false,
            show_particles: true,
            show_icon: true,
            hidden: None,
        }
    }
    fn longer_than(&self, other: &Self) -> bool {
        other.duration != -1 && (self.duration == -1 || self.duration > other.duration)
    }
    fn update(&mut self, incoming: Self) -> bool {
        if incoming.amplifier > self.amplifier {
            let previous_hidden = self.hidden.take();
            let hidden = if self.longer_than(&incoming) {
                Some(Box::new(Self {
                    hidden: previous_hidden,
                    ..self.clone()
                }))
            } else {
                previous_hidden
            };
            *self = incoming;
            self.hidden = hidden;
            return true;
        }
        let ambient = incoming.ambient;
        let show_particles = incoming.show_particles;
        let show_icon = incoming.show_icon;
        let mut changed = false;
        if incoming.longer_than(self) {
            if incoming.amplifier == self.amplifier {
                self.duration = incoming.duration;
                changed = true;
            } else if let Some(hidden) = &mut self.hidden {
                hidden.update(incoming);
            } else {
                self.hidden = Some(Box::new(incoming));
            }
        }
        if self.ambient && !ambient || changed {
            self.ambient = ambient;
            changed = true;
        }
        changed |= self.show_particles != show_particles || self.show_icon != show_icon;
        self.show_particles = show_particles;
        self.show_icon = show_icon;
        changed
    }
    fn decrement(&mut self) {
        if self.duration > 0 {
            self.duration -= 1;
        }
        if let Some(hidden) = &mut self.hidden {
            hidden.decrement();
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActiveEffects {
    pub entries: Vec<ActiveEffect>,
    pub absorption: f32,
}
#[derive(Debug, Default)]
pub struct EffectTick {
    pub damage: Vec<(DamageKind, f32)>,
    pub removed: Vec<EffectId>,
    pub updated: Vec<ActiveEffect>,
}
impl ActiveEffects {
    pub fn get(&self, id: EffectId) -> Option<&ActiveEffect> {
        self.entries.iter().find(|effect| effect.id == id)
    }
    pub fn level(&self, id: EffectId) -> f32 {
        self.get(id)
            .map_or(0.0, |effect| f32::from(effect.amplifier) + 1.0)
    }
    pub fn has(&self, id: EffectId) -> bool {
        self.get(id).is_some()
    }
    pub fn add(&mut self, effect: ActiveEffect) -> bool {
        if effect.duration == 0 || effect.duration < -1 {
            return false;
        }
        let old_absorption = self.absorption;
        let replenish = if effect.id == EffectId::Absorption {
            4.0 * (f32::from(effect.amplifier) + 1.0)
        } else {
            0.0
        };
        let changed = if let Some(old) = self.entries.iter_mut().find(|old| old.id == effect.id) {
            old.update(effect)
        } else {
            self.entries.push(effect);
            true
        };
        let max_absorption = self.level(EffectId::Absorption) * 4.0;
        self.absorption = self.absorption.max(replenish).min(max_absorption);
        changed || self.absorption != old_absorption
    }
    pub fn clear(&mut self, id: Option<EffectId>) -> Vec<EffectId> {
        let mut removed = Vec::new();
        self.entries.retain(|effect| {
            let remove = id.is_none_or(|id| id == effect.id);
            if remove {
                removed.push(effect.id);
            }
            !remove
        });
        if !self.has(EffectId::Absorption) {
            self.absorption = 0.0;
        }
        removed
    }
    /// Applied after armor/enchantment mitigation, before absorption.
    pub fn reduce_damage(&mut self, kind: DamageKind, amount: f32) -> f32 {
        if kind.bypasses_invulnerability() {
            return amount;
        }
        if self.has(EffectId::FireResistance)
            && matches!(
                kind,
                DamageKind::InFire
                    | DamageKind::OnFire
                    | DamageKind::Lava
                    | DamageKind::Fireball
                    | DamageKind::Lightning
                    | DamageKind::HotFloor
                    | DamageKind::Campfire
            )
        {
            return 0.0;
        }
        let amount = if kind == DamageKind::Starvation {
            amount
        } else {
            amount * (1.0 - self.level(EffectId::Resistance) * 0.2).max(0.0)
        };
        let absorbed = self.absorption.min(amount);
        self.absorption -= absorbed;
        amount - absorbed
    }
    pub fn dig_multiplier(&self) -> f32 {
        let haste = 1.0
            + 0.2
                * self
                    .level(EffectId::Haste)
                    .max(self.level(EffectId::ConduitPower));
        let fatigue =
            self.get(EffectId::MiningFatigue)
                .map_or(1.0, |effect| match effect.amplifier {
                    0 => 0.3,
                    1 => 0.09,
                    2 => 0.0027,
                    _ => 0.00081,
                });
        haste * fatigue
    }
    /// LivingEntity.travelInWater: Grace replaces horizontal drag 0.8 with 0.96.
    /// The player's client integrates motion from the native MobEffect packet;
    /// it is not a global movement-speed attribute or a second server impulse.
    pub fn swim_speed_multiplier(&self) -> f64 {
        if self.has(EffectId::DolphinsGrace) {
            5.0
        } else {
            1.0
        }
    }
    pub fn melee_bonus(&self) -> f32 {
        3.0 * self.level(EffectId::Strength) - 4.0 * self.level(EffectId::Weakness)
    }
    pub fn tick(&mut self, vitals: &mut Vitals, tick: u64, max_health: f32) -> EffectTick {
        let mut out = EffectTick::default();
        for effect in &mut self.entries {
            let time = if effect.duration == -1 {
                tick
            } else {
                effect.duration as u64
            };
            let interval = |base: u32| {
                u64::from(
                    base.checked_shr(u32::from(effect.amplifier))
                        .unwrap_or(0)
                        .max(1),
                )
            };
            match effect.id {
                EffectId::Regeneration
                    if time.is_multiple_of(interval(50)) && vitals.health < max_health =>
                {
                    vitals.health = (vitals.health + 1.0).min(max_health);
                }
                EffectId::Poison if time.is_multiple_of(interval(25)) && vitals.health > 1.0 => out
                    .damage
                    .push((DamageKind::Poison, (vitals.health - 1.0).min(1.0))),
                EffectId::Wither if time.is_multiple_of(interval(40)) => {
                    out.damage.push((DamageKind::Wither, 1.0))
                }
                EffectId::InstantHealth if vitals.health < max_health => {
                    vitals.health = (vitals.health
                        + 4.0 * 2.0_f32.powi(i32::from(effect.amplifier).min(30)))
                    .min(max_health);
                }
                EffectId::InstantDamage => out.damage.push((
                    DamageKind::Magic,
                    6.0 * 2.0_f32.powi(i32::from(effect.amplifier).min(30)),
                )),
                EffectId::Saturation => {
                    let food = i32::from(effect.amplifier) + 1;
                    vitals.food = (vitals.food + food).min(20);
                    vitals.saturation =
                        (vitals.saturation + (food * 2) as f32).min(vitals.food as f32);
                }
                EffectId::Hunger => {
                    vitals.add_exhaustion(0.005 * (f32::from(effect.amplifier) + 1.0))
                }
                EffectId::Absorption if self.absorption <= 0.0 => effect.duration = 0,
                _ => {}
            }
            effect.decrement();
            if effect.duration == 0 {
                if let Some(hidden) = effect.hidden.take() {
                    *effect = *hidden;
                    if effect.duration != 0 {
                        out.updated.push(effect.clone());
                    }
                }
                if effect.duration == 0 {
                    out.removed.push(effect.id);
                }
            }
        }
        self.entries.retain(|effect| effect.duration != 0);
        self.absorption = self.absorption.min(self.level(EffectId::Absorption) * 4.0);
        out
    }
    pub fn attribute_modifiers(&self, attributes: &mut [crate::boundary::PlayerAttribute]) {
        for effect in &self.entries {
            let (kind, amount, operation) = match effect.id {
                EffectId::Speed => ("movement_speed", f64::from(0.2_f32), "add_multiplied_total"),
                EffectId::Slowness => (
                    "movement_speed",
                    f64::from(-0.15_f32),
                    "add_multiplied_total",
                ),
                EffectId::Haste => ("attack_speed", f64::from(0.1_f32), "add_multiplied_total"),
                EffectId::MiningFatigue => {
                    ("attack_speed", f64::from(-0.1_f32), "add_multiplied_total")
                }
                EffectId::Strength => ("attack_damage", 3.0, "add_value"),
                EffectId::Weakness => ("attack_damage", -4.0, "add_value"),
                EffectId::HealthBoost => ("max_health", 4.0, "add_value"),
                _ => continue,
            };
            if let Some(attribute) = attributes
                .iter_mut()
                .find(|attribute| attribute.kind == kind)
            {
                let id = format!("minecraft:effect.{}", effect.id.name());
                attribute.modifiers.retain(|modifier| modifier.id != id);
                attribute
                    .modifiers
                    .push(mc_data::item_components::ItemAttributeModifier {
                        kind: format!("minecraft:{kind}"),
                        id,
                        amount: amount * (f64::from(effect.amplifier) + 1.0),
                        operation: operation.into(),
                        slot: None,
                    });
            }
        }
    }
}

/// Potion defaults from Potions.java; custom stew effects already carry their own durations.
pub fn potion_effects(name: &str) -> Vec<ActiveEffect> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    let (name, long, strong) = if let Some(name) = name.strip_prefix("long_") {
        (name, true, false)
    } else if let Some(name) = name.strip_prefix("strong_") {
        (name, false, true)
    } else {
        (name, false, false)
    };
    if name == "turtle_master" {
        let duration = if long { 800 } else { 400 };
        return vec![
            ActiveEffect::new(EffectId::Slowness, duration, if strong { 5 } else { 3 }),
            ActiveEffect::new(EffectId::Resistance, duration, if strong { 3 } else { 2 }),
        ];
    }
    let (id, normal, extended, potent) = match name {
        "swiftness" => (EffectId::Speed, 3600, 9600, 1800),
        "slowness" => (EffectId::Slowness, 1800, 4800, 400),
        "leaping" => (EffectId::JumpBoost, 3600, 9600, 1800),
        "healing" => (EffectId::InstantHealth, 1, 1, 1),
        "harming" => (EffectId::InstantDamage, 1, 1, 1),
        "poison" => (EffectId::Poison, 900, 1800, 432),
        "regeneration" => (EffectId::Regeneration, 900, 1800, 450),
        "strength" => (EffectId::Strength, 3600, 9600, 1800),
        "weakness" => (EffectId::Weakness, 1800, 4800, 1800),
        "slow_falling" => (EffectId::SlowFalling, 1800, 4800, 1800),
        "luck" => (EffectId::Luck, 6000, 6000, 6000),
        "night_vision" | "invisibility" | "fire_resistance" | "water_breathing"
        | "wind_charged" | "weaving" | "oozing" | "infested" => (
            EffectId::from_name(name).expect("known potion effect"),
            3600,
            9600,
            1800,
        ),
        _ => return Vec::new(),
    };
    vec![ActiveEffect::new(
        id,
        if long {
            extended
        } else if strong {
            potent
        } else {
            normal
        },
        if strong {
            if id == EffectId::Slowness { 3 } else { 1 }
        } else {
            0
        },
    )]
}

impl crate::player::PlayerState {
    pub fn give_effect(&mut self, effect: ActiveEffect) -> Vec<crate::boundary::PlayerChange> {
        let blend = !self.active_effects.has(effect.id);
        if !self.active_effects.add(effect.clone()) {
            return Vec::new();
        }
        self.advancement_effects_changed = true;
        self.attribute_equipment = None;
        let effect = self
            .active_effects
            .get(effect.id)
            .expect("added effect")
            .clone();
        vec![
            crate::boundary::PlayerChange::MobEffect { effect, blend },
            crate::boundary::PlayerChange::Absorption {
                amount: self.active_effects.absorption,
            },
        ]
    }
    pub fn clear_effects(&mut self, id: Option<EffectId>) -> Vec<crate::boundary::PlayerChange> {
        let removed = self.active_effects.clear(id);
        if removed.is_empty() {
            return Vec::new();
        }
        self.advancement_effects_changed = true;
        self.attribute_equipment = None;
        let mut changes: Vec<_> = removed
            .into_iter()
            .map(|id| crate::boundary::PlayerChange::RemoveMobEffect { id })
            .collect();
        changes.push(crate::boundary::PlayerChange::Absorption {
            amount: self.active_effects.absorption,
        });
        changes
    }
    pub(crate) fn consume_effects(
        &mut self,
        rules: &crate::inventory::ItemRules<'_>,
        stack: &mc_data::ItemStack,
    ) -> Vec<crate::boundary::PlayerChange> {
        let name = rules
            .items
            .name_of(stack.item_id)
            .map(|id| id.as_str())
            .unwrap_or("");
        if name == "minecraft:milk_bucket" {
            return self.clear_effects(None);
        }
        if name == "minecraft:honey_bottle" {
            return self.clear_effects(Some(EffectId::Poison));
        }
        let mut effects = match name {
            "minecraft:ominous_bottle" => vec![ActiveEffect::new(
                EffectId::BadOmen,
                120_000,
                stack
                    .extended()
                    .ominous_bottle_amplifier
                    .unwrap_or(0)
                    .clamp(0, 4) as u8,
            )],
            // Golden apples, pufferfish, spider eyes, rotten flesh, raw
            // chicken and poisonous potatoes carry vanilla consumable
            // on_consume_effects; the data-driven roll below owns them.
            "minecraft:potion" => stack
                .extended()
                .potion
                .as_ref()
                .map_or_else(Vec::new, |id| potion_effects(id.as_str())),
            _ => Vec::new(),
        };
        if name == "minecraft:potion" {
            fn custom(effect: &mc_data::item_stack::PotionEffect) -> Option<ActiveEffect> {
                let mut active = ActiveEffect::new(
                    EffectId::from_name(effect.id.as_str())?,
                    effect.duration,
                    effect.amplifier,
                );
                active.ambient = effect.ambient;
                active.show_particles = effect.show_particles;
                active.show_icon = effect.show_icon;
                active.hidden = effect.hidden.as_deref().and_then(custom).map(Box::new);
                Some(active)
            }
            effects.extend(
                stack
                    .extended()
                    .potion_custom_effects
                    .iter()
                    .filter_map(custom),
            );
        }
        if name == "minecraft:suspicious_stew" {
            effects.extend(stack.stew_effects.iter().filter_map(|effect| {
                Some(ActiveEffect::new(
                    EffectId::from_name(effect.id.as_str())?,
                    effect.duration,
                    0,
                ))
            }));
        }
        // Registered custom foods (vanilla consumable component): declared
        // consume_effects apply on top of nourishment, not only hard-coded
        // vanilla carrier names. Probability-gated apply_effects roll the
        // player's enchantment seed stream.
        if let Some(facts) = rules.facts.facts_for_stack(stack, rules.items)
            && !facts.consume_effects.is_empty()
        {
            effects.extend(self.roll_consume_effects(&facts.consume_effects));
        }
        effects
            .into_iter()
            .flat_map(|effect| self.give_effect(effect))
            .collect()
    }

    /// Evaluate declared consumable effects: probability-gated apply_effects,
    /// effect removals, full clears, random teleports (overworld-safe), and
    /// sound cues (no-op server-side beyond the existing effect changes).
    fn roll_consume_effects(
        &mut self,
        declared: &[mc_data::food::ConsumeEffect],
    ) -> Vec<ActiveEffect> {
        let mut out = Vec::new();
        for effect in declared {
            match effect {
                mc_data::food::ConsumeEffect::ApplyEffects {
                    effects,
                    probability,
                } => {
                    if *probability >= 1.0
                        || crate::enchantments::random(&mut self.enchantment_seed)
                            < f64::from(*probability)
                    {
                        out.extend(effects.iter().filter_map(|effect| {
                            let mut active = ActiveEffect::new(
                                EffectId::from_name(effect.id.as_str())?,
                                effect.duration,
                                effect.amplifier,
                            );
                            active.ambient = effect.ambient;
                            active.show_particles = effect.show_particles;
                            if let Some(icon) = effect.show_icon {
                                active.show_icon = icon;
                            }
                            Some(active)
                        }));
                    }
                }
                mc_data::food::ConsumeEffect::RemoveEffects { effects } => {
                    for id in effects.as_slice() {
                        if let Some(id) = EffectId::from_name(id.as_str()) {
                            out.retain(|active: &ActiveEffect| active.id != id);
                            let removed = self.clear_effects(Some(id));
                            let _ = removed;
                        }
                    }
                }
                mc_data::food::ConsumeEffect::ClearAllEffects => {
                    let _ = self.clear_effects(None);
                }
                mc_data::food::ConsumeEffect::TeleportRandomly { .. } => {
                    // Player teleport routing lives in the tick owner, not
                    // the effect set: record no ActiveEffect here.
                }
                mc_data::food::ConsumeEffect::PlaySound { .. } => {}
            }
        }
        out
    }
}

pub(crate) fn parse_command(
    args: &[&str],
) -> Result<crate::commands::Command, crate::commands::CommandError> {
    use crate::commands::{Command, CommandError, Target, target_of};
    let usage = CommandError::Usage(
        "effect give <targets> <effect> [<seconds>|infinite] [<amplifier>] [<hideParticles>] | effect clear [<targets>] [<effect>]",
    );
    let id = |name: &str| {
        EffectId::from_name(name)
            .ok_or_else(|| CommandError::Message(format!("Unknown effect '{name}'")))
    };
    match args {
        ["clear"] => Ok(Command::EffectClear {
            target: Target::Executor,
            effect: None,
        }),
        ["clear", target] => Ok(Command::EffectClear {
            target: target_of(target)?,
            effect: None,
        }),
        ["clear", target, name] => Ok(Command::EffectClear {
            target: target_of(target)?,
            effect: Some(id(name)?),
        }),
        ["give", target, name, rest @ ..] if rest.len() <= 3 => {
            let id = id(name)?;
            let duration = match rest.first() {
                Some(&"infinite") => -1,
                Some(seconds) => {
                    let seconds = seconds
                        .parse::<i32>()
                        .ok()
                        .filter(|seconds| (1..=1_000_000).contains(seconds))
                        .ok_or_else(|| usage.clone())?;
                    if id.instant() { seconds } else { seconds * 20 }
                }
                None => {
                    if id.instant() {
                        1
                    } else {
                        600
                    }
                }
            };
            let amplifier = rest
                .get(1)
                .map(|value| value.parse::<u8>())
                .transpose()
                .map_err(|_| usage.clone())?
                .unwrap_or(0);
            let hide = match rest.get(2) {
                None | Some(&"false") => false,
                Some(&"true") => true,
                _ => return Err(usage),
            };
            let mut effect = ActiveEffect::new(id, duration, amplifier);
            effect.show_particles = !hide;
            effect.show_icon = !hide;
            Ok(Command::EffectGive {
                target: target_of(target)?,
                effect,
            })
        }
        _ => Err(usage),
    }
}

fn advancement_effect_snapshot(effects: &ActiveEffects) -> serde_json::Value {
    let effects = effects
        .entries
        .iter()
        .map(|effect| {
            (
                mc_data::mob_effects_26_1_2::name_for_id(effect.id as u32)
                    .expect("registered effect")
                    .to_owned(),
                serde_json::json!({
                    "amplifier": effect.amplifier,
                    "duration": effect.duration,
                    "ambient": effect.ambient,
                    "visible": effect.show_particles,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    serde_json::Value::Object(effects)
}

/// Project the authoritative effects into the existing appearance fan-out.
pub(crate) fn sync_appearance(
    state: &mut crate::tick::TickState,
    entity: crate::boundary::EntityId,
) {
    use crate::boundary::EffectParticle;
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    let position = state.entities.pos(slot);
    let event = state.players.get_mut(&entity).and_then(|player| {
        if !std::mem::take(&mut player.advancement_effects_changed) {
            return None;
        }
        if player.active_effects.has(EffectId::Levitation) {
            player
                .advancement_levitation_origin
                .get_or_insert((position, state.tick));
        } else {
            player.advancement_levitation_origin = None;
        }
        Some(serde_json::json!({
            "effects": advancement_effect_snapshot(&player.active_effects),
        }))
    });
    if let Some(event) = event {
        crate::advancements::trigger(state, entity, "minecraft:effects_changed", event);
    }
    let effects = state
        .players
        .get(&entity)
        .map(|player| &player.active_effects)
        .unwrap_or(&state.entities.row(slot).active_effects);
    let flags = (if effects.has(EffectId::Invisibility) {
        0x20
    } else {
        0
    }) | (if effects.has(EffectId::Glowing) {
        0x40
    } else {
        0
    });
    let ambient = !effects.entries.is_empty()
        && effects
            .entries
            .iter()
            .filter(|effect| effect.show_particles)
            .all(|effect| effect.ambient);
    let absorption = effects.absorption;
    let particles = effects
        .entries
        .iter()
        .filter(|effect| effect.show_particles)
        .map(|effect| match effect.id {
            EffectId::TrialOmen => EffectParticle::TrialOmen,
            EffectId::RaidOmen => EffectParticle::RaidOmen,
            id => EffectParticle::EntityEffect {
                argb: id.color() | ((if effect.ambient { 38_u32 } else { 255 }) << 24) as i32,
            },
        });
    let old = &state.entities.row(slot).appearance;
    if old.shared_flags & 0x60 == flags
        && old.effects_ambient == ambient
        && old.absorption == absorption
        && old.effect_particles.iter().copied().eq(particles.clone())
    {
        return;
    }
    let particles = particles.collect();
    let appearance = std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance.shared_flags = (appearance.shared_flags & !0x60) | flags;
    appearance.effects_ambient = ambient;
    appearance.absorption = absorption;
    appearance.effect_particles = particles;
    state.appearance_changed.push(entity);
}

fn is_living(kind: crate::entities::EntityKind) -> bool {
    match kind {
        crate::entities::EntityKind::Player { .. } => true,
        crate::entities::EntityKind::Item { .. }
        | crate::entities::EntityKind::ExperienceOrb { .. } => false,
        crate::entities::EntityKind::Mob { type_id } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).is_some_and(
                |contract| {
                    contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
                },
            )
        }
    }
}

pub(crate) fn execute_command(
    state: &mut crate::tick::TickState,
    connection: crate::boundary::ConnectionId,
    executor: crate::boundary::EntityId,
    command: &crate::commands::Command,
) -> crate::command_execution::CommandResult {
    use crate::command_execution::CommandResult::{Failed, Success, Unavailable};
    use crate::commands::Command;
    let (target, give, clear) = match command {
        Command::EffectGive { target, effect } => (target, Some(effect), None),
        Command::EffectClear { target, effect } => (target, None, Some(*effect)),
        _ => return Unavailable,
    };
    let targets = crate::command_execution::resolve_target(state, executor, target);
    let mut affected = 0;
    for entity in targets {
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        if !is_living(state.entities.body(slot).kind) {
            continue;
        }
        let changed = if let Some(player) = state.players.get_mut(&entity) {
            let changes = if let Some(effect) = give {
                player.give_effect(effect.clone())
            } else {
                player.clear_effects(clear.flatten())
            };
            let changed = !changes.is_empty();
            let target_connection = player.connection;
            for change in changes {
                state.push_player(target_connection, change);
            }
            changed
        } else if let Some(effect) = give {
            state
                .entities
                .row_mut(slot)
                .active_effects
                .add(effect.clone())
        } else {
            !state
                .entities
                .row_mut(slot)
                .active_effects
                .clear(clear.flatten())
                .is_empty()
        };
        if changed {
            affected += 1;
            state.appearance_changed.push(entity);
            sync_appearance(state, entity);
        }
    }
    state.push_player(
        connection,
        crate::boundary::PlayerChange::Chat {
            message: if affected == 0 {
                "No entities were affected".into()
            } else {
                format!("Updated effects for {affected} entities")
            },
        },
    );
    if affected == 0 {
        Failed
    } else {
        Success(affected)
    }
}

pub(crate) fn run(state: &mut crate::tick::TickState, intents: &mut Vec<crate::intent::Intent>) {
    use crate::boundary::PlayerChange;
    use crate::intent::{Footprint, Intent, IntentKind};
    state.entities.prepare_motion_hints(&state.registry);
    // Effects stage damage; no dense slots move during this pass.
    for index in 0..state.entities.len() as u32 {
        let slot = crate::entities::EntitySlot(index);
        if state.entities.motion_flags(slot)
            & (crate::entities::MOTION_EFFECTS | crate::entities::MOTION_PLAYER)
            == 0
        {
            continue;
        }
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive {
            continue;
        }
        let effects = if matches!(body.kind, crate::entities::EntityKind::Player { .. }) {
            &state.players[&body.id].active_effects
        } else {
            &row.active_effects
        };
        let active = !effects.entries.is_empty()
            || !row.appearance.effect_particles.is_empty()
            || row.appearance.absorption > 0.0
            || row.appearance.shared_flags & 0x60 != 0;
        if !active || !is_living(body.kind) {
            continue;
        }
        let entity = body.id;
        let pos = state.entities.pos(slot);
        if !state.world.is_active(
            (pos[0].floor() as i32).div_euclid(16),
            (pos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let max_health_kind = match state.entities.body(slot).kind {
            crate::entities::EntityKind::Mob { type_id } => Some(type_id),
            _ => None,
        };
        let (out, health) = if let Some(player) = state.players.get_mut(&entity) {
            if player.active_effects.entries.is_empty() {
                sync_appearance(state, entity);
                continue;
            }
            let before = (
                player.vitals.health,
                player.vitals.food,
                player.vitals.saturation,
                player.active_effects.absorption,
            );
            let max_health = player.effective_attribute("max_health").unwrap_or(20.0) as f32;
            let out = player
                .active_effects
                .tick(&mut player.vitals, state.tick, max_health);
            let mut changes = Vec::new();
            if before.0 != player.vitals.health
                || before.1 != player.vitals.food
                || before.2 != player.vitals.saturation
            {
                changes.push(player.health_change());
            }
            if before.3 != player.active_effects.absorption {
                changes.push(PlayerChange::Absorption {
                    amount: player.active_effects.absorption,
                });
            }
            if !out.removed.is_empty() || !out.updated.is_empty() {
                player.attribute_equipment = None;
                player.advancement_effects_changed = true;
            }
            changes.extend(
                out.removed
                    .iter()
                    .map(|id| PlayerChange::RemoveMobEffect { id: *id }),
            );
            changes.extend(out.updated.iter().map(|effect| PlayerChange::MobEffect {
                effect: effect.clone(),
                blend: false,
            }));
            let health = player.vitals.health;
            let connection = player.connection;
            for change in changes {
                state.push_player(connection, change);
            }
            (out, health)
        } else {
            if state.entities.row(slot).active_effects.entries.is_empty() {
                continue;
            }
            // Equipment/max-health attribute evaluation is the expensive part
            // of this branch; mobs without any equipment or attribute rows keep
            // the blueprint default without touching item facts.
            let row = state.entities.row(slot);
            let max_health = match max_health_kind {
                Some(type_id) => {
                    let default = state
                        .registry
                        .blueprint(type_id)
                        .map_or(20.0, |blueprint| blueprint.params.max_health);
                    if row.loot.attributes.is_empty()
                        && row.appearance.equipment.iter().all(Option::is_none)
                        && row.loot.body_armor.is_none()
                        && row.loot.saddle.is_none()
                    {
                        default.clamp(0.0, 1024.0)
                    } else {
                        crate::entity_loot::effective_attribute(
                            &state.registry.item_rules(),
                            row,
                            "max_health",
                            f64::from(default),
                        ) as f32
                    }
                }
                None => 20.0,
            };
            let row = state.entities.row_mut(slot);
            let mut vitals = Vitals::default();
            vitals.health = row.health;
            let out = row.active_effects.tick(&mut vitals, state.tick, max_health);
            row.health = vitals.health;
            state.appearance_changed.push(entity);
            (out, vitals.health)
        };
        state.entities.row_mut(slot).health = health;
        if state.entities.row(slot).appearance.health != health {
            std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).health = health;
            state.appearance_changed.push(entity);
        }
        let effects = state
            .players
            .get(&entity)
            .map(|player| &player.active_effects)
            .unwrap_or(&state.entities.row(slot).active_effects);
        let levitation = effects.level(EffectId::Levitation);
        let slow_falling = effects.has(EffectId::SlowFalling);
        if state.players.contains_key(&entity) && levitation > 0.0 {
            let mut velocity = state.entities.vel(slot);
            velocity[1] += (0.05 * f64::from(levitation) * 20.0 - velocity[1]) * 0.2;
            state.entities.set_vel(slot, velocity);
            let player = state.players.get_mut(&entity).expect("player");
            let position = state.entities.pos(slot);
            let (origin, started) = *player
                .advancement_levitation_origin
                .get_or_insert((position, state.tick));
            let delta: [f64; 3] = std::array::from_fn(|axis| (position[axis] - origin[axis]).abs());
            let distance = serde_json::json!({
                "x": delta[0],
                "y": delta[1],
                "z": delta[2],
                "horizontal": mth::length2(delta[0], delta[2]),
                "absolute": mth::length3(delta[0], delta[1], delta[2]),
            });
            let duration = state.tick.saturating_sub(started);
            crate::advancements::trigger(
                state,
                entity,
                "minecraft:levitation",
                serde_json::json!({
                    "distance": distance,
                    "duration": duration,
                }),
            );
        } else if state.players.contains_key(&entity)
            && slow_falling
            && state.entities.vel(slot)[1] < -2.5
        {
            let mut velocity = state.entities.vel(slot);
            velocity[1] = -2.5;
            state.entities.set_vel(slot, velocity);
        }
        sync_appearance(state, entity);
        for (kind, amount) in out.damage {
            let seq = state.next_seq(entity);
            intents.push(Intent {
                actor: entity,
                seq,
                kind: IntentKind::DamageEntity {
                    target: entity,
                    amount,
                    kind,
                    source: None,
                },
                footprint: Footprint {
                    entity_reads: vec![entity],
                    entity_writes: vec![entity],
                    ..Default::default()
                },
            });
        }
    }
}

pub(crate) fn sync_join(state: &mut crate::tick::TickState, entity: crate::boundary::EntityId) {
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    if player.active_effects.entries.is_empty() && player.active_effects.absorption == 0.0 {
        return;
    }
    let connection = player.connection;
    let mut changes: Vec<_> = player
        .active_effects
        .entries
        .iter()
        .map(|effect| crate::boundary::PlayerChange::MobEffect {
            effect: effect.clone(),
            blend: false,
        })
        .collect();
    changes.push(crate::boundary::PlayerChange::Absorption {
        amount: player.active_effects.absorption,
    });
    for change in changes {
        state.push_player(connection, change);
    }
    sync_appearance(state, entity);
}

/// Authoritative per-tick dig progress, also used for instant Start admission.
pub(crate) fn mining_progress(
    state: &crate::tick::TickState,
    entity: crate::boundary::EntityId,
    block_state: u32,
) -> f32 {
    let Some(player) = state.players.get(&entity) else {
        return 0.0;
    };
    let Some(block) = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block_state))
    else {
        return 0.0;
    };
    let rules = state.registry.item_rules();
    let stack = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
    let tool = rules
        .facts
        .facts_for_stack(stack, rules.items)
        .and_then(|facts| facts.tool.as_ref());
    let matches = |rule: &mc_data::item_components::ToolRuleFacts| {
        rule.blocks.iter().any(|target| {
            if target.starts_with('#') {
                rules
                    .tags
                    .contains_raw_id("minecraft:block", target, block.block.raw_id)
            } else {
                target == block.block.id.as_str()
            }
        })
    };
    let correct = !state
        .registry
        .mining
        .requires_correct_tool_for_drops(block_state)
        .unwrap_or(false)
        || tool
            .and_then(|tool| {
                tool.rules
                    .iter()
                    .find_map(|rule| rule.correct_for_drops.filter(|_| matches(rule)))
            })
            .unwrap_or(false);
    let mut speed = tool.map_or(1.0, |tool| {
        tool.rules
            .iter()
            .find_map(|rule| rule.speed.filter(|_| matches(rule)))
            .unwrap_or(tool.default_mining_speed.unwrap_or(1.0))
    });
    if speed > 1.0 {
        let efficiency = stack
            .enchantments
            .iter()
            .find(|enchantment| enchantment.id.as_str() == "minecraft:efficiency")
            .map_or(0, |entry| entry.level.max(0));
        if efficiency > 0 {
            speed += (efficiency.saturating_mul(efficiency).saturating_add(1)) as f32;
        }
    }
    speed *= player.active_effects.dig_multiplier();
    let aqua_affinity = crate::enchantments::level(&player.inventory.slots[39], "aqua_affinity");
    let eye_in_water = state.entities.slot_of(entity).is_some_and(|slot| {
        let pos = state.entities.pos(slot);
        state
            .world
            .block_at(
                pos[0].floor() as i32,
                (pos[1] + 1.62).floor() as i32,
                pos[2].floor() as i32,
            )
            .is_some_and(|id| {
                state.registry.materials.classify(id) == mc_physics::BlockMaterial::Water
            })
    });
    let ground = state
        .entities
        .slot_of(entity)
        .is_some_and(|slot| state.entities.ground(slot));
    mc_data::block_mining::destroy_progress_per_tick(
        state
            .registry
            .mining
            .destroy_speed(block_state)
            .unwrap_or(-1.0),
        speed,
        correct,
        ground,
        eye_in_water && aqua_affinity == 0,
    )
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
