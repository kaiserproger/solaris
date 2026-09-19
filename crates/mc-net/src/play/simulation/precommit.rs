//! The native owner's side of the two pre-commit hooks.
//!
//! A game owner never lets a plugin hold an effect: it freezes the edits it is
//! about to commit, asks the registered chain about them, and commits against the
//! one [`Approval`] the chain answers with. Everything here is the build half of
//! that contract - the questions a build asks, the actor it asks them for, and the
//! single ticket the owner spends immediately before its own effects.
//!
//! Two properties are load-bearing:
//!
//! - A deployment with no registered `before-build` handler never reaches the
//!   boundary at all. [`request_build_hook`] checks `has_precommit_hooks` first,
//!   so the ordinary path allocates nothing, queues nothing and awaits nothing.
//! - The approval is spent at the commit, not at the ask. A chain that cancels, a
//!   ticket that expired and a ticket already spent by an earlier delivery all
//!   refuse the whole batch through [`refuse_build_approval`], which is the only
//!   thing between a decision and the owner's held item or world state.

use mc_script::ScriptBoundary;
use mc_script::precommit::{
    Approval, BuildContext, BuildEdit, HookActor, HookContext, HookDecision, HookFailure, HookKind,
    HookPlayer, PendingDecision,
};

use super::{BlockEdit, BlockEditPrecondition, SessionId, SimulationCommand, SimulationHandle};

/// The session registry one player's build is attributed through.
pub(in crate::play) use crate::play::session::SessionRegistry;

impl SimulationHandle {
    /// Point this world's build paths at the deployment's chain.
    ///
    /// The boundary is a clone of the one boundary every other owner uses, so the
    /// roster and the in-flight bound are the same objects on every path. The cell
    /// holds runtime input - the deployment only exists once the server has loaded
    /// its packages - which is why it is installed rather than initialized.
    pub(crate) fn install_precommit_boundary(&self, boundary: ScriptBoundary) {
        let _ = self.precommit_boundary.set(boundary);
    }

    /// The chain this world asks, if the deployment installed one.
    pub(crate) fn precommit_boundary(&self) -> Option<&ScriptBoundary> {
        self.precommit_boundary.get()
    }

    /// Ask a registered `before-build` chain about one batch of edits the caller
    /// is about to commit.
    ///
    /// `Ok(None)` means the direct path: no chain is installed or none is
    /// registered for `before-build`, and the caller commits exactly as it did
    /// before this contract existed.
    pub(crate) async fn request_build_hook(
        &self,
        actor: HookActor,
        edits: &[BlockEdit],
        preconditions: &[BlockEditPrecondition],
    ) -> Result<Option<Approval>, HookFailure> {
        request_build_hook(self.precommit_boundary.get(), actor, edits, preconditions).await
    }

    /// Resolve one queued question off the owner's turn and wake the owner with
    /// the command the answer produced, retaining the original request response.
    pub(in crate::play) fn spawn_precommit_resume<F>(
        &self,
        pending: PendingDecision,
        session_fence: Option<SessionId>,
        response: Option<super::queue::SimulationResponseSender>,
        resume: F,
    ) where
        F: FnOnce(Result<Approval, HookFailure>) -> SimulationCommand + Send + 'static,
    {
        spawn_precommit_resume(self.clone(), pending, session_fence, response, resume);
    }
}

/// The one dimension the native world owner mutates.
///
/// The server runs a single world, the same assumption every native block-edit
/// command already makes, so a hook context never has to guess which dimension an
/// edit belongs to.
pub(in crate::play) const BUILD_DIMENSION: &str = "minecraft:overworld";

/// The hook question one native batch of edits asks, or nothing when a question
/// cannot be asked truthfully.
///
/// Every edit has to name the state it expects to replace, and the owner already
/// holds that state: it is the precondition the conditional commit compares
/// against. An edit without one has no honest `previous_state` to publish, so the
/// batch is not described to the chain at all.
pub(in crate::play) fn build_context(
    actor: HookActor,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
) -> Result<HookContext, HookFailure> {
    let edits = edits
        .iter()
        .map(|edit| {
            let precondition = preconditions
                .iter()
                .find(|precondition| precondition.pos == edit.pos)?;
            Some(BuildEdit::new(
                edit.pos.x,
                edit.pos.y,
                edit.pos.z,
                precondition.expected_state.0,
                edit.new_state.0,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(HookFailure::Invalid)?;
    BuildContext::try_new(actor, BUILD_DIMENSION, edits)
        .map(HookContext::Build)
        .map_err(HookFailure::from)
}

/// The actor a player's own build is attributed to.
///
/// The identity and the session are both read now, before the question is asked:
/// the session the edit was planned on is the session the chain is asked about,
/// not whatever session the identity holds by the time the answer arrives.
pub(in crate::play) fn player_actor(
    sessions: &SessionRegistry,
    session: SessionId,
) -> Result<HookActor, HookFailure> {
    let uuid = sessions
        .player_uuid(session)
        .ok_or(HookFailure::Unavailable)?;
    HookPlayer::try_new(uuid, session)
        .map(HookActor::Player)
        .map_err(HookFailure::from)
}

/// Ask the registered chain about one frozen batch of edits.
///
/// `Ok(None)` is the direct path: no boundary is installed, or no handler is
/// registered for `before-build`, so the caller commits exactly as it did before
/// the hook contract existed. `Ok(Some(approval))` is a question that was really
/// asked and answered, and the approval is what the commit spends.
pub(in crate::play) async fn request_build_hook(
    boundary: Option<&ScriptBoundary>,
    actor: HookActor,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
) -> Result<Option<Approval>, HookFailure> {
    let Some(boundary) = boundary else {
        return Ok(None);
    };
    if !boundary.has_precommit_hooks(HookKind::Build) {
        return Ok(None);
    }
    let context = build_context(actor, edits, preconditions)?;
    let pending = boundary.begin_precommit(context)?;
    pending.resolve().await.map(Some)
}

/// Spend the one ticket a build was admitted under, at the commit.
///
/// A missing approval is the direct path and commits. A decision that is not a
/// live `Keep` refuses: cancellation is a chain's decision and never an authority
/// to commit, an expired or already spent ticket refuses, and a replacement amount
/// has no meaning for a build.
pub(in crate::play) fn refuse_build_approval(
    approval: &Option<Approval>,
) -> Result<(), HookFailure> {
    let Some(approval) = approval else {
        return Ok(());
    };
    let mut approval = approval.clone();
    match approval.consume()? {
        HookDecision::Keep => Ok(()),
        HookDecision::Cancel => Err(HookFailure::Cancelled),
        HookDecision::Replace(_) | _ => Err(HookFailure::Invalid),
    }
}

/// Hand one pending pre-commit question to a resolver task and wake the owner
/// with the command it produced.
///
/// The owner never awaits a plugin: the question is resolved off the owner's
/// turn while it keeps processing other commands.  The original response follows
/// the resume through bounded queue admission; if admission fails, that caller
/// receives the terminal queue failure.  The approval remains inside the resume
/// command until that admission completes, preserving the boundary's ticket
/// capacity rather than dropping it at task creation.
pub(in crate::play) fn spawn_precommit_resume<F>(
    handle: SimulationHandle,
    pending: PendingDecision,
    session_fence: Option<SessionId>,
    response: Option<super::queue::SimulationResponseSender>,
    resume: F,
) where
    F: FnOnce(Result<Approval, HookFailure>) -> SimulationCommand + Send + 'static,
{
    tokio::spawn(async move {
        let decision = pending.resolve().await;
        let command = resume(decision);
        handle
            .enqueue_precommit_resume_wait(session_fence, command, response)
            .await;
    });
}
