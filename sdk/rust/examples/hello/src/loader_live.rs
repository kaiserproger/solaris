//! P4 loader-live fixture: the two-owner Solaris Loader gate as a real component.
//!
//! `mode = "loader-live"` runs this module for both Loader gate packages. One
//! guest artifact serves both owners: the owner is the plugin id the host bound
//! to the instance, so `ruby-live` and `sapphire-live` differ by their manifest,
//! their client bundle and the ids they own rather than by a second component -
//! and a plugin id this fixture does not own fails the instance instead of being
//! served as one of the two.
//!
//! Everything here is a request to an existing owner, never an effect. The
//! commands are the typed client-presentation records of the contract, the
//! Loader manifest's content-and-permission pair is what admits them, and the
//! gate's own evidence is what the server does with them: the granted carrier in
//! the inventory, the frames the client renders, and the captured audio. The
//! lines this module sends are the gate's ordering barriers - they say what was
//! asked for, never that a client showed it.
//!
//! The three views of one owner, and why each is opened the way it is:
//!
//! * `<owner>:showcase` is the modal screen the bare command opens and the click
//!   target of the gate's `Confirm` button. It is always *opened*, never
//!   presented, because a replacement only reaches the instance the client is
//!   still showing: a modal the client dismissed cannot be brought back by a
//!   present, so this module retires the instance it opened before asking for the
//!   next one rather than leaving it open on the server.
//! * `<owner>:hud` is the visible panel `hud` and `update` fill with one line of
//!   text and `hide` closes. It is opened once per connection and presented after
//!   that, and the revision this module presents against is the one the owner
//!   answered with, advanced by one per replacement it asked for. Only one open
//!   is outstanding at a time: a command that arrives before the owner answers it
//!   - an `update` or an input edge - only rewrites the model the answered
//!   instance is presented with, and a `hide` among them closes that instance, so
//!   a second instance is never opened for this connection to lose track of.
//! * `<owner>:input` is the invisible HUD (`widgets: []`) whose client bundle
//!   declares the key bindings. It is opened on the join the server announces,
//!   because installing the bindings is exactly what opening it does; the model
//!   declares every press and release action enabled, because the owner admits an
//!   action only against a model that declares and enables it.
//!
//! The input counters advance only from admitted `loader.view_action` events. A
//! package whose client never produced a key edge reports zeroes, and the status
//! lines are the counts themselves rather than a claim about a key this process
//! never saw. Per-session state lives under the session the event named and is
//! dropped when the server announces that exact connection leaving, so a reconnect
//! starts from zero and an event for a connection this instance never saw creates
//! no state a later connection could inherit.

use std::collections::BTreeMap;

use solaris_plugin_sdk::bindings::solaris::plugin::client_presentation as presentation;
use solaris_plugin_sdk::commands;
use solaris_plugin_sdk::events::{
    ClientViewOpened, ClientViewOutcome, CommandInvoked, EventContext, LoaderItemGrantFailure,
    LoaderItemGrantOutcome, LoaderItemGrantResult, LoaderViewAction, ViewFailure,
};
use solaris_plugin_sdk::{log, types, Command, Event, Failure, LogLevel};

/// What one refused view open is logged with, so an operator reading the server
/// log sees which request the owner rejected and why rather than a bare silence.
const VIEW_REFUSED: &str = "LOADER_LIVE_VIEW_REFUSED";

/// The same for a refused item grant: the gate's inventory assertion is the real
/// proof the carrier arrived, and this line is how a refusal is attributed to the
/// owner's own reason.
const GRANT_REFUSED: &str = "LOADER_LIVE_GRANT_REFUSED";

/// The same for an answer to an open this instance never issued: answers are
/// associated by the request id this fixture chose, so anything else is dropped
/// and reported instead of being applied to whatever happens to be open.
const VIEW_UNKNOWN: &str = "LOADER_LIVE_VIEW_UNKNOWN";

/// The same for a command mode this owner has no answer for.
const USAGE_UNKNOWN: &str = "LOADER_LIVE_USAGE";

/// The plugin id this fixture does not serve.
const UNKNOWN_OWNER: &str = "LOADER_LIVE_UNKNOWN_OWNER";

/// The line the visible HUD shows while it is simply up, and the one `update`
/// replaces it with: the gate reads the second as the update it asked for.
const HUD_ACTIVE: &str = "active.";
const HUD_UPDATED: &str = "updated.";

/// The input edges Ruby's bundle binds: the shared key the gate presses for both
/// owners, the jump key only this owner binds, and the three keys whose native
/// behaviour the gate requires to stay intact.
const RUBY_EDGES: &[Edge] = &[Edge::Key, Edge::Jump, Edge::Escape, Edge::F2, Edge::F11];
/// Sapphire binds the shared key only: the gate requires the same press to reach
/// both owners and the jump, escape, F2 and F11 counts to stay Ruby's.
const SAPPHIRE_EDGES: &[Edge] = &[Edge::Key];

/// Which owner one deployed instance is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Ruby,
    Sapphire,
}

impl Owner {
    /// The owner one plugin id is, or `None` for an id this fixture does not own.
    fn from_id(plugin_id: &str) -> Option<Self> {
        match plugin_id {
            "ruby-live" => Some(Self::Ruby),
            "sapphire-live" => Some(Self::Sapphire),
            _ => None,
        }
    }

    /// The plugin id, which is also the namespace of every id this owner owns.
    fn id(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live",
            Self::Sapphire => "sapphire-live",
        }
    }

    /// The name the gate's own lines use.
    fn name(self) -> &'static str {
        match self {
            Self::Ruby => "Ruby",
            Self::Sapphire => "Sapphire",
        }
    }

    /// The command root the manifest declares.
    fn command(self) -> &'static str {
        match self {
            Self::Ruby => "loader_ruby",
            Self::Sapphire => "loader_sapphire",
        }
    }

    /// The prefix every id this owner owns carries.
    fn prefix(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:",
            Self::Sapphire => "sapphire-live:",
        }
    }

    /// The custom block item this owner's bundle declares and this fixture grants.
    fn block(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:ruby_block",
            Self::Sapphire => "sapphire-live:sapphire_block",
        }
    }

    /// The bundle's resource-panel entry, and the material that panel labels.
    fn material(self) -> &'static str {
        match self {
            Self::Ruby => "ruby",
            Self::Sapphire => "sapphire",
        }
    }

    /// The one sound this owner's bundle declares.
    fn tone(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:tone",
            Self::Sapphire => "sapphire-live:tone",
        }
    }

    /// The other owner's tone. Ruby's foreign-stop cancellation names it, and the
    /// server's own sound owner refuses it because this package does not own that
    /// id - which is the behaviour the audio phase measures when Sapphire keeps
    /// playing.
    fn foreign_tone(self) -> &'static str {
        match self {
            Self::Ruby => "sapphire-live:tone",
            Self::Sapphire => "ruby-live:tone",
        }
    }

    /// The modal screen the bare command opens.
    fn showcase(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:showcase",
            Self::Sapphire => "sapphire-live:showcase",
        }
    }

    /// The visible HUD `hud`, `update` and `hide` drive.
    fn hud(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:hud",
            Self::Sapphire => "sapphire-live:hud",
        }
    }

    /// The invisible HUD whose bundle declares the key bindings.
    fn input(self) -> &'static str {
        match self {
            Self::Ruby => "ruby-live:input",
            Self::Sapphire => "sapphire-live:input",
        }
    }

    /// The input edges this owner's bundle binds.
    fn edges(self) -> &'static [Edge] {
        match self {
            Self::Ruby => RUBY_EDGES,
            Self::Sapphire => SAPPHIRE_EDGES,
        }
    }

    /// The modes this owner answers. Ruby carries the quiet, pitched, world-placed
    /// and foreign-stop commands; Sapphire answers the two the gate drives on it,
    /// so a mode it has no owner for is a usage answer rather than a request sent
    /// on another owner's behalf.
    fn modes(self) -> &'static str {
        match self {
            Self::Ruby => {
                "hud|update|hide|input_status|input_modal|edge_status|sound|sound_stop|\
                 sound_quiet|sound_pitch|sound_world x y z|sound_foreign_stop"
            }
            Self::Sapphire => "hud|update|hide|input_status|sound|sound_stop",
        }
    }
}

/// One key edge a bundle binds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Edge {
    Key,
    Jump,
    Escape,
    F2,
    F11,
}

impl Edge {
    /// The word the gate's own lines use for this edge.
    fn name(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Jump => "jump",
            Self::Escape => "escape",
            Self::F2 => "f2",
            Self::F11 => "f11",
        }
    }

    /// The action id suffix one phase of this edge is bound to, as the bundle's
    /// `input_bindings` declares it. Static, so the vocabulary this fixture counts
    /// is the one the artifact binds and nothing here derives it from a key name.
    fn action_suffix(self, phase: Phase) -> &'static str {
        match (self, phase) {
            (Self::Key, Phase::Press) => "key_press",
            (Self::Key, Phase::Release) => "key_release",
            (Self::Jump, Phase::Press) => "jump_press",
            (Self::Jump, Phase::Release) => "jump_release",
            (Self::Escape, Phase::Press) => "escape_press",
            (Self::Escape, Phase::Release) => "escape_release",
            (Self::F2, Phase::Press) => "f2_press",
            (Self::F2, Phase::Release) => "f2_release",
            (Self::F11, Phase::Press) => "f11_press",
            (Self::F11, Phase::Release) => "f11_release",
        }
    }
}

/// Which side of one key edge an admitted action is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Press,
    Release,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Release => "release",
        }
    }
}

/// The press and release counts of one edge.
#[derive(Clone, Copy, Default)]
struct Phases {
    press: u64,
    release: u64,
}

impl Phases {
    /// Count one admitted action of this edge and answer the new count.
    fn advance(&mut self, phase: Phase) -> u64 {
        match phase {
            Phase::Press => {
                self.press += 1;
                self.press
            }
            Phase::Release => {
                self.release += 1;
                self.release
            }
        }
    }
}

/// Which of one owner's three views an open is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Showcase,
    Hud,
    Input,
}

impl View {
    fn name(self) -> &'static str {
        match self {
            Self::Showcase => "showcase",
            Self::Hud => "hud",
            Self::Input => "input",
        }
    }
}

/// One view instance the owner answered with, and the revision this instance
/// presents against next.
struct Instance {
    id: String,
    revision: u64,
}

/// The one visible-HUD open this connection is waiting for, and what the commands
/// asked for while it was outstanding.
///
/// The owner allocates a distinct instance per open, so a second open issued
/// before the first is answered would leave the earlier instance open on the
/// server with nothing here holding its id. The commands that arrive in the
/// meantime therefore only rewrite this record: the instance the answer names is
/// presented with the latest model it holds, or closed when it says hidden.
struct HudOpen {
    /// Whether the latest command still wants the panel.
    visible: bool,
    /// The model the latest command asked to show while the open was outstanding,
    /// or `None` when the model the open was issued with is still the latest one.
    latest: Option<presentation::ViewModel>,
}

/// One connection's own state, keyed by the session the server named.
#[derive(Default)]
struct Session {
    /// The input HUD this connection's client holds, once the owner answered.
    input: Option<Instance>,
    /// The visible HUD `hud` and `update` fill and `hide` closes, once the owner
    /// answered. Never held while `hud_open` is set: the open is what produces it.
    hud: Option<Instance>,
    /// The one visible-HUD open this connection is waiting for, if any. There is
    /// never a second one, so the instances the owner allocated for this
    /// connection are exactly the ones this state holds or is about to hold.
    hud_open: Option<HudOpen>,
    /// The last modal screen the owner answered, retired by the next open.
    showcase: Option<Instance>,
    /// The opens this instance issued that the owner has not answered, by the
    /// request id it chose: an answer is applied to what it names, never to
    /// whatever happens to be open.
    pending: BTreeMap<String, View>,
    /// The next request id, so two opens of one view are never confused.
    next_request: u64,
    /// The input edges this connection's client reported, by kind.
    edges: BTreeMap<Edge, Phases>,
    /// Whether `input_modal` armed the next press to open the modal screen.
    modal: bool,
}

impl Session {
    /// The request id for one open, unique inside this connection.
    fn request(&mut self, owner: Owner, view: View) -> String {
        self.next_request += 1;
        format!("{}-{}-{}", owner.id(), view.name(), self.next_request)
    }

    /// Remember one open awaiting its answer.
    fn awaited(&mut self, request: String, view: View) {
        self.pending.insert(request, view);
    }

    /// One answered visible-HUD open: hold the instance the owner named for the
    /// latest model the commands asked to show, or close it when one of them
    /// asked to hide the panel while the open was outstanding.
    ///
    /// The revision the answer carries is the native one, so a model coalesced
    /// during the open is presented against the revision the owner itself
    /// assigned instead of one this fixture invented.
    fn hud_answered(&mut self, session: u64, held: Instance) -> Vec<Command> {
        let open = self
            .hud_open
            .take()
            .expect("a matched HUD answer has an outstanding open");
        if !open.visible {
            return vec![close_view(session, &held.id)];
        }
        self.hud = Some(held);
        let Some(model) = open.latest else {
            return Vec::new();
        };
        let instance = self.hud.as_mut().expect("the answered HUD is held above");
        let expected = instance.revision;
        instance.revision = expected + 1;
        let id = instance.id.clone();
        vec![present_view(session, &id, expected, model)]
    }

    /// The counts of one edge, zeroed until a client reports it.
    fn phases(&self, edge: Edge) -> Phases {
        self.edges.get(&edge).copied().unwrap_or_default()
    }
}

/// One deployed Loader gate owner.
pub struct Fixture {
    owner: Owner,
    sessions: BTreeMap<u64, Session>,
}

impl Fixture {
    /// The fixture of one deployed package.
    ///
    /// The owner is the plugin id the host bound to this instance, so both gate
    /// packages can be the same component without either being served as the
    /// other. An id this fixture does not own is this instance's own failure: a
    /// deployment that meant to run the gate is stopped instead of quietly
    /// answering as an owner its manifest never claimed.
    pub fn new(plugin_id: &str) -> Result<Self, Failure> {
        let Some(owner) = Owner::from_id(plugin_id) else {
            log(
                LogLevel::Error,
                &format!("{UNKNOWN_OWNER} {plugin_id:?} is not a loader-live owner"),
            );
            return Err(Failure::Invalid);
        };
        Ok(Self {
            owner,
            sessions: BTreeMap::new(),
        })
    }

    /// One delivered batch: the server's joins and leaves, this fixture's own
    /// command, the actions its client's bindings produced, and the answers the
    /// owner sends back.
    pub fn on_events(&mut self, _context: &EventContext, events: &[Event]) -> Vec<Command> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::PlayerJoined(joined) => commands.extend(self.joined(joined.session)),
                // The connection the server named has ended. Everything this
                // instance held for it goes with it, so a later session of the
                // same player starts from zero and inherits no binding, count or
                // instance.
                Event::PlayerLeft(left) => {
                    self.sessions.remove(&left.session);
                }
                Event::CommandInvoked(invoked) if invoked.name == self.owner.command() => {
                    commands.extend(self.command(invoked));
                }
                Event::LoaderViewAction(action) => commands.extend(self.action(action)),
                Event::ClientViewOpened(opened) => commands.extend(self.opened(opened)),
                Event::LoaderItemGrantResult(result) => self.grant_result(result),
                _ => {}
            }
        }
        commands
    }

    /// One connection joined. The input HUD carries the key bindings, so this is
    /// where a connection becomes able to report an edge at all; a refused open is
    /// logged and asked again from the connection's first command rather than read
    /// as installed.
    fn joined(&mut self, session: u64) -> Vec<Command> {
        self.show_input(session)
    }

    /// One command of this owner's root, with the arguments the host split.
    fn command(&mut self, invoked: &CommandInvoked) -> Vec<Command> {
        let owner = self.owner;
        let session = invoked.session;
        let mode = invoked.arguments.first().map(String::as_str).unwrap_or("");
        let mut commands = self.show_input(session);
        match mode {
            "" => {
                // The gate's first step: the carrier the client can place, then
                // the owner's modal screen. The grant is asked for before the view
                // because the inventory assertion follows the click on that view's
                // own button.
                commands.push(self.grant(session));
                commands.extend(self.open_showcase(session));
            }
            "hud" => {
                self.disarm(session);
                // This command returns to gameplay; a HUD itself never closes a modal.
                if let Some(previous) = self.sessions.entry(session).or_default().showcase.take() {
                    commands.push(close_view(session, &previous.id));
                }
                let field = format!("{} HUD", owner.name());
                commands.extend(self.show_hud(session, &field, HUD_ACTIVE));
                commands.push(self.ack(session, "UI request: hud."));
            }
            "update" => {
                self.disarm(session);
                let field = format!("{} HUD", owner.name());
                commands.extend(self.show_hud(session, &field, HUD_UPDATED));
                commands.push(self.ack(session, "UI request: update."));
            }
            "hide" => {
                commands.extend(self.hide_hud(session));
                commands.push(self.ack(session, "UI request: hide."));
            }
            "input_status" => commands.push(self.status_message(session)),
            "input_modal" if owner == Owner::Ruby => {
                self.arm(session);
                commands.push(self.ack(session, "input modal armed."));
            }
            "edge_status" if owner == Owner::Ruby => commands.push(self.edge_message(session)),
            "sound" => commands.extend(self.play_tone(session, 1.0, 1.0, None, "sound")),
            "sound_stop" => commands.extend(self.stop_tone(session, owner.tone(), "sound_stop")),
            "sound_quiet" if owner == Owner::Ruby => {
                commands.extend(self.play_tone(session, 0.25, 1.0, None, "sound_quiet"));
            }
            "sound_pitch" if owner == Owner::Ruby => {
                commands.extend(self.play_tone(session, 1.0, 1.5, None, "sound_pitch"));
            }
            // A world-placed tone is the one sound command with arguments, and a
            // request whose position does not parse is not turned into the
            // listener-relative playback the gate never asked for: it answers the
            // usage line instead, so a mistyped command cannot pass the near, mid
            // and far attenuation assertions.
            "sound_world" if owner == Owner::Ruby => match position(&invoked.arguments) {
                Some(position) => {
                    commands.extend(self.play_tone(session, 1.0, 1.0, Some(position), "world"));
                }
                None => {
                    log(LogLevel::Error, USAGE_UNKNOWN);
                    commands.push(self.usage(session));
                }
            },
            // Stopping another owner's tone. The command is sent as asked and the
            // server's own sound owner refuses it, because this package does not
            // own that id; the line reports the request, and the audio phase is
            // what proves Sapphire kept playing.
            "sound_foreign_stop" if owner == Owner::Ruby => {
                commands.extend(self.stop_tone(
                    session,
                    owner.foreign_tone(),
                    "sound_foreign_stop",
                ));
            }
            _ => {
                log(LogLevel::Error, &format!("{USAGE_UNKNOWN} {mode:?}"));
                commands.push(self.usage(session));
            }
        }
        commands
    }

    /// One admitted client action.
    ///
    /// The owner admits it only against a live instance, the revision the client
    /// held, and an action its own model declared and enabled, so everything here
    /// is what the gate needs: the click on the modal screen's own button, or one
    /// edge of a bound key. Nothing else reaches a counter, which is why a run
    /// without a real key edge reports the counts it does.
    fn action(&mut self, action: &LoaderViewAction) -> Vec<Command> {
        let owner = self.owner;
        let session = action.session;
        let Some(suffix) = action.action_id.strip_prefix(owner.prefix()) else {
            return Vec::new();
        };
        if suffix == "confirm" {
            return vec![session_message(
                session,
                format!(
                    "{} Loader view action reached {}.",
                    owner.name(),
                    owner.id()
                ),
            )];
        }
        let Some((edge, phase)) = input_edge(owner, action.action_id.as_str()) else {
            return Vec::new();
        };
        // The revision the owner admitted the action on is authoritative for the
        // instance it names: reading it back keeps a present this instance sent
        // that the owner refused from leaving its own revision permanently stale.
        let (count, armed) = {
            let state = self.sessions.entry(session).or_default();
            if let Some(instance) = state.input.as_mut() {
                if instance.id == action.view_instance_id {
                    instance.revision = action.view_revision;
                }
            }
            let count = state.edges.entry(edge).or_default().advance(phase);
            (count, state.modal && phase == Phase::Press)
        };
        if armed {
            // The armed modal opens on the press the gate expects it to: the
            // client releases the keys it holds as the screen takes focus, so this
            // is also the press whose release edges arrive before the physical
            // key-up.
            self.disarm(session);
        }
        let mut commands = vec![session_message(
            session,
            format!(
                "{} {} {} #{}.",
                owner.name(),
                edge.name(),
                phase.name(),
                count
            ),
        )];
        if armed {
            commands.extend(self.open_showcase(session));
        } else {
            let field = format!("{} input", owner.name());
            let body = self.counts_line(session);
            commands.extend(self.show_hud(session, &field, &body));
        }
        commands
    }

    /// One answer to an open this instance issued.
    fn opened(&mut self, opened: &ClientViewOpened) -> Vec<Command> {
        let unknown = |request: &str| {
            log(
                LogLevel::Error,
                &format!(
                    "{VIEW_UNKNOWN} session={} request={request}",
                    opened.session
                ),
            );
        };
        let Some(state) = self.sessions.get_mut(&opened.session) else {
            unknown(&opened.request);
            return Vec::new();
        };
        let Some(view) = state.pending.remove(&opened.request) else {
            unknown(&opened.request);
            return Vec::new();
        };
        let mut commands = Vec::new();
        match &opened.outcome {
            ClientViewOutcome::Opened(instance) => {
                let held = Instance {
                    id: instance.view_instance_id.clone(),
                    revision: instance.revision,
                };
                match view {
                    View::Showcase => state.showcase = Some(held),
                    // The answer is applied to the one open this connection was
                    // waiting for: the instance is held for the latest model the
                    // commands asked to show while it was outstanding, or closed
                    // when one of them asked to hide the panel. No open is issued
                    // from here, so an answer can never add a second instance.
                    View::Hud => commands = state.hud_answered(opened.session, held),
                    View::Input => state.input = Some(held),
                }
            }
            // The owner refused the open, so nothing is held for it: the reason is
            // reported, the one open this connection awaited is released, and the
            // next command asks again on its own rather than this answering a
            // refusal with a retry of its own. A refusal is never turned into an
            // instance this fixture could present against.
            ClientViewOutcome::Refused(failure) => {
                if view == View::Hud {
                    state.hud_open = None;
                }
                log(
                    LogLevel::Error,
                    &format!(
                        "{VIEW_REFUSED} view={} session={} request={} reason={}",
                        view.name(),
                        opened.session,
                        opened.request,
                        view_failure_name(failure)
                    ),
                );
            }
        }
        commands
    }

    /// One answer to a grant this instance asked for. A committed grant is the
    /// inventory itself, so nothing is reported for it; a refusal is logged with
    /// the owner's own reason, which is what the gate needs to attribute a missing
    /// carrier.
    fn grant_result(&self, result: &LoaderItemGrantResult) {
        if let LoaderItemGrantOutcome::Refused(failure) = &result.outcome {
            log(
                LogLevel::Error,
                &format!(
                    "{GRANT_REFUSED} session={} block={} count={} reason={}",
                    result.session,
                    result.block,
                    result.count,
                    grant_failure_name(failure)
                ),
            );
        }
    }

    /// Ask the owner for this owner's own custom block carrier.
    fn grant(&self, session: u64) -> Command {
        let owner = self.owner;
        grant_block(session, &format!("{}-grant", owner.id()), owner.block(), 1)
    }

    /// Open one instance of the modal screen on one connection, retiring the
    /// instance this fixture opened before it: the client replaces its active
    /// screen on the new open either way, so the previous instance would otherwise
    /// stay open on the server until the connection ended.
    fn open_showcase(&mut self, session: u64) -> Vec<Command> {
        let owner = self.owner;
        let body = self.counts_line(session);
        let state = self.sessions.entry(session).or_default();
        let mut commands = Vec::new();
        if let Some(previous) = state.showcase.take() {
            commands.push(close_view(session, &previous.id));
        }
        let request = state.request(owner, View::Showcase);
        state.awaited(request.clone(), View::Showcase);
        commands.push(open_view(
            &request,
            session,
            owner.showcase(),
            showcase_model(owner, &body),
        ));
        commands
    }

    /// Ask the owner to open the connection's input HUD unless it already holds
    /// one, which is what installs this owner's key bindings for that client.
    fn show_input(&mut self, session: u64) -> Vec<Command> {
        let owner = self.owner;
        let state = self.sessions.entry(session).or_default();
        if state.input.is_some() || state.pending.values().any(|view| *view == View::Input) {
            return Vec::new();
        }
        let request = state.request(owner, View::Input);
        state.awaited(request.clone(), View::Input);
        vec![open_view(
            &request,
            session,
            owner.input(),
            input_model(owner),
        )]
    }

    /// Show one line in the connection's visible HUD: open it when this instance
    /// holds none, rewrite the one open it is waiting for, and present the
    /// instance it holds otherwise.
    ///
    /// While an open is outstanding nothing is staged for the command: the owner
    /// allocates a distinct instance per open, so a second open would leave the
    /// first one open on the server with nothing here holding its id, and the
    /// model the answer is presented with would be an older one than the last
    /// command asked for.
    fn show_hud(&mut self, session: u64, field: &str, text: &str) -> Vec<Command> {
        let owner = self.owner;
        let model = hud_model(field, text);
        let state = self.sessions.entry(session).or_default();
        if let Some(open) = state.hud_open.as_mut() {
            open.visible = true;
            open.latest = Some(model);
            return Vec::new();
        }
        if state.hud.is_none() {
            let request = state.request(owner, View::Hud);
            state.awaited(request.clone(), View::Hud);
            state.hud_open = Some(HudOpen {
                visible: true,
                latest: None,
            });
            return vec![open_view(&request, session, owner.hud(), model)];
        }
        let instance = state.hud.as_mut().expect("the HUD was checked above");
        let expected = instance.revision;
        instance.revision = expected + 1;
        let id = instance.id.clone();
        vec![present_view(session, &id, expected, model)]
    }

    /// Close the exact HUD instance this connection holds. Closing an instance
    /// this fixture is not holding is nothing to close rather than a fabricated
    /// removal, so no command is sent and the gate's acknowledgement is the only
    /// line. A hide that arrives before the open is answered is remembered: the
    /// instance the owner answers with is closed instead of being left visible on
    /// a connection that asked it away.
    fn hide_hud(&mut self, session: u64) -> Vec<Command> {
        let state = self.sessions.entry(session).or_default();
        if let Some(open) = state.hud_open.as_mut() {
            open.visible = false;
            open.latest = None;
            return Vec::new();
        }
        match state.hud.take() {
            Some(instance) => vec![close_view(session, &instance.id)],
            None => Vec::new(),
        }
    }

    /// Play this owner's own tone, reporting the request in the gate's own words.
    fn play_tone(
        &self,
        session: u64,
        volume: f32,
        pitch: f32,
        position: Option<types::Position>,
        label: &str,
    ) -> Vec<Command> {
        let owner = self.owner;
        vec![
            play_sound(session, owner.tone(), volume, pitch, position),
            self.ack(session, &format!("sound: {label}.")),
        ]
    }

    /// Stop one sound, with the same order of request and report as playback.
    fn stop_tone(&self, session: u64, sound: &str, label: &str) -> Vec<Command> {
        vec![
            stop_sound(session, sound),
            self.ack(session, &format!("sound: {label}.")),
        ]
    }

    /// One line to one connection. Every line of this fixture addresses the
    /// session the event named, never the stable identity: a line meant for a
    /// connection that has ended is dropped instead of reaching whoever holds that
    /// identity next.
    fn ack(&self, session: u64, detail: &str) -> Command {
        session_message(session, format!("{} {detail}", self.owner.name()))
    }

    /// The counts body both the status line and the input HUD carry.
    fn counts_line(&self, session: u64) -> String {
        let key = self.phases(session, Edge::Key);
        match self.owner {
            Owner::Ruby => {
                let jump = self.phases(session, Edge::Jump);
                format!(
                    "key={}/{} jump={}/{}.",
                    key.press, key.release, jump.press, jump.release
                )
            }
            Owner::Sapphire => format!("key={}/{}.", key.press, key.release),
        }
    }

    /// The gate's input-status barrier.
    fn status_message(&self, session: u64) -> Command {
        session_message(
            session,
            format!(
                "{} input status: {}",
                self.owner.name(),
                self.counts_line(session)
            ),
        )
    }

    /// The gate's edge-status barrier.
    fn edge_message(&self, session: u64) -> Command {
        let escape = self.phases(session, Edge::Escape);
        let f2 = self.phases(session, Edge::F2);
        let f11 = self.phases(session, Edge::F11);
        session_message(
            session,
            format!(
                "{} edge status: escape={}/{} f2={}/{} f11={}/{}.",
                self.owner.name(),
                escape.press,
                escape.release,
                f2.press,
                f2.release,
                f11.press,
                f11.release
            ),
        )
    }

    /// The counts of one edge of one connection, which may not exist yet.
    fn phases(&self, session: u64, edge: Edge) -> Phases {
        self.sessions
            .get(&session)
            .map(|state| state.phases(edge))
            .unwrap_or_default()
    }

    fn arm(&mut self, session: u64) {
        self.sessions.entry(session).or_default().modal = true;
    }

    fn disarm(&mut self, session: u64) {
        self.sessions.entry(session).or_default().modal = false;
    }

    /// What a mode this owner has no answer for reports. The gate never drives an
    /// undeclared mode, and a package that does not answer one says so instead of
    /// staging a command its manifest does not cover.
    fn usage(&self, session: u64) -> Command {
        session_message(
            session,
            format!(
                "{} usage: {} <{}>.",
                self.owner.name(),
                self.owner.command(),
                self.owner.modes()
            ),
        )
    }
}

/// The edge one admitted action names, or `None` for an action this fixture does
/// not count. Only this owner's own declared ids are read, so a substituted or
/// foreign action reaches no counter.
fn input_edge(owner: Owner, action_id: &str) -> Option<(Edge, Phase)> {
    let suffix = action_id.strip_prefix(owner.prefix())?;
    for edge in owner.edges() {
        for phase in [Phase::Press, Phase::Release] {
            if suffix == edge.action_suffix(phase) {
                return Some((*edge, phase));
            }
        }
    }
    None
}

/// The position of a `sound_world x y z` command, or `None` when the three
/// coordinates are missing or do not parse as finite numbers.
fn position(arguments: &[String]) -> Option<types::Position> {
    let [_, x, y, z] = arguments else {
        return None;
    };
    let coordinate = |value: &str| value.parse::<f64>().ok().filter(|value| value.is_finite());
    Some(types::Position {
        x: coordinate(x)?,
        y: coordinate(y)?,
        z: coordinate(z)?,
    })
}

/// The model of the modal screen: this fixture's own row for the table widget, the
/// one line the panel shows, the declared `Confirm` action the gate clicks, and
/// the resource that panel labels.
#[must_use]
fn showcase_model(owner: Owner, counts: &str) -> presentation::ViewModel {
    presentation::ViewModel {
        page: 0,
        page_count: 1,
        rows: vec![presentation::ViewRow {
            cells: vec![format!("{} Loader Fixture", owner.name())],
        }],
        fields: vec![presentation::ViewField {
            id: format!("{} input", owner.name()),
            value: presentation::ViewFieldValue::Text(counts.to_owned()),
        }],
        actions: vec![view_action(
            &format!("{}confirm", owner.prefix()),
            Some(format!("Confirm {}", owner.name())),
        )],
        tabs: Vec::new(),
        resource_entries: vec![presentation::ViewResourceEntry {
            id: owner.material().to_owned(),
            have: 0.0,
            need: 1.0,
        }],
        markers: Vec::new(),
        reason: None,
    }
}

/// One declared table row in the visible HUD; input-only HUDs have no widgets.
#[must_use]
fn hud_model(field: &str, text: &str) -> presentation::ViewModel {
    presentation::ViewModel {
        page: 0,
        page_count: 1,
        rows: vec![presentation::ViewRow {
            cells: vec![field.to_owned(), text.to_owned()],
        }],
        fields: Vec::new(),
        actions: Vec::new(),
        tabs: Vec::new(),
        resource_entries: Vec::new(),
        markers: Vec::new(),
        reason: None,
    }
}

/// The model of the input HUD: no visible content at all, and every press and
/// release action this owner's bundle binds declared and enabled. An action is
/// admitted only against the model the owner holds, so this model is what makes a
/// real key edge possible; the view itself is what installs the bindings.
#[must_use]
fn input_model(owner: Owner) -> presentation::ViewModel {
    let mut actions = Vec::with_capacity(owner.edges().len() * 2);
    for edge in owner.edges() {
        for phase in [Phase::Press, Phase::Release] {
            actions.push(view_action(
                &format!("{}{}", owner.prefix(), edge.action_suffix(phase)),
                None,
            ));
        }
    }
    presentation::ViewModel {
        page: 0,
        page_count: 1,
        rows: Vec::new(),
        fields: Vec::new(),
        actions,
        tabs: Vec::new(),
        resource_entries: Vec::new(),
        markers: Vec::new(),
        reason: None,
    }
}

/// One declared action of a presented model, always enabled: a disabled action is
/// one the owner refuses, and the gate requires the input actions to be available.
#[must_use]
fn view_action(action_id: &str, label: Option<String>) -> presentation::ViewAction {
    presentation::ViewAction {
        action_id: action_id.to_owned(),
        enabled: true,
        label,
        deny_reason: None,
    }
}

/// One typed client-presentation request.
#[must_use]
fn client_command(request: presentation::ClientCommand) -> Command {
    commands::Command::ClientPresentation(request)
}

/// One line the connection's client reads.
#[must_use]
fn session_message(session: u64, text: impl Into<String>) -> Command {
    commands::Command::SendMessage(commands::SendMessage {
        target: commands::MessageTarget::Session(session),
        text: text.into(),
    })
}

/// Ask the owner to open one of this plugin's own views on one connection.
#[must_use]
fn open_view(request: &str, session: u64, view: &str, model: presentation::ViewModel) -> Command {
    client_command(presentation::ClientCommand::OpenClientView(
        presentation::OpenClientView {
            request: request.to_owned(),
            session,
            owned_view_id: view.to_owned(),
            model,
        },
    ))
}

/// Ask the owner to replace the model of one instance it answered with.
#[must_use]
fn present_view(
    session: u64,
    instance: &str,
    expected_revision: u64,
    model: presentation::ViewModel,
) -> Command {
    client_command(presentation::ClientCommand::PresentClientView(
        presentation::PresentClientView {
            session,
            view_instance_id: instance.to_owned(),
            expected_revision,
            model,
        },
    ))
}

/// Ask the owner to close one instance it answered with.
#[must_use]
fn close_view(session: u64, instance: &str) -> Command {
    client_command(presentation::ClientCommand::CloseClientView(
        presentation::CloseClientView {
            session,
            view_instance_id: instance.to_owned(),
        },
    ))
}

/// Ask the owner to play one sound this plugin's bundle declares.
#[must_use]
fn play_sound(
    session: u64,
    sound: &str,
    volume: f32,
    pitch: f32,
    position: Option<types::Position>,
) -> Command {
    client_command(presentation::ClientCommand::PlayClientSound(
        presentation::PlayClientSound {
            session,
            sound_id: sound.to_owned(),
            volume,
            pitch,
            position,
        },
    ))
}

/// Ask the owner to stop one sound on one connection.
#[must_use]
fn stop_sound(session: u64, sound: &str) -> Command {
    client_command(presentation::ClientCommand::StopClientSound(
        presentation::StopClientSound {
            session,
            sound_id: sound.to_owned(),
        },
    ))
}

/// Ask the player-inventory authority for one of this plugin's own custom block
/// items: the carrier the gate places and breaks to exercise the world projection.
#[must_use]
fn grant_block(session: u64, request: &str, block: &str, count: u8) -> Command {
    client_command(presentation::ClientCommand::GrantLoaderBlockItem(
        presentation::GrantLoaderBlockItem {
            request: request.to_owned(),
            session,
            block: block.to_owned(),
            count,
        },
    ))
}

/// Why the owner refused one view lifecycle request, in the contract's own words:
/// the five reasons are the server's, so a gate reads the reason the owner decided
/// rather than a wording this plugin chose.
#[must_use]
fn view_failure_name(failure: &ViewFailure) -> &'static str {
    match failure {
        ViewFailure::Refused => "refused",
        ViewFailure::PlayerUnavailable => "player-unavailable",
        ViewFailure::UnknownInstance => "unknown-instance",
        ViewFailure::StaleRevision => "stale-revision",
        ViewFailure::TooLarge => "too-large",
    }
}

/// The same for a refused item grant.
#[must_use]
fn grant_failure_name(failure: &LoaderItemGrantFailure) -> &'static str {
    match failure {
        LoaderItemGrantFailure::LoaderUnavailable => "loader-unavailable",
        LoaderItemGrantFailure::NotOwned => "not-owned",
        LoaderItemGrantFailure::PlayerUnavailable => "player-unavailable",
        LoaderItemGrantFailure::InventoryFull => "inventory-full",
        LoaderItemGrantFailure::RuntimeUnavailable => "runtime-unavailable",
        LoaderItemGrantFailure::Rejected => "rejected",
    }
}
