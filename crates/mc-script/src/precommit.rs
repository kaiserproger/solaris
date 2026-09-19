//! The native side of the two pre-commit hooks: what a game owner asks a
//! registered package before it commits a build or applies damage.
//!
//! A hook is a question, so the answer is native rather than guest-visible: the
//! owner builds one [`HookContext`] out of the raw request it is about to apply,
//! hands it to [`ScriptBoundary::begin_precommit`], and receives a
//! [`PendingDecision`] it resolves to one [`Approval`]. The approval is the one
//! authority the owner commits against: it is single-use, it expires with the
//! deadline the request was queued under, and it stops being valid the moment the
//! registration roster changes. A guest never sees a ticket, and a decision the
//! owner does not consume is not a decision the owner applied.
//!
//! The bound the whole feature runs under is one absolute deadline, created when
//! the request is admitted and covering both the queueing and the whole handler
//! chain. The value is a conservative watchdog of the host work the chain may
//! spend, not a latency promise: it exists so an owner waiting on a plugin can
//! never wait forever, and the host re-arms each guest only inside what is left
//! of it.
//!
//! Raw, not reduced. [`DamageContext::amount`] is the amount the native owner is
//! about to apply *before* armor, shields, resistance or any other reduction, and
//! an approved [`HookDecision::Replace`] is likewise a raw amount: the owner
//! applies it through its own damage pipeline exactly as it would have applied
//! the request, so a decision is a statement about what was asked for and not a
//! shortcut past the game's own rules.
//!
//! Fail closed, and only where it is safe to fail. A decision the owner asked for
//! is committed only through [`Approval::consume`], which refuses everything that
//! is not a decision the owner may apply: a passed deadline, a roster that changed,
//! a withdrawn source registration, a second consumer, and a cancellation. The
//! failures the *boundary* found always refuse, whatever an operator's
//! [`HookFailurePolicy`] says - the policy is what a handler's own failure means,
//! not a licence to commit an effect nobody decided. And a package that is
//! registered but whose store was retired keeps its registration: its handler
//! fails, its registration's policy decides, and the chain never silently proceeds
//! as though the package were not asked.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::ScriptPosition;

/// The absolute bound one pre-commit request lives under.
///
/// It covers admission, the queue wait and the whole handler chain, and it is the
/// host's own watchdog rather than a promise about how long a decision takes. It
/// is deliberately conservative - a plugin that cannot answer a question about an
/// effect inside a hundred milliseconds must not be holding that effect.
pub const PRECOMMIT_DEADLINE: Duration = Duration::from_millis(100);

/// Edits one before-build question may carry: the same bound a guest's own build
/// command has, so a chain is never asked to decide a batch no single guest could
/// have asked for.
pub const MAX_PRECOMMIT_BUILD_EDITS: usize = 512;

/// Pre-commit requests one boundary may have in flight at once.
///
/// The count is what bounds the native side's request state: a request holds one
/// of these slots through the answer until its approval is consumed or dropped,
/// so an owner that asks faster than it commits is refused with
/// [`HookFailure::QueueFull`] instead of growing an unbounded ledger.
pub const MAX_PRECOMMIT_REQUESTS: usize = 64;

/// Which of the two questions a context or a registration is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HookKind {
    /// Before a batch of block edits is committed.
    Build,
    /// Before a damage amount is applied.
    Damage,
}

impl HookKind {
    /// The hook's contract name, as an operator writes it in a manifest
    /// declaration or a server registration.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Build => "before-build",
            Self::Damage => "before-damage",
        }
    }

    /// The hook one contract name means, or nothing when no hook has it.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "before-build" => Some(Self::Build),
            "before-damage" => Some(Self::Damage),
            _ => None,
        }
    }
}

impl fmt::Display for HookKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One player a hook is about: the stable identity and the live connection.
///
/// The session is the connection that was current when the owner built the
/// context, not a later lookup of whatever session the identity holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPlayer {
    uuid: String,
    session: u64,
}

impl HookPlayer {
    /// The player and the session the effect is about.
    pub fn try_new(uuid: impl Into<String>, session: u64) -> Result<Self, HookContextError> {
        let uuid = uuid.into();
        if uuid.is_empty() {
            return Err(HookContextError::EmptyValue {
                field: "hook player uuid",
            });
        }
        Ok(Self { uuid, session })
    }

    /// The player's stable identity.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// The live connection the question is about.
    #[must_use]
    pub const fn session(&self) -> u64 {
        self.session
    }
}

/// Who asked for the effect the hook is about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookActor {
    /// A connected player.
    Player(HookPlayer),
    /// One entity the owner identifies by its runtime id.
    Entity(u64),
    /// A package's own programmatic request.
    Plugin(String),
    /// The game itself: no player and no entity, as in a natural cause.
    Environment,
}

/// One block change the build is about to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BuildEdit {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// What the block holds now.
    pub previous_state: u32,
    /// What the build would put there.
    pub proposed_state: u32,
}

impl BuildEdit {
    /// Constructs one edit; [`BuildContext`] validates the batch question.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32, previous_state: u32, proposed_state: u32) -> Self {
        Self {
            x,
            y,
            z,
            previous_state,
            proposed_state,
        }
    }
}

/// One before-build question: the edits one actor is about to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildContext {
    actor: HookActor,
    dimension: String,
    edits: Vec<BuildEdit>,
}

impl BuildContext {
    /// A question about `edits`, refused when it is not one a guest could have
    /// asked: more edits than the native build-command bound, or no dimension.
    pub fn try_new(
        actor: HookActor,
        dimension: impl Into<String>,
        edits: Vec<BuildEdit>,
    ) -> Result<Self, HookContextError> {
        let dimension = dimension.into();
        if dimension.is_empty() {
            return Err(HookContextError::EmptyValue {
                field: "build dimension",
            });
        }
        if edits.len() > MAX_PRECOMMIT_BUILD_EDITS {
            return Err(HookContextError::TooManyEdits {
                count: edits.len(),
                max: MAX_PRECOMMIT_BUILD_EDITS,
            });
        }
        Ok(Self {
            actor,
            dimension,
            edits,
        })
    }

    /// Who is asking for the build.
    #[must_use]
    pub fn actor(&self) -> &HookActor {
        &self.actor
    }

    /// The dimension the edits are in.
    #[must_use]
    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    /// The edits the build would commit, in the order the owner holds them.
    #[must_use]
    pub fn edits(&self) -> &[BuildEdit] {
        &self.edits
    }
}

/// What the damage the hook is about is applied to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DamageTarget {
    Player(HookPlayer),
    Entity(u64),
}

/// One before-damage question.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageContext {
    source: HookActor,
    target: DamageTarget,
    kind: String,
    dimension: String,
    position: ScriptPosition,
    amount: f32,
}

impl DamageContext {
    /// A question about one raw damage request.
    ///
    /// `amount` is what the owner is about to apply before any native reduction
    /// and has to be a finite, non-negative number: a decision made about a
    /// negative or a not-a-number request would be a decision about nothing, so
    /// such a request is refused here instead of reaching a plugin.
    pub fn try_new(
        source: HookActor,
        target: DamageTarget,
        kind: impl Into<String>,
        dimension: impl Into<String>,
        position: ScriptPosition,
        amount: f32,
    ) -> Result<Self, HookContextError> {
        let kind = kind.into();
        let dimension = dimension.into();
        if kind.is_empty() {
            return Err(HookContextError::EmptyValue {
                field: "damage kind",
            });
        }
        if dimension.is_empty() {
            return Err(HookContextError::EmptyValue {
                field: "damage dimension",
            });
        }
        if !amount.is_finite() || amount < 0.0 {
            return Err(HookContextError::InvalidAmount { amount });
        }
        Ok(Self {
            source,
            target,
            kind,
            dimension,
            position,
            amount,
        })
    }

    /// Who produced the damage.
    #[must_use]
    pub fn source(&self) -> &HookActor {
        &self.source
    }

    /// What the damage would be applied to.
    #[must_use]
    pub fn target(&self) -> &DamageTarget {
        &self.target
    }

    /// The damage type, as the owner names it.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The dimension the effect happens in.
    #[must_use]
    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    /// Where the effect was produced.
    #[must_use]
    pub const fn position(&self) -> ScriptPosition {
        self.position
    }

    /// The raw amount the owner is about to apply, before every native reduction.
    #[must_use]
    pub const fn amount(&self) -> f32 {
        self.amount
    }
}

/// One question a game owner asks before it commits an effect.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum HookContext {
    Build(BuildContext),
    Damage(DamageContext),
}

impl HookContext {
    /// Which of the two hooks this question belongs to.
    #[must_use]
    pub const fn kind(&self) -> HookKind {
        match self {
            Self::Build(_) => HookKind::Build,
            Self::Damage(_) => HookKind::Damage,
        }
    }

    /// The package whose own admitted request opened this question, if one did.
    ///
    /// A programmatic request is only the authority the boundary gave a package
    /// when it admitted that package's command, so the package it came from is
    /// carried on the context and the boundary fences the decision against that
    /// package's live registration.
    #[must_use]
    pub fn plugin_source(&self) -> Option<&str> {
        match self {
            Self::Build(build) => match &build.actor {
                HookActor::Plugin(plugin_id) => Some(plugin_id),
                _ => None,
            },
            Self::Damage(damage) => match &damage.source {
                HookActor::Plugin(plugin_id) => Some(plugin_id),
                _ => None,
            },
        }
    }
}

/// A live authority a decision was admitted under, captured when it was admitted.
///
/// A package's programmatic request is authority the boundary granted, and that
/// authority can be taken away while the question is in flight - a retired
/// instance loses its routes, a reload replaces them, an operator withdraws a
/// grant. The boundary therefore captures the registration it admitted the
/// question under and re-checks it where the decision is consumed, so a decision
/// made under authority that no longer exists is never committed. The
/// implementation is the boundary's own registration authority; this trait only
/// names what a fence has to answer.
pub trait SourceRegistration: fmt::Debug + Send + Sync {
    /// Whether the captured registration is still live and unchanged.
    fn is_live(&self) -> bool;
}

/// Why a hook context was refused before a plugin ever saw it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum HookContextError {
    /// More edits than one build question may carry.
    TooManyEdits { count: usize, max: usize },
    /// A damage amount that is negative, infinite or not a number.
    InvalidAmount { amount: f32 },
    /// A value the contract requires to be present.
    EmptyValue { field: &'static str },
}

impl fmt::Display for HookContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyEdits { count, max } => write!(
                formatter,
                "the build question carries {count} edits, the bound is {max}"
            ),
            Self::InvalidAmount { amount } => {
                write!(
                    formatter,
                    "the damage amount {amount} is not finite and >= 0"
                )
            }
            Self::EmptyValue { field } => write!(formatter, "{field} is empty"),
        }
    }
}

impl std::error::Error for HookContextError {}

/// What a hook chain concluded.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum HookDecision {
    /// Commit the effect as the owner asked for it.
    Keep,
    /// Do not commit the effect at all.
    Cancel,
    /// Apply this raw damage amount instead of the one the context carried.
    ///
    /// Only a damage question may be answered this way, and the amount has to be
    /// finite and at least zero. Zero is a deliberately admitted no-damage
    /// outcome: it is a decision the chain made, not the cancellation it is not.
    Replace(f32),
}

impl HookDecision {
    /// A replacement amount, or nothing when the amount is not one a decision may
    /// carry.
    #[must_use]
    pub fn replacement(amount: f32) -> Option<Self> {
        (amount.is_finite() && amount >= 0.0).then_some(Self::Replace(amount))
    }

    /// The amount this decision applies instead of the requested one, if any.
    #[must_use]
    pub const fn amount(self) -> Option<f32> {
        if let Self::Replace(amount) = self {
            Some(amount)
        } else {
            None
        }
    }

    /// Whether this decision refuses the effect.
    #[must_use]
    pub const fn is_cancelled(self) -> bool {
        !matches!(self, Self::Keep | Self::Replace(_))
    }

    /// Whether this decision is one the given hook kind can be answered with.
    #[must_use]
    pub fn is_valid_for(self, kind: HookKind) -> bool {
        matches!(
            (self, kind),
            (
                Self::Keep | Self::Cancel,
                HookKind::Build | HookKind::Damage
            )
        ) || matches!(
            (self, kind),
            (Self::Replace(amount), HookKind::Damage)
                if amount.is_finite() && amount >= 0.0
        )
    }
}

/// What a registered handler's failure means for the effect it was asked about.
///
/// The operator decides: `Deny` is the default because a hook exists to hold an
/// effect back, so a handler that cannot answer leaves the effect uncommitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HookFailurePolicy {
    /// A handler that failed refuses the effect.
    #[default]
    Deny,
    /// A handler that failed is skipped and the chain continues.
    Keep,
}

/// One operator registration: which package is asked which question, when, and
/// what its failure means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistration {
    plugin_id: String,
    kind: HookKind,
    order: i32,
    on_failure: HookFailurePolicy,
}

impl HookRegistration {
    /// The operator's decision to ask `plugin_id` the `kind` question.
    ///
    /// Nothing here checks that the package exists or that it declares the hook:
    /// both are the deployment's own validation, which refuses a registration no
    /// discovered package can serve instead of letting it sit in a roster that
    /// never answers.
    #[must_use]
    pub fn new(
        plugin_id: impl Into<String>,
        kind: HookKind,
        order: i32,
        on_failure: HookFailurePolicy,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            kind,
            order,
            on_failure,
        }
    }

    /// The package this registration asks.
    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// The question it asks.
    #[must_use]
    pub const fn kind(&self) -> HookKind {
        self.kind
    }

    /// Where the handler sits in the chain.
    #[must_use]
    pub const fn order(&self) -> i32 {
        self.order
    }

    /// What this handler's failure means for the effect.
    #[must_use]
    pub const fn on_failure(&self) -> HookFailurePolicy {
        self.on_failure
    }
}

/// The chain's own order: ascending `order`, then plugin id.
///
/// Both halves of the boundary sort with this one function so the roster an
/// operator configured and the chain a host runs can never disagree about which
/// handler is asked first.
#[must_use]
pub fn by_roster_order(left: &HookRegistration, right: &HookRegistration) -> std::cmp::Ordering {
    (left.order, left.plugin_id.as_str()).cmp(&(right.order, right.plugin_id.as_str()))
}

/// Why a roster of registrations was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookRosterError {
    /// The same package is registered twice for one hook: the chain would ask it
    /// twice and count its answer once.
    Duplicate { plugin_id: String, kind: HookKind },
    /// A registration that names no package cannot be asked anything.
    EmptyPluginId,
}

impl fmt::Display for HookRosterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate { plugin_id, kind } => write!(
                formatter,
                "plugin {plugin_id:?} is registered twice for {kind}"
            ),
            Self::EmptyPluginId => formatter.write_str("a hook registration names no plugin"),
        }
    }
}

impl std::error::Error for HookRosterError {}

/// Why a pre-commit request produced no decision.
///
/// Every member is terminal: the owner refuses the effect it asked about rather
/// than retrying the question, because an effect held back is the safe outcome and
/// a retry loop would be a plugin deciding how long gameplay waits.
///
/// Which members are policy-mediated is deliberate. A handler's own failure is
/// decided by its registration's [`HookFailurePolicy`]: `Deny` answers
/// [`Self::Cancelled`], `Keep` skips that handler and lets the chain continue.
/// Everything the *boundary* found - an exhausted deadline, a full FIFO, a closed
/// host, a registration that is no longer live, a context or answer that broke the
/// contract - always refuses, whatever any policy says, because none of them is a
/// handler's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookFailure {
    /// The effect is refused: a handler answered cancel, or a handler failed and
    /// its registration's policy denies.
    Cancelled,
    /// The request's absolute deadline passed before the chain answered.
    Expired,
    /// The host's input FIFO or this boundary's in-flight bound was full.
    QueueFull,
    /// No host could answer: the boundary's host side is closed, or the request
    /// was dropped unanswered.
    Unavailable,
    /// The decision's ticket is not live any more: it was consumed already, or
    /// the registration roster changed after the request was admitted.
    Stale,
    /// The owner's own fence refused the commit: its captured revisions,
    /// session or current permissions no longer admit it.
    ///
    /// The boundary itself never answers this. It is the vocabulary the owner
    /// uses for its own refusal, so an owner and its callers can name the same
    /// reason whichever side found it.
    PermissionDenied,
    /// The context or the chain's answer broke the contract.
    Invalid,
}

impl HookFailure {
    /// The failure's contract name, for an operator-facing diagnostic.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::QueueFull => "queue-full",
            Self::Unavailable => "unavailable",
            Self::Stale => "stale",
            Self::PermissionDenied => "permission-denied",
            Self::Invalid => "invalid",
        }
    }
}

impl fmt::Display for HookFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::error::Error for HookFailure {}

impl From<HookContextError> for HookFailure {
    fn from(_error: HookContextError) -> Self {
        Self::Invalid
    }
}

/// Why a request whose requester went away was still answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookAnswerError {
    /// The owner dropped the decision before the chain answered it.
    Abandoned,
}

impl fmt::Display for HookAnswerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the pre-commit requester is gone")
    }
}

impl std::error::Error for HookAnswerError {}

/// One queued question, as the host receives it from the boundary's own FIFO.
///
/// A request is answered exactly once, by value, and the answer is what the owner
/// is waiting on: dropping it unread answers the owner with
/// [`HookFailure::Unavailable`], so a host that stops serving can never leave an
/// owner waiting for a decision that will not come.
#[must_use]
pub struct Request {
    kind: HookKind,
    context: HookContext,
    deadline: Instant,
    reply: oneshot::Sender<Answered>,
    /// The authoritative roster snapshot this request was admitted under.
    hooks: Arc<[HookRegistration]>,
    /// Shared across approval copies so terminal consumption releases this slot.
    permit: Arc<StdMutex<Option<OutstandingPermit>>>,
}

impl Request {
    /// Which question this is.
    #[must_use]
    pub const fn kind(&self) -> HookKind {
        self.kind
    }

    /// The question itself.
    #[must_use]
    pub fn context(&self) -> &HookContext {
        &self.context
    }

    /// The ordered handler roster this request was admitted under.
    #[must_use]
    pub fn hooks(&self) -> &[HookRegistration] {
        &self.hooks
    }

    /// The absolute instant the whole chain has to answer by.
    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// What is left of the deadline: the budget a host may spend on the chain,
    /// and the arming a guest call takes.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Whether the deadline has already passed.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Answer the owner with a decision.
    ///
    /// A decision the question cannot be answered with - a replacement on a build,
    /// or an amount that is not finite and at least zero - is answered as
    /// [`HookFailure::Invalid`] instead: a chain that produced one has answered
    /// nothing the owner may commit.
    pub fn answer(self, decision: HookDecision) -> Result<(), HookAnswerError> {
        if !decision.is_valid_for(self.kind) {
            return self.fail(HookFailure::Invalid);
        }
        self.deliver(Ok(decision))
    }

    /// Answer the owner with a failure.
    ///
    /// A late answer is still delivered: the deadline is the owner's, and refusing
    /// the answer at [`Approval::consume`] keeps the one authority that decides
    /// whether the effect may be committed.
    pub fn fail(self, failure: HookFailure) -> Result<(), HookAnswerError> {
        self.deliver(Err(failure))
    }

    fn deliver(self, answer: Result<HookDecision, HookFailure>) -> Result<(), HookAnswerError> {
        self.reply
            .send(Answered {
                answer,
                permit: self.permit,
            })
            .map_err(|_| HookAnswerError::Abandoned)
    }
}

/// The host's terminal answer, carrying the request's admission slot to the
/// approval that decides whether native code spends it.
struct Answered {
    answer: Result<HookDecision, HookFailure>,
    permit: Arc<StdMutex<Option<OutstandingPermit>>>,
}

/// The owner's side of one queued question.
#[must_use]
pub struct PendingDecision {
    state: Pending,
    authority: Arc<PrecommitAuthority>,
    generation: u64,
    kind: HookKind,
    deadline: Instant,
    source: Option<Arc<dyn SourceRegistration>>,
    ticket: Arc<AtomicBool>,
}

enum Pending {
    /// The boundary answered before anything was queued: no hook of this kind is
    /// registered, so the effect takes the direct native path it always took.
    Direct(HookDecision),
    Queued(oneshot::Receiver<Answered>),
}

impl PendingDecision {
    /// The decision a boundary with no registered hook of this kind answers with.
    ///
    /// It carries the caller's own deadline like a queued request does: the direct
    /// path has nothing to wait for, but the owner's commit window is still the
    /// bound it asked under, so an approval is never unbounded.
    fn direct(
        authority: &Arc<PrecommitAuthority>,
        kind: HookKind,
        generation: u64,
        deadline: Instant,
    ) -> Self {
        Self {
            state: Pending::Direct(HookDecision::Keep),
            authority: Arc::clone(authority),
            generation,
            kind,
            deadline,
            source: None,
            ticket: Arc::new(AtomicBool::new(true)),
        }
    }

    /// The absolute instant this decision has to be committed by.
    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Wait for the chain's answer.
    ///
    /// This answers an [`Approval`] as soon as the host has decided, and it ends
    /// the wait by itself at the deadline: an owner can never wait longer than the
    /// bound its request was admitted under, whatever the host is doing. A
    /// decision that arrived past the deadline is handed on - the approval is what
    /// refuses it, and it does so for the commit rather than for the wait.
    pub async fn resolve(self) -> Result<Approval, HookFailure> {
        let Self {
            state,
            authority,
            generation,
            kind,
            deadline,
            source,
            ticket,
        } = self;
        let (decision, permit) = match state {
            Pending::Direct(decision) => (decision, None),
            Pending::Queued(reply) => {
                match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), reply).await
                {
                    Err(_elapsed) => return Err(HookFailure::Expired),
                    Ok(Err(_dropped)) => return Err(HookFailure::Unavailable),
                    Ok(Ok(Answered {
                        answer: Err(failure),
                        ..
                    })) => return Err(failure),
                    Ok(Ok(Answered {
                        answer: Ok(decision),
                        permit,
                    })) => (decision, Some(permit)),
                }
            }
        };
        Ok(Approval {
            authority,
            generation,
            kind,
            deadline,
            source,
            ticket,
            decision,
            _permit: permit,
        })
    }
}

/// One native decision the owner may commit against, exactly once.
///
/// The approval is the ticket: it stops being usable when its deadline passes,
/// when the registration roster it was created under changes, and when the
/// authority the question came from is gone. It can be consumed once however many
/// copies of it exist. Consuming it is the commit point; reading it is not, which
/// is what lets an owner check its own fences first.
#[derive(Debug, Clone)]
pub struct Approval {
    authority: Arc<PrecommitAuthority>,
    generation: u64,
    kind: HookKind,
    deadline: Instant,
    source: Option<Arc<dyn SourceRegistration>>,
    ticket: Arc<AtomicBool>,
    decision: HookDecision,
    /// Shared across approval copies; any terminal consume releases capacity.
    _permit: Option<Arc<StdMutex<Option<OutstandingPermit>>>>,
}

impl Approval {
    /// The decision the chain reached, readable without consuming the approval.
    ///
    /// A cancellation is visible here so an owner can refuse an effect before it
    /// prepares anything; [`Self::consume`] refuses it as well, so an owner that
    /// only consumes is refused too.
    #[must_use]
    pub const fn decision(&self) -> HookDecision {
        self.decision
    }

    /// The raw damage amount the chain decided on, if it decided one.
    #[must_use]
    pub const fn amount(&self) -> Option<f32> {
        self.decision.amount()
    }

    /// Which hook the decision answers.
    #[must_use]
    pub const fn kind(&self) -> HookKind {
        self.kind
    }

    /// The instant this approval stops being usable.
    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The registration generation this approval was created under.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Consume the one live ticket and answer the decision it carries.
    ///
    /// This is the atomic commit point. It refuses, in this order, an approval
    /// whose deadline has passed with [`HookFailure::Expired`]; one whose
    /// registration roster is no longer the roster it was admitted under, one
    /// whose captured source authority is no longer live, and any second consumer
    /// - the same approval used twice or another copy of it - with
    ///   [`HookFailure::Stale`]; and a cancelled decision with
    ///   [`HookFailure::Cancelled`]. Everything that succeeds therefore carries a
    ///   decision the owner may apply: the only members left are [`HookDecision::Keep`]
    ///   and a damage [`HookDecision::Replace`], so `?` on this call is already the
    ///   deny path and an owner cannot commit a cancellation by accident.
    ///
    /// The owner still checks its own captured revisions, session and permissions
    /// before it applies anything; this is only the plugin-side half of the commit.
    pub fn consume(&mut self) -> Result<HookDecision, HookFailure> {
        if Instant::now() >= self.deadline {
            self.release_permit();
            return Err(HookFailure::Expired);
        }
        if self.authority.generation() != self.generation {
            self.release_permit();
            return Err(HookFailure::Stale);
        }
        if let Some(source) = &self.source
            && !source.is_live()
        {
            self.release_permit();
            return Err(HookFailure::Stale);
        }
        if !self.ticket.swap(false, Ordering::AcqRel) {
            return Err(HookFailure::Stale);
        }
        self.release_permit();
        if matches!(self.decision, HookDecision::Cancel) {
            Err(HookFailure::Cancelled)
        } else if matches!(self.decision, HookDecision::Keep | HookDecision::Replace(_)) {
            Ok(self.decision)
        } else {
            Err(HookFailure::Invalid)
        }
    }

    fn release_permit(&self) {
        let Some(permit) = &self._permit else {
            return;
        };
        match permit.lock() {
            Ok(mut permit) => {
                permit.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
    }
}

/// The registration roster and the in-flight requests admitted under it.
///
/// It is shared by every clone of one boundary, because a game owner holds its own
/// clone while the host publishes the roster: the roster a request was admitted
/// under is the same one an approval checks before it commits.
#[derive(Debug)]
pub(crate) struct PrecommitAuthority {
    roster: StdMutex<HookRoster>,
    outstanding: Arc<AtomicUsize>,
}

#[derive(Debug, Default)]
struct HookRoster {
    generation: u64,
    hooks: Vec<HookRegistration>,
}

impl HookRoster {
    fn has(&self, kind: HookKind) -> bool {
        self.hooks.iter().any(|hook| hook.kind == kind)
    }
}

impl PrecommitAuthority {
    pub(crate) fn new() -> Self {
        Self {
            roster: StdMutex::new(HookRoster::default()),
            outstanding: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HookRoster> {
        match self.roster.lock() {
            Ok(roster) => roster,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The roster's current generation, whether or not it still holds this hook.
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    pub(crate) fn has(&self, kind: HookKind) -> bool {
        self.lock().has(kind)
    }

    /// Publish the operator's registrations, in the chain's own order.
    ///
    /// A refused roster is left unpublished, so a malformed one never replaces the
    /// registrations a live chain is already answering under.
    pub(crate) fn set_hooks(
        &self,
        mut hooks: Vec<HookRegistration>,
    ) -> Result<(), HookRosterError> {
        hooks.sort_by(by_roster_order);
        let mut seen = BTreeSet::new();
        for hook in &hooks {
            if hook.plugin_id.is_empty() {
                return Err(HookRosterError::EmptyPluginId);
            }
            if !seen.insert((hook.plugin_id.as_str(), hook.kind)) {
                return Err(HookRosterError::Duplicate {
                    plugin_id: hook.plugin_id.clone(),
                    kind: hook.kind,
                });
            }
        }
        let mut roster = self.lock();
        roster.hooks = hooks;
        // A write is a generation, whatever it holds: an owner's approval is
        // admitted under one roster, and any later roster is not that one.
        roster.generation += 1;
        Ok(())
    }

    /// Take one in-flight slot for a request about to be queued.
    fn reserve(&self) -> Result<OutstandingPermit, HookFailure> {
        let mut current = self.outstanding.load(Ordering::Acquire);
        loop {
            if current >= MAX_PRECOMMIT_REQUESTS {
                return Err(HookFailure::QueueFull);
            }
            match self.outstanding.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(OutstandingPermit {
                        outstanding: Arc::clone(&self.outstanding),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

/// One in-flight request's slot, released when the request's life ends.
#[derive(Debug)]
struct OutstandingPermit {
    outstanding: Arc<AtomicUsize>,
}

impl Drop for OutstandingPermit {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Admit one question onto the boundary's own FIFO.
///
/// A boundary with no registration for this hook answers the direct path
/// immediately: nothing is queued, nothing is allocated for a chain that does not
/// exist, and the owner commits the effect exactly as it did before the hook
/// contract existed. Otherwise the request is queued nonblocking - a full FIFO or
/// a full in-flight bound is refused rather than awaited, because a game owner is
/// holding the effect it asked about - and the deadline is absolute from this
/// moment, so the queue wait is inside the same bound as the chain.
///
/// `source` is the live registration the question came from, when it came from a
/// package's own admitted request: the approval re-checks it at the commit, so
/// authority withdrawn while the chain runs cannot be used to commit.
pub(crate) fn begin(
    authority: &Arc<PrecommitAuthority>,
    context: HookContext,
    deadline: Instant,
    source: Option<Arc<dyn SourceRegistration>>,
    publish: impl FnOnce(Request) -> Result<(), HookFailure>,
) -> Result<PendingDecision, HookFailure> {
    let kind = context.kind();
    let (generation, hooks) = {
        let roster = authority.lock();
        (
            roster.generation,
            roster
                .hooks
                .iter()
                .filter(|hook| hook.kind == kind)
                .cloned()
                .collect::<Arc<[_]>>(),
        )
    };
    if hooks.is_empty() {
        return Ok(PendingDecision::direct(
            authority, kind, generation, deadline,
        ));
    }
    let permit = Arc::new(StdMutex::new(Some(authority.reserve()?)));
    let (reply, queued) = oneshot::channel();
    let pending = PendingDecision {
        state: Pending::Queued(queued),
        authority: Arc::clone(authority),
        generation,
        kind,
        deadline,
        source,
        ticket: Arc::new(AtomicBool::new(true)),
    };
    let request = Request {
        kind,
        context,
        deadline,
        reply,
        hooks,
        permit,
    };
    // A refused publication drops the request, and with it the requester's own
    // ticket: the owner learns from the error that no decision is coming.
    publish(request)?;
    Ok(pending)
}

/// The deadline a request published now lives under.
#[must_use]
pub(crate) fn deadline_from_now() -> Instant {
    Instant::now() + PRECOMMIT_DEADLINE
}
