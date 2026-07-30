//! Data-driven common mob behavior profiles for Java Edition 26.1.2.
//!
//! This table deliberately separates common movement/attack families from
//! species-specific behavior that Solaris has not implemented yet. Consumers
//! must not silently run `UnsupportedSpecial` mobs through generic melee logic.

use std::collections::BTreeMap;

use crate::Identifier;
use crate::entity_contract_26_1_2::{
    EntityArchetype, MobCategory, PhysicalSimulationClass, entity_type_contracts_26_1_2,
};

const MAX_SPEED: f64 = 8.0;
const MAX_PERIOD_TICKS: u32 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMovementPolicy {
    Immobile,
    GroundWander,
    FlyingWander,
    AquaticWander,
    AmphibiousWander,
    HostilePursuit,
    VillagerSchedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobCombatPolicy {
    None,
    Melee,
    Arrow,
    Crossbow,
    GuardianBeam,
    CreeperFuse,
    UnsupportedSpecial,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MobBehaviorProfile {
    pub movement: MobMovementPolicy,
    pub combat: MobCombatPolicy,
    pub wander_speed: f64,
    pub pursuit_speed: f64,
    pub wander_period_ticks: u32,
    /// Stable diagnostic key for unsupported species-specific attacks.
    pub special_attack: Option<&'static str>,
}

impl MobBehaviorProfile {
    pub fn validate(&self) -> Result<(), MobBehaviorError> {
        for speed in [self.wander_speed, self.pursuit_speed] {
            if !speed.is_finite() || !(0.0..=MAX_SPEED).contains(&speed) {
                return Err(MobBehaviorError::InvalidSpeed);
            }
        }
        if self.wander_period_ticks == 0 || self.wander_period_ticks > MAX_PERIOD_TICKS {
            return Err(MobBehaviorError::InvalidPeriod(self.wander_period_ticks));
        }
        if self.combat == MobCombatPolicy::UnsupportedSpecial && self.special_attack.is_none() {
            return Err(MobBehaviorError::MissingSpecialAttack);
        }
        if self.combat != MobCombatPolicy::UnsupportedSpecial && self.special_attack.is_some() {
            return Err(MobBehaviorError::UnexpectedSpecialAttack);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MobBehaviorTable {
    profiles: BTreeMap<Identifier, MobBehaviorProfile>,
}

impl MobBehaviorTable {
    #[must_use]
    pub fn vanilla_26_1_2() -> Self {
        let profiles = entity_type_contracts_26_1_2()
            .filter(|contract| {
                contract.behavior.archetype == EntityArchetype::Living
                    && contract.name != "minecraft:player"
            })
            .map(|contract| {
                let id = Identifier::parse(contract.name).expect("canonical entity identifier");
                let hostile = contract.category == MobCategory::Monster;
                let movement = if matches!(
                    contract.name,
                    "minecraft:armor_stand" | "minecraft:mannequin"
                ) {
                    MobMovementPolicy::Immobile
                } else if contract.name == "minecraft:villager" {
                    MobMovementPolicy::VillagerSchedule
                } else {
                    match contract.behavior.physical_simulation {
                        PhysicalSimulationClass::Immobile
                        | PhysicalSimulationClass::LivingAttached => MobMovementPolicy::Immobile,
                        PhysicalSimulationClass::LivingAquatic => MobMovementPolicy::AquaticWander,
                        PhysicalSimulationClass::LivingAmphibious => {
                            MobMovementPolicy::AmphibiousWander
                        }
                        PhysicalSimulationClass::LivingFlying => MobMovementPolicy::FlyingWander,
                        PhysicalSimulationClass::LivingGround if hostile => {
                            MobMovementPolicy::HostilePursuit
                        }
                        PhysicalSimulationClass::LivingGround => MobMovementPolicy::GroundWander,
                        _ => MobMovementPolicy::Immobile,
                    }
                };
                let (combat, special_attack) = combat_policy(contract.name, hostile);
                let profile =
                    MobBehaviorProfile {
                        movement,
                        combat,
                        wander_speed: match movement {
                            MobMovementPolicy::Immobile => 0.0,
                            MobMovementPolicy::GroundWander
                            | MobMovementPolicy::FlyingWander
                            | MobMovementPolicy::VillagerSchedule => 0.0,
                            MobMovementPolicy::AquaticWander
                            | MobMovementPolicy::AmphibiousWander => 0.8,
                            MobMovementPolicy::HostilePursuit => 1.25,
                        },
                        pursuit_speed: if hostile { 1.25 } else { 0.8 },
                        wander_period_ticks: match movement {
                            MobMovementPolicy::AquaticWander
                            | MobMovementPolicy::AmphibiousWander => 45,
                            MobMovementPolicy::HostilePursuit => 20,
                            _ => 80,
                        },
                        special_attack,
                    };
                debug_assert!(profile.validate().is_ok());
                (id, profile)
            })
            .collect();
        Self { profiles }
    }

    #[must_use]
    pub fn get(&self, entity_type: &Identifier) -> Option<&MobBehaviorProfile> {
        self.profiles.get(entity_type)
    }

    #[must_use]
    pub fn get_by_name(&self, entity_type: &str) -> Option<&MobBehaviorProfile> {
        Identifier::parse(entity_type)
            .ok()
            .and_then(|id| self.get(&id))
    }

    pub fn insert_override(
        &mut self,
        entity_type: Identifier,
        profile: MobBehaviorProfile,
    ) -> Result<Option<MobBehaviorProfile>, MobBehaviorError> {
        profile.validate()?;
        if !self.profiles.contains_key(&entity_type) {
            return Err(MobBehaviorError::UnknownEntityType(entity_type));
        }
        Ok(self.profiles.insert(entity_type, profile))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub fn validate(&self) -> Result<(), MobBehaviorError> {
        for profile in self.profiles.values() {
            profile.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MobBehaviorError {
    InvalidSpeed,
    InvalidPeriod(u32),
    MissingSpecialAttack,
    UnexpectedSpecialAttack,
    UnknownEntityType(Identifier),
}

fn combat_policy(name: &'static str, hostile: bool) -> (MobCombatPolicy, Option<&'static str>) {
    if !hostile {
        return (MobCombatPolicy::None, None);
    }
    match name {
        "minecraft:creeper" => (MobCombatPolicy::CreeperFuse, None),
        "minecraft:skeleton" | "minecraft:stray" | "minecraft:bogged" => {
            (MobCombatPolicy::Arrow, None)
        }
        "minecraft:blaze" => (MobCombatPolicy::UnsupportedSpecial, Some("small_fireball")),
        "minecraft:breeze" => (MobCombatPolicy::UnsupportedSpecial, Some("wind_charge")),
        "minecraft:elder_guardian" | "minecraft:guardian" => (MobCombatPolicy::GuardianBeam, None),
        "minecraft:ender_dragon" => (MobCombatPolicy::UnsupportedSpecial, Some("dragon_boss")),
        "minecraft:evoker" => (MobCombatPolicy::UnsupportedSpecial, Some("evoker_spell")),
        "minecraft:ghast" => (MobCombatPolicy::UnsupportedSpecial, Some("large_fireball")),
        "minecraft:pillager" => (MobCombatPolicy::Crossbow, None),
        "minecraft:shulker" => (MobCombatPolicy::UnsupportedSpecial, Some("shulker_bullet")),
        "minecraft:warden" => (MobCombatPolicy::UnsupportedSpecial, Some("sonic_boom")),
        "minecraft:witch" => (MobCombatPolicy::UnsupportedSpecial, Some("thrown_potion")),
        "minecraft:wither" => (MobCombatPolicy::UnsupportedSpecial, Some("wither_boss")),
        _ => (MobCombatPolicy::Melee, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_non_player_living_type_has_one_valid_profile() {
        let table = MobBehaviorTable::vanilla_26_1_2();
        let expected = entity_type_contracts_26_1_2()
            .filter(|contract| {
                contract.behavior.archetype == EntityArchetype::Living
                    && contract.name != "minecraft:player"
            })
            .count();
        assert_eq!(table.len(), expected);
        table.validate().unwrap();
        assert_eq!(
            table.get_by_name("minecraft:cow").unwrap().movement,
            MobMovementPolicy::GroundWander
        );
        assert_eq!(
            table.get_by_name("minecraft:cod").unwrap().movement,
            MobMovementPolicy::AquaticWander
        );
        assert_eq!(
            table.get_by_name("minecraft:zombie").unwrap().combat,
            MobCombatPolicy::Melee
        );
        assert_eq!(
            table.get_by_name("minecraft:skeleton").unwrap().combat,
            MobCombatPolicy::Arrow
        );
        assert_eq!(
            table.get_by_name("minecraft:creeper").unwrap().combat,
            MobCombatPolicy::CreeperFuse
        );
        assert_eq!(
            table.get_by_name("minecraft:pillager").unwrap().combat,
            MobCombatPolicy::Crossbow
        );
        assert_eq!(
            table
                .get_by_name("minecraft:pillager")
                .unwrap()
                .special_attack,
            None
        );
        assert_eq!(
            table.get_by_name("minecraft:villager").unwrap().movement,
            MobMovementPolicy::VillagerSchedule
        );
        assert_eq!(
            table.get_by_name("minecraft:armor_stand").unwrap().movement,
            MobMovementPolicy::Immobile
        );
        assert_eq!(
            table.get_by_name("minecraft:mannequin").unwrap().movement,
            MobMovementPolicy::Immobile
        );
        for guardian in ["minecraft:guardian", "minecraft:elder_guardian"] {
            let profile = table.get_by_name(guardian).unwrap();
            assert_eq!(profile.combat, MobCombatPolicy::GuardianBeam);
            assert_eq!(profile.special_attack, None);
        }
        assert!(table.get_by_name("minecraft:player").is_none());
    }

    #[test]
    fn override_is_bounded_and_unknown_types_fail_closed() {
        let mut table = MobBehaviorTable::vanilla_26_1_2();
        let cow = Identifier::parse("minecraft:cow").unwrap();
        let mut profile = table.get(&cow).unwrap().clone();
        profile.wander_speed = 0.5;
        assert!(table.insert_override(cow, profile).unwrap().is_some());

        let unknown = Identifier::parse("example:mob").unwrap();
        let profile = MobBehaviorProfile {
            movement: MobMovementPolicy::GroundWander,
            combat: MobCombatPolicy::None,
            wander_speed: 0.5,
            pursuit_speed: 0.5,
            wander_period_ticks: 80,
            special_attack: None,
        };
        assert_eq!(
            table.insert_override(unknown.clone(), profile),
            Err(MobBehaviorError::UnknownEntityType(unknown))
        );
    }
}
