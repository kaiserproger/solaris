use super::*;

#[test]
fn late_reliable_results_do_not_revive_removed_connection() {
    let registry = ConnectionRegistry::new();
    let id = registry.joining_id();
    let (reliable_tx, _reliable_rx) = mpsc::channel(4);
    let (world_tx, _world_rx) = mpsc::channel(4);
    registry
        .register(
            id,
            ConnectionHandles {
                reliable_tx,
                world_tx,
            },
        )
        .unwrap();
    registry.remove(id);

    let mut late = vec![mc_core::boundary::PlayerChange::Chat {
        message: "already closed".to_owned(),
    }];
    assert!(registry.handles_or_stash(id, &mut late).is_none());
    let inner = registry.inner.lock();
    assert!(
        !inner.pending_reliable.contains_key(&id),
        "closed connection must not regain a pending queue"
    );
    assert!(!inner.pending_joins.contains_key(&id));
}

#[test]
fn closing_slot_is_released_only_after_lifecycle_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let admin = crate::admin::Admin::open(dir.path()).unwrap();
    let registry = ConnectionRegistry::new();
    let id = registry.joining_id();
    let lifecycle = ConnectionLifecycle::new(&registry, &admin, id);
    registry.remove(id);
    let next = registry.alloc_id();
    assert!(
        registry
            .admit(next, 1, |full| full.then(|| serde_json::json!("full")))
            .is_some(),
        "closing must retain admission until native/admin cleanup runs"
    );
    drop(lifecycle);
    assert!(
        registry
            .admit(next, 1, |full| full.then(|| serde_json::json!("full")))
            .is_none()
    );
}

#[test]
fn concurrent_admission_reserves_the_last_slot_before_registration() {
    let registry = ConnectionRegistry::new();
    let barrier = std::sync::Barrier::new(2);
    let accepted = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let id = registry.alloc_id();
                    barrier.wait();
                    registry
                        .admit(id, 1, |full| full.then(|| serde_json::json!("full")))
                        .is_none()
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|accepted| *accepted)
            .count()
    });
    assert_eq!(accepted, 1);
    assert_eq!(registry.inner.lock().joining.len(), 1);
    assert_eq!(
        registry.len(),
        0,
        "the winner has not registered Play handles yet"
    );
}

#[test]
fn admission_retains_the_real_operator_player_limit_bypass() {
    let dir = tempfile::tempdir().unwrap();
    let uuid = uuid::Uuid::from_u128(42);
    std::fs::write(
        dir.path().join("ops.json"),
        serde_json::to_vec(&serde_json::json!([{
            "uuid": uuid, "name": "Operator", "level": 4,
            "bypassesPlayerLimit": true
        }]))
        .unwrap(),
    )
    .unwrap();
    let admin = crate::admin::Admin::open(dir.path()).unwrap();
    let registry = ConnectionRegistry::new();
    let id = registry.alloc_id();
    assert!(
        registry
            .admit(id, 0, |full| admin.admission(uuid, None, full))
            .is_none()
    );
    assert!(registry.inner.lock().joining.contains(&id));
}

#[tokio::test]
async fn removed_join_rejects_late_join_reason_waiter_and_registration() {
    let registry = ConnectionRegistry::new();
    let id = registry.joining_id();
    registry.remove(id);
    registry.stash_join(
        id,
        (
            mc_core::boundary::EntityId(1),
            [0.0; 3],
            0,
            Default::default(),
        ),
    );
    registry.stash_disconnect_reason(id, "late".into());
    assert!(registry.take_join(id).is_none());
    assert!(registry.take_disconnect_reason(id).is_none());
    assert!(registry.wait_join(id).await.is_err());
    let (reliable_tx, _) = mpsc::channel(1);
    let (world_tx, _) = mpsc::channel(1);
    assert!(
        registry
            .register(
                id,
                ConnectionHandles {
                    reliable_tx,
                    world_tx
                }
            )
            .is_err()
    );
    assert!(registry.is_empty());
}

#[tokio::test]
async fn delivery_close_wakes_owner_and_aborts_pending_writer() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(ConnectionRegistry::new());
    let admin = Arc::new(crate::admin::Admin::open(dir.path()).unwrap());
    let id = registry.joining_id();
    admin.register(
        id,
        uuid::Uuid::nil(),
        "SlowWriter".into(),
        "127.0.0.1".parse().unwrap(),
    );
    let writer = tokio::spawn(std::future::pending::<(&'static str, Option<String>)>());
    let abort = writer.abort_handle();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let task_registry = Arc::clone(&registry);
    let task_admin = Arc::clone(&admin);
    let owner = tokio::spawn(async move {
        let mut lifecycle = ConnectionLifecycle::new(&task_registry, &task_admin, id);
        lifecycle.writer = Some(writer);
        let cancelled = lifecycle.cancelled.take().unwrap();
        ready.send(()).unwrap();
        cancelled.await.unwrap();
        lifecycle.stop_writer().await;
        drop(lifecycle);
    });
    waiting.await.unwrap();
    registry.remove(id);
    owner.await.unwrap();
    assert!(
        abort.is_finished(),
        "owner completion must wait for the writer"
    );
    assert!(admin.roster(false).is_empty());
    assert!(registry.inner.lock().admitted.is_empty());
    assert!(registry.inner.lock().closers.is_empty());
}

#[tokio::test]
async fn task_cancellation_releases_admission_admin_and_writer() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(ConnectionRegistry::new());
    let admin = Arc::new(crate::admin::Admin::open(dir.path()).unwrap());
    let id = registry.joining_id();
    admin.register(
        id,
        uuid::Uuid::nil(),
        "Cancelled".into(),
        "127.0.0.1".parse().unwrap(),
    );
    let (writer_closed, writer_dropped) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let _closed = writer_closed;
        std::future::pending::<(&'static str, Option<String>)>().await
    });
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let task_registry = Arc::clone(&registry);
    let task_admin = Arc::clone(&admin);
    let task = tokio::spawn(async move {
        let mut lifecycle = ConnectionLifecycle::new(&task_registry, &task_admin, id);
        lifecycle.writer = Some(writer);
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(lifecycle);
    });
    waiting.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(writer_dropped.await.is_err());
    assert!(admin.roster(false).is_empty());
    assert!(registry.inner.lock().joining.is_empty());
    let next = registry.alloc_id();
    assert!(
        registry
            .admit(next, 1, |full| full.then(|| serde_json::json!("full")))
            .is_none()
    );
}
