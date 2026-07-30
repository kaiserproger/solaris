use mc_data::mob_behavior_26_1_2::MobBehaviorTable;
use mc_entity::SpawnEntity;

pub(super) fn apply_default_mob_goal(entity: &mut SpawnEntity, behaviors: &MobBehaviorTable) {
    mc_entity::natural_spawn_26_1_2::apply_default_mob_goal(entity, behaviors);
}

pub(super) fn passive_ground_wander_speed(entity: &SpawnEntity) -> f64 {
    mc_entity::natural_spawn_26_1_2::passive_ground_wander_speed(entity)
}
