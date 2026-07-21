use super::{
    MAX_ACTIVE_VILLAGER_BINDINGS, RegionOwnerLaneError, RegionalEntityStore,
    RegionalOwnerCoordinator, RegionalOwnerRuntime, VillagerBindingAuthority,
};
use crate::{EntityId, GoalState, SpawnEntity, Vec3};
use std::sync::{Arc, Barrier};

fn entity(kind: &str, position: Vec3) -> SpawnEntity {
    SpawnEntity::new(1, kind, position)
}

#[test]
fn claim_nearest_villager_is_atomic_and_expires_on_the_exact_lifecycle_tick() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    handle
        .spawn(entity("minecraft:cow", Vec3::new(128.0, 64.0, 0.0)))
        .expect("spawn closer non-villager");
    let first = handle
        .spawn(entity("minecraft:villager", Vec3::new(127.0, 64.0, 0.0)))
        .expect("spawn first villager");
    let second = handle
        .spawn(entity("minecraft:villager", Vec3::new(129.0, 64.0, 0.0)))
        .expect("spawn second villager");
    handle
        .spawn(entity("minecraft:villager", Vec3::new(128.0, 129.0, 0.0)))
        .expect("spawn vertically distant villager");

    let first_claim = handle
        .claim_nearest_villager(Vec3::new(128.0, 64.0, 0.0), 64.0, "token-a")
        .expect("first binding query")
        .expect("first binding claim");
    assert_eq!(first_claim.token(), "token-a");
    assert_eq!(first_claim.expires_at_tick(), 600);

    let second_claim = handle
        .claim_nearest_villager(Vec3::new(128.0, 64.0, 0.0), 64.0, "token-b")
        .expect("second binding query")
        .expect("second binding claim");
    assert_eq!(second_claim.token(), "token-b");
    assert_eq!(handle.snapshot(first).unwrap().unwrap().id, first);
    assert_eq!(handle.snapshot(second).unwrap().unwrap().id, second);

    assert_eq!(
        handle.claim_nearest_villager(Vec3::new(128.0, 64.0, 0.0), 64.0, "token-c"),
        Ok(None)
    );

    handle
        .advance_lifecycle_epoch(599)
        .expect("advance before expiry");
    assert_eq!(
        handle.claim_nearest_villager(Vec3::new(128.0, 64.0, 0.0), 64.0, "token-d"),
        Ok(None)
    );
    handle
        .advance_lifecycle_epoch(600)
        .expect("advance to expiry");
    let reclaimed = handle
        .claim_nearest_villager(Vec3::new(128.0, 64.0, 0.0), 64.0, "token-e")
        .expect("reclaim query")
        .expect("expired villager is available");
    assert_eq!(reclaimed.token(), "token-e");
    assert_eq!(reclaimed.expires_at_tick(), 1_200);

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn claim_nearest_villager_rejects_invalid_queries_and_token_collisions() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    handle
        .spawn(entity("minecraft:villager", Vec3::new(0.0, 64.0, 0.0)))
        .expect("spawn villager");

    for (center, radius, token) in [
        (Vec3::new(f64::NAN, 64.0, 0.0), 16.0, "valid"),
        (Vec3::new(0.0, 64.0, 0.0), 0.0, "valid"),
        (Vec3::new(0.0, 64.0, 0.0), -1.0, "valid"),
        (Vec3::new(0.0, 64.0, 0.0), f64::NAN, "valid"),
        (Vec3::new(0.0, 64.0, 0.0), 64.000_1, "valid"),
        (Vec3::new(0.0, 64.0, 0.0), 16.0, ""),
    ] {
        assert_eq!(
            handle.claim_nearest_villager(center, radius, token),
            Err(RegionOwnerLaneError::InvalidQuery)
        );
    }

    assert!(
        handle
            .claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "duplicate")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        handle.claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "duplicate"),
        Err(RegionOwnerLaneError::BindingTokenCollision)
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn claim_nearest_villager_reports_closed_owner() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    runtime.shutdown().expect("regional owner shutdown");

    assert_eq!(
        handle.claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "closed"),
        Err(RegionOwnerLaneError::Closed)
    );
}

#[test]
fn binding_goal_applies_follow_position_and_idle_without_consuming_token() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    let villager = handle
        .spawn(entity("minecraft:villager", Vec3::new(0.0, 64.0, 0.0)))
        .expect("spawn villager");
    handle
        .claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "goal-token")
        .expect("binding query")
        .expect("binding claim");

    let follow = GoalState::FollowPosition {
        target: Vec3::new(8.0, 64.0, -4.0),
        speed: 0.35,
    };
    assert_eq!(
        handle.apply_villager_binding_goal("goal-token".to_owned(), follow.clone()),
        Ok(true)
    );
    assert_eq!(handle.snapshot(villager).unwrap().unwrap().goal, follow);

    assert_eq!(
        handle.apply_villager_binding_goal("goal-token".to_owned(), GoalState::Idle),
        Ok(true)
    );
    assert_eq!(
        handle.snapshot(villager).unwrap().unwrap().goal,
        GoalState::Idle
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn binding_goal_returns_false_for_missing_token() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();

    assert_eq!(
        handle.apply_villager_binding_goal("missing".to_owned(), GoalState::Idle),
        Ok(false)
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn binding_goal_expires_on_the_exact_lifecycle_tick() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    handle
        .spawn(entity("minecraft:villager", Vec3::new(0.0, 64.0, 0.0)))
        .expect("spawn villager");
    handle
        .claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "expiring")
        .expect("binding query")
        .expect("binding claim");

    handle
        .advance_lifecycle_epoch(599)
        .expect("advance before expiry");
    assert_eq!(
        handle.apply_villager_binding_goal("expiring".to_owned(), GoalState::Idle),
        Ok(true)
    );

    handle
        .advance_lifecycle_epoch(600)
        .expect("advance to exact expiry");
    assert_eq!(
        handle.apply_villager_binding_goal("expiring".to_owned(), GoalState::Idle),
        Ok(false)
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn binding_goal_releases_claim_when_bound_entity_was_removed() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    let removed = handle
        .spawn(entity("minecraft:villager", Vec3::new(0.0, 64.0, 0.0)))
        .expect("spawn villager");
    handle
        .claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "reusable")
        .expect("binding query")
        .expect("binding claim");
    handle.remove(removed).expect("remove bound villager");

    assert_eq!(
        handle.apply_villager_binding_goal("reusable".to_owned(), GoalState::Idle),
        Ok(false)
    );

    handle
        .spawn(entity("minecraft:villager", Vec3::new(1.0, 64.0, 0.0)))
        .expect("spawn replacement villager");
    assert!(
        handle
            .claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "reusable")
            .expect("reused binding query")
            .is_some()
    );

    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn concurrent_claims_cannot_bind_the_same_villager() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    handle
        .spawn(entity("minecraft:villager", Vec3::new(0.0, 64.0, 0.0)))
        .expect("spawn villager");
    let ready = Arc::new(Barrier::new(3));
    let first = {
        let handle = handle.clone();
        let ready = Arc::clone(&ready);
        std::thread::spawn(move || {
            ready.wait();
            handle.claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "concurrent-a")
        })
    };
    let second = {
        let handle = handle.clone();
        let ready = Arc::clone(&ready);
        std::thread::spawn(move || {
            ready.wait();
            handle.claim_nearest_villager(Vec3::new(0.0, 64.0, 0.0), 16.0, "concurrent-b")
        })
    };
    ready.wait();

    let accepted = usize::from(first.join().unwrap().unwrap().is_some())
        + usize::from(second.join().unwrap().unwrap().is_some());
    assert_eq!(accepted, 1);
    runtime.shutdown().expect("regional owner shutdown");
}

#[test]
fn coordinator_uses_global_entity_id_tie_break_across_owner_lanes() {
    let mut coordinator = RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
        .expect("regional owner coordinator");
    let lower = coordinator
        .spawn(entity("minecraft:villager", Vec3::new(127.0, 64.0, 0.0)))
        .expect("spawn lower-id villager");
    coordinator
        .spawn(entity("minecraft:villager", Vec3::new(129.0, 64.0, 0.0)))
        .expect("spawn higher-id villager");

    coordinator
        .claim_nearest_villager(
            Vec3::new(128.0, 64.0, 0.0),
            4.0,
            "cross-lane-tie".to_owned(),
        )
        .expect("cross-lane query")
        .expect("cross-lane claim");
    assert_eq!(
        coordinator.villager_bindings["cross-lane-tie"].entity,
        lower
    );
    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn binding_capacity_is_an_explicit_error_without_querying_lanes() {
    let mut coordinator = RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner coordinator");
    for index in 0..MAX_ACTIVE_VILLAGER_BINDINGS {
        let entity = EntityId(index as i32);
        let token = format!("occupied-{index}");
        coordinator
            .villager_binding_by_entity
            .insert(entity, token.clone());
        coordinator.villager_bindings.insert(
            token,
            VillagerBindingAuthority {
                entity,
                expires_at_tick: 600,
            },
        );
    }

    assert_eq!(
        coordinator.claim_nearest_villager(
            Vec3::new(0.0, 64.0, 0.0),
            16.0,
            "over-capacity".to_owned(),
        ),
        Err(RegionOwnerLaneError::BindingCapacityExceeded)
    );
    coordinator.shutdown().expect("coordinator shutdown");
}
