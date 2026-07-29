//! Bounded Java Edition 26.1.2 zombie-villager conversion state.
//!
//! The session/simulation owner remains responsible for player inventory,
//! reach, registry resolution and wire publication. This module owns only the
//! retained conversion state and the deterministic snapshot transitions.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::effects_26_1_2::{
    ActiveEffects, ActiveEffectsSnapshot, EffectFlags, EffectId, EffectInstance, EffectKind,
    EffectLimits, MAX_ACTIVE_EFFECTS, MAX_HIDDEN_EFFECTS,
};
use crate::villager_26_1_2::{VillagerBrainState, VillagerPoiSet};
use crate::villager_gossip_26_1_2::{VillagerGossipEvent, VillagerGossipState};
use crate::{
    EntityActiveEffectsState, EntityLifecycle, EntitySnapshot, GoalState, VillagerData,
    VillagerKind, VillagerProfession,
};

pub const CONVERSION_WAIT_MIN_TICKS: u32 = 3_600;
pub const CONVERSION_WAIT_MAX_TICKS: u32 = 6_000;
pub const CONVERSION_WAIT_RANGE: u64 =
    (CONVERSION_WAIT_MAX_TICKS - CONVERSION_WAIT_MIN_TICKS + 1) as u64;

// BuiltInRegistries.MOB_EFFECT raw ids in 26.1.2 are the historical one-based ids.
pub const STRENGTH_EFFECT_ID: EffectId = EffectId::new(5);
pub const NAUSEA_EFFECT_ID: EffectId = EffectId::new(9);
pub const WEAKNESS_EFFECT_ID: EffectId = EffectId::new(18);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZombieVillagerConversionState {
    pub started_by: Option<Uuid>,
    pub completes_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZombieVillagerConversionError {
    NotAlive,
    NotZombieVillager,
    AlreadyConverting,
    MissingWeakness,
    InvalidDuration,
    InvalidEffects,
    InvalidVillagerType,
}

#[must_use]
pub fn conversion_duration_from_seed(seed: u64) -> u32 {
    CONVERSION_WAIT_MIN_TICKS
        + u32::try_from(seed % CONVERSION_WAIT_RANGE).expect("conversion range fits u32")
}

pub fn start_conversion(
    snapshot: &EntitySnapshot,
    started_by: Option<Uuid>,
    current_tick: u64,
    duration_ticks: u32,
) -> Result<EntitySnapshot, ZombieVillagerConversionError> {
    if snapshot.lifecycle != EntityLifecycle::Alive {
        return Err(ZombieVillagerConversionError::NotAlive);
    }
    if snapshot.type_name != "minecraft:zombie_villager" {
        return Err(ZombieVillagerConversionError::NotZombieVillager);
    }
    if snapshot.retained.zombie_villager_conversion.is_some() {
        return Err(ZombieVillagerConversionError::AlreadyConverting);
    }
    if !(CONVERSION_WAIT_MIN_TICKS..=CONVERSION_WAIT_MAX_TICKS).contains(&duration_ticks) {
        return Err(ZombieVillagerConversionError::InvalidDuration);
    }

    let retained_effects = snapshot
        .retained
        .active_effects
        .as_ref()
        .ok_or(ZombieVillagerConversionError::MissingWeakness)?;
    let mut effects = effects_from_snapshot(&retained_effects.effects)?;
    if effects.get(WEAKNESS_EFFECT_ID).is_none() {
        return Err(ZombieVillagerConversionError::MissingWeakness);
    }
    effects.remove(WEAKNESS_EFFECT_ID);
    effects
        .add(EffectInstance::new(
            STRENGTH_EFFECT_ID,
            EffectKind::CallerOwned,
            i32::try_from(duration_ticks).expect("conversion duration fits i32"),
            0,
            EffectFlags::default(),
        ))
        .map_err(|_| ZombieVillagerConversionError::InvalidEffects)?;

    let mut action_order = retained_effects.action_order.clone();
    action_order.retain(|id| *id != WEAKNESS_EFFECT_ID);
    if !action_order.contains(&STRENGTH_EFFECT_ID) {
        action_order.push(STRENGTH_EFFECT_ID);
    }

    let mut next = snapshot.clone();
    next.retained.active_effects = Some(EntityActiveEffectsState {
        effects: effects.snapshot(),
        action_order,
    });
    next.retained.zombie_villager_conversion = Some(ZombieVillagerConversionState {
        started_by,
        completes_tick: current_tick.saturating_add(u64::from(duration_ticks)),
    });
    Ok(next)
}

pub fn finish_conversion(
    snapshot: &EntitySnapshot,
    current_tick: u64,
    villager_type_id: i32,
) -> Result<Option<EntitySnapshot>, ZombieVillagerConversionError> {
    let Some(conversion) = snapshot.retained.zombie_villager_conversion else {
        return Ok(None);
    };
    if conversion.completes_tick > current_tick {
        return Ok(None);
    }
    if snapshot.lifecycle != EntityLifecycle::Alive {
        return Err(ZombieVillagerConversionError::NotAlive);
    }
    if snapshot.type_name != "minecraft:zombie_villager" {
        return Err(ZombieVillagerConversionError::NotZombieVillager);
    }
    if villager_type_id < 0 {
        return Err(ZombieVillagerConversionError::InvalidVillagerType);
    }

    let empty_effects = ActiveEffectsSnapshot::default();
    let retained_effects = snapshot.retained.active_effects.as_ref();
    let mut effects = effects_from_snapshot(
        retained_effects
            .map(|state| &state.effects)
            .unwrap_or(&empty_effects),
    )?;
    effects
        .add(EffectInstance::new(
            NAUSEA_EFFECT_ID,
            EffectKind::CallerOwned,
            200,
            0,
            EffectFlags::default(),
        ))
        .map_err(|_| ZombieVillagerConversionError::InvalidEffects)?;
    let mut action_order = retained_effects
        .map(|state| state.action_order.clone())
        .unwrap_or_default();
    action_order.retain(|id| *id != NAUSEA_EFFECT_ID);
    action_order.push(NAUSEA_EFFECT_ID);

    let mut next = snapshot.clone();
    next.type_id = villager_type_id;
    next.type_name = "minecraft:villager".to_owned();
    next.goal = GoalState::Idle;
    next.animal = None;
    next.retained.zombie_villager_conversion = None;
    next.retained.active_effects = Some(EntityActiveEffectsState {
        effects: effects.snapshot(),
        action_order,
    });
    next.retained.villager.get_or_insert_with(|| {
        VillagerData::new(VillagerKind::Plains, VillagerProfession::None, 1)
    });
    next.retained
        .villager_brain
        .get_or_insert_with(|| VillagerBrainState::adult(VillagerPoiSet::default()));
    let gossip = next
        .retained
        .villager_gossip
        .get_or_insert_with(VillagerGossipState::default);
    if let Some(player) = conversion.started_by {
        gossip.record_event(VillagerGossipEvent::ZombieVillagerCured { player });
    }
    Ok(Some(next))
}

fn effects_from_snapshot(
    snapshot: &ActiveEffectsSnapshot,
) -> Result<ActiveEffects, ZombieVillagerConversionError> {
    let limits = EffectLimits::new(MAX_ACTIVE_EFFECTS, MAX_HIDDEN_EFFECTS)
        .expect("published effect limits are valid");
    ActiveEffects::try_from_snapshot(limits, snapshot)
        .map_err(|_| ZombieVillagerConversionError::InvalidEffects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects_26_1_2::{ActiveEffectChainSnapshot, ActiveEffectsSnapshot};
    use crate::{AttributeSet, EntityId, EntityRetainedState, Rotation, Vec3};

    fn zombie() -> EntitySnapshot {
        let weakness = EffectInstance::new(
            WEAKNESS_EFFECT_ID,
            EffectKind::CallerOwned,
            600,
            0,
            EffectFlags::default(),
        );
        EntitySnapshot {
            id: EntityId(7),
            uuid: Uuid::from_u128(7),
            type_id: 153,
            type_name: "minecraft:zombie_villager".to_owned(),
            position: Vec3::new(1.0, 64.0, 2.0),
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: EntityRetainedState {
                active_effects: Some(EntityActiveEffectsState {
                    effects: ActiveEffectsSnapshot {
                        chains: vec![ActiveEffectChainSnapshot {
                            current: weakness,
                            hidden: Vec::new(),
                        }],
                    },
                    action_order: vec![WEAKNESS_EFFECT_ID],
                }),
                ..EntityRetainedState::default()
            },
        }
    }

    #[test]
    fn cure_start_requires_weakness_and_replaces_it_with_strength() {
        let player = Uuid::from_u128(99);
        let next = start_conversion(&zombie(), Some(player), 100, 3_600).unwrap();
        let conversion = next.retained.zombie_villager_conversion.unwrap();
        assert_eq!(conversion.started_by, Some(player));
        assert_eq!(conversion.completes_tick, 3_700);
        assert_eq!(
            next.attributes,
            zombie().attributes,
            "effect application must not rewrite base attributes"
        );
        let effects = next.retained.active_effects.unwrap();
        assert_eq!(effects.action_order, [STRENGTH_EFFECT_ID]);
        assert_eq!(effects.effects.chains[0].current.id, STRENGTH_EFFECT_ID);
        assert_eq!(effects.effects.chains[0].current.duration, 3_600);
    }

    #[test]
    fn preexisting_strength_is_not_owned_or_removed_by_cure() {
        let mut preexisting = zombie();
        let strength = EffectInstance::new(
            STRENGTH_EFFECT_ID,
            EffectKind::CallerOwned,
            200,
            1,
            EffectFlags::default(),
        );
        let effects = preexisting.retained.active_effects.as_mut().unwrap();
        effects.effects.chains.insert(
            0,
            ActiveEffectChainSnapshot {
                current: strength,
                hidden: Vec::new(),
            },
        );
        effects.action_order.insert(0, STRENGTH_EFFECT_ID);

        let converting = start_conversion(&preexisting, Some(Uuid::from_u128(99)), 100, 3_600)
            .expect("start conversion with preexisting strength");
        assert_eq!(converting.attributes, preexisting.attributes);
        let cured = finish_conversion(&converting, 3_700, 110)
            .unwrap()
            .expect("conversion due");
        let effects = cured.retained.active_effects.unwrap();
        assert_eq!(effects.action_order, [STRENGTH_EFFECT_ID, NAUSEA_EFFECT_ID]);
        assert!(
            effects
                .effects
                .chains
                .iter()
                .any(|chain| chain.current.id == STRENGTH_EFFECT_ID)
        );
    }

    #[test]
    fn completion_is_exact_once_and_records_positive_gossip() {
        let player = Uuid::from_u128(99);
        let converting = start_conversion(&zombie(), Some(player), 100, 3_600).unwrap();
        assert!(
            finish_conversion(&converting, 3_699, 110)
                .unwrap()
                .is_none()
        );
        let cured = finish_conversion(&converting, 3_700, 110)
            .unwrap()
            .expect("conversion due");
        assert_eq!(cured.type_name, "minecraft:villager");
        assert_eq!(cured.type_id, 110);
        assert!(cured.retained.zombie_villager_conversion.is_none());
        let effects = cured.retained.active_effects.as_ref().unwrap();
        assert_eq!(effects.action_order, [STRENGTH_EFFECT_ID, NAUSEA_EFFECT_ID]);
        assert!(
            effects
                .effects
                .chains
                .iter()
                .any(|chain| chain.current.id == STRENGTH_EFFECT_ID)
        );
        assert!(
            effects
                .effects
                .chains
                .iter()
                .any(|chain| chain.current.id == NAUSEA_EFFECT_ID)
        );
        let gossip = cured.retained.villager_gossip.as_ref().unwrap();
        assert_eq!(gossip.player_reputation(player), 125);
        assert!(finish_conversion(&cured, 3_701, 110).unwrap().is_none());
    }

    #[test]
    fn duration_mapping_covers_the_exact_closed_range() {
        assert_eq!(conversion_duration_from_seed(0), 3_600);
        assert_eq!(conversion_duration_from_seed(2_400), 6_000);
        assert_eq!(conversion_duration_from_seed(2_401), 3_600);
    }
}
