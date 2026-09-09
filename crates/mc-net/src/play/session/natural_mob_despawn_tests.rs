use super::*;
use std::cell::Cell;
use std::collections::HashSet;
use tokio::sync::mpsc;

thread_local! {
    static REMOVE_AFTER_PROJECTION: Cell<Option<EntityId>> = const { Cell::new(None) };
    static MOVE_AFTER_PROJECTION: Cell<Option<(EntityId, Vec3)>> = const { Cell::new(None) };
    static PAUSE_BEFORE_PROJECTION: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn after_projection(inner: &mut SessionEntityGuards<'_>) {
    REMOVE_AFTER_PROJECTION.with(|pending| {
        if let Some(id) = pending.take() {
            let expected = inner.entities.snapshot(id).unwrap();
            assert!(inner.entities.remove_if_current(expected).is_some());
        }
    });
    MOVE_AFTER_PROJECTION.with(|pending| {
        if let Some((id, position)) = pending.take() {
            let expected = inner.entities.snapshot(id).unwrap();
            let mut next = expected.clone();
            next.position = position;
            assert!(inner.entities.replace_snapshot_if_current(expected, next));
        }
    });
}

pub(super) fn before_projection() {
    if let Some((reached, resume)) =
        PAUSE_BEFORE_PROJECTION.with(|pending| pending.borrow_mut().take())
    {
        reached.send(()).expect("projection pause receiver");
        resume.recv().expect("projection resume sender");
    }
}

fn distant_natural_zombie() -> (SessionRegistry, EntityId, mpsc::Receiver<OutboundCommand>) {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DespawnRace"),
        name: "DespawnRace".to_owned(),
    };
    let (_session, _) = registry.register(
        &profile,
        (0, 0),
        8,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        150,
        "minecraft:zombie".to_owned(),
        Vec3::new(240.5, 64.0, 0.5),
    );
    let id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut inner = registry.lock_inner("install natural race fixture");
        inner.natural_hostile_mobs.insert(id);
        inner.natural_mob_no_action_since_tick.insert(id, 0);
    }
    (registry, id, _rx)
}

#[test]
fn vanished_after_projection_clears_natural_idle_clock() {
    let (registry, id, _rx) = distant_natural_zombie();
    REMOVE_AFTER_PROJECTION.with(|pending| pending.set(Some(id)));
    assert_eq!(registry.tick_natural_mob_despawn(700).removed(), 0);
    assert!(registry.persisted_entity_records().is_empty());
    let inner = registry.lock_inner("verify vanished natural entity clock");
    assert!(!inner.natural_mob_no_action_since_tick.contains_key(&id));
}

#[test]
fn session_join_progresses_during_despawn_projection_and_protects_nearby_mob() {
    let registry = std::sync::Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    registry.register(
        &LoggedInProfile {
            uuid: crate::login::offline_uuid("DistantDespawnObserver"),
            name: "DistantDespawnObserver".to_owned(),
        },
        (0, 0),
        8,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        150,
        "minecraft:zombie".to_owned(),
        Vec3::new(240.5, 64.0, 0.5),
    );
    let id = registry.persisted_entity_records()[0].snapshot.id;
    registry
        .lock_inner("install natural despawn join fixture")
        .natural_hostile_mobs
        .insert(id);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let worker_registry = std::sync::Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        PAUSE_BEFORE_PROJECTION.with(|pending| {
            *pending.borrow_mut() = Some((reached_tx, resume_rx));
        });
        worker_registry.tick_natural_mob_despawn(700).removed()
    });
    reached_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("despawn reached owner projection");
    let (near_tx, _near_rx) = mpsc::channel(8);
    let (joined_tx, joined_rx) = std::sync::mpsc::channel();
    let joining_registry = std::sync::Arc::clone(&registry);
    let joining = std::thread::spawn(move || {
        joining_registry.register(
            &LoggedInProfile {
                uuid: crate::login::offline_uuid("NearbyDespawnObserver"),
                name: "NearbyDespawnObserver".to_owned(),
            },
            (15, 0),
            8,
            HashSet::new(),
            near_tx,
            PlayerPose::new(240.5, 64.0, 0.5),
        );
        joined_tx.send(()).expect("join progress receiver");
    });
    let progressed = joined_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .is_ok();
    resume_tx.send(()).expect("release paused projection");
    let removed = worker.join().expect("despawn worker");
    joining.join().expect("joining session");
    assert!(
        progressed,
        "owner projection must not block session registration"
    );
    assert_eq!(removed, 0, "the newly nearby player prevents despawn");
    assert!(registry.server_entity_snapshot(id).is_some());
}

#[test]
fn movement_after_projection_prevents_stale_despawn() {
    let (registry, id, _rx) = distant_natural_zombie();
    let nearby = Vec3::new(0.5, 64.0, 0.5);
    MOVE_AFTER_PROJECTION.with(|pending| pending.set(Some((id, nearby))));

    assert_eq!(registry.tick_natural_mob_despawn(700).removed(), 0);
    assert_eq!(
        registry.server_entity_snapshot(id).unwrap().position,
        nearby
    );
}
