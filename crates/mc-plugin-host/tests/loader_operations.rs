//! The loader-live gate's own guest surface: two owners, one real component.
//!
//! `examples/loader-live-gate` deploys the same `solaris-hello-plugin` component
//! twice - as `ruby-live` and as `sapphire-live` - and the Loader gate is what a
//! real client does with the result. This file is the part of that gate a native
//! test can establish without a client: the component is really built and started,
//! every command it stages is admitted by the contract's own DTOs, and the
//! answers the owners would send are fed back exactly as the native owners build
//! them.
//!
//! What is asserted here is what the Loader platform depends on and nothing about
//! rendering:
//!
//! * each package answers only its own command root, and grants, opens, plays and
//!   stops only ids it owns;
//! * the input counters advance only from admitted `loader.view_action` events,
//!   are keyed by the session the event named, and start from zero on a
//!   reconnect;
//! * a refused open never becomes an instance this fixture presents against, and
//!   a present uses the revision the owner answered with rather than one this
//!   guest invented;
//! * one visible HUD is open at a time: the commands that arrive before the owner
//!   answers the open coalesce into the instance that answer names, and a hide
//!   among them closes that instance instead of being lost;
//! * a plugin id the fixture does not own fails its instance instead of being
//!   served as one of the two owners;
//! * a mode an owner has no answer for, or a world-sound command whose position
//!   does not parse, is reported as a usage line and stages no request at all.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, HostServices, LogLevel, PlayerSessions,
    PluginHost, PluginLimits, discover, start_deployment, start_deployment_with,
};
use mc_script::{
    AdmittedScriptCommand, ScriptBoundary, ScriptClientViewFailure, ScriptClientViewModel,
    ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId,
};
use sha2::{Digest, Sha256};

/// The session the gate's client connects on.
const SESSION: u64 = 7;

/// A second connection: a reconnect is a new session, which is the whole reason
/// this fixture keys its state by session rather than by player.
const RECONNECTED: u64 = 8;

/// The gate's offline player identity.
const PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// One deployed owner: its plugin id and the command root its manifest declares.
struct Owner {
    id: &'static str,
    command: &'static str,
    item: &'static str,
    show: &'static str,
    hud: &'static str,
    input: &'static str,
    tone: &'static str,
    foreign_tone: &'static str,
}

/// The two owners the canonical gate deploys.
const RUBY: Owner = Owner {
    id: "ruby-live",
    command: "loader_ruby",
    item: "ruby-live:ruby_block",
    show: "ruby-live:showcase",
    hud: "ruby-live:hud",
    input: "ruby-live:input",
    tone: "ruby-live:tone",
    foreign_tone: "sapphire-live:tone",
};

const SAPPHIRE: Owner = Owner {
    id: "sapphire-live",
    command: "loader_sapphire",
    item: "sapphire-live:sapphire_block",
    show: "sapphire-live:showcase",
    hud: "sapphire-live:hud",
    input: "sapphire-live:input",
    tone: "sapphire-live:tone",
    foreign_tone: "ruby-live:tone",
};

/// The identity the host resolves a stable player id through. Loader requests
/// name sessions, so nothing in this file depends on it beyond the trait's
/// contract.
struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER).then_some(SESSION)
    }
}

/// The guest diagnostics one deployment records, so a refusal the fixture only
/// reports in the log can be asserted instead of inferred from a missing line.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn recorded(&self) -> Vec<String> {
        self.0.lock().expect("the recorder is not poisoned").clone()
    }
}

struct Recorder {
    id: String,
    lines: Lines,
}

impl HostServices for Recorder {
    fn log(&mut self, _level: LogLevel, message: &str) {
        self.lines
            .0
            .lock()
            .expect("the recorder is not poisoned")
            .push(format!("{}: {message}", self.id));
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// One owner's package directory: the manifest the gate deploys and the
/// configuration that selects this fixture's mode.
fn write_package(root: &Path, owner: &Owner) {
    let directory = root.join(owner.id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{}\"\nname = \"{}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n\
             events = [\"player.joined\", \"player.left\"]\nplayer_commands = [\"{}\"]\n",
            owner.id, owner.id, owner.command
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), "mode = \"loader-live\"\n").expect("config");
}

fn packages(root: &Path, owners: &[&Owner]) -> Vec<mc_plugin_host::LoadedPackage> {
    let limits = PluginLimits::default();
    discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: owners.iter().map(|owner| owner.id.to_owned()).collect(),
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("the fixture packages are discoverable")
    .into_packages()
}

/// Start both owners with the process-wide tracing services.
fn start(root: &Path, owners: &[&Owner]) -> PluginHost {
    let limits = PluginLimits::default();
    start_deployment(
        packages(root, owners),
        limits,
        HostQueues::default(),
        Arc::new(Sessions),
    )
    .expect("the loader-live deployment starts")
}

/// The same, with every guest line recorded for this test to read.
fn start_recording(
    root: &Path,
    owners: &[&Owner],
) -> Result<(PluginHost, Lines), mc_plugin_host::HostStartError> {
    let limits = PluginLimits::default();
    let lines = Lines::default();
    let recorder = lines.clone();
    start_deployment_with(
        packages(root, owners),
        limits,
        HostQueues::default(),
        Arc::new(Sessions),
        move |id: &str| Recorder {
            id: id.to_owned(),
            lines: recorder.clone(),
        },
    )
    .map(|host| (host, lines))
}

fn context() -> ScriptPlayerContext {
    ScriptPlayerContext::try_new(PLAYER, "GateFabric", false, 0.0, 64.0, 0.0)
        .expect("the gate's own context is bounded")
}

fn joined(session: u64) -> ScriptEvent {
    ScriptEvent::player_joined_with_context(ScriptPlayerId::new(session), context())
}

fn left(session: u64) -> ScriptEvent {
    ScriptEvent::player_left(ScriptPlayerId::new(session), "gate disconnect")
}

/// Run one of this owner's commands on one connection, exactly as the server
/// routes a player command: the root decides the owner.
fn run(boundary: &ScriptBoundary, owner: &Owner, session: u64, arguments: &str) {
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(session),
            context(),
            &format!("{} {arguments}", owner.command),
        ),
        Ok(mc_script::PlayerCommandAdmission::Enqueued),
        "{} must own its own root",
        owner.id
    );
}

/// One command a guest staged, admitted by the contract's own DTOs.
async fn staged(boundary: &ScriptBoundary) -> AdmittedScriptCommand {
    let command = tokio::time::timeout(Duration::from_secs(15), boundary.recv_command())
        .await
        .expect("the guest answers or the host closes its command queue")
        .expect("the guest staged a command");
    boundary
        .accept_host_command(command)
        .expect("the guest's command is admissible")
}

/// One line a guest staged: the owner, the session it addressed, and the text.
///
/// Every line of this fixture addresses the session, never the stable identity,
/// so a line that named something else is a failure rather than a detail.
async fn line(boundary: &ScriptBoundary) -> (String, u64, String) {
    let admitted = staged(boundary).await;
    let owner = admitted.plugin_id().to_owned();
    let ScriptCommand::SendChatMessage { player_id, message } = admitted.request() else {
        panic!(
            "expected {owner} to answer with a line, got {:?}",
            admitted.request()
        );
    };
    (owner, player_id.value(), message.clone())
}

/// One view open a guest staged: the owner and the request it is awaiting.
async fn open(boundary: &ScriptBoundary) -> (String, mc_script::ScriptClientViewOpen) {
    let admitted = staged(boundary).await;
    let owner = admitted.plugin_id().to_owned();
    let (_, request) = admitted
        .into_open_client_view()
        .expect("the guest staged a view open");
    (owner, request)
}

/// The answer one open receives from its owner: the instance and revision the
/// server assigned, or the owner's own refusal.
fn opened(owner: &str, request: &str, session: u64, instance: &str, revision: u64) -> ScriptEvent {
    ScriptEvent::client_view_opened(
        owner,
        request,
        ScriptPlayerId::new(session),
        Some(instance.to_owned()),
        Some(revision),
        None,
    )
    .expect("the answered open is a valid event")
}

/// The same, refused before it reached the owner.
fn refused(owner: &str, request: &str, session: u64) -> ScriptEvent {
    ScriptEvent::client_view_opened(
        owner,
        request,
        ScriptPlayerId::new(session),
        None,
        None,
        Some(ScriptClientViewFailure::PlayerUnavailable),
    )
    .expect("the refused open is a valid event")
}

/// One admitted key edge, as the native owner publishes it after it re-read the
/// instance the client acted on.
fn action(
    owner: &Owner,
    session: u64,
    instance: &str,
    revision: u64,
    action_id: &str,
) -> ScriptEvent {
    ScriptEvent::loader_view_action(
        owner.id,
        ScriptPlayerId::new(session),
        instance,
        revision,
        action_id,
        1,
        Vec::new(),
        None,
    )
    .expect("the admitted action is a valid event")
}

/// Close event admission and drain what is left, so the deployment stops without
/// a guest waiting on a full command queue.
async fn quiesce(boundary: &ScriptBoundary) {
    boundary.close_event_admission();
    while let Some(command) = tokio::time::timeout(Duration::from_secs(15), boundary.recv_command())
        .await
        .expect("the host closes its command queue")
    {
        let _ = boundary.accept_host_command(command);
    }
}

/// The input HUD both owners open on a join, answered with the instance the
/// server would have created.
async fn answer_input_opens(boundary: &ScriptBoundary, session: u64, revision: u64) {
    let mut opened_views = BTreeMap::new();
    for _ in 0..2 {
        let (owner, request) = open(boundary).await;
        assert_eq!(request.player_id().value(), session);
        opened_views.insert(owner, request);
    }
    assert_eq!(
        opened_views.keys().cloned().collect::<Vec<_>>(),
        [RUBY.id, SAPPHIRE.id]
    );
    for (owner, request) in &opened_views {
        assert!(
            request.owned_view_id() == RUBY.input || request.owned_view_id() == SAPPHIRE.input,
            "{owner} opened {}",
            request.owned_view_id()
        );
        // The bindings travel with the view: the model declares both directions of
        // every key this owner binds, and the owner admits an action only against
        // a model that declares and enables it.
        let expected: &[&str] = if owner == RUBY.id {
            &[
                "ruby-live:key_press",
                "ruby-live:key_release",
                "ruby-live:jump_press",
                "ruby-live:jump_release",
                "ruby-live:escape_press",
                "ruby-live:escape_release",
                "ruby-live:f2_press",
                "ruby-live:f2_release",
                "ruby-live:f11_press",
                "ruby-live:f11_release",
            ]
        } else {
            &["sapphire-live:key_press", "sapphire-live:key_release"]
        };
        let declared = request
            .model()
            .actions()
            .iter()
            .map(|action| (action.action_id().to_owned(), action.enabled()))
            .collect::<Vec<_>>();
        assert_eq!(
            declared.len(),
            expected.len(),
            "{owner} declared {declared:?}"
        );
        for action_id in expected {
            assert!(
                declared
                    .iter()
                    .any(|(declared, enabled)| declared == action_id && *enabled),
                "{owner} did not enable {action_id}: {declared:?}"
            );
        }
        let instance = format!("solaris:view-{}", owner.len());
        boundary
            .try_enqueue_event(opened(
                owner,
                request.request_id(),
                session,
                &instance,
                revision,
            ))
            .expect("the open answer reaches its own owner");
    }
}

#[tokio::test]
async fn each_owner_answers_only_its_own_root_and_owns_only_its_own_ids() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    write_package(root.path(), &SAPPHIRE);
    let host = start(root.path(), &[&RUBY, &SAPPHIRE]);
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        [RUBY.command.to_owned(), SAPPHIRE.command.to_owned()]
    );

    boundary
        .try_enqueue_event(joined(SESSION))
        .expect("the join reaches both owners");
    answer_input_opens(&boundary, SESSION, 1).await;

    // The bare command is the gate's first step: the carrier, then the modal
    // screen, and both belong to the owner the root named.
    run(&boundary, &RUBY, SESSION, "");
    let admitted = staged(&boundary).await;
    let owner = admitted.plugin_id().to_owned();
    let ScriptCommand::GrantLoaderBlockItem { request } = admitted.request() else {
        panic!("expected the ruby grant, got {:?}", admitted.request());
    };
    assert_eq!(owner, RUBY.id);
    assert_eq!(request.block_id(), RUBY.item);
    assert_eq!(request.count(), 1);
    assert_eq!(request.player_id().value(), SESSION);
    // The grant is answered by the player-inventory authority; a committed grant
    // is the inventory itself, so the guest publishes nothing for it.
    let grant = admitted
        .loader_item_grant_result(None)
        .expect("grant answer");
    boundary.try_enqueue_event(grant).expect("the grant answer");

    let (owner, screen) = open(&boundary).await;
    assert_eq!(owner, RUBY.id);
    assert_eq!(screen.owned_view_id(), RUBY.show);
    assert!(
        screen
            .model()
            .actions()
            .iter()
            .any(|action| action.action_id() == "ruby-live:confirm" && action.enabled()),
        "the modal screen must offer its declared confirm action"
    );
    assert_eq!(
        screen.model().resource_entries()[0].id(),
        "ruby",
        "the panel entry is the one the bundle declares"
    );

    // The same root sent to the other package is not the other owner's root: the
    // sapphire guest stages nothing, so the next line is the barrier that follows.
    let foreign = ScriptEvent::try_player_command_with_context(
        SAPPHIRE.id,
        ScriptPlayerId::new(SESSION),
        context(),
        RUBY.command,
        "",
    )
    .expect("the foreign command event is valid");
    boundary
        .try_enqueue_event(foreign)
        .expect("the foreign command is delivered");
    run(&boundary, &SAPPHIRE, SESSION, "input_status");
    let (owner, session, text) = line(&boundary).await;
    assert_eq!(
        (owner.as_str(), session, text.as_str()),
        (SAPPHIRE.id, SESSION, "Sapphire input status: key=0/0.")
    );

    // The sapphire command is the sapphire owner's: its own grant and screen.
    run(&boundary, &SAPPHIRE, SESSION, "");
    let admitted = staged(&boundary).await;
    let owner = admitted.plugin_id().to_owned();
    let ScriptCommand::GrantLoaderBlockItem { request } = admitted.request() else {
        panic!("expected the sapphire grant, got {:?}", admitted.request());
    };
    assert_eq!(owner, SAPPHIRE.id);
    assert_eq!(request.block_id(), SAPPHIRE.item);
    let (owner, screen) = open(&boundary).await;
    assert_eq!(owner, SAPPHIRE.id);
    assert_eq!(screen.owned_view_id(), SAPPHIRE.show);

    quiesce(&boundary).await;
    host.stop();
}

#[tokio::test]
async fn the_counters_come_from_admitted_actions_and_a_reconnect_starts_from_zero() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let host = start(root.path(), &[&RUBY]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, SESSION).await;

    run(&boundary, &RUBY, SESSION, "input_status");
    let (_, session, text) = line(&boundary).await;
    assert_eq!(
        (session, text.as_str()),
        (SESSION, "Ruby input status: key=0/0 jump=0/0.")
    );

    // One real admitted edge, on the instance the owner answered with.
    let instance = format!("solaris:view-{}", RUBY.id.len());
    boundary
        .try_enqueue_event(action(&RUBY, SESSION, &instance, 1, "ruby-live:key_press"))
        .unwrap();
    let (owner, session, text) = line(&boundary).await;
    assert_eq!(
        (owner.as_str(), session, text.as_str()),
        (RUBY.id, SESSION, "Ruby key press #1.")
    );
    // The edge also refreshes the visible HUD, which the owner answers with the
    // instance the fixture then presents against.
    let (_, hud) = open(&boundary).await;
    assert_eq!(hud.owned_view_id(), RUBY.hud);
    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            hud.request_id(),
            SESSION,
            "solaris:hud-1",
            1,
        ))
        .unwrap();

    let release = action(&RUBY, SESSION, &instance, 1, "ruby-live:key_release");
    boundary.try_enqueue_event(release).unwrap();
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby key release #1.");
    let admitted = staged(&boundary).await;
    let (_, present) = admitted
        .into_present_client_view()
        .expect("the HUD is refreshed by a present");
    assert_eq!(present.view_instance_id(), "solaris:hud-1");
    assert_eq!(present.expected_revision(), 1);

    run(&boundary, &RUBY, SESSION, "input_status");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby input status: key=1/1 jump=0/0.");

    // The connection ends. A reconnect is a new session, and the counters are the
    // connection's own: nothing the old client reported is inherited.
    boundary.try_enqueue_event(left(SESSION)).unwrap();
    boundary.try_enqueue_event(joined(RECONNECTED)).unwrap();
    let (owner, request) = open(&boundary).await;
    assert_eq!(
        (owner.as_str(), request.owned_view_id()),
        (RUBY.id, RUBY.input)
    );
    assert_eq!(request.player_id().value(), RECONNECTED);
    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            request.request_id(),
            RECONNECTED,
            "solaris:reconnected-input",
            1,
        ))
        .unwrap();
    run(&boundary, &RUBY, RECONNECTED, "input_status");
    let (_, session, text) = line(&boundary).await;
    assert_eq!(
        (session, text.as_str()),
        (RECONNECTED, "Ruby input status: key=0/0 jump=0/0.")
    );

    // And the first edge of the new connection is its first, not a continuation.
    boundary
        .try_enqueue_event(action(
            &RUBY,
            RECONNECTED,
            "solaris:reconnected-input",
            1,
            "ruby-live:key_press",
        ))
        .unwrap();
    let (_, session, text) = line(&boundary).await;
    assert_eq!(
        (session, text.as_str()),
        (RECONNECTED, "Ruby key press #1.")
    );

    quiesce(&boundary).await;
    host.stop();
}

#[tokio::test]
async fn a_refused_open_is_never_presented_against_and_the_answered_revision_is_used() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let (host, lines) = start_recording(root.path(), &[&RUBY]).unwrap();
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();

    let (_, request) = open(&boundary).await;
    assert_eq!(request.owned_view_id(), RUBY.input);
    // The owner refuses the input HUD, which is the connection that would have
    // installed the bindings.
    boundary
        .try_enqueue_event(refused(RUBY.id, request.request_id(), SESSION))
        .unwrap();

    run(&boundary, &RUBY, SESSION, "hud");
    // The refusal left no instance to present against: the guest asks for the
    // input HUD again and opens the visible one, and stages no present at all.
    let (_, retried) = open(&boundary).await;
    assert_eq!(retried.owned_view_id(), RUBY.input);
    let (_, hud) = open(&boundary).await;
    assert_eq!(hud.owned_view_id(), RUBY.hud);
    let (_, session, text) = line(&boundary).await;
    assert_eq!((session, text.as_str()), (SESSION, "Ruby UI request: hud."));

    // The revision of the update is the one the owner answered with.
    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            hud.request_id(),
            SESSION,
            "solaris:hud-4",
            3,
        ))
        .unwrap();
    run(&boundary, &RUBY, SESSION, "update");
    let admitted = staged(&boundary).await;
    assert_eq!(admitted.plugin_id(), RUBY.id);
    let (_, present) = admitted
        .into_present_client_view()
        .expect("update presents the held instance");
    assert_eq!(present.view_instance_id(), "solaris:hud-4");
    assert_eq!(present.expected_revision(), 3);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: update.");

    // Hide closes exactly that instance, and hiding again is nothing to close
    // rather than a second close of an instance the fixture no longer holds.
    run(&boundary, &RUBY, SESSION, "hide");
    let admitted = staged(&boundary).await;
    let (_, _, instance) = admitted
        .into_close_client_view()
        .expect("hide closes the held instance");
    assert_eq!(instance, "solaris:hud-4");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hide.");
    run(&boundary, &RUBY, SESSION, "hide");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hide.");

    let recorded = lines.recorded();
    assert!(
        recorded
            .iter()
            .any(|line| line.contains("LOADER_LIVE_VIEW_REFUSED")
                && line.contains("player-unavailable")),
        "the refusal is reported with the owner's own reason: {recorded:?}"
    );

    quiesce(&boundary).await;
    host.stop();
}

/// The cells of the one table row a HUD model carries, which is the line the panel
/// shows - `("Ruby HUD", "active.")` for the panel's own line, `("Ruby input",
/// "key=1/1 jump=0/0.")` for a body of Ruby's input counts.
fn hud_line(model: &ScriptClientViewModel) -> (&str, &str) {
    let [row] = model.rows() else {
        panic!("the visible HUD carries exactly one row: {model:?}");
    };
    let [field, text] = row.cells() else {
        panic!("the visible HUD row is the field and its line: {row:?}");
    };
    (field.as_str(), text.as_str())
}

/// One visible panel, one outstanding open: everything the connection asks for
/// before the owner answers that open reaches the instance the answer names.
///
/// A second open while the first is unanswered is the defect this covers. The
/// owner allocates a distinct instance per open, so the earlier one would stay
/// open on the server with nothing here holding its id, and every later update or
/// hide would reach only the newest instance. The tape below is therefore read in
/// order and never skipped: a second open would arrive as the wrong command for
/// the read that follows it.
#[tokio::test]
async fn one_pending_hud_open_coalesces_the_latest_model_and_leaves_no_orphan_instance() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let host = start(root.path(), &[&RUBY]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, SESSION).await;
    let input_instance = format!("solaris:view-{}", RUBY.id.len());

    // The panel is opened, and the owner's answer is withheld.
    run(&boundary, &RUBY, SESSION, "hud");
    let (_, pending) = open(&boundary).await;
    assert_eq!(pending.owned_view_id(), RUBY.hud);
    assert_eq!(pending.player_id().value(), SESSION);
    assert_eq!(
        hud_line(pending.model()),
        ("Ruby HUD", "active."),
        "the open carries the panel's own line"
    );
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    // An update and two real key edges arrive while the open is unanswered. Each
    // is a request the connection made, and each stages only its own report: the
    // next command read is never a second open.
    run(&boundary, &RUBY, SESSION, "update");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: update.");
    boundary
        .try_enqueue_event(action(
            &RUBY,
            SESSION,
            &input_instance,
            1,
            "ruby-live:key_press",
        ))
        .unwrap();
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby key press #1.");
    boundary
        .try_enqueue_event(action(
            &RUBY,
            SESSION,
            &input_instance,
            1,
            "ruby-live:key_release",
        ))
        .unwrap();
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby key release #1.");

    // The answer arrives for the one open that was issued. The instance it names
    // is presented once, against the revision the owner itself assigned, with the
    // latest model the connection asked for - the counts of the release edge, not
    // the line the open was issued with.
    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            pending.request_id(),
            SESSION,
            "solaris:hud-1",
            4,
        ))
        .unwrap();
    let admitted = staged(&boundary).await;
    assert_eq!(admitted.plugin_id(), RUBY.id);
    let (_, present) = admitted
        .into_present_client_view()
        .expect("the answered open is presented, never opened again");
    assert_eq!(present.player_id().value(), SESSION);
    assert_eq!(present.view_instance_id(), "solaris:hud-1");
    assert_eq!(present.expected_revision(), 4);
    assert_eq!(
        hud_line(present.model()),
        ("Ruby input", "key=1/1 jump=0/0."),
        "the coalesced model is the latest one the connection asked for"
    );

    // Hiding closes that exact instance and nothing else, so the one instance the
    // owner allocated for this panel is the one this connection retires.
    run(&boundary, &RUBY, SESSION, "hide");
    let admitted = staged(&boundary).await;
    assert_eq!(admitted.plugin_id(), RUBY.id);
    let (_, _, instance) = admitted
        .into_close_client_view()
        .expect("hide closes the instance the answer named");
    assert_eq!(instance, "solaris:hud-1");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hide.");

    // The connection holds no instance and waits for none, so the next request
    // opens a fresh panel with the line it asked for rather than presenting the
    // instance hide closed.
    run(&boundary, &RUBY, SESSION, "hud");
    let (_, fresh) = open(&boundary).await;
    assert_eq!(fresh.owned_view_id(), RUBY.hud);
    assert_eq!(fresh.player_id().value(), SESSION);
    assert_eq!(hud_line(fresh.model()), ("Ruby HUD", "active."));
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    quiesce(&boundary).await;
    host.stop();
}

/// A hide that arrives before the open is answered closes the instance the answer
/// names: the connection asked the panel away, so nothing is left visible for it.
#[tokio::test]
async fn a_hide_during_a_pending_hud_open_closes_the_answered_instance() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let host = start(root.path(), &[&RUBY]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, SESSION).await;

    run(&boundary, &RUBY, SESSION, "hud");
    let (_, pending) = open(&boundary).await;
    assert_eq!(pending.owned_view_id(), RUBY.hud);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    // Nothing is held yet, so the hide stages no close - and it stages nothing
    // else either, so the acknowledgement is the next command and not a second
    // open.
    run(&boundary, &RUBY, SESSION, "hide");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hide.");

    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            pending.request_id(),
            SESSION,
            "solaris:hud-2",
            1,
        ))
        .unwrap();
    let admitted = staged(&boundary).await;
    assert_eq!(admitted.plugin_id(), RUBY.id);
    let (_, _, instance) = admitted
        .into_close_client_view()
        .expect("the hidden panel's answered instance is closed");
    assert_eq!(instance, "solaris:hud-2");

    // The hide released the pending open with the instance it produced, so the
    // next request opens a new panel instead of presenting a closed one.
    run(&boundary, &RUBY, SESSION, "hud");
    let (_, reopened) = open(&boundary).await;
    assert_eq!(reopened.owned_view_id(), RUBY.hud);
    assert_eq!(hud_line(reopened.model()), ("Ruby HUD", "active."));
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    quiesce(&boundary).await;
    host.stop();
}

/// A refused HUD open releases the one open the connection awaited: the next
/// command opens a fresh panel instead of coalescing into a request the owner
/// already answered, and nothing is retried on the refusal itself.
#[tokio::test]
async fn a_refused_hud_open_releases_the_pending_open_without_retrying() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let host = start(root.path(), &[&RUBY]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, SESSION).await;

    run(&boundary, &RUBY, SESSION, "hud");
    let (_, refused_open) = open(&boundary).await;
    assert_eq!(refused_open.owned_view_id(), RUBY.hud);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    boundary
        .try_enqueue_event(refused(RUBY.id, refused_open.request_id(), SESSION))
        .unwrap();

    // A status reply fences the refusal. An automatic retry would put an open
    // ahead of this reply, which line() rejects.
    run(&boundary, &RUBY, SESSION, "input_status");
    let (owner, session, _) = line(&boundary).await;
    assert_eq!((owner.as_str(), session), (RUBY.id, SESSION));

    // The next explicit command opens again rather than coalescing into the
    // already-refused request.
    run(&boundary, &RUBY, SESSION, "hud");
    let (_, retried) = open(&boundary).await;
    assert_eq!(retried.owned_view_id(), RUBY.hud);
    assert_eq!(retried.player_id().value(), SESSION);
    assert_eq!(hud_line(retried.model()), ("Ruby HUD", "active."));
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    quiesce(&boundary).await;
    host.stop();
}

/// A connection that ends while its open is unanswered takes the pending state
/// with it: the late answer reaches no session, and the connection that replaces
/// it opens its own panel rather than inheriting an instance it never asked for.
#[tokio::test]
async fn a_hud_open_left_pending_by_a_disconnect_is_not_attached_to_the_next_session() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    let host = start(root.path(), &[&RUBY]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, SESSION).await;

    run(&boundary, &RUBY, SESSION, "hud");
    let (_, pending) = open(&boundary).await;
    assert_eq!(pending.owned_view_id(), RUBY.hud);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    // The connection ends with the open still unanswered.
    boundary.try_enqueue_event(left(SESSION)).unwrap();
    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            pending.request_id(),
            SESSION,
            "solaris:stale-hud",
            1,
        ))
        .unwrap();

    // The next connection is its own. Its join opens its input HUD, which is the
    // barrier proving the late answer and the join were both processed.
    boundary.try_enqueue_event(joined(RECONNECTED)).unwrap();
    answer_input_opens_alone(&boundary, &RUBY, RECONNECTED).await;

    // And its panel is a new open, never a present against the instance the ended
    // connection's answer named.
    run(&boundary, &RUBY, RECONNECTED, "hud");
    let (_, reopened) = open(&boundary).await;
    assert_eq!(reopened.owned_view_id(), RUBY.hud);
    assert_eq!(reopened.player_id().value(), RECONNECTED);
    assert_eq!(hud_line(reopened.model()), ("Ruby HUD", "active."));
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: hud.");

    boundary
        .try_enqueue_event(opened(
            RUBY.id,
            reopened.request_id(),
            RECONNECTED,
            "solaris:reconnected-hud",
            2,
        ))
        .unwrap();
    run(&boundary, &RUBY, RECONNECTED, "update");
    let admitted = staged(&boundary).await;
    assert_eq!(admitted.plugin_id(), RUBY.id);
    let (_, present) = admitted
        .into_present_client_view()
        .expect("the new connection presents the instance it opened");
    assert_eq!(present.view_instance_id(), "solaris:reconnected-hud");
    assert_eq!(present.expected_revision(), 2);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby UI request: update.");

    quiesce(&boundary).await;
    host.stop();
}

#[tokio::test]
async fn a_plugin_id_the_fixture_does_not_own_fails_instead_of_being_served_as_an_owner() {
    let root = tempfile::tempdir().unwrap();
    let observer = Owner {
        id: "loader-live-observer",
        command: "loader_observer",
        ..RUBY
    };
    write_package(root.path(), &observer);
    let error = match start_recording(root.path(), &[&observer]) {
        Ok(_) => panic!("an id this fixture does not own must not start"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("loader-live-observer"),
        "the failed instance names itself: {message}"
    );
}

#[tokio::test]
async fn sounds_are_the_owners_own_and_a_bad_request_stages_no_playback() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), &RUBY);
    write_package(root.path(), &SAPPHIRE);
    let host = start(root.path(), &[&RUBY, &SAPPHIRE]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens(&boundary, SESSION, 1).await;

    run(&boundary, &RUBY, SESSION, "sound_quiet");
    let (owner, session, sound_id, playback) = sound(&boundary).await;
    assert_eq!((owner.as_str(), session), (RUBY.id, SESSION));
    assert_eq!(sound_id, RUBY.tone);
    let played = playback.expect("a quiet tone is still playback");
    assert_eq!(played.volume(), 0.25);
    assert_eq!(played.pitch(), 1.0);
    assert!(played.position().is_none());
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby sound: sound_quiet.");

    run(&boundary, &RUBY, SESSION, "sound_pitch");
    let (_, _, sound_id, playback) = sound(&boundary).await;
    assert_eq!(sound_id, RUBY.tone);
    let played = playback.expect("a pitched tone is playback");
    assert_eq!(played.volume(), 1.0);
    assert_eq!(played.pitch(), 1.5);
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby sound: sound_pitch.");

    run(&boundary, &RUBY, SESSION, "sound_world 10 70 10");
    let (_, session, sound_id, playback) = sound(&boundary).await;
    assert_eq!(session, SESSION);
    assert_eq!(sound_id, RUBY.tone);
    let at = playback
        .expect("a world tone is playback")
        .position()
        .expect("a world tone carries its position");
    assert_eq!((at.x(), at.y(), at.z()), (10.0, 70.0, 10.0));
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby sound: world.");

    // A position that does not parse is not turned into listener-relative audio:
    // the command answers its usage line and stages no sound at all.
    run(&boundary, &RUBY, SESSION, "sound_world 10 70");
    let (_, _, text) = line(&boundary).await;
    assert!(
        text.starts_with("Ruby usage: loader_ruby <"),
        "a malformed world request is refused: {text}"
    );

    // Stopping another owner's tone is sent as asked: the server's own sound owner
    // is what refuses it, and the gate's audio phase is the proof Sapphire kept
    // playing.
    run(&boundary, &RUBY, SESSION, "sound_foreign_stop");
    let (_, session, sound_id, playback) = sound(&boundary).await;
    assert_eq!((session, sound_id.as_str()), (SESSION, RUBY.foreign_tone));
    assert!(playback.is_none(), "a stop carries no playback");
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Ruby sound: sound_foreign_stop.");

    // The other owner's own tone, and a mode it has no answer for.
    run(&boundary, &SAPPHIRE, SESSION, "sound");
    let (owner, session, sound_id, playback) = sound(&boundary).await;
    assert_eq!((owner.as_str(), session), (SAPPHIRE.id, SESSION));
    assert_eq!(sound_id, SAPPHIRE.tone);
    assert!(playback.is_some());
    let (_, _, text) = line(&boundary).await;
    assert_eq!(text, "Sapphire sound: sound.");
    run(&boundary, &SAPPHIRE, SESSION, "sound_quiet");
    let (_, _, text) = line(&boundary).await;
    assert!(
        text.starts_with("Sapphire usage: loader_sapphire <"),
        "a mode this owner has no answer for is reported: {text}"
    );

    quiesce(&boundary).await;
    host.stop();
}

/// One sound command a guest staged: the owner, the session, the sound id and
/// its playback, if it is a play.
async fn sound(
    boundary: &ScriptBoundary,
) -> (
    String,
    u64,
    String,
    Option<mc_script::ScriptClientSoundPlayback>,
) {
    let admitted = staged(boundary).await;
    let owner = admitted.plugin_id().to_owned();
    let (_, player, sound) = admitted
        .into_client_sound()
        .expect("the guest staged a sound command");
    (
        owner,
        player.value(),
        sound.sound_id().to_owned(),
        sound.playback(),
    )
}

/// The input HUD of one owner, answered with the instance the server would have
/// created: the single-owner form of [`answer_input_opens`].
async fn answer_input_opens_alone(boundary: &ScriptBoundary, owner: &Owner, session: u64) {
    let (staged_owner, request) = open(boundary).await;
    assert_eq!(
        (staged_owner.as_str(), request.owned_view_id()),
        (owner.id, owner.input)
    );
    assert_eq!(request.player_id().value(), session);
    let instance = format!("solaris:view-{}", owner.id.len());
    boundary
        .try_enqueue_event(opened(
            owner.id,
            request.request_id(),
            session,
            &instance,
            1,
        ))
        .unwrap();
}

/// The tracked gate fixture itself: the same manifests the harness prepares and
/// deploys, discovered and started as component packages.
///
/// The component is the one piece a source tree does not carry - the harness
/// stages it with `tools/build-loader-live-gate-fixture.sh --prepare` - so this
/// test puts the built component next to the tracked manifest and bundle and
/// reads back what the package contract made of them: both owners are
/// discoverable with their exact bundle identity, and both start and register
/// their own command root.
#[tokio::test]
async fn the_tracked_gate_fixture_is_discoverable_and_runnable_as_two_components() {
    let fixture = fixture::repo_root().join("examples/loader-live-gate/plugins");
    let root = tempfile::tempdir().unwrap();
    let limits = PluginLimits::default();

    for owner in ["ruby-live", "sapphire-live"] {
        let source = fixture.join(owner);
        let staged = root.path().join(owner);
        std::fs::create_dir_all(&staged).unwrap();
        for name in ["plugin.toml", "config.toml"] {
            std::fs::copy(source.join(name), staged.join(name)).unwrap();
        }
        std::fs::create_dir_all(staged.join("client")).unwrap();
        std::fs::copy(
            source.join("client/rich-content.zip"),
            staged.join("client/rich-content.zip"),
        )
        .unwrap();
        std::fs::write(staged.join("plugin.wasm"), fixture::component_bytes()).unwrap();

        let package = mc_plugin_host::load_package(&staged, &limits)
            .unwrap_or_else(|error| panic!("{owner} is a valid component package: {error}"));
        assert_eq!(package.manifest().plugin_id(), owner);
        let [bundle] = package.client_bundles() else {
            panic!("{owner} declares exactly one client bundle");
        };
        assert_eq!(bundle.owner_plugin_id(), owner);
        assert_eq!(bundle.id(), "rich-content");
        assert_eq!(bundle.version(), "1");
        assert_eq!(bundle.artifact(), "client/rich-content.zip");
        assert_eq!(
            bundle.loaders(),
            &[
                mc_script::ClientLoader::Fabric,
                mc_script::ClientLoader::NeoForge,
                mc_script::ClientLoader::Forge,
            ]
        );
        assert_eq!(
            bundle.content(),
            &[
                mc_script::ClientContentKind::Blocks,
                mc_script::ClientContentKind::Items,
                mc_script::ClientContentKind::Views,
                mc_script::ClientContentKind::ViewActions,
                mc_script::ClientContentKind::Assets,
            ]
        );
        assert_eq!(
            bundle.permissions(),
            &[
                mc_script::ClientPermission::RegisterBlocks,
                mc_script::ClientPermission::RegisterItems,
                mc_script::ClientPermission::PresentViews,
                mc_script::ClientPermission::SendViewActions,
                mc_script::ClientPermission::LoadAssets,
            ]
        );
        // The bytes the manifest pins are the bytes on disk, and the cache
        // identity the client stores them under names this owner alone.
        let bytes = bundle.artifact_bytes();
        assert_eq!(bytes.len() as u64, bundle.size_bytes());
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            bundle.sha256(),
            "{owner} bundle hash"
        );
        assert_eq!(
            bundle.cache_key(),
            format!("{owner}:rich-content/1/{}", bundle.sha256())
        );
    }

    let owners = [&RUBY, &SAPPHIRE];
    let host = start(root.path(), &owners);
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        [RUBY.command.to_owned(), SAPPHIRE.command.to_owned()]
    );
    boundary.try_enqueue_event(joined(SESSION)).unwrap();
    answer_input_opens(&boundary, SESSION, 1).await;
    quiesce(&boundary).await;
    host.stop();
}
