use mc_data::mob_behavior_26_1_2::{MobBehaviorTable, MobMovementPolicy};
use mc_entity::{AttributeKind, GoalState, SpawnEntity};

pub(super) fn apply_default_mob_goal(entity: &mut SpawnEntity, behaviors: &MobBehaviorTable) {
    let Some(profile) = behaviors.get_by_name(&entity.type_name) else {
        entity.goal = GoalState::Idle;
        return;
    };
    entity.goal = match profile.movement {
        MobMovementPolicy::Immobile => GoalState::Idle,
        MobMovementPolicy::AquaticWander | MobMovementPolicy::AmphibiousWander => {
            entity.on_ground = false;
            GoalState::AquaticWander {
                speed: profile.wander_speed * 0.9,
                vertical_speed: 0.18,
                period_ticks: profile.wander_period_ticks.max(20),
            }
        }
        MobMovementPolicy::HostilePursuit => GoalState::Wander {
            speed: profile.wander_speed,
            period_ticks: profile.wander_period_ticks,
        },
        MobMovementPolicy::GroundWander
        | MobMovementPolicy::FlyingWander
        | MobMovementPolicy::VillagerSchedule => GoalState::Wander {
            speed: if profile.wander_speed > 0.0 {
                profile.wander_speed
            } else {
                passive_ground_wander_speed(entity)
            },
            period_ticks: profile.wander_period_ticks,
        },
    };
}

pub(super) fn passive_ground_wander_speed(entity: &SpawnEntity) -> f64 {
    entity
        .attributes
        .base(&AttributeKind::MovementSpeed)
        .unwrap_or(0.2)
        * 10.0
}
