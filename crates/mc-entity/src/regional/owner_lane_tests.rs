use super::{RegionEpoch, RegionKey};
use crate::effects_26_1_2::{
    EffectAction, EffectFlags, EffectId, EffectInstance, EffectKind, TargetEffectContext,
};
use crate::living_26_1_2::DamageContext;
use crate::runtime_26_1_2::TargetKind;
use crate::{
    AnimalBreedingState, EntityDamageRequest, EntityEffectOperation, EntityEffectRejection,
    EntityEffectRequest, EntityEffectResult, EntityId, EntityRetainedState, EntityStore, GoalState,
    PathingBudget, PathingProbe, PathingProbeResult, SpawnEntity, Vec3,
};
use uuid::Uuid;

fn cow(position: Vec3) -> SpawnEntity {
    SpawnEntity::new(4, "minecraft:cow", position)
}

fn heal_request(amount: f32) -> EntityEffectRequest {
    EntityEffectRequest {
        operation: EntityEffectOperation::ApplyAction {
            effect_id: EffectId::new(6),
            action: EffectAction::Heal { amount },
            damage_context: None,
        },
        target_kind: TargetKind::NonPlayer,
        death_remove_tick: 20,
    }
}

#[test]
fn regional_owner_effect_transaction_is_version_fenced_and_returns_ecs_projection() {
    let runtime = super::RegionalOwnerRuntime::from_store(super::RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    let id = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("spawn effect target");
    let initial = handle
        .snapshot(id)
        .expect("effect target read")
        .expect("effect target snapshot");
    let damaged = handle
        .damage_if_current(
            initial,
            EntityDamageRequest {
                amount: 12.0,
                tick: 1,
                death_remove_tick: 21,
            },
        )
        .expect("damage owner request")
        .expect("damage accepted")
        .snapshot;
    let accepted = handle
        .apply_effect_if_current(damaged.clone(), heal_request(100.0))
        .expect("effect owner request");
    let EntityEffectResult::Applied(accepted) = accepted else {
        panic!("heal must commit through regional owner");
    };
    assert_eq!(accepted.snapshot.health, 20.0);
    assert_eq!(
        handle.snapshot(id).expect("committed effect target read"),
        Some(accepted.snapshot.clone())
    );
    assert_eq!(
        handle
            .apply_effect_if_current(damaged, heal_request(1.0))
            .expect("stale effect owner request"),
        EntityEffectResult::Rejected(EntityEffectRejection::Stale)
    );
    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn effect_only_mutation_invalidates_full_snapshot_cas() {
    let runtime = super::RegionalOwnerRuntime::from_store(super::RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    let id = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("spawn effect target");
    let stale = handle.snapshot(id).unwrap().unwrap();
    let effect = EffectInstance::new(
        EffectId::new(10),
        EffectKind::Regeneration,
        80,
        0,
        EffectFlags::default(),
    );
    assert!(matches!(
        handle
            .apply_effect_if_current(
                stale.clone(),
                EntityEffectRequest {
                    operation: EntityEffectOperation::Add(effect),
                    target_kind: TargetKind::NonPlayer,
                    death_remove_tick: 20,
                },
            )
            .unwrap(),
        EntityEffectResult::Applied(_)
    ));
    let mut stale_replacement = stale.clone();
    stale_replacement.velocity = Vec3::new(1.0, 0.0, 0.0);
    assert!(
        !handle
            .replace_snapshot_if_current(stale, stale_replacement)
            .unwrap()
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn regional_owner_effect_rollback_restores_active_effect_checkpoint() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let id = store.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let expected = store.snapshot(id).unwrap();
    let effect = EffectInstance::new(
        EffectId::new(6),
        EffectKind::InstantHealth,
        1,
        0,
        EffectFlags::default(),
    );
    let lane = super::RegionalOwnerLane::spawn(0, [(lease, store)]).expect("owner lane");
    let phase = super::RegionPhase(1);
    lane.prepare(super::RegionOwnerBatch {
        phase,
        sequence_watermark: 1,
        mutations: vec![super::SequencedRegionMutation {
            sequence: 1,
            lease,
            mutation: super::RegionOwnerMutation::ApplyEffectIfCurrent {
                expected: Box::new(expected),
                request: Box::new(EntityEffectRequest {
                    operation: EntityEffectOperation::Add(effect),
                    target_kind: TargetKind::NonPlayer,
                    death_remove_tick: 20,
                }),
            },
        }],
    })
    .expect("prepare effect")
    .recv()
    .expect("prepare completion")
    .expect("prepared effect");
    let completion = lane
        .commit(phase)
        .expect("commit effect")
        .recv()
        .expect("commit completion")
        .expect("committed effect");
    assert!(matches!(
        completion.effect_results.as_slice(),
        [(1, EntityEffectResult::Applied(_))]
    ));
    lane.rollback(phase)
        .expect("rollback effect")
        .recv()
        .expect("rollback completion")
        .expect("rolled back effect");

    let mut stores = lane.shutdown().expect("clean owner shutdown");
    assert_eq!(
        stores.get_mut(&lease.key).unwrap().apply_effect(
            id,
            EntityEffectRequest {
                operation: EntityEffectOperation::Tick {
                    entity_tick_count: 1,
                    target_context: TargetEffectContext::LIVING,
                    damage_context: DamageContext::default(),
                },
                target_kind: TargetKind::NonPlayer,
                death_remove_tick: 20,
            },
        ),
        EntityEffectResult::Rejected(EntityEffectRejection::NoActiveEffects)
    );
}

struct WalkablePathing;

impl PathingProbe for WalkablePathing {
    fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
        PathingProbeResult::Walkable
    }
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
fn persistent_owner_lane_insert_rollback_restores_store_and_id_cursor() {
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
    assert!(store.is_empty());
    assert_eq!(store.spawn(cow(Vec3::new(1.5, 64.0, 0.5))), entity);
}

#[test]
fn persistent_owner_lane_damage_rollback_restores_snapshot() {
    let lease = super::RegionLease {
        key: RegionKey::new(0, 0),
        epoch: RegionEpoch::INITIAL,
        lane: 0,
    };
    let mut store = EntityStore::new();
    let mut spawned = cow(Vec3::new(0.5, 64.0, 0.5));
    spawned.goal = GoalState::Wander {
        speed: 0.8,
        period_ticks: 80,
    };
    let entity = store.spawn(spawned);
    store.tick_goals_with_pathing(1, &WalkablePathing, PathingBudget::DEFAULT);
    let expected = store.snapshot(entity).expect("spawned snapshot");
    assert_ne!(expected.retained, EntityRetainedState::default());
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
                request: EntityDamageRequest {
                    amount: 20.0,
                    tick: 7,
                    death_remove_tick: 27,
                },
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
    assert_eq!(
        store.spawn(cow(Vec3::new(1.5, 64.0, 0.5))),
        EntityId(entity.0 + 1),
        "rollback must preserve the entity ID cursor"
    );
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
