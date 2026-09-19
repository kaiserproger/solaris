//! The P3 player vertical: the bounded query the contract already carries and the
//! teleport that joins it, both driven by the real component through the real
//! boundary.
//!
//! Every case here exists to pin one property of the migration rather than the
//! plumbing under it:
//!
//! * a guest's teleport becomes the exact DTO the server applies - the request id
//!   it chose, the session it named and the coordinates it sent, with nothing
//!   invented in between;
//! * a package that never declared `player_teleport` is refused by *name*, both
//!   by the adapter's conversion and by the deployment that loses the route;
//! * a request past a bound the contract declares is refused, never clamped to
//!   the bound - a plugin that asked for zero players must not be handed 256;
//! * the server's own typed answer comes back to the guest as the contract's
//!   event, correlated by the request id the plugin chose, and an answer another
//!   package asked for never reaches this instance.

mod fixture;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixture::component_bytes;
use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::bindings::solaris::plugin::commands::{
    Command, ListOnlinePlayers, TeleportPlayer,
};
use mc_plugin_host::bindings::solaris::plugin::types::Position;
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, HostQueues, HostServices,
    LoadedPackage, LogLevel, NoSessions, PlayerSessions, PluginInstance, PluginLimits,
    PluginStartup, start_deployment, to_script_batch,
};
use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, CommandCapabilities, HostCommandAdmission,
    MAX_ONLINE_PLAYER_QUERY_LIMIT, MAX_SCRIPT_ID_BYTES, SCRIPT_HORIZONTAL_COORDINATE_LIMIT,
    SCRIPT_VERTICAL_COORDINATE_LIMIT, ScriptCommand, ScriptEvent, ScriptOnlinePlayersRequest,
    ScriptPlayerContext, ScriptPlayerId, ScriptPlayerTeleportFailure, ScriptPlayerTeleportRequest,
    ScriptPluginManifest, ScriptPosition,
};

/// The one player every case knows, and the session their connection holds.
const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const SESSION: u64 = 7;

/// The declaration an operator writes to grant the teleport, in the manifest's
/// own capability vocabulary.
const TELEPORT_CAPABILITY: &str = "capabilities = [\"player_teleport\"]\n";

/// The services the fixture plugin is given: it has an id, and it logs.
#[derive(Default)]
struct Services {
    id: String,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {}

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// A resolver that knows exactly one player, as a live server would.
struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER_UUID).then_some(SESSION)
    }
}

/// Instantiate the shared fixture with `config` as its package's `config.toml`.
///
/// The two startup phases run in the two stores the host uses: the startup phase
/// in a store of its own that is dropped here, `init` in the runtime store this
/// answers with.
fn guest(config: &str) -> PluginInstance<Services> {
    let limits = PluginLimits::default();
    let bytes = component_bytes();
    let engine = mc_plugin_host::engine(&limits).expect("engine");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = mc_plugin_host::linker::<Services>(&engine).expect("linker");
    let _ = PluginStartup::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: "hello".to_owned(),
        },
        limits,
    )
    .expect("the startup store instantiates")
    .configure(config)
    .expect("configure");
    let mut instance = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: "hello".to_owned(),
        },
        limits,
    )
    .expect("instantiate");
    instance
        .init(
            config,
            InitContext {
                plugin_id: "hello".to_owned(),
                api_version: "0.7.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect("init");
    instance
}

/// The join one guest callback answers, in the contract's own shape.
fn join_event() -> Event {
    Event::PlayerJoined(PlayerJoined {
        player: PLAYER_UUID.to_owned(),
        session: SESSION,
        name: "Ada".to_owned(),
    })
}

/// The grants of a package that declares exactly these capabilities, by the
/// names a manifest writes.
///
/// The declaration is the manifest's own vocabulary - the same strings an
/// operator writes and the refusal names - so no case here can pass by inventing
/// a grant the deployment could never produce.
fn grants(declarations: &[&str]) -> CommandCapabilities {
    let mut manifest =
        ScriptPluginManifest::new("hello", "Hello", "0.1.0", COMPONENT_PLUGIN_API_VERSION);
    for declaration in declarations {
        manifest = manifest
            .declare_capability(declaration)
            .expect("a capability the contract's vocabulary knows");
    }
    let validated = manifest
        .validate_for(COMPONENT_PLUGIN_API_VERSION)
        .expect("a component manifest validates");
    HostCommandAdmission::from_manifest(&validated)
        .capabilities()
        .clone()
}

/// One teleport as the contract's own record spells it.
fn teleport(request: &str, session: u64, x: f64, y: f64, z: f64) -> Command {
    Command::TeleportPlayer(TeleportPlayer {
        request: request.to_owned(),
        session,
        position: Position { x, y, z },
    })
}

/// One query as the contract's own record spells it.
fn query(request: &str, limit: u32) -> Command {
    Command::ListOnlinePlayers(ListOnlinePlayers {
        request: request.to_owned(),
        limit,
    })
}

/// Stage one contract command, the way a guest's callback stages its own.
fn stage(command: Command) -> CommandBatch {
    let mut batch = CommandBatch::new();
    batch
        .push(command, &PluginLimits::default())
        .expect("one command fits the staging bound");
    batch
}

/// Convert one staged batch with the grants named in `declarations`, and answer
/// the server's own DTO or the refusal.
fn convert(
    batch: CommandBatch,
    declarations: &[&str],
) -> Result<mc_script::CommandBatch, AdapterError> {
    to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        // A teleport names the session it was issued for, so the conversion needs
        // no lookup: a resolver that knows nobody is enough, and if the adapter
        // had grown one, this case would refuse a request a live server accepts.
        &NoSessions,
        &grants(declarations),
    )
}

#[test]
fn a_guest_teleport_becomes_the_dto_the_server_applies() {
    let mut plugin = guest("mode = \"teleport\"\n");
    let batch = plugin
        .on_events(
            EventContext {
                tick: 42,
                first_sequence: 0,
                count: 1,
            },
            &[join_event()],
        )
        .expect("the guest answers the join");
    let converted =
        convert(batch, &["player_teleport"]).expect("the guest's own teleport converts");

    assert_eq!(
        converted.commands(),
        [ScriptCommand::TeleportPlayer {
            request: ScriptPlayerTeleportRequest::try_new(
                "warp-home",
                ScriptPlayerId::new(SESSION),
                ScriptPosition::try_new(12.5, 70.0, -4.5).expect("the fixture's coordinates"),
            )
            .expect("the fixture's request"),
        }],
        "the request id, the session and the coordinates are the guest's own"
    );
}

#[test]
fn a_teleport_the_manifest_does_not_grant_is_refused_by_name() {
    let mut plugin = guest("mode = \"teleport\"\n");
    let batch = plugin
        .on_events(
            EventContext {
                tick: 42,
                first_sequence: 0,
                count: 1,
            },
            &[join_event()],
        )
        .expect("the guest answers the join");
    // The very same batch the granted package converts above.
    let refusal = to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        &NoSessions,
        &grants(&[]),
    )
    .expect_err("a package that never declared the teleport cannot teleport");
    assert_eq!(
        refusal,
        AdapterError::PermissionDenied {
            capability: "player_teleport"
        },
        "the refusal names the capability the manifest would have declared"
    );
    assert!(
        refusal.to_string().contains("player_teleport"),
        "an operator reads the same name, saw {refusal}"
    );
}

#[test]
fn a_request_past_a_contract_bound_is_refused_rather_than_clamped() {
    let long_id = "r".repeat(MAX_SCRIPT_ID_BYTES + 1);
    for (case, command) in [
        (
            "an empty correlation id",
            teleport("", SESSION, 12.5, 70.0, -4.5),
        ),
        (
            "an over-long correlation id",
            teleport(&long_id, SESSION, 12.5, 70.0, -4.5),
        ),
        (
            "a correlation id the contract does not accept",
            teleport("warp home", SESSION, 12.5, 70.0, -4.5),
        ),
        (
            "a not-a-number coordinate",
            teleport("warp-home", SESSION, f64::NAN, 70.0, -4.5),
        ),
        (
            "an infinite coordinate",
            teleport("warp-home", SESSION, 12.5, f64::INFINITY, -4.5),
        ),
        (
            "a horizontal coordinate past the server's limit",
            teleport(
                "warp-home",
                SESSION,
                SCRIPT_HORIZONTAL_COORDINATE_LIMIT + 1.0,
                70.0,
                -4.5,
            ),
        ),
        (
            "a vertical coordinate past the server's limit",
            teleport(
                "warp-home",
                SESSION,
                12.5,
                SCRIPT_VERTICAL_COORDINATE_LIMIT + 1.0,
                -4.5,
            ),
        ),
    ] {
        let refusal = convert(stage(command), &["player_teleport"]).expect_err(
            "a value outside the contract's bound is the plugin's own malformed answer",
        );
        assert!(
            matches!(refusal, AdapterError::InvalidCommand { .. }),
            "{case} must be refused as an invalid command, saw {refusal:?}"
        );
    }

    // The query's limit is the plugin's own bound on the answer the server builds
    // for it: a value the contract does not admit is refused rather than clamped
    // to the maximum, because a plugin that asked for nothing must not be handed
    // 256 players it never asked for.
    for (case, command) in [
        ("a limit of zero", query("who", 0)),
        (
            "a limit past the contract's maximum",
            query(
                "who",
                u32::try_from(MAX_ONLINE_PLAYER_QUERY_LIMIT).expect("fits") + 1,
            ),
        ),
        ("a limit no snapshot could satisfy", query("who", u32::MAX)),
        ("an over-long correlation id", query(&long_id, 8)),
        (
            "a correlation id the contract does not accept",
            query("who?", 8),
        ),
    ] {
        let refusal = convert(stage(command), &["player_queries"]).expect_err(
            "a value outside the contract's bound is the plugin's own malformed answer",
        );
        assert!(
            matches!(refusal, AdapterError::InvalidCommand { .. }),
            "{case} must be refused as an invalid command, saw {refusal:?}"
        );
    }

    // The bounds themselves are the contract's, so a value at them is carried
    // unchanged: refused is not the same as moved to the edge, and a plugin that
    // stays inside the contract reads its own numbers back.
    let inside = convert(
        stage(teleport(
            &"r".repeat(MAX_SCRIPT_ID_BYTES),
            SESSION,
            SCRIPT_HORIZONTAL_COORDINATE_LIMIT,
            SCRIPT_VERTICAL_COORDINATE_LIMIT,
            -SCRIPT_HORIZONTAL_COORDINATE_LIMIT,
        )),
        &["player_teleport"],
    )
    .expect("the bounds themselves are admissible");
    assert_eq!(
        inside.commands(),
        [ScriptCommand::TeleportPlayer {
            request: ScriptPlayerTeleportRequest::try_new(
                "r".repeat(MAX_SCRIPT_ID_BYTES),
                ScriptPlayerId::new(SESSION),
                ScriptPosition::try_new(
                    SCRIPT_HORIZONTAL_COORDINATE_LIMIT,
                    SCRIPT_VERTICAL_COORDINATE_LIMIT,
                    -SCRIPT_HORIZONTAL_COORDINATE_LIMIT,
                )
                .expect("the limits themselves are inside the limits"),
            )
            .expect("a 64-byte id is the contract's own bound"),
        }]
    );

    let at_the_limit = convert(
        stage(query(
            "who",
            u32::try_from(MAX_ONLINE_PLAYER_QUERY_LIMIT).expect("fits"),
        )),
        &["player_queries"],
    )
    .expect("the contract's own limit is admissible");
    assert_eq!(
        at_the_limit.commands(),
        [ScriptCommand::ListOnlinePlayers {
            request: ScriptOnlinePlayersRequest::try_new("who", MAX_ONLINE_PLAYER_QUERY_LIMIT)
                .expect("the contract's own limit"),
        }]
    );
}

/// One package directory of the fixture, with the manifest an operator writes.
fn write_package(root: &Path, id: &str, manifest_extra: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n\
             events = [\"player.joined\", \"player.teleport_result\"]\n\
             player_commands = [\"{id}\"]\n{manifest_extra}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
}

/// Discover the packages an operator placed under `root`.
fn deployment(root: &Path, ids: &[&str]) -> Vec<LoadedPackage> {
    let limits = PluginLimits::default();
    mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: ids.iter().map(|id| (*id).to_owned()).collect(),
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages()
}

/// The join a live server would hand the host, for one connection.
fn server_join(session: u64) -> ScriptEvent {
    let context = ScriptPlayerContext::try_new(PLAYER_UUID, "Ada", false, 0.0, 64.0, 0.0)
        .expect("player context");
    ScriptEvent::player_joined_with_context(ScriptPlayerId::new(session), context)
}

/// The chat line one admitted command carries, with the package that answered.
///
/// The provenance is the host's own: a guest cannot choose it, which is what
/// makes "who answered" a fact of the test rather than of the guest's story.
fn chat_answer(command: &ScriptCommand) -> (&str, &str) {
    let ScriptCommand::HostAttached {
        provenance,
        request,
    } = command
    else {
        panic!("expected an admitted command, saw {command:?}");
    };
    let ScriptCommand::SendChatMessage { message, .. } = request.as_ref() else {
        panic!("expected the guest's chat answer, saw {request:?}");
    };
    (provenance.plugin_id(), message)
}

#[tokio::test]
async fn a_package_that_never_declared_the_capability_loses_its_route_instead_of_teleporting() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        "",
        "greeting = \"Hi there\"\nmode = \"teleport\"\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package starts registered, so losing the route is the event under test"
    );

    boundary
        .try_enqueue_event(server_join(SESSION))
        .expect("event queued");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), boundary.recv_command())
            .await
            .is_err(),
        "an ungranted teleport is refused before admission, not handed to the server"
    );
    assert!(
        boundary.player_command_roots().is_empty(),
        "a batch refused for a missing capability retires the instance that cannot be granted it"
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one instance ran");
    assert_eq!(
        counters[0].1.commands_refused, 1,
        "the refusal is counted against the batch, not the instance's health"
    );
    assert_eq!(counters[0].1.commands_submitted, 0);
}

#[tokio::test]
async fn the_servers_typed_teleport_answer_returns_to_the_guest_that_asked() {
    // Two phases of the same operation: the guest asks, the server's DTO carries
    // the request, its own typed result comes back, and what the guest does with
    // the answer is what a player sees. Both the committed pose and a refusal are
    // read back, so neither half can pass by the guest reporting a constant.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        TELEPORT_CAPABILITY,
        "greeting = \"Hi there\"\nmode = \"teleport\"\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();

    boundary
        .try_enqueue_event(server_join(SESSION))
        .expect("event queued");
    let asked = boundary
        .recv_command()
        .await
        .expect("the teleport reaches the boundary");
    let admitted = boundary
        .accept_host_command(asked)
        .expect("the server accepts what the host attached");
    assert!(matches!(
        admitted.request(),
        ScriptCommand::TeleportPlayer { .. }
    ));

    boundary
        .try_enqueue_event(
            admitted
                .player_teleport_result(Some(ScriptPlayerTeleportFailure::TeleportPending))
                .expect("a teleport result answers a teleport"),
        )
        .expect("result queued");
    let refused = boundary
        .recv_command()
        .await
        .expect("the guest answers the result");
    assert_eq!(
        chat_answer(&refused),
        (
            "hello",
            "Hi there warp-home 7 refused teleport-pending at 12.5/70/-4.5"
        ),
        "the plugin reads the request it chose, the session it named, the exact \
         coordinates and the server's own reason"
    );

    // The same operation, on a second connection, committing: the guest's report
    // is the answer the server gave, correlated again by the plugin's own id.
    boundary
        .try_enqueue_event(server_join(SESSION + 1))
        .expect("event queued");
    let asked = boundary
        .recv_command()
        .await
        .expect("the second teleport reaches the boundary");
    let admitted = boundary
        .accept_host_command(asked)
        .expect("the server accepts what the host attached");
    boundary
        .try_enqueue_event(
            admitted
                .player_teleport_result(None)
                .expect("a committed teleport is a teleport result"),
        )
        .expect("result queued");
    let committed = boundary
        .recv_command()
        .await
        .expect("the guest answers the result");
    assert_eq!(
        chat_answer(&committed),
        ("hello", "Hi there warp-home 8 committed at 12.5/70/-4.5"),
        "a committed teleport echoes the request and the pose the owner took"
    );

    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "answering its own request costs the instance nothing"
    );
    host.stop();
}

#[tokio::test]
async fn a_teleport_answer_reaches_only_the_package_that_asked() {
    // Two live packages, each with the teleport granted and each asking on the
    // same join. An answer is targeted by plugin id, so the package that did not
    // issue the request must stay silent: if the host handed every result to
    // every instance, the second package's guest answers its own request only
    // after this phase fails.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        TELEPORT_CAPABILITY,
        "greeting = \"Hello\"\nmode = \"teleport\"\n",
    );
    write_package(
        root.path(),
        "other",
        TELEPORT_CAPABILITY,
        "greeting = \"Other\"\nmode = \"teleport\"\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello", "other"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned(), "other".to_owned()],
        "both packages are live, so silence below is targeting rather than a dead guest"
    );

    boundary
        .try_enqueue_event(server_join(SESSION))
        .expect("event queued");
    let mut asked = BTreeMap::new();
    for _ in 0..2 {
        let command = boundary
            .recv_command()
            .await
            .expect("both packages answer the join");
        let asker = match &command {
            ScriptCommand::HostAttached {
                provenance,
                request,
            } => {
                assert!(matches!(
                    request.as_ref(),
                    ScriptCommand::TeleportPlayer { .. }
                ));
                provenance.plugin_id().to_owned()
            }
            other => panic!("expected an admitted command, saw {other:?}"),
        };
        let admitted = boundary
            .accept_host_command(command)
            .expect("the server accepts what the host attached");
        asked.insert(asker, admitted);
    }

    boundary
        .try_enqueue_event(
            asked
                .remove("hello")
                .expect("hello asked")
                .player_teleport_result(Some(ScriptPlayerTeleportFailure::PlayerUnavailable))
                .expect("a teleport result answers a teleport"),
        )
        .expect("result queued");
    let answer = boundary
        .recv_command()
        .await
        .expect("the asking package answers");
    assert_eq!(
        chat_answer(&answer),
        (
            "hello",
            "Hello warp-home 7 refused player-unavailable at 12.5/70/-4.5"
        )
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), boundary.recv_command())
            .await
            .is_err(),
        "the package that did not ask is not answered"
    );

    boundary
        .try_enqueue_event(
            asked
                .remove("other")
                .expect("other asked")
                .player_teleport_result(None)
                .expect("a committed teleport is a teleport result"),
        )
        .expect("result queued");
    let answer = boundary
        .recv_command()
        .await
        .expect("the other package answers its own request");
    assert_eq!(
        chat_answer(&answer),
        ("other", "Other warp-home 7 committed at 12.5/70/-4.5")
    );
    host.stop();
}
