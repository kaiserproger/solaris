use std::collections::HashSet;
use std::sync::Arc;

use uuid::Uuid;

use crate::{
    AnimalBreedingState, EntityBlazeAttackState, EntityBowAttackState, EntityBreezeAttackState,
    EntityCrossbowAttackState, EntityEvokerAttackState, EntityGhastAttackState,
    EntityGuardianBeamState, EntityId, EntityLifecycle, EntityRuntime, EntityShulkerAttackState,
    EntityStore, EntityWardenSonicBoomState, EntityWitchAttackState, GoalState, Rotation, Vec3,
    VillagerData, projectile_26_1_2, villager_26_1_2,
};

#[derive(Debug, Clone, PartialEq)]
pub struct EntitySimulationProjection {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_name: Arc<str>,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub lifecycle: EntityLifecycle,
    pub last_damage_tick: Option<u64>,
    pub follow_range: f64,
    pub attack_damage: f64,
    pub goal: GoalState,
    pub primed_tnt: bool,
    pub guardian_beam_active: bool,
    pub has_item_stack: bool,
    pub has_experience_value: bool,
    pub has_block_state: bool,
    pub has_vehicle: bool,
    pub animal: Option<AnimalBreedingState>,
    pub fall_distance: f64,
    pub arrow_revision: Option<u64>,
    pub arrow_embedded_block: Option<projectile_26_1_2::BlockPosition>,
    pub hurting_projectile_revision: Option<u64>,
    pub hurting_projectile_acceleration_power_bits: Option<u64>,
    pub hurting_projectile_air_inertia_bits: Option<u64>,
    pub hurting_projectile_water_inertia_bits: Option<u64>,
    pub throwable_projectile_revision: Option<u64>,
    pub shulker_bullet_target_entity_id: Option<i32>,
    pub sheep_grazing_ticks: Option<u8>,
    pub crossbow_attack: Option<EntityCrossbowAttackState>,
    pub bow_attack: Option<EntityBowAttackState>,
    pub blaze_attack: Option<EntityBlazeAttackState>,
    pub ghast_attack: Option<EntityGhastAttackState>,
    pub breeze_attack: Option<EntityBreezeAttackState>,
    pub guardian_beam: Option<EntityGuardianBeamState>,
    pub warden_sonic_boom: Option<EntityWardenSonicBoomState>,
    pub shulker_attack: Option<EntityShulkerAttackState>,
    pub evoker_attack: Option<EntityEvokerAttackState>,
    pub witch_attack: Option<EntityWitchAttackState>,
    pub villager: Option<VillagerData>,
    pub villager_job_site: Option<Vec3>,
    pub villager_schedule: Option<villager_26_1_2::VillagerScheduleKind>,
    pub villager_last_slept_tick: Option<u64>,
    pub villager_golem_detected_until_tick: Option<u64>,
    pub villager_override_expires_tick: Option<u64>,
    pub villager_override_order_present: bool,
}

/// Authoritative inputs for natural despawn decisions and their conditional-removal fence.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDespawnProjection {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_name: Arc<str>,
    pub position: Vec3,
    pub lifecycle: EntityLifecycle,
    pub last_damage_tick: Option<u64>,
}

pub(crate) trait EntityProjection: Sized {
    fn read(runtime: &EntityRuntime, id: EntityId) -> Option<Self>;
    fn id(&self) -> EntityId;
    fn position(&self) -> Vec3;
}

impl EntityProjection for EntitySimulationProjection {
    fn read(runtime: &EntityRuntime, id: EntityId) -> Option<Self> {
        runtime.simulation_projection(id)
    }

    fn id(&self) -> EntityId {
        self.id
    }

    fn position(&self) -> Vec3 {
        self.position
    }
}

impl EntityProjection for EntityDespawnProjection {
    fn read(runtime: &EntityRuntime, id: EntityId) -> Option<Self> {
        runtime.despawn_projection(id)
    }

    fn id(&self) -> EntityId {
        self.id
    }

    fn position(&self) -> Vec3 {
        self.position
    }
}

impl EntityStore {
    pub(crate) fn projections_for_ids<P: EntityProjection>(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Vec<P> {
        let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
        ordered_ids.sort_unstable();
        ordered_ids
            .into_iter()
            .filter_map(|id| P::read(&self.runtime, id))
            .collect()
    }
}
