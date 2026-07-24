use crate::play::persistence::{PersistedEntityCheckpoint, PersistedEntityRecord};
use mc_entity::{EntitySnapshot, RegionPhase, RegionalOwnerSaveSnapshot};

pub(super) struct EntityPersistenceMetadata {
    pub(super) lifecycle_tick: u64,
}

pub(super) fn project_owner_save(
    saved: RegionalOwnerSaveSnapshot,
    metadata: &EntityPersistenceMetadata,
) -> (PersistedEntityCheckpoint, Vec<RegionPhase>) {
    let journal_phases = saved.journal_phases().to_vec();
    let lifecycle_tick = saved.lifecycle_epoch();
    let regional_sequence_watermark = saved.sequence_watermark();
    debug_assert_eq!(metadata.lifecycle_tick, lifecycle_tick);
    let owner_metadata = EntityPersistenceMetadata { lifecycle_tick };
    let records = project_snapshots(saved.into_snapshots(), &owner_metadata);
    (
        PersistedEntityCheckpoint {
            lifecycle_clock: lifecycle_tick,
            regional_sequence_watermark,
            records,
            settlement_claims: Default::default(),
        },
        journal_phases,
    )
}

fn project_snapshots(
    snapshots: Vec<EntitySnapshot>,
    metadata: &EntityPersistenceMetadata,
) -> Vec<PersistedEntityRecord> {
    snapshots
        .into_iter()
        .map(|mut entity| {
            if entity.type_name == "minecraft:creeper" {
                entity.retained.primed_tnt = None;
            }
            PersistedEntityRecord::from_snapshot_at_lifecycle_clock(entity, metadata.lifecycle_tick)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mc_entity::{
        AttributeSet, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, EntityStore,
        GoalState, Rotation, SpawnEntity, Vec3,
    };
    use uuid::Uuid;

    use super::{EntityPersistenceMetadata, project_snapshots};

    fn snapshot(id: i32, type_name: &str, lifecycle: EntityLifecycle) -> EntitySnapshot {
        EntitySnapshot {
            id: EntityId(id),
            uuid: Uuid::from_u128(id as u128),
            type_id: id,
            type_name: type_name.to_owned(),
            position: Vec3::ZERO,
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: (type_name == "minecraft:item").then(|| EntityItemStack::new(1, 1)),
            experience_value: None,
            block_state: None,
            lifecycle,
            health: 20.0,
            attributes: AttributeSet::new(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        }
    }

    #[test]
    fn save_projection_uses_ecs_timing_and_persists_primed_tnt() {
        let mut item = snapshot(1, "minecraft:item", EntityLifecycle::Alive);
        item.retained.spawn_tick = 8;
        item.retained.item_pickup_ready_tick = Some(23);
        let mut tnt = snapshot(3, "minecraft:tnt", EntityLifecycle::Alive);
        tnt.retained.primed_tnt = Some(mc_entity::EntityPrimedTntState {
            expires_tick: 80,
            air_block_state: 0,
        });
        let mut creeper = snapshot(4, "minecraft:creeper", EntityLifecycle::Alive);
        creeper.retained.primed_tnt = Some(mc_entity::EntityPrimedTntState {
            expires_tick: 30,
            air_block_state: 0,
        });
        let snapshots = vec![
            item,
            snapshot(2, "minecraft:cow", EntityLifecycle::Despawning),
            tnt,
            creeper,
        ];
        let metadata = EntityPersistenceMetadata { lifecycle_tick: 20 };

        let records = project_snapshots(snapshots, &metadata);

        assert_eq!(records.len(), 4);
        assert_eq!(records[0].snapshot.id, EntityId(1));
        assert_eq!(records[0].age, 12);
        assert_eq!(records[0].pickup_delay, 3);
        assert_eq!(records[1].snapshot.id, EntityId(2));
        assert_eq!(records[1].snapshot.lifecycle, EntityLifecycle::Despawning);

        assert_eq!(records[2].snapshot.type_name, "minecraft:tnt");
        assert!(records[2].snapshot.retained.primed_tnt.is_some());
        assert_eq!(records[3].snapshot.type_name, "minecraft:creeper");
        assert!(records[3].snapshot.retained.primed_tnt.is_none());

        let mut delayed = snapshot(4, "minecraft:item", EntityLifecycle::Alive);
        delayed.retained.item_pickup_ready_tick = Some(u64::MAX);
        let pickup_records = project_snapshots(
            vec![delayed],
            &EntityPersistenceMetadata { lifecycle_tick: 0 },
        );
        assert_eq!(pickup_records[0].pickup_delay, i32::from(i16::MAX));
    }

    #[test]
    fn removed_entity_is_absent_before_save_projection() {
        let mut entities = EntityStore::new();
        let removed_id = entities.spawn(SpawnEntity::new(1, "minecraft:cow", Vec3::ZERO));
        assert!(entities.remove(removed_id).is_some());

        let records = project_snapshots(
            entities.snapshots().collect(),
            &EntityPersistenceMetadata { lifecycle_tick: 0 },
        );

        assert!(records.is_empty());
    }
}
