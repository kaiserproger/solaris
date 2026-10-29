use super::*;

#[test]
fn invalid_utf8_line_does_not_stop_console_reader() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    read_console(std::io::Cursor::new(b"bad\xff\nlist\nstop\n"), sender);
    let lines: Vec<_> = std::iter::from_fn(|| receiver.blocking_recv())
        .filter_map(Result::ok)
        .collect();
    assert_eq!(lines, ["list", "stop"]);
}

pub(crate) fn server(root: &std::path::Path) -> EdgeServer {
    let mut config = mc_core::CoreConfig::new(root.join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.view_distance = 0;
    config.simulation_distance = 0;
    let core = Arc::new(mc_core::Core::open(config).unwrap());
    let mut cfg = crate::config::EdgeConfig {
        server_dir: root.join("server"),
        ..Default::default()
    };
    std::fs::create_dir_all(&cfg.server_dir).unwrap();
    cfg.data.world_dir = root.join("world");
    cfg.data.vanilla_data_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    cfg.auth.online_mode = false;
    let data = crate::server::load_vanilla_data(&cfg).unwrap();
    let tags = crate::server::load_tags(&cfg, &data).unwrap();
    EdgeServer::new(core, data, tags, cfg).unwrap()
}

#[tokio::test]
async fn console_runs_native_and_admin_commands_and_stop_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path());
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute(&server, "/time set 4321"),
    )
    .await
    .unwrap()
    .unwrap();
    execute(&server, "op Alice").await.unwrap();
    let ops: serde_json::Value =
        serde_json::from_slice(&std::fs::read(server.cfg.server_dir.join("ops.json")).unwrap())
            .unwrap();
    assert_eq!(ops[0]["name"], "Alice");
    assert_eq!(ops[0]["level"], 4);
    // No default executor is invented; missing self targets return normal feedback.
    execute(&server, "gamemode creative").await.unwrap();
    let changes: Vec<_> = server
        .hub
        .try_drain_outbox()
        .into_iter()
        .flat_map(|(_, changes)| changes)
        .collect();
    assert!(format!("{changes:?}").contains("argument.entity.notfound.player"));
    execute(&server, "stop extra").await.unwrap();
    execute(&server, "stop").await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let world = server.cfg.data.world_dir.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        crate::server::serve(listener, server),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(world.join("native/state.redb").is_file());
    let reopened = mc_core::Core::open({
        let mut config = mc_core::CoreConfig::new(world, 42);
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        config
    })
    .unwrap();
    reopened.shutdown();
}

#[test]
fn console_feedback_keeps_translated_arguments_and_literal_text() {
    let language = [("commands.time.set".into(), "Set the time to %s".into())].into();
    let content = mc_nbt::Tag::Compound(vec![
        (
            "translate".into(),
            mc_nbt::Tag::String("commands.time.set".into()),
        ),
        (
            "with".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: vec![mc_nbt::Tag::String("4321".into())],
            }),
        ),
    ]);
    assert_eq!(component_text(&content, &language), "Set the time to 4321");
    assert_eq!(
        component_text(&mc_nbt::Tag::String("saved".into()), &language),
        "saved"
    );
}

#[test]
fn console_substitutes_indexed_nested_components() {
    let language = [
        ("outer".into(), "%2$s then %1$s and %%".into()),
        ("inner".into(), "diamond".into()),
    ]
    .into();
    let content = mc_nbt::Tag::Compound(vec![
        ("translate".into(), mc_nbt::Tag::String("outer".into())),
        (
            "with".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![
                    mc_nbt::Tag::Compound(vec![(
                        "text".into(),
                        mc_nbt::Tag::String("Alice".into()),
                    )]),
                    mc_nbt::Tag::Compound(vec![(
                        "translate".into(),
                        mc_nbt::Tag::String("inner".into()),
                    )]),
                ],
            }),
        ),
    ]);
    assert_eq!(
        component_text(&content, &language),
        "diamond then Alice and %"
    );
}

#[test]
fn serverops_console_capture_is_bounded_and_cancellation_clears_it() {
    let state = ConsoleState::default();
    *state.response.lock() = Some(Ok("x".repeat(1024 * 1024)));
    state.feedback("overflow");
    assert!(state.response.lock().as_ref().unwrap().is_err());
    drop(Capture(&state));
    assert!(state.response.lock().is_none());
}

#[tokio::test]
async fn serverops_console_drain_wait_is_event_driven_and_handles_early_delivery() {
    let state = ConsoleState::default();
    state.drained(12);
    state.wait_drained(12).await;
    let waiting = state.wait_drained(13);
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    state.drained(13);
    waiting.await;
}
