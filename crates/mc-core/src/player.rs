//! One player's own state (ADR 0013) and the inputs that touch only it:
//! hotbar, window 0 and the open menu, recipe book, hand swap, drops,
//! eating, death, respawn and the player-local commands. Pure: the tick
//! routes inputs here, spawns `dropped` stacks as item entities at the
//! player and sends `changes` to the connection.

use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::{decrement_stack, take_from_stack};
use mc_data::statistics::{CustomStat, StatType};

use crate::boundary::{ClickMode, ConnectionId, Hand, PlayerChange};
use crate::commands::{self, Command, GameMode};
use crate::inventory::{ClickOutcome, ItemRules, Menu, MenuKind, OFF_HAND, PlayerInventory};
use crate::survival::Vitals;

pub(crate) use reservations::NativeToolWearPlan;

/// Eating time of food without its own `consumable` duration (1.6 s).
const DEFAULT_EAT_TICKS: u32 = 32;
/// Vanilla `containerCounter` wraps back to 1 after 100.
const MAX_WINDOW: u8 = 100;
pub(crate) const PLAYER_ATTRIBUTES: &[(&str, f64)] = &[
    ("attack_speed", 4.0),
    ("attack_damage", 1.0),
    ("armor", 0.0),
    ("armor_toughness", 0.0),
    ("knockback_resistance", 0.0),
    ("movement_speed", 0.1),
    ("oxygen_bonus", 0.0),
    ("submerged_mining_speed", 0.2),
    ("water_movement_efficiency", 0.0),
    ("sneaking_speed", 0.3),
    ("movement_efficiency", 0.0),
    ("max_health", 20.0),
    ("scale", 1.0),
    ("step_height", 0.6),
    ("block_interaction_range", 4.5),
    ("entity_interaction_range", 3.0),
];

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub connection: ConnectionId,
    /// Retained player root fields, including personal EnderItems owned by ender_chest.
    pub(crate) opaque_player_fields: Vec<u8>,
    pub statistics: mc_data::statistics::PlayerStatistics,
    pub inventory: PlayerInventory,
    /// Vanilla advancement document, including retained unknown entries.
    pub advancement_progress: std::sync::Arc<serde_json::Value>,
    pub unlocked_recipes: std::sync::Arc<std::collections::BTreeSet<String>>,
    pub(crate) advancement_inventory: Vec<ItemStack>,
    pub(crate) advancement_effects_changed: bool,
    pub(crate) advancement_levitation_origin: Option<([f64; 3], u64)>,
    /// Window 0, open for the player's lifetime.
    pub inventory_menu: Menu,
    /// The open container menu, if any.
    pub open: Option<Menu>,
    pub vitals: Vitals,
    pub active_effects: crate::effects::ActiveEffects,
    /// Retained deterministic gameplay RNG (equipment and consumption draws).
    pub enchantment_seed: i32,
    /// Vanilla XpSeed: changes only after an enchanting transaction.
    pub enchantment_offer_seed: i32,
    pub warden_warning: crate::shrieker::Warning,
    pub game_mode: GameMode,
    /// Blocks fallen since the player last stood on ground (vanilla
    /// `fallDistance`); not persisted, like a rejoin in vanilla mid-fall.
    pub fall_distance: f64,
    /// Vanilla FallFlying; player motion itself remains client-authoritative.
    pub(crate) fall_flying: bool,
    /// Runtime-only flight age; equipment loses durability every 20 ticks.
    pub(crate) fall_flying_ticks: u32,
    /// Vanilla current_explosion_impact_pos; protects the next landing.
    pub(crate) current_impulse_impact_pos: Option<[f64; 3]>,
    /// Transient movement mode; never restored across a connection.
    pub sprinting: bool,
    /// Last accepted client displacement squared, in blocks/tick, for sweeps.
    pub(crate) known_horizontal_movement_squared: f64,
    /// Last accepted vanilla movement ground flag, for jump/movement awards.
    /// Collision resolution alone loses support on zero-vertical walking moves.
    pub(crate) statistics_grounded: bool,
    /// Connection-local anti-flight sample count and last support/descending
    /// position. They reset on reconnect and are never persisted.
    pub(crate) airtime_ticks: u32,
    pub(crate) airtime_sample_tick: Option<u64>,
    pub(crate) airtime_safe_position: Option<[f64; 3]>,
    /// Connection-local fence: vehicle physics/corrections run at most once per tick.
    pub(crate) last_vehicle_move_tick: Option<u64>,
    /// A client steering packet was accepted this tick.
    pub(crate) last_vehicle_motion_tick: Option<u64>,
    /// Consecutive unsupported boat hover samples; connection-local, never saved.
    pub(crate) vehicle_airtime_ticks: u32,
    /// Last admitted client boat height, before the native gravity step.
    pub(crate) vehicle_last_reported_y: Option<f64>,
    /// Connection-local chat/communication budget; never persisted.
    pub(crate) chat_spam_ticks: u16,
    /// Connection-local vanilla teleport sequence; retained across respawns
    /// and dimension transfers, never persisted across reconnects.
    pub(crate) teleport_id: i32,
    pub(crate) awaiting_teleport: Option<i32>,
    /// Connection-local authoritative mining progress; never persisted.
    pub(crate) dig_progress: Option<crate::tick::mining::DigProgress>,
    pub(crate) delayed_dig: Option<crate::tick::mining::DelayedDig>,
    using: Option<ItemUse>,
    shield_cooldown: u32,
    /// Vanilla item cooldowns are connection-local and reset on rejoin.
    pub(crate) item_cooldowns: std::collections::BTreeMap<u32, u32>,
    pub(crate) last_contact_position: Option<[f64; 3]>,
    /// Bed head or command-selected cell. Forced command points do not
    /// require a surviving bed; sleeping replaces them with a bed point.
    pub respawn_point: Option<[i32; 3]>,
    pub respawn_dimension: crate::dimensions::Dimension,
    pub respawn_forced: bool,
    pub respawn_yaw: f32,
    pub respawn_pitch: f32,
    last_window: u8,
    pub(crate) attribute_equipment: Option<[ItemStack; 6]>,
    pub(crate) sent_attributes: Vec<crate::boundary::PlayerAttribute>,
    pub(crate) soul_speed_active: bool,
    /// Held main-hand stack as of the previous tick's end (MC-28289): the
    /// melee path resolves `attack_damage`/`attack_speed` attribute values
    /// from this snapshot, so a same-tick swap+attack hits with the OLD
    /// item's attributes. `None` until the first tick snapshots it.
    pub(crate) attribute_snapshot_held: Option<ItemStack>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ItemUse {
    hand: Hand,
    remaining: u32,
    elapsed: u32,
    item_id: u32,
}

/// Slot writes and hotbar selection of one vanilla pick item.
#[derive(Debug)]
pub(crate) struct PickItemPlan {
    selected: u8,
    writes: Vec<(usize, ItemStack)>,
}

impl PickItemPlan {
    /// Borrowed slot replacements for reservation-floor admission.
    pub(crate) fn replacements(&self) -> Vec<(usize, &ItemStack)> {
        self.writes
            .iter()
            .map(|(slot, stack)| (*slot, stack))
            .collect()
    }
}

/// What one input did to the player.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerEffects {
    pub changes: Vec<PlayerChange>,
    /// Stacks to spawn as item entities at the player.
    pub dropped: Vec<ItemStack>,
    /// The open container's storage changed (block entity dirty).
    pub container_changed: bool,
    /// Furnace menus: items left the output slot (award experience).
    pub furnace_output_taken: bool,
    /// Furnace menus: input slot `(before, after)` when it changed.
    pub furnace_input_change: Option<(ItemStack, ItemStack)>,
    pub(crate) station_output_taken: bool,
    pub experience_orbs: i32,
    pub anvil_used: bool,
    /// A finished chorus fruit requests world-aware teleport resolution.
    pub(crate) chorus_consumed: bool,
    pub(crate) consumed_item: Option<ItemStack>,
    pub(crate) crafted_recipes: Vec<(String, Vec<ItemStack>)>,
    pub(crate) brewed_potions: Vec<String>,
    pub(crate) enchanted_item: Option<(ItemStack, i32)>,
    pub(crate) crafted: Vec<(u32, i32, MenuKind)>,
}

impl PlayerState {
    pub(crate) fn has_active_use(&self) -> bool {
        self.using.is_some()
    }

    pub(crate) fn brush_use(&self, rules: &ItemRules<'_>) -> Option<(Hand, u32)> {
        let using = self.using?;
        let held = &self.inventory.slots[self.held_slot(using.hand)];
        (!held.is_empty()
            && held.item_id == using.item_id
            && rules
                .items
                .name_of(held.item_id)
                .is_some_and(|id| id.as_str() == "minecraft:brush"))
        .then_some((using.hand, using.elapsed))
    }

    #[must_use]
    pub fn new(connection: ConnectionId, game_mode: GameMode) -> Self {
        Self {
            connection,
            opaque_player_fields: Vec::new(),
            statistics: mc_data::statistics::PlayerStatistics::default(),
            inventory: PlayerInventory::default(),
            advancement_progress: crate::journal::empty_advancement_progress(),
            unlocked_recipes: std::sync::Arc::new(Default::default()),
            advancement_inventory: Vec::new(),
            advancement_effects_changed: false,
            advancement_levitation_origin: None,
            inventory_menu: Menu::new(0, MenuKind::Player),
            open: None,
            vitals: Vitals::default(),
            active_effects: crate::effects::ActiveEffects::default(),
            enchantment_seed: crate::inventory::enchanting::next_seed(connection.0 as i32),
            enchantment_offer_seed: crate::inventory::enchanting::next_seed(connection.0 as i32),
            warden_warning: Default::default(),
            game_mode,
            fall_distance: 0.0,
            fall_flying: false,
            fall_flying_ticks: 0,
            current_impulse_impact_pos: None,
            sprinting: false,
            known_horizontal_movement_squared: 0.0,
            statistics_grounded: true,
            airtime_ticks: 0,
            airtime_sample_tick: None,
            airtime_safe_position: None,
            last_vehicle_move_tick: None,
            last_vehicle_motion_tick: None,
            vehicle_airtime_ticks: 0,
            vehicle_last_reported_y: None,
            chat_spam_ticks: 0,
            teleport_id: 0,
            awaiting_teleport: None,
            dig_progress: None,
            delayed_dig: None,
            using: None,
            shield_cooldown: 0,
            item_cooldowns: std::collections::BTreeMap::new(),
            last_contact_position: None,
            last_window: 0,
            respawn_point: None,
            respawn_dimension: crate::dimensions::Dimension::Overworld,
            respawn_forced: false,
            respawn_yaw: 0.0,
            respawn_pitch: 0.0,
            attribute_equipment: None,
            sent_attributes: Vec::new(),
            soul_speed_active: false,
            attribute_snapshot_held: None,
        }
    }

    pub(crate) fn begin_teleport(&mut self) -> i32 {
        self.dig_progress = None;
        self.delayed_dig = None;
        // ServerGamePacketListenerImpl.teleport wraps before MAX_VALUE.
        self.teleport_id = if self.teleport_id == i32::MAX - 1 {
            0
        } else {
            self.teleport_id + 1
        };
        self.awaiting_teleport = Some(self.teleport_id);
        self.teleport_id
    }

    /// Diff equipment after all inventory mutations (including durability breaks).
    pub(crate) fn equipment_attributes(
        &mut self,
        rules: &ItemRules<'_>,
        retained: &[crate::entity_loot::EntityAttribute],
    ) -> Option<PlayerChange> {
        let slots = [
            usize::from(self.inventory.selected.min(8)),
            OFF_HAND,
            36,
            37,
            38,
            39,
        ];
        if self.attribute_equipment.as_ref().is_some_and(|old| {
            old.iter()
                .zip(slots)
                .all(|(stack, slot)| stack == &self.inventory.slots[slot])
        }) {
            return None;
        }
        let equipment = slots.map(|slot| self.inventory.slots[slot].clone());
        let mut attributes: Vec<crate::boundary::PlayerAttribute> = PLAYER_ATTRIBUTES
            .iter()
            .copied()
            .map(|(kind, base)| crate::boundary::PlayerAttribute {
                kind,
                base,
                modifiers: Vec::new(),
            })
            .collect();
        for saved in retained {
            if let Some(attribute) = attributes
                .iter_mut()
                .find(|attribute| saved.id.strip_prefix("minecraft:") == Some(attribute.kind))
            {
                attribute.base = saved.base;
                attribute
                    .modifiers
                    .extend(saved.modifiers.iter().map(|modifier| {
                        mc_data::item_components::ItemAttributeModifier {
                            kind: saved.id.clone(),
                            id: modifier.id.clone(),
                            amount: modifier.amount,
                            operation: modifier.operation.clone(),
                            slot: None,
                        }
                    }));
            }
        }
        for (slot, stack) in equipment.iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            let Some(facts) = rules.facts.facts_for_stack(stack, rules.items) else {
                continue;
            };
            for modifier in &facts.attribute_modifiers {
                let group = modifier
                    .slot
                    .as_deref()
                    .unwrap_or("any")
                    .trim_start_matches("minecraft:");
                let matches = match group {
                    "any" => true,
                    "hand" => slot < 2,
                    "armor" => slot >= 2,
                    "mainhand" => slot == 0,
                    "offhand" => slot == 1,
                    "feet" => slot == 2,
                    "legs" => slot == 3,
                    "chest" => slot == 4,
                    "head" => slot == 5,
                    _ => false,
                };
                if !matches {
                    continue;
                }
                if let Some(attribute) = attributes.iter_mut().find(|attribute| {
                    attribute.kind == modifier.kind.trim_start_matches("minecraft:")
                }) {
                    // AttributeInstance replaces duplicate modifier ids.
                    attribute.modifiers.retain(|old| old.id != modifier.id);
                    attribute.modifiers.push(modifier.clone());
                }
            }
        }
        crate::enchantments::equipment_attributes(
            &equipment,
            self.soul_speed_active,
            &mut attributes,
        );
        self.active_effects.attribute_modifiers(&mut attributes);
        if self.sprinting
            && let Some(speed) = attributes
                .iter_mut()
                .find(|attribute| attribute.kind == "movement_speed")
        {
            speed
                .modifiers
                .retain(|modifier| modifier.id != "minecraft:sprinting");
            speed
                .modifiers
                .push(mc_data::item_components::ItemAttributeModifier {
                    kind: "minecraft:movement_speed".into(),
                    id: "minecraft:sprinting".into(),
                    amount: f64::from(0.3_f32),
                    operation: "add_multiplied_total".into(),
                    slot: None,
                });
        }
        let changed = attributes
            .iter()
            .filter(|attribute| !self.sent_attributes.contains(attribute))
            .cloned()
            .collect::<Vec<_>>();
        self.attribute_equipment = Some(equipment);
        self.sent_attributes = attributes;
        (!changed.is_empty()).then_some(PlayerChange::Attributes {
            attributes: changed,
        })
    }

    pub(crate) fn effective_attribute(&self, kind: &str) -> Option<f64> {
        let attribute = self
            .sent_attributes
            .iter()
            .find(|attribute| attribute.kind == kind)?;
        let mut add = 0.0;
        let mut multiplied_base = 0.0;
        let mut multiplied_total = 1.0;
        for modifier in &attribute.modifiers {
            match modifier.operation.as_str() {
                "add_value" => add += modifier.amount,
                "add_multiplied_base" => multiplied_base += modifier.amount,
                "add_multiplied_total" => multiplied_total *= 1.0 + modifier.amount,
                _ => {}
            }
        }
        let (minimum, maximum) = match kind {
            "max_health" => (1.0, 1024.0),
            "movement_speed" | "attack_speed" => (0.0, 1024.0),
            "attack_damage" => (0.0, 2048.0),
            "armor" => (0.0, 30.0),
            "armor_toughness" => (0.0, 20.0),
            "scale" => (0.0625, 16.0),
            _ => (0.0, f64::MAX),
        };
        Some(
            ((attribute.base + add) * (1.0 + multiplied_base) * multiplied_total)
                .clamp(minimum, maximum),
        )
    }

    /// One accepted move's vertical travel (vanilla `checkFallDamage`):
    /// falling accumulates, landing returns the fall damage owed and
    /// resets; water, climbing, creative and spectator reset without
    /// damage.
    pub fn fall(&mut self, dy: f64, on_ground: bool, cushioned: bool) -> f32 {
        if cushioned
            || self.creative()
            || self.spectator()
            || self
                .active_effects
                .has(crate::effects::EffectId::SlowFalling)
            || self
                .active_effects
                .has(crate::effects::EffectId::Levitation)
        {
            self.fall_distance = 0.0;
            self.current_impulse_impact_pos = None;
            return 0.0;
        }
        // Vanilla `Entity.checkFallDamage` adds the move's own downward travel
        // before the ground test: the landing packet's drop counts too.
        if dy < 0.0 {
            self.fall_distance -= f64::from(dy as f32);
        }
        if on_ground {
            if self.current_impulse_impact_pos.take().is_some() {
                self.fall_distance = 0.0;
                return 0.0;
            }
            if self.fall_distance >= 2.0 {
                self.statistics.increment_custom(
                    CustomStat::FallOneCm,
                    (self.fall_distance * 100.0).round() as i32,
                );
            }
            let damage = crate::survival::fall_damage(
                (self.fall_distance
                    - f64::from(
                        self.active_effects
                            .level(crate::effects::EffectId::JumpBoost),
                    ))
                .max(0.0),
            );
            self.fall_distance = 0.0;
            return damage;
        }
        0.0
    }

    pub(crate) fn creative(&self) -> bool {
        self.game_mode == GameMode::Creative
    }

    pub(crate) fn spectator(&self) -> bool {
        self.game_mode == GameMode::Spectator
    }

    fn current_window(&self) -> u8 {
        self.open.as_ref().unwrap_or(&self.inventory_menu).window
    }

    fn held_slot(&self, hand: Hand) -> usize {
        match hand {
            Hand::Main => usize::from(self.inventory.selected.min(8)),
            Hand::Off => OFF_HAND,
        }
    }

    /// Full window 0 contents, for the join burst.
    #[must_use]
    pub fn inventory_contents(&self) -> PlayerChange {
        contents_change(&self.inventory_menu, &self.inventory, &[])
    }

    #[must_use]
    pub fn health_change(&self) -> PlayerChange {
        PlayerChange::Health {
            health: self.vitals.health,
            food: u8::try_from(self.vitals.food.clamp(0, 20)).unwrap_or(0),
            saturation: self.vitals.saturation,
        }
    }

    #[must_use]
    pub fn air_change(&self) -> PlayerChange {
        PlayerChange::AirSupply {
            air: self.vitals.air,
        }
    }

    #[must_use]
    pub fn experience_change(&self) -> PlayerChange {
        let experience = self.vitals.experience;
        PlayerChange::Experience {
            progress: experience.progress,
            level: experience.level,
            total: experience.total,
        }
    }

    /// Vanilla `handleSetCarriedItem`: out-of-range slots are ignored.
    pub fn select_slot(&mut self, slot: u8) {
        if slot > 8 || slot == self.inventory.selected {
            return;
        }
        if self
            .using
            .is_some_and(|item_use| item_use.hand == Hand::Main)
        {
            self.using = None;
        }
        self.inventory.selected = slot;
    }

    /// Vanilla `tryPickItem`: select a hotbar stack matching `picked`, swap a
    /// main-inventory match into the suitable hotbar slot, or, in creative only,
    /// put `picked` there (moving the displaced stack to a free slot). `None`
    /// changes nothing.
    pub(crate) fn plan_pick_item(&self, picked: &ItemStack) -> Option<PickItemPlan> {
        if self.spectator() || picked.is_empty() {
            return None;
        }
        let slots = &self.inventory.slots;
        let target = self.suitable_hotbar_slot();
        let mut writes = Vec::new();
        let selected = match (0..36)
            .find(|&slot| mc_data::inventory_semantics_26_1_2::can_stack(&slots[slot], picked))
        {
            Some(found) if found < 9 => found,
            Some(found) => {
                writes.push((target, slots[found].clone()));
                writes.push((found, slots[target].clone()));
                target
            }
            None if self.creative() => {
                if !slots[target].is_empty()
                    && let Some(free) = (0..36).find(|&slot| slots[slot].is_empty())
                {
                    writes.push((free, slots[target].clone()));
                }
                writes.push((target, picked.clone()));
                target
            }
            None => return None,
        };
        Some(PickItemPlan {
            selected: selected as u8,
            writes,
        })
    }

    pub(crate) fn apply_pick_item(&mut self, plan: PickItemPlan) -> PlayerEffects {
        let before = self.snapshot();
        for (slot, stack) in plan.writes {
            self.inventory.slots[slot] = stack;
        }
        self.select_slot(plan.selected);
        let mut changes = self.sync(&before);
        changes.push(PlayerChange::HeldSlot {
            slot: self.inventory.selected,
        });
        PlayerEffects {
            changes,
            ..PlayerEffects::default()
        }
    }

    /// Vanilla `getSuitableHotbarSlot`: the first empty hotbar slot from the
    /// selected one, else the first unenchanted one, else the selected slot.
    fn suitable_hotbar_slot(&self) -> usize {
        let selected = usize::from(self.inventory.selected.min(8));
        let order = || (0..9).map(move |offset| (selected + offset) % 9);
        let slots = &self.inventory.slots;
        order()
            .find(|&slot| slots[slot].is_empty())
            .or_else(|| order().find(|&slot| slots[slot].enchantments.is_empty()))
            .unwrap_or(selected)
    }

    /// Opens a container menu over `container`, closing any open one first.
    pub fn open_menu(
        &mut self,
        rules: &ItemRules<'_>,
        kind: MenuKind,
        title: String,
        container: &[ItemStack],
    ) -> PlayerEffects {
        if kind == MenuKind::Player {
            return PlayerEffects::default();
        }
        let mut effects = match self.open.as_ref().map(|menu| menu.window) {
            Some(window) => {
                let mut closed = self.close(rules);
                closed
                    .changes
                    .insert(0, PlayerChange::MenuClosed { window });
                closed
            }
            None => self.close(rules),
        };
        self.last_window = self.last_window % MAX_WINDOW + 1;
        let menu = Menu::new(self.last_window, kind);
        effects.changes.push(match kind {
            MenuKind::Mount {
                entity, columns, ..
            } => PlayerChange::MountMenuOpened {
                window: menu.window,
                columns,
                entity,
            },
            _ => PlayerChange::MenuOpened {
                window: menu.window,
                menu_type: kind.menu_type().expect("ordinary menu type"),
                title,
            },
        });
        effects
            .changes
            .push(contents_change(&menu, &self.inventory, container));
        self.open = Some(menu);
        effects
    }

    /// Vanilla `handleContainerClick`. Clicks on a window that is not the
    /// current one are ignored; spectators only get a resync.
    #[allow(clippy::too_many_arguments)]
    pub fn click(
        &mut self,
        rules: &ItemRules<'_>,
        window: u8,
        state_id: i32,
        slot: i16,
        button: i8,
        mode: ClickMode,
        container: &mut [ItemStack],
    ) -> PlayerEffects {
        if window != self.current_window() {
            return PlayerEffects::default();
        }
        let creative = self.creative();
        let spectator = self.spectator();
        let menu = self.open.as_mut().unwrap_or(&mut self.inventory_menu);
        if spectator || menu.kind == MenuKind::Lectern {
            return PlayerEffects {
                changes: vec![contents_change(menu, &self.inventory, container)],
                ..PlayerEffects::default()
            };
        }
        let outcome = menu.click(
            rules,
            &mut self.inventory,
            container,
            state_id,
            slot,
            button,
            mode,
            creative,
        );
        if let Some((item_id, count)) = outcome.crafted_output {
            self.statistics.increment(StatType::Crafted, item_id, count);
        }
        for &(item_id, count) in &outcome.manual_drops {
            self.statistics.increment(StatType::Dropped, item_id, count);
            self.statistics.increment_custom(CustomStat::Drop, 1);
        }
        click_effects(menu, &self.inventory, container, outcome)
    }

    /// Vanilla `handlePlaceRecipe` into the current crafting menu.
    pub fn place_recipe(
        &mut self,
        rules: &ItemRules<'_>,
        window: u8,
        recipe: u32,
        make_all: bool,
        container: &mut [ItemStack],
    ) -> PlayerEffects {
        if self.spectator() || window != self.current_window() {
            return PlayerEffects::default();
        }
        if !rules
            .recipes
            .get(recipe as usize)
            .is_some_and(|recipe| self.unlocked_recipes.contains(recipe.id.as_str()))
        {
            return PlayerEffects::default();
        }
        let menu = self.open.as_mut().unwrap_or(&mut self.inventory_menu);
        let outcome = menu.place_recipe(rules, &mut self.inventory, recipe, make_all, container);
        click_effects(menu, &self.inventory, container, outcome)
    }

    /// Vanilla `doCloseContainer`: closes the current menu whatever window
    /// the client names. Cursor and crafting grid go back into the
    /// inventory; what does not fit is dropped.
    pub fn close(&mut self, rules: &ItemRules<'_>) -> PlayerEffects {
        let before = self.inventory_menu.contents(&self.inventory, &[]);
        let dropped = match self.open.take() {
            Some(mut menu) => menu.close(rules, &mut self.inventory),
            None => self.inventory_menu.close(rules, &mut self.inventory),
        };
        PlayerEffects {
            changes: self.sync(&before),
            dropped,
            ..PlayerEffects::default()
        }
    }

    /// Vanilla `SWAP_ITEM_WITH_OFFHAND`.
    pub fn swap_hands(&mut self) -> PlayerEffects {
        if self.spectator() {
            return PlayerEffects::default();
        }
        let before = self.snapshot();
        let main = self.held_slot(Hand::Main);
        self.inventory.slots.swap(main, OFF_HAND);
        self.using = None;
        PlayerEffects {
            changes: self.sync(&before),
            ..PlayerEffects::default()
        }
    }

    /// Wear the main-hand tool by `amount` damage (block breaking):
    /// no-op when empty or not damageable; breaks (empties) at max damage.
    /// Returns slot-sync changes.
    pub fn damage_held_tool(&mut self, rules: &ItemRules<'_>, amount: i32) -> PlayerEffects {
        let plan = self.plan_held_tool_wear(rules, amount);
        self.apply_tool_wear(plan)
    }

    /// Take `amount` from the held slot (block placement): no-op when
    /// empty; creative never consumes. Returns slot-sync changes.
    pub fn consume_held(&mut self, hand: Hand, amount: i32) -> PlayerEffects {
        if self.spectator() || amount <= 0 {
            return PlayerEffects::default();
        }
        let slot = self.held_slot(hand);
        let held = &self.inventory.slots[slot];
        if held.is_empty() {
            return PlayerEffects::default();
        }
        self.statistics.increment(StatType::Used, held.item_id, 1);
        if self.creative() {
            return PlayerEffects::default();
        }
        let before = self.snapshot();
        let slot = self.held_slot(hand);
        let taken = take_from_stack(&mut self.inventory.slots[slot], amount);
        if taken.is_empty() {
            return PlayerEffects::default();
        }
        PlayerEffects {
            changes: self.sync(&before),
            ..Default::default()
        }
    }

    pub(crate) fn replace_held(&mut self, hand: Hand, stack: ItemStack) -> Vec<PlayerChange> {
        let before = self.snapshot();
        let slot = self.held_slot(hand);
        self.inventory.slots[slot] = stack;
        self.sync(&before)
    }

    pub(crate) fn receive_stack(
        &mut self,
        rules: &ItemRules<'_>,
        mut stack: ItemStack,
    ) -> PlayerEffects {
        let before = self.snapshot();
        self.inventory.add(rules, &mut stack);
        PlayerEffects {
            changes: self.sync(&before),
            dropped: if stack.is_empty() {
                Vec::new()
            } else {
                vec![stack]
            },
            ..Default::default()
        }
    }

    pub(crate) fn harvest_held(
        &mut self,
        rules: &ItemRules<'_>,
        hand: Hand,
        mut product: ItemStack,
        bottle: bool,
    ) -> PlayerEffects {
        let before = self.snapshot();
        let slot = self.held_slot(hand);
        self.statistics
            .increment(StatType::Used, self.inventory.slots[slot].item_id, 1);
        if !self.creative() {
            if bottle {
                take_from_stack(&mut self.inventory.slots[slot], 1);
                if self.inventory.slots[slot].is_empty() {
                    self.inventory.slots[slot] = std::mem::take(&mut product);
                }
            } else {
                let stack = &mut self.inventory.slots[slot];
                let amount = crate::enchantments::durability_damage(
                    stack,
                    1,
                    false,
                    &mut self.enchantment_seed,
                );
                let damage = stack.damage.unwrap_or(0).saturating_add(amount);
                if rules
                    .facts
                    .facts_for_stack(stack, rules.items)
                    .and_then(|f| f.max_damage)
                    .is_some_and(|max| damage as u32 >= max)
                {
                    self.statistics
                        .increment(StatType::Broken, stack.item_id, 1);
                    *stack = ItemStack::EMPTY;
                } else if amount > 0 {
                    stack.damage = Some(damage);
                }
            }
        }
        self.inventory.add(rules, &mut product);
        PlayerEffects {
            changes: self.sync(&before),
            dropped: if product.is_empty() {
                Vec::new()
            } else {
                vec![product]
            },
            ..Default::default()
        }
    }

    /// Test-only: overwrite an inventory slot with an exact stack and
    /// sync. Unit tests use this to place component stacks no protocol
    /// input can materialize.
    #[cfg(test)]
    pub fn test_set_slot(&mut self, slot: usize, stack: ItemStack) -> PlayerEffects {
        let before = self.snapshot();
        if let Some(target) = self.inventory.slots.get_mut(slot) {
            *target = stack;
        }
        PlayerEffects {
            changes: self.sync(&before),
            ..Default::default()
        }
    }

    /// Vanilla `handleSetCreativeModeSlot`: a creative client sets an
    /// inventory menu slot (5-45: armor, main, hotbar, off hand) outright,
    /// or throws the stack (`slot` -1). Over-stacked stacks are refused.
    pub fn creative_slot(
        &mut self,
        rules: &ItemRules<'_>,
        slot: i16,
        stack: ItemStack,
    ) -> PlayerEffects {
        if !self.creative() || !valid_creative_stack(rules, &stack) {
            return PlayerEffects::default();
        }
        if slot == -1 {
            if !stack.is_empty() {
                self.statistics
                    .increment(StatType::Dropped, stack.item_id, stack.count);
                self.statistics.increment_custom(CustomStat::Drop, 1);
            }
            return PlayerEffects {
                dropped: (!stack.is_empty()).then_some(stack).into_iter().collect(),
                ..PlayerEffects::default()
            };
        }
        let Some(index) = creative_inventory_slot(slot) else {
            return PlayerEffects::default();
        };
        let before = self.snapshot();
        self.inventory.slots[index] = stack;
        PlayerEffects {
            changes: self.sync(&before),
            ..PlayerEffects::default()
        }
    }

    pub fn drop_selected(&mut self, whole_stack: bool) -> PlayerEffects {
        if self.spectator() {
            return PlayerEffects::default();
        }
        let before = self.snapshot();
        let held = usize::from(self.inventory.selected.min(8));
        let slot = &mut self.inventory.slots[held];
        let amount = if whole_stack { slot.count } else { 1 };
        let dropped = take_from_stack(slot, amount);
        if dropped.is_empty() {
            return PlayerEffects::default();
        }
        self.statistics
            .increment(StatType::Dropped, dropped.item_id, dropped.count);
        self.statistics.increment_custom(CustomStat::Drop, 1);
        if self
            .using
            .is_some_and(|item_use| item_use.hand == Hand::Main)
        {
            self.using = None;
        }
        PlayerEffects {
            changes: self.sync(&before),
            dropped: vec![dropped],
            ..PlayerEffects::default()
        }
    }

    /// The shield becomes effective after vanilla's five-tick raise delay.
    pub(crate) fn raised_shield(&self, rules: &ItemRules<'_>) -> Option<Hand> {
        let item_use = self.using?;
        let held = &self.inventory.slots[self.held_slot(item_use.hand)];
        (item_use.elapsed >= 5
            && self.shield_cooldown == 0
            && !self.spectator()
            && !self.vitals.is_dead()
            && !held.is_empty()
            && held.item_id == item_use.item_id
            && rules
                .items
                .name_of(held.item_id)
                .is_some_and(|item| item.as_str() == "minecraft:shield"))
        .then_some(item_use.hand)
    }

    pub(crate) fn disable_shield(&mut self) -> PlayerChange {
        self.using = None;
        self.shield_cooldown = 100;
        PlayerChange::ShieldCooldown { ticks: 100 }
    }

    /// Item.use's swappable equippable branch. `Some` also owns a refused
    /// swap, so it cannot fall through to food/shield use.
    pub(crate) fn swap_equippable(
        &mut self,
        rules: &ItemRules<'_>,
        hand: Hand,
        player_type: i32,
    ) -> Option<PlayerEffects> {
        let held_slot = self.held_slot(hand);
        let held = &self.inventory.slots[held_slot];
        if held.is_empty() {
            return None;
        }
        let facts = rules.facts.facts_for_stack(held, rules.items)?;
        if !facts.equippable_swappable.unwrap_or(true) {
            return None;
        }
        let target = match facts.equippable_slot.as_deref()? {
            "mainhand" => usize::from(self.inventory.selected.min(8)),
            "offhand" => OFF_HAND,
            "feet" => 36,
            "legs" => 37,
            "chest" => 38,
            "head" => 39,
            _ => return None, // Players cannot use body/saddle slots.
        };
        if facts
            .equippable_allowed_entities
            .as_ref()
            .is_some_and(|allowed| {
                !allowed.iter().any(|entity| {
                    entity == "minecraft:player"
                        || entity.strip_prefix('#').is_some_and(|tag| {
                            rules
                                .tags
                                .contains_raw_id("minecraft:entity_type", tag, player_type)
                        })
                })
            })
        {
            return None;
        }
        let old = &self.inventory.slots[target];
        if self.spectator()
            || self.vitals.is_dead()
            || self.item_on_cooldown(held.item_id)
            || (!self.creative() && crate::enchantments::level(old, "binding_curse") > 0)
            || mc_data::inventory_semantics_26_1_2::can_stack(held, old)
        {
            return Some(PlayerEffects::default());
        }
        let creative = self.creative();
        let single = held.count <= 1;
        let mut old = ItemStack::EMPTY;
        if single && !creative {
            self.inventory.slots.swap(held_slot, target);
        } else {
            let mut equipped = held.clone();
            if !single {
                equipped.count = 1;
            }
            old = std::mem::replace(&mut self.inventory.slots[target], equipped);
            if single {
                if !old.is_empty() {
                    self.inventory.slots[held_slot] = old;
                }
                old = ItemStack::EMPTY;
            } else {
                if !creative {
                    decrement_stack(&mut self.inventory.slots[held_slot]);
                }
                self.inventory.add(rules, &mut old);
            }
        }
        Some(PlayerEffects {
            changes: vec![self.inventory_contents()],
            dropped: if old.is_empty() {
                Vec::new()
            } else {
                vec![old]
            },
            ..PlayerEffects::default()
        })
    }

    /// Starts food consumption or raises the shield held in either hand.
    pub fn use_item(&mut self, rules: &ItemRules<'_>, hand: Hand) {
        if self.spectator() || self.vitals.is_dead() {
            return;
        }
        // Vanilla startUsingItem does not restart an already-active use. Keep
        // the original food/drink duration and shield raise delay across packets.
        if self.using.is_some_and(|using| {
            let held = &self.inventory.slots[self.held_slot(using.hand)];
            !held.is_empty() && held.item_id == using.item_id
        }) {
            return;
        }
        let held = &self.inventory.slots[self.held_slot(hand)];
        if self.item_on_cooldown(held.item_id) {
            return;
        }
        let Some(facts) = (!held.is_empty())
            .then(|| rules.facts.facts_for_stack(held, rules.items))
            .flatten()
        else {
            return;
        };
        if rules
            .items
            .name_of(held.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:brush")
        {
            self.using = Some(ItemUse {
                hand,
                remaining: 200,
                elapsed: 0,
                item_id: held.item_id,
            });
            return;
        }
        if rules
            .items
            .name_of(held.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:shield")
        {
            if self.shield_cooldown == 0 {
                self.using = Some(ItemUse {
                    hand,
                    remaining: 72_000,
                    elapsed: 0,
                    item_id: held.item_id,
                });
            }
            return;
        }
        let drink = rules.items.name_of(held.item_id).is_some_and(|item| {
            matches!(
                item.as_str(),
                "minecraft:potion"
                    | "minecraft:milk_bucket"
                    | "minecraft:honey_bottle"
                    | "minecraft:ominous_bottle"
            )
        });
        if drink
            || self.creative()
            || facts
                .food
                .as_ref()
                .is_some_and(|food| self.vitals.can_eat(food))
        {
            self.using = Some(ItemUse {
                hand,
                remaining: facts.use_duration_ticks.unwrap_or(DEFAULT_EAT_TICKS).max(1),
                elapsed: 0,
                item_id: held.item_id,
            });
        }
    }

    /// Releasing the use key stops eating or blocking.
    pub fn release_use_item(&mut self) {
        self.using = None;
    }

    pub(crate) fn finishing_consumable_event(
        &self,
        rules: &ItemRules<'_>,
    ) -> Option<crate::vibration::GameEvent> {
        let using = self.using?;
        let held = &self.inventory.slots[self.held_slot(using.hand)];
        if using.remaining != 1 || held.is_empty() || held.item_id != using.item_id {
            return None;
        }
        let facts = rules.facts.facts_for_stack(held, rules.items)?;
        if facts.food.is_some() {
            Some(crate::vibration::GameEvent::Eat)
        } else if rules
            .items
            .name_of(held.item_id)
            .is_some_and(|i| matches!(i.path(), "potion" | "milk_bucket" | "honey_bottle"))
        {
            Some(crate::vibration::GameEvent::Drink)
        } else {
            None
        }
    }

    /// One tick of item use: finishes eating once its time has passed.
    pub fn tick_item_use(&mut self, rules: &ItemRules<'_>) -> PlayerEffects {
        self.shield_cooldown = self.shield_cooldown.saturating_sub(1);
        self.item_cooldowns.retain(|_, remaining| {
            *remaining = remaining.saturating_sub(1);
            *remaining > 0
        });
        let Some(mut item_use) = self.using else {
            return PlayerEffects::default();
        };
        let slot = self.held_slot(item_use.hand);
        let held = &self.inventory.slots[slot];
        if held.is_empty() || held.item_id != item_use.item_id {
            self.using = None;
            return PlayerEffects::default();
        }
        item_use.elapsed = item_use.elapsed.saturating_add(1);
        if rules
            .items
            .name_of(held.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:shield")
        {
            self.using = Some(item_use);
            return PlayerEffects::default();
        }
        let facts = rules.facts.facts_for_stack(held, rules.items);
        let Some(facts) = facts else {
            self.using = None;
            return PlayerEffects::default();
        };
        item_use.remaining -= 1;
        if item_use.remaining > 0 {
            self.using = Some(item_use);
            return PlayerEffects::default();
        }
        self.using = None;
        if rules
            .items
            .name_of(held.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:brush")
        {
            return PlayerEffects::default();
        }
        let before = self.snapshot();
        let consumed = self.inventory.slots[slot].clone();
        self.statistics
            .increment(StatType::Used, consumed.item_id, 1);
        if let Some(food) = &facts.food {
            self.vitals.eat(food);
        }
        let mut effect_changes = self.consume_effects(rules, &consumed);
        let mut dropped = Vec::new();
        if !self.creative() {
            decrement_stack(&mut self.inventory.slots[slot]);
            let remainder = facts
                .use_remainder
                .as_ref()
                .and_then(|id| rules.items.id_of(id))
                .or_else(|| {
                    let name = rules.items.name_of(consumed.item_id)?.as_str();
                    let remainder = match name {
                        "minecraft:potion" => "minecraft:glass_bottle",
                        "minecraft:milk_bucket" => "minecraft:bucket",
                        "minecraft:honey_bottle" => "minecraft:glass_bottle",
                        _ => return None,
                    };
                    rules
                        .items
                        .id_of(&mc_data::Identifier::parse(remainder).ok()?)
                })
                .map(|id| ItemStack::new(id, 1));
            if let Some(mut remainder) = remainder {
                if self.inventory.slots[slot].is_empty() {
                    self.inventory.slots[slot] = remainder;
                } else {
                    self.inventory.add(rules, &mut remainder);
                    if !remainder.is_empty() {
                        dropped.push(remainder);
                    }
                }
            }
        }
        let mut changes = self.sync(&before);
        changes.append(&mut effect_changes);
        changes.push(self.health_change());
        let chorus_consumed = rules
            .items
            .name_of(consumed.item_id)
            .is_some_and(|name| name.as_str() == "minecraft:chorus_fruit");
        if chorus_consumed
            && let Some(change) = self.start_item_cooldown(rules, consumed.item_id, 20)
        {
            changes.push(change);
        }
        PlayerEffects {
            changes,
            dropped,
            chorus_consumed,
            consumed_item: Some(consumed),
            ..PlayerEffects::default()
        }
    }

    /// Death closes the menu; only non-retained inventories and XP are lost.
    ///
    /// `keep_experience` is separate from `keep_inventory` because the two answers
    /// come from different owners: the gamerule retains both, while native grave
    /// custody takes the items into a container and still leaves the bar alone.
    pub fn die(
        &mut self,
        rules: &ItemRules<'_>,
        keep_inventory: bool,
        keep_experience: bool,
    ) -> PlayerEffects {
        self.statistics.increment_custom(CustomStat::Deaths, 1);
        self.statistics.set_custom(CustomStat::TimeSinceDeath, 0);
        self.statistics.set_custom(CustomStat::TimeSinceRest, 0);
        let mut effects = PlayerEffects::default();
        if let Some(menu) = &self.open {
            effects.changes.push(PlayerChange::MenuClosed {
                window: menu.window,
            });
        }
        let closed = self.close(rules);
        effects.dropped = closed.dropped;
        let before = self.snapshot();
        if !keep_inventory && !self.spectator() {
            effects.dropped.extend(
                self.inventory
                    .slots
                    .iter_mut()
                    .map(std::mem::take)
                    .filter(|stack| !stack.is_empty()),
            );
            effects
                .dropped
                .retain(|stack| crate::enchantments::level(stack, "vanishing_curse") == 0);
            if !keep_experience {
                self.vitals.experience = crate::survival::Experience::default();
                effects.changes.push(self.experience_change());
            }
        }
        self.using = None;
        effects.changes.extend(closed.changes);
        effects.changes.extend(self.sync(&before));
        effects
    }

    /// Respawn: fresh vitals, retaining the post-death experience. Emits `Respawned` (real spawn
    /// position/rotation/game mode) BEFORE Health/Experience so the edge
    /// sends vanilla `Respawn` then the position sync, like vanilla death.
    /// The caller moves the entity to the respawn position first and passes
    /// it here (the menu state cannot know the world spawn).
    pub fn respawn(
        &mut self,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        dimension: crate::dimensions::Dimension,
    ) -> PlayerEffects {
        let experience = self.vitals.experience;
        self.vitals.respawn();
        self.active_effects = crate::effects::ActiveEffects::default();
        self.vitals.experience = experience;
        self.attribute_equipment = None;
        self.sent_attributes.clear();
        self.using = None;
        self.fall_distance = 0.0;
        self.fall_flying = false;
        self.fall_flying_ticks = 0;
        self.current_impulse_impact_pos = None;
        let game_mode = self.game_mode as u8;
        PlayerEffects {
            changes: vec![
                PlayerChange::Respawned {
                    teleport_id: self.begin_teleport(),
                    dimension,
                    position,
                    yaw,
                    pitch,
                    game_mode,
                },
                self.health_change(),
                self.air_change(),
                self.experience_change(),
            ],
            ..PlayerEffects::default()
        }
    }

    /// The effect of a parsed command on this player as its target. World
    /// commands (teleport, time, weather, summon, kill, ...) belong to the
    /// tick and do nothing here.
    pub fn apply_command(&mut self, rules: &ItemRules<'_>, command: &Command) -> PlayerEffects {
        let before = self.snapshot();
        let mut effects = PlayerEffects::default();
        match command {
            Command::GameMode { mode, .. } => {
                if *mode != self.game_mode {
                    self.game_mode = *mode;
                    self.using = None;
                    effects.changes.push(PlayerChange::GameModeChanged {
                        game_mode: mode.id(),
                    });
                }
                return effects;
            }
            Command::Experience { amount, levels, .. } => {
                commands::add_experience(&mut self.vitals.experience, *amount, *levels);
                effects.changes.push(self.experience_change());
                return effects;
            }
            Command::ExperienceSet { amount, levels, .. } => {
                let experience = &mut self.vitals.experience;
                if *levels {
                    experience.level = *amount;
                } else {
                    let needed = crate::survival::Experience::points_for_level(experience.level);
                    if *amount >= needed {
                        return effects;
                    }
                    experience.progress = *amount as f32 / needed as f32;
                }
                effects.changes.push(self.experience_change());
                return effects;
            }
            Command::Enchant(command) => {
                if crate::command_enchant::apply(rules, &mut self.inventory, command).is_err() {
                    return effects;
                }
            }
            Command::Give { item, count, .. } => {
                if let Ok(stack) = item.stack_with(rules) {
                    effects.dropped = commands::give(rules, &mut self.inventory, &stack, *count);
                }
            }
            Command::Clear { item, max, .. } => {
                if item
                    .as_ref()
                    .is_some_and(|item| item.validate(rules).is_err())
                {
                    return effects;
                }
                commands::clear(
                    &mut self.inventory,
                    |stack| item.as_ref().is_none_or(|item| item.matches(rules, stack)),
                    *max,
                );
            }
            Command::DebugGive {
                item,
                count,
                hotbar_slot,
            } => {
                if let Ok(stack) = item.stack(rules.items) {
                    commands::debug_give(&mut self.inventory, stack, *count, *hotbar_slot);
                }
            }
            _ => return effects,
        }
        effects.changes = self.sync(&before);
        effects
    }

    /// Current menu contents without container storage (for [`Self::sync`]).
    fn snapshot(&self) -> Vec<ItemStack> {
        self.open
            .as_ref()
            .unwrap_or(&self.inventory_menu)
            .contents(&self.inventory, &[])
    }

    /// Sends the current menu's slots that differ from `before`.
    fn sync(&mut self, before: &[ItemStack]) -> Vec<PlayerChange> {
        let menu = self.open.as_mut().unwrap_or(&mut self.inventory_menu);
        let after = menu.contents(&self.inventory, &[]);
        let changed: Vec<(usize, ItemStack)> = before
            .iter()
            .zip(after)
            .enumerate()
            .filter(|(_, (old, new))| *old != new)
            .map(|(slot, (_, new))| (slot, new))
            .collect();
        if !changed.is_empty() {
            menu.state_id = (menu.state_id + 1) & 0x7FFF;
        }
        changed
            .into_iter()
            .map(|(slot, item)| slot_change(menu, slot, &item))
            .collect()
    }

    /// Re-image this viewer's open container menu against the authoritative
    /// `container` storage (shared-container fan-out). Always sends the
    /// fresh full image: the storage is shared, menus are small, and a stale
    /// revision lets a second viewer click an already-taken item. The caller
    /// bumps `state_id` first; the image carries the fresh revision.
    pub fn sync_container_view(&self, container: &[ItemStack]) -> Vec<PlayerChange> {
        let Some(menu) = self.open.as_ref() else {
            return Vec::new();
        };
        vec![contents_change(menu, &self.inventory, container)]
    }
}

fn valid_creative_stack(rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
    stack.is_empty()
        || (rules.items.name_of(stack.item_id).is_some() && stack.count <= rules.max_stack(stack))
}

fn creative_inventory_slot(slot: i16) -> Option<usize> {
    match slot {
        5 => Some(39),
        6 => Some(38),
        7 => Some(37),
        8 => Some(36),
        9..=35 => Some(slot as usize),
        36..=44 => Some(slot as usize - 36),
        45 => Some(OFF_HAND),
        _ => None,
    }
}

fn wire(stack: &ItemStack) -> Option<ItemStack> {
    (!stack.is_empty()).then(|| stack.clone())
}

fn slot_change(menu: &Menu, slot: usize, item: &ItemStack) -> PlayerChange {
    PlayerChange::MenuSlot {
        window: menu.window,
        state_id: menu.state_id,
        slot: i16::try_from(slot).unwrap_or(i16::MAX),
        item: wire(item),
    }
}

fn contents_change(
    menu: &Menu,
    inventory: &PlayerInventory,
    container: &[ItemStack],
) -> PlayerChange {
    PlayerChange::MenuContents {
        window: menu.window,
        state_id: menu.state_id,
        slots: menu
            .contents(inventory, container)
            .iter()
            .map(wire)
            .collect(),
        cursor: wire(&menu.carried),
    }
}

fn click_effects(
    menu: &Menu,
    inventory: &PlayerInventory,
    container: &[ItemStack],
    outcome: ClickOutcome,
) -> PlayerEffects {
    // One atomic slots + cursor image for every current-window click, including
    // refusals/no-ops: discarded client predictions must converge to authority.
    let changes = vec![contents_change(menu, inventory, container)];
    PlayerEffects {
        changes,
        dropped: outcome.dropped,
        container_changed: outcome.container_changed,
        furnace_output_taken: outcome.furnace_output_taken,
        furnace_input_change: outcome.furnace_input_change,
        station_output_taken: outcome.station_output_taken,
        crafted_recipes: outcome.crafted_recipes,
        brewed_potions: outcome.brewed_potions,
        crafted: outcome.crafted,
        ..PlayerEffects::default()
    }
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod tests;

mod stations;

#[path = "player_reservations.rs"]
mod reservations;

#[cfg(test)]
#[path = "player_reservations_tests.rs"]
mod reservation_tests;
