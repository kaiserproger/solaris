use mc_entity::{AttributeKind, GoalState, SpawnEntity};

use crate::play::{HOSTILE_WANDER_SPEED, PASSIVE_WANDER_SPEED};

use super::entity_physics_class::entity_type_uses_aquatic_physics;

pub(super) fn apply_default_mob_goal(entity: &mut SpawnEntity, hostile: bool) {
    entity.goal = if entity_type_uses_aquatic_physics(&entity.type_name) {
        entity.on_ground = false;
        GoalState::AquaticWander {
            speed: PASSIVE_WANDER_SPEED * 0.9,
            vertical_speed: 0.18,
            period_ticks: 45,
        }
    } else if hostile {
        GoalState::Wander {
            speed: HOSTILE_WANDER_SPEED,
            period_ticks: 20,
        }
    } else {
        GoalState::Wander {
            speed: passive_ground_wander_speed(entity),
            period_ticks: 80,
        }
    };
}

pub(super) fn passive_ground_wander_speed(entity: &SpawnEntity) -> f64 {
    entity
        .attributes
        .base(&AttributeKind::MovementSpeed)
        .unwrap_or(0.2)
        * 10.0
}
