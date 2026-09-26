//! The host runtime end to end: a real component, admitted through the real
//! boundary, answering a real server event.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fixture::component_bytes;
use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, HostServices, LogLevel, PlayerSessions,
    PluginLimits, load_package, start_deployment, start_deployment_with,
};
use mc_script::{ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId};

/// The session registry a live server owns: this test knows one player.
struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").then_some(7)
    }
}

/// A host service that is slow exactly where a guest spends no fuel: inside a
/// host import. Fuel bounds a guest that computes; only the epoch deadline ends a
/// guest that is blocked in the host.
struct SlowLog {
    id: String,
    slow_calls: Arc<AtomicUsize>,
}

impl HostServices for SlowLog {
    fn log(&mut self, _level: LogLevel, message: &str) {
        if message != "block" {
            return;
        }
        self.slow_calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

fn write_package(root: &Path) {
    write_package_with(root, "");
}

/// The same package, with one more manifest line: the capability a storage
/// command needs is granted by the manifest, exactly as the operator grants it.
fn write_package_with(root: &Path, manifest_extra: &str) {
    let directory = root.join("hello");
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"hello\"\nname = \"Hello\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\nplayer_commands = [\"hello\"]\n{manifest_extra}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), "greeting = \"Hi there\"\n").expect("config");
}

#[tokio::test]
async fn a_join_event_becomes_an_admitted_chat_command() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["hello".to_owned()],
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: Vec::new(),
    };
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(&config, &limits)
        .expect("deployment")
        .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package's declared command root is registered on the boundary"
    );

    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context,
        ))
        .expect("event queued");

    let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the host answers within the wait")
        .expect("the host answers");
    match command {
        // The command crosses the boundary with the host's own provenance: the
        // plugin id and a one-shot admission, never anything the guest chose.
        ScriptCommand::HostAttached {
            provenance,
            request,
        } => {
            assert_eq!(provenance.plugin_id(), "hello");
            match request.as_ref() {
                ScriptCommand::SendChatMessage { player_id, message } => {
                    assert_eq!(
                        *player_id,
                        ScriptPlayerId::new(7),
                        "the answer reaches the session that joined"
                    );
                    assert_eq!(message, "Hi there Ada");
                }
                other => panic!("expected one chat message, saw {other:?}"),
            }
        }
        other => panic!("expected an admitted chat message, saw {other:?}"),
    }

    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one instance ran");
    assert_eq!(counters[0].0, "hello");
    assert_eq!(counters[0].1.events_delivered, 1);
    assert_eq!(counters[0].1.commands_submitted, 1);
    assert_eq!(counters[0].1.commands_refused, 0);
}

#[tokio::test]
async fn a_guest_that_exhausts_its_startup_budget_refuses_the_deployment() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    // A second package whose guest can never finish its first callback.
    let doomed = root.path().join("doomed");
    std::fs::create_dir_all(&doomed).expect("package directory");
    std::fs::write(
        doomed.join("plugin.toml"),
        "id = \"doomed\"\nname = \"Doomed\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\n",
    )
    .expect("manifest");
    std::fs::write(doomed.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(doomed.join("config.toml"), "mode = \"spin\"\n").expect("config");

    let limits = PluginLimits {
        epoch_ticks_per_call: 4,
        ..PluginLimits::default()
    };
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["doomed".to_owned(), "hello".to_owned()],
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: Vec::new(),
    };
    let packages = mc_plugin_host::discover(&config, &limits)
        .expect("deployment")
        .into_packages();
    // `doomed` cannot start at all: its own init never returns, so the deployment
    // refuses to start rather than half-run with a package that is already dead.
    let error = match start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
    {
        Ok(_) => panic!("a package that cannot finish init must fail the deployment"),
        Err(error) => error,
    };
    assert!(format!("{error}").contains("doomed"), "{error}");
}

#[test]
fn a_deployment_loads_a_package_through_the_public_path() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let package = load_package(&root.path().join("hello"), &PluginLimits::default())
        .expect("the package loads");
    assert_eq!(package.manifest().plugin_id(), "hello");
}

#[tokio::test]
async fn a_refusing_guest_keeps_its_routes_and_a_dead_player_does_not_retire_it() {
    // Two legitimate refusals that must never cost an instance its registration:
    // the guest answers a plugin error, and a message names a player who holds no
    // session. Both drop the batch and leave the plugin serving.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    std::fs::write(
        root.path().join("hello/config.toml"),
        "greeting = \"Hi there\"\nmode = \"refuse\"\n",
    )
    .expect("refusing config");
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();

    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context,
        ))
        .expect("event queued");
    // No command is produced by a refusing guest, and the wait must not be the
    // assertion: the route staying registered is.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "a guest that answers with a plugin error keeps its routes"
    );

    let counters = host.stop();
    assert_eq!(counters[0].1.commands_submitted, 0);
    assert_eq!(
        counters[0].1.events_delivered, 0,
        "a refused batch is not reported as delivered commands"
    );
}

#[tokio::test]
async fn a_callback_that_traps_publishes_none_of_the_batch_it_built() {
    // The guest builds its batch *before* it faults, so this is an unpublished
    // batch rather than a missing one: the host may not publish a half-built
    // batch out of a guest that never returned them, and it may not keep serving
    // a route whose instance died inside its own callback.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    std::fs::write(
        root.path().join("hello/config.toml"),
        "mode = \"trap\"\ncount = 4\n",
    )
    .expect("trapping config");
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package starts registered, so losing the route is the event under test"
    );

    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        boundary.player_command_roots().is_empty(),
        "a guest that traps is retired and loses the routes it can no longer answer"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), boundary.recv_command())
            .await
            .is_err(),
        "none of the four commands the guest built reaches the server"
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one instance ran");
    assert_eq!(
        counters[0].1.commands_submitted, 0,
        "a trapped callback publishes nothing, however much it built first"
    );
    assert_eq!(
        counters[0].1.commands_refused, 0,
        "a trap is not a refused answer: nothing of the batch was ever admitted"
    );
    assert_eq!(
        counters[0].1.events_delivered, 0,
        "a callback that never returns delivers no events"
    );
}

#[tokio::test]
async fn a_message_to_an_offline_player_drops_the_batch_and_keeps_the_instance() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    // A resolver that knows nobody: every message to a stable identity fails.
    struct Nobody;
    impl PlayerSessions for Nobody {
        fn session_of(&self, _player: &str) -> Option<u64> {
            None
        }
    }
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Nobody))
        .expect("host starts");
    let boundary = host.boundary().clone();
    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context,
        ))
        .expect("event queued");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "a player who left before admission costs the batch, not the plugin"
    );
    let counters = host.stop();
    assert_eq!(counters[0].1.commands_submitted, 0);
    assert_eq!(counters[0].1.commands_refused, 1);
}

#[tokio::test]
async fn a_guest_blocked_in_a_host_import_is_ended_by_the_epoch_deadline() {
    // The ticker belongs to the host, not to the caller: a host that dropped it
    // before serving would leave this guest blocked in `log` until it finished on
    // its own, and `epoch_ticks_per_call` would be decorative.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    std::fs::write(root.path().join("hello/config.toml"), "mode = \"block\"\n")
        .expect("blocking config");
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let slow_calls = Arc::new(AtomicUsize::new(0));
    let host = start_deployment_with(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(Sessions),
        {
            let slow_calls = Arc::clone(&slow_calls);
            move |id: &str| SlowLog {
                id: id.to_owned(),
                slow_calls: Arc::clone(&slow_calls),
            }
        },
    )
    .expect("host starts - the slow import is only reached from a callback");
    let boundary = host.boundary().clone();
    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context,
        ))
        .expect("event queued");

    // One slow import is 500 ms, the deadline is 4 ticks of 25 ms, and the guest
    // calls the slow import eight times: a live watchdog ends it inside the first
    // few, a dead one lets it log all eight.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let calls = slow_calls.load(Ordering::SeqCst);
    assert!(
        calls >= 1,
        "the guest must reach the slow import, otherwise this proves nothing"
    );
    assert!(
        boundary.player_command_roots().is_empty(),
        "a guest past its deadline is retired and loses its routes, after {calls} slow imports"
    );
    assert!(
        calls < 8,
        "the guest was interrupted inside the import, not left to finish: {calls} slow imports"
    );
    // The retired instance is still shut down and reported.
    let counters = host.stop();
    assert_eq!(counters.len(), 1);
}

#[tokio::test]
async fn a_full_command_queue_drops_the_batch_and_keeps_the_instance() {
    // Backpressure is not misbehaviour: with a command queue of one and a guest
    // answering two messages, the batch comes back refused and the plugin keeps
    // serving. A retirement here would unregister `/hello` for the whole process
    // because the server happened to be busy.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    std::fs::write(root.path().join("hello/config.toml"), "mode = \"burst\"\n")
        .expect("burst config");
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues {
            events: 16,
            commands: 1,
        },
        Arc::new(Sessions),
    )
    .expect("host starts");
    let boundary = host.boundary().clone();
    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context,
        ))
        .expect("event queued");

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "a refused batch costs the commands, not the plugin's routes"
    );
    let counters = host.stop();
    assert_eq!(
        counters[0].1.commands_submitted, 0,
        "a batch that does not fit is refused whole, never half-submitted"
    );
    assert_eq!(
        counters[0].1.commands_refused, 2,
        "both messages of the refused batch are counted"
    );
}

/// The manifest line that grants a package the durable storage it asks for.
const STORAGE_CAPABILITY: &str = "capabilities = [\"storage\"]\n";

fn storage_deployment(root: &Path, capability: bool) -> Vec<mc_plugin_host::LoadedPackage> {
    write_package_with(root, if capability { STORAGE_CAPABILITY } else { "" });
    std::fs::write(
        root.join("hello/config.toml"),
        "greeting = \"Hi there\"\nmode = \"storage\"\n",
    )
    .expect("storage config");
    let limits = PluginLimits::default();
    mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages()
}

fn join_event() -> ScriptEvent {
    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "Ada",
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    ScriptEvent::player_joined_with_context(ScriptPlayerId::new(7), context)
}

#[tokio::test]
async fn a_storage_read_becomes_the_servers_own_request_and_its_answer_returns_to_the_plugin() {
    // The whole two-phase path through the real boundary: the guest asks, the
    // server's own storage DTO carries the request, the server's typed result
    // comes back, and the guest's reaction to the value it was given is what a
    // player sees. The request is validated at the component boundary.
    let root = tempfile::tempdir().expect("deployment root");
    let limits = PluginLimits::default();
    let packages = storage_deployment(root.path(), true);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");

    // Phase one: the guest's read and swap reach the boundary as server DTOs.
    let mut reads = 0;
    let mut swaps = 0;
    let mut asker = None;
    for _ in 0..2 {
        let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .expect("the host answers within the wait")
            .expect("the host answers");
        match command {
            ScriptCommand::HostAttached {
                provenance,
                request,
            } => {
                assert_eq!(provenance.plugin_id(), "hello");
                match request.as_ref() {
                    ScriptCommand::PluginStorageGet { request } => {
                        assert_eq!(request.request_id(), "coins", "the guest's correlation id");
                        assert_eq!(request.key(), "coins:player");
                        asker = Some(request.clone());
                        reads += 1;
                    }
                    ScriptCommand::PluginStorageCompareAndSwap { request } => {
                        assert_eq!(request.request_id(), "bump");
                        assert_eq!(request.key(), "coins:player");
                        assert_eq!(request.expected_version(), None, "only if absent");
                        assert_eq!(request.value(), "3");
                        swaps += 1;
                    }
                    other => panic!("expected a storage request, saw {other:?}"),
                }
            }
            other => panic!("expected an admitted command, saw {other:?}"),
        }
    }
    assert_eq!((reads, swaps), (1, 1), "the guest asked once for each");
    let get = asker.expect("the read was seen");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), boundary.recv_command())
            .await
            .is_err(),
        "a storage read produces exactly two requests and no player-visible noise"
    );

    // Phase two: the server's own result event reaches the guest, which reports
    // the value it was given.
    boundary
        .try_enqueue_event(
            ScriptEvent::plugin_storage_get_result("hello", &get, Some("7".to_owned()), Some(4))
                .expect("result event"),
        )
        .expect("result queued");
    let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the guest answers within the wait")
        .expect("the guest answers");
    match command {
        ScriptCommand::HostAttached { request, .. } => match request.as_ref() {
            ScriptCommand::SendChatMessage { player_id, message } => {
                assert_eq!(player_id.value(), 7);
                assert_eq!(
                    message, "Hi there coins 7 at 4",
                    "the player sees exactly what storage reported"
                );
            }
            other => panic!("expected the guest's report, saw {other:?}"),
        },
        other => panic!("expected an admitted command, saw {other:?}"),
    }
    host.stop();
}

/// A result created after reload still belongs to the Store that issued its
/// request, even when the replacement immediately reuses the same request id.
#[tokio::test]
async fn late_storage_reply_does_not_answer_the_replacement_guest() {
    let root = tempfile::tempdir().expect("deployment root");
    let limits = PluginLimits {
        epoch_ticks_per_call: 64,
        ..PluginLimits::default()
    };
    let host = start_deployment(
        storage_deployment(root.path(), true),
        limits,
        HostQueues::default(),
        Arc::new(Sessions),
    )
    .expect("host starts");
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(join_event()).unwrap();
    let mut old_read = None;
    for _ in 0..2 {
        let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .expect("old guest answered")
            .expect("old guest command");
        let admitted = boundary
            .accept_host_command(command)
            .expect("host admission");
        if matches!(admitted.request(), ScriptCommand::PluginStorageGet { .. }) {
            old_read = Some(admitted);
        }
    }

    host.reload(storage_deployment(root.path(), true))
        .await
        .expect("same-contract replacement starts");
    boundary.try_enqueue_event(join_event()).unwrap();
    let mut new_read = None;
    for _ in 0..2 {
        let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .expect("new guest answered")
            .expect("new guest command");
        let admitted = boundary
            .accept_host_command(command)
            .expect("host admission");
        if matches!(admitted.request(), ScriptCommand::PluginStorageGet { .. }) {
            new_read = Some(admitted);
        }
    }

    boundary
        .try_enqueue_event(
            old_read
                .expect("old read")
                .plugin_storage_get_result(Some("7"), Some(4))
                .unwrap(),
        )
        .expect("old reply enters the bounded queue");
    boundary
        .try_enqueue_event(
            new_read
                .expect("new read")
                .plugin_storage_get_result(Some("9"), Some(5))
                .unwrap(),
        )
        .expect("new reply enters the bounded queue");
    let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("replacement guest reports its own read")
        .expect("replacement guest command");
    let ScriptCommand::HostAttached { request, .. } = command else {
        panic!("replacement report has host provenance");
    };
    assert!(
        matches!(
            request.as_ref(),
            ScriptCommand::SendChatMessage { message, .. } if message == "Hi there coins 9 at 5"
        ),
        "the late old result cannot be mistaken for the replacement's same-id read: {request:?}"
    );
    host.stop();
}

#[tokio::test]
async fn a_storage_failure_reaches_the_plugin_as_a_failure_not_as_an_empty_key() {
    // A server without world-backed storage, and a storage actor that stopped,
    // are different operator problems, and neither may look like "the key holds
    // nothing": a plugin that read a failure as an empty key would overwrite
    // durable data it believes is gone.
    let root = tempfile::tempdir().expect("deployment root");
    let limits = PluginLimits::default();
    let packages = storage_deployment(root.path(), true);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");
    // The server accepts the read through the same path its own router uses, and
    // answers a durability failure with the typed result the admission builds.
    // The swap the guest also issued is taken off the boundary first: this test
    // is about the read.
    let read = boundary
        .recv_command()
        .await
        .expect("the read reaches the boundary");
    let _swap = boundary
        .recv_command()
        .await
        .expect("the swap reaches the boundary");
    let admitted = boundary
        .accept_host_command(read)
        .expect("the server accepts what the host attached");
    let event = admitted
        .plugin_storage_failure_result(mc_script::ScriptPluginStorageFailure::DurabilityFailed)
        .expect("a durability failure is a valid answer to a read");
    boundary.try_enqueue_event(event).expect("result queued");
    let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the guest answers within the wait")
        .expect("the guest answers");
    match command {
        ScriptCommand::HostAttached { request, .. } => match request.as_ref() {
            ScriptCommand::SendChatMessage { message, .. } => assert_eq!(
                message, "Hi there storage durability-failed",
                "the plugin is told storage failed, not that the key is empty"
            ),
            other => panic!("expected the guest's report, saw {other:?}"),
        },
        other => panic!("expected an admitted command, saw {other:?}"),
    }
    host.stop();
}

#[tokio::test]
async fn a_storage_command_without_the_capability_is_refused_and_retires_the_instance() {
    // The capability gate is the server's, not the guest's: a package that asks
    // for durable storage it never declared is a package bug, and it is stopped
    // and un-routed instead of silently losing the commands.
    let root = tempfile::tempdir().expect("deployment root");
    let limits = PluginLimits::default();
    let packages = storage_deployment(root.path(), false);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package starts registered, so losing the route is the event under test"
    );
    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        boundary.player_command_roots().is_empty(),
        "a batch the server refused for a missing capability retires the instance"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), boundary.recv_command())
            .await
            .is_err(),
        "nothing of a refused batch is admitted"
    );
    host.stop();
}

#[tokio::test]
async fn an_online_players_query_carries_the_servers_own_snapshot_back_to_the_plugin() {
    // The second family of typed results: the guest asks a bounded question, the
    // server answers with the snapshot it built, and the host renames it without
    // re-deriving anything - the stable identity, the session and the dimension a
    // player sees all come from the server's own DTO.
    let root = tempfile::tempdir().expect("deployment root");
    write_package_with(root.path(), "capabilities = [\"player_queries\"]\n");
    std::fs::write(
        root.path().join("hello/config.toml"),
        "greeting = \"Hi there\"\nmode = \"players\"\n",
    )
    .expect("players config");
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");

    let query = boundary
        .recv_command()
        .await
        .expect("the query reaches the boundary");
    let ScriptCommand::HostAttached {
        provenance,
        request,
    } = &query
    else {
        panic!("expected an admitted command, saw {query:?}");
    };
    assert_eq!(provenance.plugin_id(), "hello");
    let ScriptCommand::ListOnlinePlayers { request } = request.as_ref() else {
        panic!("expected the online-players query, saw {request:?}");
    };
    assert_eq!(request.request_id(), "who");
    assert_eq!(
        request.limit(),
        8,
        "the plugin's own bound travels unchanged"
    );

    // The server answers with the snapshot only it can build.
    let ada = mc_script::ScriptOnlinePlayerSnapshot::try_new(
        ScriptPlayerId::new(7),
        ScriptPlayerContext::try_new(
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "Ada",
            false,
            0.0,
            64.0,
            0.0,
        )
        .expect("player context"),
        "minecraft:overworld",
    )
    .expect("snapshot");
    let admitted = boundary
        .accept_host_command(query)
        .expect("the server accepts what the host attached");
    boundary
        .try_enqueue_event(
            admitted
                .online_players_result(vec![ada], true)
                .expect("a query result answers a query"),
        )
        .expect("result queued");

    let answer = boundary
        .recv_command()
        .await
        .expect("the guest answers the query");
    let ScriptCommand::HostAttached { request, .. } = &answer else {
        panic!("expected an admitted command, saw {answer:?}");
    };
    match request.as_ref() {
        ScriptCommand::SendChatMessage { message, .. } => assert_eq!(
            message, "Hi there Ada@minecraft:overworld more",
            "the player sees the server's own name, dimension and truncation"
        ),
        other => panic!("expected the guest's report, saw {other:?}"),
    }
    host.stop();
}

#[tokio::test]
async fn a_result_addressed_to_another_plugin_is_not_delivered_to_this_instance() {
    // Results are targeted by plugin id, and the guest correlates them by a
    // request id it chose itself. The test reads the same event twice: once
    // addressed to another package, which must be silence, and once addressed to
    // this one, which must be answered - so neither half can pass by the host
    // having dropped the event wholesale.
    let root = tempfile::tempdir().expect("deployment root");
    let limits = PluginLimits::default();
    let packages = storage_deployment(root.path(), true);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(join_event())
        .expect("event queued");
    let read = boundary.recv_command().await.expect("the read arrives");
    let _swap = boundary.recv_command().await.expect("the swap arrives");
    let admitted = boundary
        .accept_host_command(read)
        .expect("the server accepts what the host attached");
    let ScriptCommand::PluginStorageGet { request } = admitted.request() else {
        panic!("expected the storage read, saw {:?}", admitted.request());
    };

    boundary
        .try_enqueue_event(
            ScriptEvent::plugin_storage_get_result("other", request, Some("1".to_owned()), Some(1))
                .expect("result event"),
        )
        .expect("result queued");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), boundary.recv_command())
            .await
            .is_err(),
        "another package's result is not this instance's answer"
    );

    boundary
        .try_enqueue_event(
            ScriptEvent::plugin_storage_get_result("hello", request, Some("7".to_owned()), Some(4))
                .expect("result event"),
        )
        .expect("result queued");
    let answer = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the guest answers its own result")
        .expect("the guest answers");
    match answer {
        ScriptCommand::HostAttached { request, .. } => match request.as_ref() {
            ScriptCommand::SendChatMessage { message, .. } => {
                assert_eq!(message, "Hi there coins 7 at 4");
            }
            other => panic!("expected the guest's report, saw {other:?}"),
        },
        other => panic!("expected an admitted command, saw {other:?}"),
    }
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "a foreign result costs the instance nothing"
    );
    host.stop();
}
