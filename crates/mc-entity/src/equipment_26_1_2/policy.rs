use super::{EquipmentSlot, ItemStackState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSlotFacts {
    pub equippable_slot: Option<EquipmentSlot>,
    pub can_use_equippable_slot: bool,
    pub can_use_main_hand: bool,
    pub can_be_equipped_by_entity: bool,
}

/// Mirrors `LivingEntity.getEquipmentSlotForItem` after caller-owned component
/// and `canUseSlot` resolution.
pub const fn equipment_slot_for_item(facts: ItemSlotFacts) -> EquipmentSlot {
    match facts.equippable_slot {
        Some(slot) if facts.can_use_equippable_slot => slot,
        _ => EquipmentSlot::MainHand,
    }
}

/// Mirrors `LivingEntity.isEquippableInSlot` after registry/type checks.
pub const fn is_equippable_in_slot(facts: ItemSlotFacts, slot: EquipmentSlot) -> bool {
    match facts.equippable_slot {
        None => matches!(slot, EquipmentSlot::MainHand) && facts.can_use_main_hand,
        Some(equippable_slot) => {
            slot.ordinal() == equippable_slot.ordinal()
                && facts.can_use_equippable_slot
                && facts.can_be_equipped_by_entity
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackEquipKind {
    Empty,
    Ordinary,
    Equippable(EquipmentSlot),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EquipmentSlotMask(u8);

impl EquipmentSlotMask {
    pub const fn new() -> Self {
        Self(0)
    }

    pub const fn contains(self, slot: EquipmentSlot) -> bool {
        self.0 & (1 << slot.ordinal()) != 0
    }

    pub fn insert(&mut self, slot: EquipmentSlot) -> bool {
        let bit = 1 << slot.ordinal();
        let inserted = self.0 & bit == 0;
        self.0 |= bit;
        inserted
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Mirrors `EquipmentUser.resolveSlot`. A duplicate equippable slot is skipped;
/// it does not fall back to the main hand.
pub const fn resolve_equipment_table_slot(
    stack: StackEquipKind,
    inserted: EquipmentSlotMask,
) -> Option<EquipmentSlot> {
    match stack {
        StackEquipKind::Empty => None,
        StackEquipKind::Equippable(slot) => {
            if inserted.contains(slot) {
                None
            } else {
                Some(slot)
            }
        }
        StackEquipKind::Ordinary => {
            if inserted.contains(EquipmentSlot::MainHand) {
                None
            } else {
                Some(EquipmentSlot::MainHand)
            }
        }
    }
}

/// `HEAD`, `MAINHAND`, and `OFFHAND` use an unguarded vanilla `SlotAccess`.
/// Every other equipment slot accepts empty or a stack resolving to itself.
pub const fn slot_access_accepts(
    slot: EquipmentSlot,
    stack_is_empty: bool,
    resolved_slot: EquipmentSlot,
) -> bool {
    match slot {
        EquipmentSlot::Head | EquipmentSlot::MainHand | EquipmentSlot::OffHand => true,
        _ => stack_is_empty || slot.ordinal() == resolved_slot.ordinal(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EqualItemFacts {
    pub new_enchantment_entries: usize,
    pub current_enchantment_entries: usize,
    pub new_damage: i32,
    pub current_damage: i32,
    pub new_has_custom_name: bool,
    pub current_has_custom_name: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmorReplacementFacts {
    pub current_prevents_armor_change: bool,
    pub new_defense: f64,
    pub current_defense: f64,
    pub new_toughness: f64,
    pub current_toughness: f64,
    pub equal_item: EqualItemFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferredWeaponFacts {
    pub new_matches: bool,
    pub current_matches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponReplacementFacts {
    pub preferred_weapon: Option<PreferredWeaponFacts>,
    pub new_attack_damage: f64,
    pub current_attack_damage: f64,
    pub equal_item: EqualItemFacts,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReplacementComparison {
    Armor(ArmorReplacementFacts),
    MainHand(WeaponReplacementFacts),
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurrentItemFacts {
    Empty,
    Occupied(ReplacementComparison),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementReason {
    EmptySlot,
    ArmorChangePrevented,
    BetterDefense,
    WorseDefense,
    BetterToughness,
    WorseToughness,
    GainsPreferredWeapon,
    LosesPreferredWeapon,
    BetterAttackDamage,
    WorseAttackDamage,
    MoreEnchantments,
    FewerEnchantments,
    LessDamaged,
    MoreDamaged,
    NewCustomName,
    EqualItem,
    OccupiedSlotNotReplaceable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementDecision {
    Replace(ReplacementReason),
    Keep(ReplacementReason),
}

impl ReplacementDecision {
    pub const fn should_replace(self) -> bool {
        matches!(self, Self::Replace(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementPolicyError {
    MissingArmorFacts { slot: EquipmentSlot },
    MissingMainHandFacts,
    WrongFacts { slot: EquipmentSlot },
}

pub fn decide_replacement(
    slot: EquipmentSlot,
    current: CurrentItemFacts,
) -> Result<ReplacementDecision, ReplacementPolicyError> {
    let CurrentItemFacts::Occupied(comparison) = current else {
        return Ok(ReplacementDecision::Replace(ReplacementReason::EmptySlot));
    };

    if slot.is_armor() {
        return match comparison {
            ReplacementComparison::Armor(facts) => Ok(compare_armor(facts)),
            ReplacementComparison::NotApplicable => {
                Err(ReplacementPolicyError::MissingArmorFacts { slot })
            }
            ReplacementComparison::MainHand(_) => Err(ReplacementPolicyError::WrongFacts { slot }),
        };
    }

    if matches!(slot, EquipmentSlot::MainHand) {
        return match comparison {
            ReplacementComparison::MainHand(facts) => Ok(compare_weapon(facts)),
            ReplacementComparison::NotApplicable => {
                Err(ReplacementPolicyError::MissingMainHandFacts)
            }
            ReplacementComparison::Armor(_) => Err(ReplacementPolicyError::WrongFacts { slot }),
        };
    }

    Ok(ReplacementDecision::Keep(
        ReplacementReason::OccupiedSlotNotReplaceable,
    ))
}

fn compare_armor(facts: ArmorReplacementFacts) -> ReplacementDecision {
    if facts.current_prevents_armor_change {
        return ReplacementDecision::Keep(ReplacementReason::ArmorChangePrevented);
    }
    if facts.new_defense != facts.current_defense {
        return if facts.new_defense > facts.current_defense {
            ReplacementDecision::Replace(ReplacementReason::BetterDefense)
        } else {
            ReplacementDecision::Keep(ReplacementReason::WorseDefense)
        };
    }
    if facts.new_toughness != facts.current_toughness {
        return if facts.new_toughness > facts.current_toughness {
            ReplacementDecision::Replace(ReplacementReason::BetterToughness)
        } else {
            ReplacementDecision::Keep(ReplacementReason::WorseToughness)
        };
    }
    compare_equal_item(facts.equal_item)
}

fn compare_weapon(facts: WeaponReplacementFacts) -> ReplacementDecision {
    if let Some(preferred) = facts.preferred_weapon {
        if preferred.current_matches && !preferred.new_matches {
            return ReplacementDecision::Keep(ReplacementReason::LosesPreferredWeapon);
        }
        if !preferred.current_matches && preferred.new_matches {
            return ReplacementDecision::Replace(ReplacementReason::GainsPreferredWeapon);
        }
    }
    if facts.new_attack_damage != facts.current_attack_damage {
        return if facts.new_attack_damage > facts.current_attack_damage {
            ReplacementDecision::Replace(ReplacementReason::BetterAttackDamage)
        } else {
            ReplacementDecision::Keep(ReplacementReason::WorseAttackDamage)
        };
    }
    compare_equal_item(facts.equal_item)
}

fn compare_equal_item(facts: EqualItemFacts) -> ReplacementDecision {
    if facts.new_enchantment_entries != facts.current_enchantment_entries {
        return if facts.new_enchantment_entries > facts.current_enchantment_entries {
            ReplacementDecision::Replace(ReplacementReason::MoreEnchantments)
        } else {
            ReplacementDecision::Keep(ReplacementReason::FewerEnchantments)
        };
    }
    if facts.new_damage != facts.current_damage {
        return if facts.new_damage < facts.current_damage {
            ReplacementDecision::Replace(ReplacementReason::LessDamaged)
        } else {
            ReplacementDecision::Keep(ReplacementReason::MoreDamaged)
        };
    }
    if facts.new_has_custom_name && !facts.current_has_custom_name {
        ReplacementDecision::Replace(ReplacementReason::NewCustomName)
    } else {
        ReplacementDecision::Keep(ReplacementReason::EqualItem)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipGameEvent {
    Equip,
    Unequip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipTransitionContext {
    pub server_side: bool,
    pub spectator: bool,
    pub first_tick: bool,
    pub silent: bool,
    pub emits_equip_event: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipTransitionFacts {
    pub play_equip_sound: bool,
    pub game_event: Option<EquipGameEvent>,
}

impl EquipTransitionFacts {
    pub const NONE: Self = Self {
        play_equip_sound: false,
        game_event: None,
    };
}

pub fn plan_equip_transition(
    slot: EquipmentSlot,
    previous: ItemStackState,
    current: ItemStackState,
    current_equippable_slot: Option<EquipmentSlot>,
    context: EquipTransitionContext,
) -> EquipTransitionFacts {
    if !context.server_side
        || context.spectator
        || context.first_tick
        || previous.same_item_same_components(&current)
    {
        return EquipTransitionFacts::NONE;
    }

    let play_equip_sound = !context.silent
        && match current_equippable_slot {
            Some(equippable_slot) => slot.ordinal() == equippable_slot.ordinal(),
            None => false,
        };
    let game_event = if context.emits_equip_event {
        Some(if current_equippable_slot.is_some() {
            EquipGameEvent::Equip
        } else {
            EquipGameEvent::Unequip
        })
    } else {
        None
    };

    EquipTransitionFacts {
        play_equip_sound,
        game_event,
    }
}
