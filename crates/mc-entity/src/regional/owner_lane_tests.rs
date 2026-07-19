use super::{RegionEpoch, RegionKey};
use crate::{AnimalBreedingState, EntityId, EntityStore, ShadowStage, SpawnEntity, Vec3};
use uuid::Uuid;

fn cow(position: Vec3) -> SpawnEntity {
    SpawnEntity::new(4, "minecraft:cow", position)
}

#[test]
fn persistent_owner_lane_applies_phase_commands_in_deterministic_order() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
    entity.animal = Some(AnimalBreedingState::baby());
    let id = store.spawn(entity);
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");

    let phase = super::RegionPhase(1);
    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase,
            sequence_watermark: 3,
            mutations: vec![
                super::SequencedRegionMutation {
                    sequence: 2,
                    lease,
                    mutation: super::RegionOwnerMutation::SetVelocity {
                        entity: id,
                        velocity: Vec3::new(0.2, 0.0, 0.0),
                    },
                },
                super::SequencedRegionMutation {
                    sequence: 1,
                    lease,
                    mutation: super::RegionOwnerMutation::SetVelocity {
                        entity: id,
                        velocity: Vec3::new(0.1, 0.0, 0.0),
                    },
                },
                super::SequencedRegionMutation {
                    sequence: 3,
                    lease,
                    mutation: super::RegionOwnerMutation::SetAnimalState {
                        entity: id,
                        animal: AnimalBreedingState::adult(),
                    },
                },
            ],
        })
        .expect("prepare phase")
        .recv()
        .expect("prepare completion"),
        Ok(phase)
    );
    let completion = lane
        .commit(phase)
        .expect("commit phase")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    assert_eq!(completion.applied_sequences, vec![1, 2, 3]);
    assert_eq!(
        lane.finalize(phase)
            .expect("finalize phase")
            .recv()
            .expect("finalize completion"),
        Ok(phase)
    );
    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase: super::RegionPhase(2),
            sequence_watermark: 3,
            mutations: vec![super::SequencedRegionMutation {
                sequence: 3,
                lease,
                mutation: super::RegionOwnerMutation::SetVelocity {
                    entity: id,
                    velocity: Vec3::new(0.3, 0.0, 0.0),
                },
            }],
        })
        .expect("prepare replay")
        .recv()
        .expect("replay completion"),
        Err(super::RegionOwnerLaneError::DuplicateSequence)
    );

    let stores = lane.shutdown().expect("clean owner shutdown");
    assert_eq!(
        stores
            .get(&lease.key)
            .and_then(|store| store.snapshot(id))
            .map(|entity| entity.velocity),
        Some(Vec3::new(0.2, 0.0, 0.0))
    );
    assert_eq!(
        stores
            .get(&lease.key)
            .and_then(|store| store.snapshot(id))
            .and_then(|entity| entity.animal),
        Some(AnimalBreedingState::adult())
    );
}

#[test]
fn selected_lane_reads_reject_unfinalized_phase_state() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let id = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let reader = lane.reader();
    let phase = super::RegionPhase(1);
    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase,
            sequence_watermark: 1,
            mutations: vec![super::SequencedRegionMutation {
                sequence: 1,
                lease,
                mutation: super::RegionOwnerMutation::SetVelocity {
                    entity: id,
                    velocity: Vec3::new(0.25, 0.0, 0.0),
                },
            }],
        })
        .expect("prepare phase")
        .recv()
        .expect("prepare completion"),
        Ok(phase)
    );
    assert_eq!(
        reader
            .request_snapshots_for_ids(vec![(lease, id)])
            .expect("pending read request")
            .recv()
            .expect("pending read response"),
        Err(super::RegionOwnerLaneError::Busy)
    );
    lane.commit(phase)
        .expect("commit phase")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    assert_eq!(
        reader
            .request_snapshots_for_ids(vec![(lease, id)])
            .expect("committed read request")
            .recv()
            .expect("committed read response"),
        Err(super::RegionOwnerLaneError::Busy)
    );
    lane.finalize(phase)
        .expect("finalize phase")
        .recv()
        .expect("finalize completion")
        .expect("finalized phase");
    assert_eq!(
        reader
            .request_snapshots_for_ids(vec![(lease, id)])
            .expect("finalized read request")
            .recv()
            .expect("finalized read response")
            .expect("finalized snapshots")[0]
            .velocity,
        Vec3::new(0.25, 0.0, 0.0)
    );
    lane.shutdown().expect("owner lane shutdown");
}

#[test]
fn persistent_owner_lane_rejects_stale_batch_without_partial_mutation() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let first = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let second = store.spawn(cow(Vec3::new(1.5, 64.0, 0.5)));
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let stale = super::RegionLease {
        epoch: RegionEpoch(2),
        ..lease
    };

    let rejected = lane
        .prepare(super::RegionOwnerBatch {
            phase: super::RegionPhase(1),
            sequence_watermark: 2,
            mutations: vec![
                super::SequencedRegionMutation {
                    sequence: 1,
                    lease,
                    mutation: super::RegionOwnerMutation::SetVelocity {
                        entity: first,
                        velocity: Vec3::new(0.1, 0.0, 0.0),
                    },
                },
                super::SequencedRegionMutation {
                    sequence: 2,
                    lease: stale,
                    mutation: super::RegionOwnerMutation::SetVelocity {
                        entity: second,
                        velocity: Vec3::new(0.2, 0.0, 0.0),
                    },
                },
            ],
        })
        .expect("prepare stale phase")
        .recv()
        .expect("prepare completion");
    assert_eq!(rejected, Err(super::RegionOwnerLaneError::StaleLease));
    let phase = super::RegionPhase(1);
    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase,
            sequence_watermark: 1,
            mutations: vec![super::SequencedRegionMutation {
                sequence: 1,
                lease,
                mutation: super::RegionOwnerMutation::SetVelocity {
                    entity: first,
                    velocity: Vec3::new(0.1, 0.0, 0.0),
                },
            }],
        })
        .expect("retry corrected phase")
        .recv()
        .expect("retry completion"),
        Ok(phase)
    );
    assert_eq!(
        lane.abort(phase)
            .expect("abort corrected retry")
            .recv()
            .expect("abort completion"),
        Ok(phase)
    );

    let stores = lane.shutdown().expect("clean owner shutdown");
    let store = stores.get(&lease.key).expect("owned region");
    assert_eq!(store.snapshot(first).expect("first").velocity, Vec3::ZERO);
    assert_eq!(store.snapshot(second).expect("second").velocity, Vec3::ZERO);
}

#[test]
fn persistent_owner_lane_rolls_back_a_committed_unfinalized_phase() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let entity = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let phase = super::RegionPhase(1);
    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase,
            sequence_watermark: 1,
            mutations: vec![super::SequencedRegionMutation {
                sequence: 1,
                lease,
                mutation: super::RegionOwnerMutation::SetVelocity {
                    entity,
                    velocity: Vec3::new(0.4, 0.0, 0.0),
                },
            }],
        })
        .expect("prepare")
        .recv()
        .expect("prepare completion"),
        Ok(phase)
    );
    lane.commit(phase)
        .expect("commit")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    assert_eq!(
        lane.rollback(phase)
            .expect("rollback")
            .recv()
            .expect("rollback completion"),
        Ok(phase)
    );

    let stores = lane.shutdown().expect("clean owner shutdown");
    assert_eq!(
        stores[&lease.key]
            .snapshot(entity)
            .expect("entity")
            .velocity,
        Vec3::ZERO
    );
}

#[test]
fn persistent_owner_lane_rollback_restores_entity_id_allocation() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let lane =
        super::RegionalOwnerLane::spawn(0, [(lease, EntityStore::new())]).expect("owner lane");
    let phase = super::RegionPhase(1);
    let id = EntityId(i32::MAX);
    let snapshot =
        super::snapshot_from_spawn(id, Uuid::from_u128(91), cow(Vec3::new(0.5, 64.0, 0.5)));
    lane.prepare(super::RegionOwnerBatch {
        phase,
        sequence_watermark: 1,
        mutations: vec![super::SequencedRegionMutation {
            sequence: 1,
            lease,
            mutation: super::RegionOwnerMutation::InsertSnapshot(Box::new(snapshot)),
        }],
    })
    .expect("prepare")
    .recv()
    .expect("prepare completion")
    .expect("prepared phase");
    lane.commit(phase)
        .expect("commit")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    lane.rollback(phase)
        .expect("rollback")
        .recv()
        .expect("rollback completion")
        .expect("rolled back phase");

    let mut stores = lane.shutdown().expect("clean owner shutdown");
    let store = stores.get_mut(&lease.key).expect("owned region");
    assert_eq!(store.spawn(cow(Vec3::new(0.5, 64.0, 0.5))), EntityId(1));
}

#[test]
fn persistent_owner_lane_rollback_restores_removed_snapshot() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let entity = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let expected = store.snapshot(entity).expect("spawned snapshot");
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let phase = super::RegionPhase(1);
    lane.prepare(super::RegionOwnerBatch {
        phase,
        sequence_watermark: 1,
        mutations: vec![super::SequencedRegionMutation {
            sequence: 1,
            lease,
            mutation: super::RegionOwnerMutation::RemoveEntity(entity),
        }],
    })
    .expect("prepare")
    .recv()
    .expect("prepare completion")
    .expect("prepared phase");
    lane.commit(phase)
        .expect("commit")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    lane.rollback(phase)
        .expect("rollback")
        .recv()
        .expect("rollback completion")
        .expect("rolled back phase");

    let stores = lane.shutdown().expect("clean owner shutdown");
    assert_eq!(stores[&lease.key].snapshot(entity), Some(expected));
}

#[test]
fn persistent_owner_lane_rollback_discards_speculative_semantic_events() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let entity = EntityId(1);
    let snapshot =
        super::snapshot_from_spawn(entity, Uuid::from_u128(93), cow(Vec3::new(0.5, 64.0, 0.5)));
    let lane =
        super::RegionalOwnerLane::spawn(0, [(lease, EntityStore::new())]).expect("owner lane");
    let phase = super::RegionPhase(1);
    lane.prepare(super::RegionOwnerBatch {
        phase,
        sequence_watermark: 1,
        mutations: vec![super::SequencedRegionMutation {
            sequence: 1,
            lease,
            mutation: super::RegionOwnerMutation::InsertSnapshot(Box::new(snapshot)),
        }],
    })
    .expect("prepare")
    .recv()
    .expect("prepare completion")
    .expect("prepared phase");
    lane.commit(phase)
        .expect("commit")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    lane.rollback(phase)
        .expect("rollback")
        .recv()
        .expect("rollback completion")
        .expect("rolled back phase");

    let mut stores = lane.shutdown().expect("clean owner shutdown");
    let store = stores.get_mut(&lease.key).expect("owned region");
    store.shadow.run_stage(ShadowStage::OutputEvents);
    assert!(store.shadow.take_output_events().is_empty());
}

#[test]
fn persistent_owner_lane_damage_rollback_restores_snapshot_and_events() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let entity = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let expected = store.snapshot(entity).expect("spawned snapshot");
    store.shadow.run_stage(ShadowStage::OutputEvents);
    let _ = store.shadow.take_output_events();
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let phase = super::RegionPhase(1);
    lane.prepare(super::RegionOwnerBatch {
        phase,
        sequence_watermark: 1,
        mutations: vec![super::SequencedRegionMutation {
            sequence: 1,
            lease,
            mutation: super::RegionOwnerMutation::DamageIfCurrent {
                expected: Box::new(expected.clone()),
                amount: 20.0,
            },
        }],
    })
    .expect("prepare")
    .recv()
    .expect("prepare completion")
    .expect("prepared phase");
    lane.commit(phase)
        .expect("commit")
        .recv()
        .expect("commit completion")
        .expect("committed phase");
    lane.rollback(phase)
        .expect("rollback")
        .recv()
        .expect("rollback completion")
        .expect("rolled back phase");

    let mut stores = lane.shutdown().expect("clean owner shutdown");
    let store = stores.get_mut(&lease.key).expect("owned region");
    assert_eq!(store.snapshot(entity), Some(expected));
    store.shadow.run_stage(ShadowStage::OutputEvents);
    assert!(store.shadow.take_output_events().is_empty());
}

#[test]
fn persistent_owner_lane_rejects_duplicate_insert_identity_across_regions() {
    let west = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let east = super::RegionLease {
        key: RegionKey::new(1, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let lane = super::RegionalOwnerLane::spawn(
        0,
        [(west, EntityStore::new()), (east, EntityStore::new())],
    )
    .expect("owner lane");
    let id = EntityId(7);
    let uuid = Uuid::from_u128(92);
    let west_snapshot = super::snapshot_from_spawn(id, uuid, cow(Vec3::new(0.5, 64.0, 0.5)));
    let east_snapshot = super::snapshot_from_spawn(id, uuid, cow(Vec3::new(128.5, 64.0, 0.5)));

    assert_eq!(
        lane.prepare(super::RegionOwnerBatch {
            phase: super::RegionPhase(1),
            sequence_watermark: 2,
            mutations: vec![
                super::SequencedRegionMutation {
                    sequence: 1,
                    lease: west,
                    mutation: super::RegionOwnerMutation::InsertSnapshot(Box::new(west_snapshot,)),
                },
                super::SequencedRegionMutation {
                    sequence: 2,
                    lease: east,
                    mutation: super::RegionOwnerMutation::InsertSnapshot(Box::new(east_snapshot,)),
                },
            ],
        })
        .expect("submit duplicate identity")
        .recv()
        .expect("prepare completion"),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );
    let stores = lane.shutdown().expect("clean owner shutdown");
    assert!(stores.values().all(EntityStore::is_empty));
}
#[test]
fn owner_lane_start_validation_returns_the_physical_store() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 1,
    };
    let mut store = EntityStore::new();
    let entity = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let error = match super::RegionalOwnerLane::spawn(0, [(lease, store)]) {
        Ok(_) => panic!("wrong-lane startup must fail"),
        Err(error) => error,
    };
    assert_eq!(error.error, super::RegionOwnerLaneError::WrongLane);
    assert_eq!(error.regions.len(), 1);
    assert!(error.regions[0].1.contains(entity));
}
