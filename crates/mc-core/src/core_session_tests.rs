use super::*;

#[tokio::test]
async fn leave_waits_for_capacity_instead_of_shedding_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(root.path().join("world"), 42);
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.input_queue_cap = 1;
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    let core = Core::open(config).unwrap();
    let connection = ConnectionId(77);
    let mut ticks = core.subscribe();
    assert!(core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(77),
        name: "Lifecycle".into(),
        permission_level: 0,
    }));
    let entity = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let _ = ticks.recv().await;
            for (id, changes) in core.try_drain_outbox() {
                if id == connection {
                    for change in changes {
                        if let PlayerChange::Joined { entity, .. } = change {
                            return entity;
                        }
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let permit = core
        .inner
        .input_tx
        .reserve_many(core.inner.input_tx.max_capacity())
        .await
        .unwrap();
    let mut leaving = std::pin::pin!(core.leave(connection));
    std::future::poll_fn(|cx| {
        assert!(leaving.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(permit);
    leaving.await;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            if let Ok(output) = ticks.recv().await
                && output.snapshot.entities.get(entity).is_none()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    core.shutdown();
}

#[test]
fn session_cleanup_has_reserved_capacity_and_preserves_fifo() {
    let (sender, mut receiver) = mpsc_channel(4);
    let connection = ConnectionId(101);
    let session = PlayerSession::admit(
        &sender,
        connection,
        uuid::Uuid::from_u128(101),
        "Session".to_owned(),
        0,
        4,
        "ru_ru".into(),
    )
    .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::SetViewDistance { connection: id, .. } if id == connection)
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::SetLocale { connection: id, locale } if id == connection && locale == "ru_ru")
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::Join { connection: id, .. } if id == connection)
    );
    for _ in 0..3 {
        sender
            .try_send(PlayerInput::SelectSlot {
                connection,
                slot: 2,
            })
            .unwrap();
    }
    assert!(
        sender
            .try_send(PlayerInput::SelectSlot {
                connection,
                slot: 3
            })
            .is_err()
    );
    drop(session);
    for _ in 0..3 {
        assert!(matches!(
            receiver.try_recv().unwrap(),
            PlayerInput::SelectSlot { slot: 2, .. }
        ));
    }
    assert!(
        matches!(receiver.try_recv().unwrap(), PlayerInput::Leave { connection: id } if id == connection)
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn failed_session_admission_emits_no_partial_join() {
    let (sender, mut receiver) = mpsc_channel(4);
    sender
        .try_send(PlayerInput::SelectSlot {
            connection: ConnectionId(1),
            slot: 0,
        })
        .unwrap();
    assert!(matches!(
        PlayerSession::admit(
            &sender,
            ConnectionId(2),
            uuid::Uuid::nil(),
            "Full".into(),
            0,
            2,
            "en_us".into()
        ),
        Err(CoreError::InputFull)
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        PlayerInput::SelectSlot { .. }
    ));
    assert!(receiver.try_recv().is_err());
    drop(receiver);
    assert!(matches!(
        PlayerSession::admit(
            &sender,
            ConnectionId(2),
            uuid::Uuid::nil(),
            "Stopped".into(),
            0,
            2,
            "en_us".into()
        ),
        Err(CoreError::TickStopped)
    ));
}

#[test]
fn session_drop_allows_immediate_rejoin_with_saved_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(dir.path().join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.input_queue_cap = 8;
    config.view_distance = 2;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    let core = Core::open(config).unwrap();
    let mut outputs = core.subscribe();
    let connection = ConnectionId(101);
    let uuid = uuid::Uuid::from_u128(101);
    let item = core
        .registry()
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let session = core
        .join_session(connection, uuid, "Saved".into(), 4, 2, "en_us".into())
        .unwrap();
    let mut joined = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        joined |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes
                    .iter()
                    .any(|change| matches!(change, PlayerChange::Joined { .. }))
        });
        if joined {
            break;
        }
    }
    assert!(joined);
    assert!(core.submit(PlayerInput::Command {
        connection,
        command: "gamemode creative".into(),
        permission_level: 4
    }));
    // Commands cross the shared dimension boundary after ordinary input. Wait
    // for committed creative mode before sending a creative-only inventory edit.
    let mut creative = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        creative |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes
                    .iter()
                    .any(|change| matches!(change, PlayerChange::GameModeChanged { game_mode: 1 }))
        });
        if creative {
            break;
        }
    }
    assert!(creative, "creative mode must commit before inventory setup");
    assert!(core.submit(PlayerInput::CreativeSlot {
        connection,
        slot: 36,
        stack: mc_data::ItemStack::new(item, 7)
    }));
    let mut given = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        given |= core.try_drain_outbox().iter().any(|(id, changes)| {
            *id == connection
                && changes.iter().any(|change| match change {
                    PlayerChange::MenuSlot {
                        slot: 36,
                        item: Some(stack),
                        ..
                    } => stack.item_id == item && stack.count == 7,
                    PlayerChange::MenuContents { slots, .. } => slots
                        .iter()
                        .flatten()
                        .any(|stack| stack.item_id == item && stack.count == 7),
                    _ => false,
                })
        });
        if given {
            break;
        }
    }
    assert!(given, "inventory setup must commit before disconnect");
    let ordinary_credit = core
        .inner
        .input_tx
        .try_reserve_many(core.inner.input_tx.capacity())
        .unwrap();
    assert!(!core.submit(PlayerInput::SelectSlot {
        connection,
        slot: 3
    }));
    drop(session);
    drop(ordinary_credit);
    let rejoined = ConnectionId(102);
    let next = core
        .join_session(rejoined, uuid, "Saved".into(), 4, 2, "en_us".into())
        .unwrap();
    let mut restored = false;
    for _ in 0..20 {
        outputs.blocking_recv().unwrap();
        for (id, changes) in core.try_drain_outbox() {
            if id == rejoined {
                assert!(
                    !changes
                        .iter()
                        .any(|change| matches!(change, PlayerChange::Disconnect { .. })),
                    "old session must not block rejoin"
                );
                restored |= changes.iter().any(|change| matches!(change, PlayerChange::MenuContents { slots, .. } if slots.iter().flatten().any(|stack| stack.item_id == item && stack.count == 7)));
            }
        }
        if restored {
            break;
        }
    }
    assert!(
        restored,
        "the real native rejoin restores the old inventory"
    );
    drop(next);
    core.shutdown();
}
