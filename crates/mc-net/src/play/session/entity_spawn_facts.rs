use mc_entity::SpawnEntity;

pub(in crate::play::session) fn apply_entity_facts(entity: &mut SpawnEntity) {
    mc_entity::natural_spawn_26_1_2::apply_entity_facts(entity);
}
