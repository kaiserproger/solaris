//! The pre-commit fixture: one real guest that answers the two hook questions,
//! remembers what it was asked, and is driven at runtime by its own command.
//!
//! `mode = "precommit"` in `config.toml` runs this module. The two hooks are the
//! one phase of the contract that answers a decision instead of a batch: a
//! before-build question ends in `keep` or `cancel`, a before-damage question ends
//! in `keep`, `cancel` or a replacement amount, and there is no shape in either
//! answer that carries a command, because the host is asking about an effect it
//! has not committed. A hook also runs where the logging import and every command
//! and asynchronous import are refused, so this fixture's hooks do nothing but
//! read their context and record it. Everything a test must *read* about a hook
//! question - how many arrived, what each carried, what it was answered - is
//! reported later by an ordinary command, `status` below.
//!
//! # The command
//!
//! One command root per package, named by the `root` key (default `precommit`,
//! and every acceptance case sets it): two deployments of this fixture are two
//! plugin identities, and a chain test reads each one's own state through its own
//! root. The manifest has to declare that root, exactly as it declares `hello`
//! today. Its first argument is the action:
//!
//! - `status` and `status <fence>` report every recorded question, oldest first,
//!   and the totals each kind reached. They change nothing, so a test may ask
//!   twice and compare. A fence is the caller's own token of up to 64 ASCII
//!   letters, digits, `_`, `-` and `.`: it is echoed as `fence=<token>` on the
//!   `end` line, so a caller holds a line proving *this* request produced *this*
//!   report instead of trusting the newest report-shaped line in the chat stream.
//! - `reset` and `reset <fence>` drop the recorded questions and both totals, then
//!   report what is left - an `end` line of zeros - so a scenario can start from a
//!   clean state without deploying again.
//! - `build keep` and `build cancel` set what the before-build hook answers.
//! - `damage keep`, `damage cancel` and `damage replace <scale> <delta>` set what
//!   the before-damage hook answers; `replace` answers
//!   `replace(amount * scale + delta)` for the amount its context carried, where
//!   `keep` leaves that amount exactly as it stands - the accumulated one when an
//!   earlier handler already replaced it, never the raw request again.
//! - `fault none` and `fault <hook> <kind>` set the deliberate failure, where
//!   `<hook>` is `build` or `damage` and `<kind>` is `trap`, `log`, `spin` or
//!   `error`: the first three are the misbehaviours below, and `error` answers the
//!   plugin's own failure instead of a decision.
//!
//! Every accepted control answers one `control` line naming the state it left, so
//! a test that changes a decision and then sees a hook answered the old way knows
//! the control, not the hook, was what did not arrive. An action this fixture does
//! not implement answers an `error` line and changes nothing.
//!
//! # The keys
//!
//! - `root = "<command>"` (default `precommit`) is this instance's command root.
//! - `build = "keep" | "cancel"` (default `keep`) is the before-build answer.
//! - `damage = "keep" | "cancel" | "replace"` (default `keep`) is the
//!   before-damage answer; `damage_scale` (default `1.0`) and `damage_delta`
//!   (default `0.0`) are the replacement's own numbers. Two deployments of this
//!   fixture at two orders make a chain's arithmetic readable in the two status
//!   reports: a raw 20 answered by `scale = 1, delta = 10` and then
//!   `scale = 2, delta = 0` ends at 60, and the same two swapped end at 50.
//!   A `cancel` is the other half of what a chain test reads: it settles the
//!   chain, so a later handler is not asked at all - its own report shows no
//!   question for that event, and the earlier handler's `replace` is what the
//!   damage never became.
//! - `hook_fault = "trap" | "log" | "spin" | "error"` (absent: answer normally)
//!   makes one hook misbehave from the start, and `fault_hook = "build" |
//!   "damage"` (default `build`) says which of the two it is:
//!     - `trap` faults the call outright, with no decision, so the host's own
//!       failure path is what a chain sees;
//!     - `log` calls the logging import, which the hook phase refuses; the hook
//!       still means to answer, so its decision stays on its record, and an answer
//!       that arrives after it is the evidence that the host let the import
//!       through;
//!     - `spin` never returns, so only the host's fuel and epoch bounds end the
//!       call;
//!     - `error` answers the plugin's own failure instead of a decision, which is
//!       the answer path a trap does not take.
//!
//! A key or an action this fixture does not implement is refused - at `init` for a
//! key, with an `error` line for an action - rather than deciding for the operator:
//! a misspelled `build = "KEEP"` that silently kept would make a chain test pass
//! while testing nothing.

use std::collections::VecDeque;

use solaris_plugin_sdk::events::CommandInvoked;
use solaris_plugin_sdk::{
    log, message_player, BuildContext, BuildDecision, Command, Config, DamageContext,
    DamageDecision, DamageTarget, Event, Failure, HookActor, InitContext, LogLevel,
};

/// The command root this fixture answers when `root` is absent.
pub const DEFAULT_ROOT: &str = "precommit";

/// The prefix of every line this fixture answers with.
pub const LINE_PREFIX: &str = "P5_PRECOMMIT_WITNESS";

/// The line the `log` fault asks the host to print. It is a sentinel rather than a
/// sentence so that a host which wrongly admitted the import is identifiable from
/// what it captured.
const HOOK_LOG_LINE: &str = "P5_PRECOMMIT_HOOK_LOG";

/// How many questions one instance reports back. It is the *last* that many: a
/// scenario that drives more keeps the newest and says how many older ones are not
/// on the report, so the ledger is bounded and the report still carries the hooks
/// a test just ran.
// Reserve one of the host's 32 commands for the terminal report/fence.
const REPORT_LIMIT: usize = 31;

/// The most bytes one caller's fence token may carry. It is echoed on the `end`
/// line, which the host copies out of this instance's memory, so it is bounded
/// here rather than by whoever calls the command.
const MAX_FENCE_BYTES: usize = 64;

/// Which of the two hooks a configured fault happens in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hook {
    Build,
    Damage,
}

impl Hook {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "build" => Some(Self::Build),
            "damage" => Some(Self::Damage),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Damage => "damage",
        }
    }
}

/// The deliberate failure one hook makes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Fault the call outright: no decision is answered.
    Trap,
    /// Call the logging import, which the hook phase refuses, and then answer.
    Log,
    /// Never return, so only the host's fuel and epoch bounds end the call.
    Spin,
    /// Answer the plugin's own failure instead of a decision.
    Error,
}

impl Fault {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "trap" => Some(Self::Trap),
            "log" => Some(Self::Log),
            "spin" => Some(Self::Spin),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Trap => "trap",
            Self::Log => "log",
            Self::Spin => "spin",
            Self::Error => "error",
        }
    }

    /// Take the fault.
    ///
    /// Only `log` comes back, and that is deliberate: whether the forbidden
    /// import is refused is the host's decision, not this fixture's, so the hook
    /// goes on to answer a decision the host either receives or never sees. The
    /// other faults have already ended the call when this returns, which is what
    /// their `decision=none` record says.
    fn take(self) {
        match self {
            Self::Log => log(LogLevel::Info, HOOK_LOG_LINE),
            Self::Trap => core::arch::wasm32::unreachable(),
            Self::Spin => loop {
                let _ = std::hint::black_box(1_u64).wrapping_mul(3);
            },
            // `Error` never reaches here: the hook answers the plugin's own
            // failure instead of taking the fault, so this arm exists only to keep
            // the match exhaustive.
            Self::Error => {}
        }
    }
}

/// What the before-build hook answers.
#[derive(Clone, Copy)]
enum BuildAnswer {
    Keep,
    Cancel,
}

impl BuildAnswer {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "keep" => Some(Self::Keep),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Cancel => "cancel",
        }
    }
}

/// What the before-damage hook answers.
#[derive(Clone, Copy)]
enum DamageAnswer {
    Keep,
    Cancel,
    /// Answer `replace(amount * scale + delta)` for the amount the context
    /// carried. Nothing bounds the product here: a test that configures a
    /// negative or infinite replacement is driving the host's own refusal of a
    /// malformed answer, which is a case this fixture has to be able to produce.
    Replace {
        scale: f32,
        delta: f32,
    },
}

/// The pre-commit fixture of one instance.
pub struct Hooks {
    /// The command root this instance answers. A chain test deploys this same
    /// fixture twice, and this is what tells the two identities' answers apart.
    root: String,
    /// The id the host bound to this instance, on every line it answers.
    plugin: String,
    build: BuildAnswer,
    damage: DamageAnswer,
    /// The deliberate fault and the hook kind it happens in.
    fault: Option<(Hook, Fault)>,
    /// The questions this instance was asked, oldest first, at most
    /// [`REPORT_LIMIT`] of the newest.
    lines: VecDeque<String>,
    /// How many questions of each kind reached this instance, including the ones
    /// that are no longer on the report.
    build_calls: u32,
    damage_calls: u32,
}

impl Hooks {
    /// Read `config.toml` into this fixture's own vocabulary.
    ///
    /// A key this fixture does not implement is the plugin's own invalid answer,
    /// so it fails `init` instead of falling back to an answer the operator did
    /// not ask for.
    pub fn configure(config: &Config, context: &InitContext) -> Result<Self, Failure> {
        let table = config.toml();
        let value = |key: &str| table.as_ref().and_then(|table| table.get(key));
        let number = |key: &str, fallback: f32| -> f32 {
            value(key).map_or(fallback, |number| {
                number
                    .as_float()
                    .map(|number| number as f32)
                    .or_else(|| number.as_integer().map(|number| number as f32))
                    .unwrap_or(fallback)
            })
        };
        let root = match value("root") {
            None => DEFAULT_ROOT.to_owned(),
            Some(value) => match value.as_str() {
                Some(root) => root.to_owned(),
                None => return Err(Failure::Invalid),
            },
        };
        let build = match value("build") {
            None => BuildAnswer::Keep,
            Some(value) => match value.as_str().and_then(BuildAnswer::parse) {
                Some(answer) => answer,
                None => return Err(Failure::Invalid),
            },
        };
        let damage = match value("damage") {
            None => DamageAnswer::Keep,
            Some(value) => match value.as_str() {
                Some("keep") => DamageAnswer::Keep,
                Some("cancel") => DamageAnswer::Cancel,
                Some("replace") => DamageAnswer::Replace {
                    scale: number("damage_scale", 1.0),
                    delta: number("damage_delta", 0.0),
                },
                _ => return Err(Failure::Invalid),
            },
        };
        let fault = match value("hook_fault") {
            None => None,
            Some(value) => match value.as_str().and_then(Fault::parse) {
                Some(fault) => Some(fault),
                None => return Err(Failure::Invalid),
            },
        };
        let fault_hook = match value("fault_hook") {
            None => Hook::Build,
            Some(value) => match value.as_str().and_then(Hook::parse) {
                Some(hook) => hook,
                None => return Err(Failure::Invalid),
            },
        };
        Ok(Self {
            root,
            plugin: context.plugin_id.clone(),
            build,
            damage,
            fault: fault.map(|fault| (fault_hook, fault)),
            lines: VecDeque::new(),
            build_calls: 0,
            damage_calls: 0,
        })
    }

    /// The before-build answer: record the question, then decide or fault.
    ///
    /// The record is written before the fault, so the report tells "the host asked
    /// this hook" apart from "the host never reached it": a call that traps still
    /// leaves its question in this instance's own state.
    pub fn before_build(&mut self, context: &BuildContext) -> Result<BuildDecision, Failure> {
        let index = self.build_calls + 1;
        let fault = self.fault_for(Hook::Build);
        let decision = match self.build {
            BuildAnswer::Keep => BuildDecision::Keep,
            BuildAnswer::Cancel => BuildDecision::Cancel,
        };
        let marker = decision_field(self.build.marker(), fault);
        self.record(
            Hook::Build,
            build_line(&self.plugin, index, context, &marker),
        );
        match fault {
            Some(Fault::Error) => Err(Failure::Failed),
            Some(fault) => {
                fault.take();
                Ok(decision)
            }
            None => Ok(decision),
        }
    }

    /// The before-damage answer: the same record-then-decide shape as the build
    /// hook, over the amount the context carried.
    pub fn before_damage(&mut self, context: &DamageContext) -> Result<DamageDecision, Failure> {
        let index = self.damage_calls + 1;
        let fault = self.fault_for(Hook::Damage);
        let decision = match self.damage {
            DamageAnswer::Keep => DamageDecision::Keep,
            DamageAnswer::Cancel => DamageDecision::Cancel,
            DamageAnswer::Replace { scale, delta } => {
                DamageDecision::Replace(context.amount * scale + delta)
            }
        };
        let marker = decision_field(&damage_marker(&decision), fault);
        self.record(
            Hook::Damage,
            damage_line(&self.plugin, index, context, &marker),
        );
        match fault {
            Some(Fault::Error) => Err(Failure::Failed),
            Some(fault) => {
                fault.take();
                Ok(decision)
            }
            None => Ok(decision),
        }
    }

    /// The commands one delivered batch answers with. This fixture answers its own
    /// root and nothing else: a hook question is not an event, so no callback of
    /// this mode ever has to be silent about one.
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            if let Event::CommandInvoked(invoked) = event {
                if invoked.name == self.root {
                    commands.extend(self.control(invoked));
                }
            }
        }
        Ok(commands)
    }

    /// One command of this instance's root: the report, a reset, or a control that
    /// changes what a hook answers from here on.
    fn control(&mut self, invoked: &CommandInvoked) -> Vec<Command> {
        let mut arguments = invoked.arguments.iter().map(String::as_str);
        let Some(action) = arguments.next() else {
            return vec![self.error(invoked, "no-action")];
        };
        let rest: Vec<&str> = arguments.collect();
        match action {
            "status" | "reset" => {
                let fence = match rest.as_slice() {
                    [] => None,
                    [fence] if is_fence(fence) => Some(*fence),
                    [_] => return vec![self.error(invoked, "bad-fence")],
                    _ => return vec![self.error(invoked, "report-takes-one-fence")],
                };
                if action == "reset" {
                    self.lines.clear();
                    self.build_calls = 0;
                    self.damage_calls = 0;
                }
                self.status(invoked, fence)
            }
            "build" => match rest.as_slice() {
                [answer] => match BuildAnswer::parse(answer) {
                    Some(answer) => {
                        self.build = answer;
                        vec![self.control_line(invoked, &format!("build {}", answer.marker()))]
                    }
                    None => vec![self.error(invoked, "unknown-build-answer")],
                },
                _ => vec![self.error(invoked, "build-needs-one-answer")],
            },
            "damage" => match rest.as_slice() {
                ["keep"] => {
                    self.damage = DamageAnswer::Keep;
                    vec![self.control_line(invoked, "damage keep")]
                }
                ["cancel"] => {
                    self.damage = DamageAnswer::Cancel;
                    vec![self.control_line(invoked, "damage cancel")]
                }
                ["replace", scale, delta] => match (scale.parse::<f32>(), delta.parse::<f32>()) {
                    (Ok(scale), Ok(delta)) => {
                        self.damage = DamageAnswer::Replace { scale, delta };
                        vec![self.control_line(invoked, &format!("damage replace {scale} {delta}"))]
                    }
                    _ => vec![self.error(invoked, "replace-needs-two-numbers")],
                },
                _ => vec![self.error(invoked, "unknown-damage-answer")],
            },
            "fault" => match rest.as_slice() {
                ["none"] => {
                    self.fault = None;
                    vec![self.control_line(invoked, "fault none")]
                }
                [hook, kind] => match (Hook::parse(hook), Fault::parse(kind)) {
                    (Some(hook), Some(fault)) => {
                        self.fault = Some((hook, fault));
                        vec![self.control_line(
                            invoked,
                            &format!("fault {} {}", hook.name(), fault.name()),
                        )]
                    }
                    _ => vec![self.error(invoked, "unknown-fault")],
                },
                _ => vec![self.error(invoked, "fault-needs-none-or-hook-kind")],
            },
            _ => vec![self.error(invoked, "unknown-action")],
        }
    }

    /// The report: every question still on the ledger, oldest first, and the
    /// totals each kind reached. Reading it changes nothing, so a test may ask
    /// twice and compare the two reports.
    ///
    /// `fence` is echoed on the `end` line. A caller that names one therefore
    /// holds a line that says this request produced this report, rather than
    /// trusting the most recent report-shaped line in a chat stream, where an
    /// earlier request's answer (or an earlier `reset`'s zeros) would read exactly
    /// the same.
    fn status(&self, invoked: &CommandInvoked, fence: Option<&str>) -> Vec<Command> {
        let mut commands: Vec<Command> = self
            .lines
            .iter()
            .map(|line| message_player(&invoked.player, line.clone()))
            .collect();
        let total = self.build_calls + self.damage_calls;
        let fence = fence.map_or_else(String::new, |fence| format!(" fence={fence}"));
        let end = format!(
            "{LINE_PREFIX} plugin={} end build={} damage={} shown={} dropped={}{fence}",
            self.plugin,
            self.build_calls,
            self.damage_calls,
            self.lines.len(),
            total - self.lines.len() as u32,
        );
        commands.push(message_player(&invoked.player, end));
        commands
    }

    /// The one line an accepted control answers with, naming the state it left.
    fn control_line(&self, invoked: &CommandInvoked, state: &str) -> Command {
        message_player(
            &invoked.player,
            format!("{LINE_PREFIX} plugin={} control {state}", self.plugin),
        )
    }

    /// The one line a refused action answers with. Nothing was changed, which is
    /// what makes the line a result rather than a report.
    fn error(&self, invoked: &CommandInvoked, reason: &str) -> Command {
        message_player(
            &invoked.player,
            format!("{LINE_PREFIX} plugin={} error {reason}", self.plugin),
        )
    }

    /// The fault this hook kind was configured with, if it is this one's.
    fn fault_for(&self, hook: Hook) -> Option<Fault> {
        match self.fault {
            Some((fault_hook, fault)) if fault_hook == hook => Some(fault),
            _ => None,
        }
    }

    /// Record one question, oldest first, keeping the newest [`REPORT_LIMIT`].
    fn record(&mut self, hook: Hook, line: String) {
        match hook {
            Hook::Build => self.build_calls += 1,
            Hook::Damage => self.damage_calls += 1,
        }
        if self.lines.len() == REPORT_LIMIT {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// Whether one report argument is a fence this fixture echoes.
///
/// A fence is the caller's own token, bounded because it comes back on a line the
/// host copies out of this instance's memory: 1..=64 bytes of ASCII letters,
/// digits, `_`, `-` and `.`. Anything else is refused rather than echoed.
fn is_fence(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_FENCE_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Who asked for the effect, in the one short form the report lines use.
fn actor_tag(actor: &HookActor) -> String {
    match actor {
        HookActor::Player(player) => format!("player:{}:{}", player.uuid, player.session),
        HookActor::Entity(entity) => format!("entity:{entity}"),
        HookActor::Plugin(plugin) => format!("plugin:{plugin}"),
        HookActor::Environment => "environment".to_owned(),
    }
}

/// What the damage is about to reach, in the same short form.
fn target_tag(target: &DamageTarget) -> String {
    match target {
        DamageTarget::Player(player) => format!("player:{}:{}", player.uuid, player.session),
        DamageTarget::Entity(entity) => format!("entity:{entity}"),
    }
}

/// The decision field of one record.
///
/// A hook that traps, spins past its deadline or answers a plugin error never
/// returns a decision, and that is what `decision=none` says. A hook that calls
/// the forbidden logging import still means to answer, so its decision stays on
/// the record.
fn decision_field(decision: &str, fault: Option<Fault>) -> String {
    match fault {
        None => format!("decision={decision}"),
        Some(Fault::Log) => format!("decision={decision} fault=log"),
        Some(fault) => format!("decision=none fault={}", fault.name()),
    }
}

/// The decision a damage hook answered, as the report names it.
fn damage_marker(decision: &DamageDecision) -> String {
    match decision {
        DamageDecision::Keep => "keep".to_owned(),
        DamageDecision::Cancel => "cancel".to_owned(),
        DamageDecision::Replace(amount) => format!("replace({amount:?})"),
    }
}

/// One before-build question, with the first edit's own coordinates and state ids
/// and the dimension the batch is about.
fn build_line(plugin: &str, index: u32, context: &BuildContext, marker: &str) -> String {
    let first = context.edits.first().map_or_else(
        || "none".to_owned(),
        |edit| {
            format!(
                "{},{},{},{},{}",
                edit.x, edit.y, edit.z, edit.previous_state, edit.proposed_state
            )
        },
    );
    format!(
        "{} plugin={plugin} build {index} actor={} dimension={} edits={} first={first} {marker}",
        LINE_PREFIX,
        actor_tag(&context.actor),
        context.dimension,
        context.edits.len(),
    )
}

/// One before-damage question, with the amount the context carried - the amount
/// the previous handler in a chain left, when an earlier one replaced it.
fn damage_line(plugin: &str, index: u32, context: &DamageContext, marker: &str) -> String {
    let position = &context.position;
    format!(
        "{} plugin={plugin} damage {index} actor={} target={} kind={} dimension={} position={:?},{:?},{:?} amount={:?} {marker}",
        LINE_PREFIX,
        actor_tag(&context.source),
        target_tag(&context.target),
        context.kind,
        context.dimension,
        position.x,
        position.y,
        position.z,
        context.amount,
    )
}
