use crate::play::persistence::PersistedEntityRecord;
use mc_entity::{
    EntityId, EntityLifecycle, EntitySnapshot, RegionPhase, RegionalOwnerSaveSnapshot,
};
use std::collections::HashMap;

const TRANSIENT_TNT_ENTITY_TYPE_NAME: &str = "minecraft:tnt";

pub(super) struct EntityPersistenceMetadata {
    pub(super) lifecycle_tick: u64,
    pub(super) spawn_ticks: HashMap<EntityId, u64>,
    pub(super) item_pickup_ready_ticks: HashMap<EntityId, u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RestoredEntityTiming {
    pub(super) entity_id: EntityId,
    pub(super) spawn_tick: u64,
    pub(super) item_spawn_tick: Option<u64>,
    pub(super) item_pickup_ready_tick: Option<u64>,
    pub(super) arrow_spawn_tick: Option<u64>,
}

pub(super) fn maximum_persisted_age(records: &[PersistedEntityRecord]) -> u64 {
    records
        .iter()
        .map(|record| normalized_nonnegative(record.age))
        .max()
        .unwrap_or(0)
}

pub(super) fn restore_timing(
    records: &[PersistedEntityRecord],
    lifecycle_tick: u64,
) -> Vec<RestoredEntityTiming> {
    records
        .iter()
        .map(|record| {
            let spawn_tick = lifecycle_tick.saturating_sub(normalized_nonnegative(record.age));
            let is_item = record.snapshot.item_stack.is_some();
            RestoredEntityTiming {
                entity_id: record.snapshot.id,
                spawn_tick,
                item_spawn_tick: is_item.then_some(spawn_tick),
                item_pickup_ready_tick: (is_item && record.pickup_delay > 0).then(|| {
                    lifecycle_tick.saturating_add(normalized_nonnegative(record.pickup_delay))
                }),
                arrow_spawn_tick: (record.snapshot.type_name == "minecraft:arrow")
                    .then_some(spawn_tick),
            }
        })
        .collect()
}

pub(super) fn project_owner_save(
    saved: RegionalOwnerSaveSnapshot,
    metadata: &EntityPersistenceMetadata,
) -> (Vec<PersistedEntityRecord>, Vec<RegionPhase>) {
    let journal_phases = saved.journal_phases().to_vec();
    let records = project_snapshots(saved.into_snapshots(), metadata);
    (records, journal_phases)
}

fn project_snapshots(
    snapshots: Vec<EntitySnapshot>,
    metadata: &EntityPersistenceMetadata,
) -> Vec<PersistedEntityRecord> {
    snapshots
        .into_iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive
                && entity.type_name != TRANSIENT_TNT_ENTITY_TYPE_NAME
        })
        .map(|entity| {
            let age = metadata
                .spawn_ticks
                .get(&entity.id)
                .map(|spawn_tick| metadata.lifecycle_tick.saturating_sub(*spawn_tick))
                .unwrap_or(0)
                .min(i32::MAX as u64) as i32;
            let pickup_delay = metadata
                .item_pickup_ready_ticks
                .get(&entity.id)
                .map(|ready_tick| ready_tick.saturating_sub(metadata.lifecycle_tick))
                .unwrap_or(0)
                .min(i32::from(i16::MAX) as u64) as i32;
            PersistedEntityRecord {
                snapshot: entity,
                age,
                pickup_delay,
            }
        })
        .collect()
}

fn normalized_nonnegative(value: i32) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use crate::play::persistence::PersistedEntityRecord;
    use mc_entity::{
        AttributeSet, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, GoalState,
        Rotation, Vec3,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    use super::{
        EntityPersistenceMetadata, maximum_persisted_age, project_snapshots, restore_timing,
    };

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
        }
    }

    #[test]
    fn restore_timing_normalizes_values_and_builds_absolute_ticks() {
        let records = vec![
            PersistedEntityRecord {
                snapshot: snapshot(1, "minecraft:item", EntityLifecycle::Alive),
                age: 12,
                pickup_delay: 3,
            },
            PersistedEntityRecord {
                snapshot: snapshot(2, "minecraft:arrow", EntityLifecycle::Alive),
                age: -4,
                pickup_delay: -8,
            },
        ];

        let projection = restore_timing(&records, 20);

        assert_eq!(maximum_persisted_age(&records), 12);
        assert_eq!(projection[0].entity_id, EntityId(1));
        assert_eq!(projection[0].spawn_tick, 8);
        assert_eq!(projection[0].item_pickup_ready_tick, Some(23));
        assert_eq!(projection[0].arrow_spawn_tick, None);
        assert_eq!(projection[1].entity_id, EntityId(2));
        assert_eq!(projection[1].spawn_tick, 20);
        assert_eq!(projection[1].item_pickup_ready_tick, None);
        assert_eq!(projection[1].arrow_spawn_tick, Some(20));
    }

    #[test]
    fn save_projection_filters_transient_entities_and_clamps_timing() {
        let snapshots = vec![
            snapshot(1, "minecraft:item", EntityLifecycle::Alive),
            snapshot(2, "minecraft:cow", EntityLifecycle::Despawning),
            snapshot(3, "minecraft:tnt", EntityLifecycle::Alive),
        ];
        let metadata = EntityPersistenceMetadata {
            lifecycle_tick: u64::MAX,
            spawn_ticks: HashMap::from([(EntityId(1), 0)]),
            item_pickup_ready_ticks: HashMap::from([(EntityId(1), u64::MAX)]),
        };

        let records = project_snapshots(snapshots, &metadata);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].snapshot.id, EntityId(1));
        assert_eq!(records[0].age, i32::MAX);
        assert_eq!(records[0].pickup_delay, 0);

        let pickup_records = project_snapshots(
            vec![snapshot(4, "minecraft:item", EntityLifecycle::Alive)],
            &EntityPersistenceMetadata {
                lifecycle_tick: 0,
                spawn_ticks: HashMap::new(),
                item_pickup_ready_ticks: HashMap::from([(EntityId(4), u64::MAX)]),
            },
        );
        assert_eq!(pickup_records[0].pickup_delay, i32::from(i16::MAX));
    }
}
