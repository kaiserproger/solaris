use super::{
    AiGoalState, AnimalState, EntityRuntime, EntityTypeState, ExperienceState, FallingBlockState,
    GameplayDecisionState, ItemStackState, LifecycleState, LivingState, MotionState,
    RuntimeEntityIndex, StableIdentity, TransformState, VehicleKindState, villager_job_site,
};
use crate::{EntityDespawnProjection, EntityId, EntitySimulationProjection};

impl EntityRuntime {
    pub(crate) fn simulation_projection(&self, id: EntityId) -> Option<EntitySimulationProjection> {
        let world = &self.world;
        let ecs_entity = *world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = world.get_entity(ecs_entity).ok()?;
        let identity = entity.get::<StableIdentity>()?;
        let entity_type = entity.get::<EntityTypeState>()?;
        let transform = entity.get::<TransformState>()?;
        let motion = entity.get::<MotionState>()?;
        let lifecycle = entity.get::<LifecycleState>()?;
        let living = entity.get::<LivingState>()?;
        let goal = entity.get::<AiGoalState>()?;
        let gameplay = entity.get::<GameplayDecisionState>()?;
        let arrow_state = gameplay.arrow_state;
        let hurting_projectile_state = gameplay.hurting_projectile_state;
        let throwable_projectile_state = gameplay.throwable_projectile_state;
        let villager_brain = gameplay.villager_brain.as_ref();

        Some(EntitySimulationProjection {
            id: identity.id,
            uuid: identity.uuid,
            type_name: entity_type.name.clone(),
            position: transform.position,
            rotation: transform.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
            lifecycle: lifecycle.0,
            last_damage_tick: gameplay.last_damage_tick,
            follow_range: living
                .attributes
                .base(&crate::AttributeKind::FollowRange)
                .unwrap_or(16.0),
            attack_damage: living
                .attributes
                .base(&crate::AttributeKind::AttackDamage)
                .unwrap_or(0.0),
            goal: goal.0.clone(),
            primed_tnt: gameplay.primed_tnt.is_some(),
            guardian_beam_active: gameplay.guardian_beam.is_some(),
            has_item_stack: entity.get::<ItemStackState>().is_some(),
            has_experience_value: entity.get::<ExperienceState>().is_some(),
            has_block_state: entity.get::<FallingBlockState>().is_some(),
            has_vehicle: entity.get::<VehicleKindState>().is_some(),
            animal: entity.get::<AnimalState>().map(|state| state.0),
            fall_distance: motion.fall_distance,
            arrow_revision: arrow_state.map(|state| state.projectile.revision),
            arrow_embedded_block: arrow_state
                .filter(|state| state.in_ground)
                .and_then(|state| state.last_block_position),
            hurting_projectile_revision: hurting_projectile_state
                .map(|state| state.projectile.revision),
            hurting_projectile_acceleration_power_bits: hurting_projectile_state
                .map(|state| state.acceleration_power.to_bits()),
            hurting_projectile_air_inertia_bits: hurting_projectile_state
                .map(|state| state.air_inertia.to_bits()),
            hurting_projectile_water_inertia_bits: hurting_projectile_state
                .map(|state| state.water_inertia.to_bits()),
            throwable_projectile_revision: throwable_projectile_state
                .map(|state| state.projectile.revision),
            shulker_bullet_target_entity_id: gameplay
                .shulker_bullet
                .map(|state| state.target_entity_id),

            sheep_grazing_ticks: gameplay.sheep_grazing_ticks,
            crossbow_attack: gameplay.crossbow_attack,
            blaze_attack: gameplay.blaze_attack,
            ghast_attack: gameplay.ghast_attack,
            breeze_attack: gameplay.breeze_attack,
            guardian_beam: gameplay.guardian_beam,
            warden_sonic_boom: gameplay.warden_sonic_boom,
            shulker_attack: gameplay.shulker_attack,
            evoker_attack: gameplay.evoker_attack,
            witch_attack: gameplay.witch_attack,
            villager: gameplay.villager,
            villager_schedule: villager_brain.map(|brain| brain.schedule),
            villager_last_slept_tick: villager_brain.and_then(|brain| brain.last_slept_tick),
            villager_golem_detected_until_tick: villager_brain
                .and_then(|brain| brain.golem_detected_until_tick),
            villager_job_site: villager_job_site(gameplay, transform.position),
            villager_override_expires_tick: villager_brain
                .and_then(|brain| brain.override_expires_tick),
            villager_override_order_present: villager_brain
                .is_some_and(|brain| brain.override_order.is_some()),
        })
    }

    pub(crate) fn despawn_projection(&self, id: EntityId) -> Option<EntityDespawnProjection> {
        let ecs_entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = self.world.get_entity(ecs_entity).ok()?;
        let identity = entity.get::<StableIdentity>()?;
        let entity_type = entity.get::<EntityTypeState>()?;
        let transform = entity.get::<TransformState>()?;
        let lifecycle = entity.get::<LifecycleState>()?;
        let gameplay = entity.get::<GameplayDecisionState>()?;

        Some(EntityDespawnProjection {
            id: identity.id,
            uuid: identity.uuid,
            type_name: entity_type.name.clone(),
            position: transform.position,
            lifecycle: lifecycle.0,
            last_damage_tick: gameplay.last_damage_tick,
        })
    }
}
