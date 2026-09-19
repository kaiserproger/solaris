//! The Loader-bound client presentation events of `events.wit`, mapped from the
//! server's own already-decided observations.
//!
//! Every variant here is built from a `ScriptEventKind` the server's Loader
//! owners publish *after* they decided something: a key-driven view request they
//! routed to its declared owner, a view action they admitted, the outcome of an
//! open they performed, or the result of a custom-block item grant they
//! committed. This module translates those snapshots into the contract's records
//! and adds nothing: it reads no Loader manifest, resolves no session and
//! invents no field.
//!
//! Three rules of the boundary are visible in the mapping and are the reason
//! this is a translation rather than a copy:
//!
//! - The session is the runtime player id the event was built with, never a
//!   lookup of the identity's current connection. The server keeps no
//!   session -> identity map for Loader events, so a session a reconnect replaced
//!   is still reported as the one the request happened on - and a plugin that
//!   acts on a later session would be acting on a connection that never asked.
//! - The outcome is one of the two shapes the owner decided, never both and
//!   never neither: `opened` carries the instance and revision the owner created,
//!   `refused` the owner's own reason, and `granted`/`refused` the inventory
//!   authority's one bit plus its reason.
//! - A reason or a value this contract does not name yet - the server's enums are
//!   `non_exhaustive` - answers `None`. The caller drops such an event rather than
//!   reporting it as one of the values this contract does carry: telling a plugin
//!   "refused" for a reason the owner never decided would be telling it something
//!   no owner said.

use mc_script::{
    ScriptClientViewFailure, ScriptClientViewFieldValue, ScriptClientViewRequestKind,
    ScriptEventKind, ScriptLoaderItemGrantFailure, ScriptPlayerId,
};

use crate::bindings::exports::solaris::plugin::events::{
    ClientViewOpened, ClientViewOutcome, Event, LoaderItemGrantFailure, LoaderItemGrantOutcome,
    LoaderItemGrantResult, LoaderViewAction, LoaderViewRequest, ViewFailure, ViewOpened,
    ViewRequestKind,
};
use crate::bindings::solaris::plugin::client_presentation::{ViewField, ViewFieldValue};

/// The contract's event for one decided Loader observation, or `None` when this
/// contract carries no observation of that kind.
///
/// Crate-visible on purpose: this is the host's own mapping behind the one event
/// fallback in `host.rs`, not a public API a consumer outside the host may call.
pub(crate) fn map_event(kind: &ScriptEventKind) -> Option<Event> {
    match kind {
        // One key-driven request the server already routed to this plugin as the
        // declared owner of that view kind. The kind is the owner's own closed
        // set; one it cannot name yet is dropped rather than reported as another.
        ScriptEventKind::LoaderViewRequest {
            player_id,
            request_kind,
        } => Some(Event::LoaderViewRequest(LoaderViewRequest {
            session: session_of(*player_id),
            request_kind: view_request_kind(*request_kind)?,
        })),
        // One action the owner admitted: the instance, revision, action,
        // sequence, fields and selection token are all the owner's own validated
        // snapshot, carried through unchanged.
        ScriptEventKind::LoaderViewAction {
            player_id,
            view_instance_id,
            view_revision,
            action_id,
            action_sequence,
            fields,
            selection_token,
        } => Some(Event::LoaderViewAction(LoaderViewAction {
            session: session_of(*player_id),
            view_instance_id: view_instance_id.clone(),
            view_revision: *view_revision,
            action_id: action_id.clone(),
            action_sequence: *action_sequence,
            fields: fields.iter().map(view_field).collect::<Option<Vec<_>>>()?,
            selection_token: selection_token.clone(),
        })),
        // The outcome of one open this plugin issued: the instance and revision
        // it may present against, or the owner's refusal.
        ScriptEventKind::ClientViewOpened {
            request_id,
            player_id,
            view_instance_id,
            revision,
            failure,
        } => Some(Event::ClientViewOpened(ClientViewOpened {
            request: request_id.clone(),
            session: session_of(*player_id),
            outcome: client_view_outcome(view_instance_id.as_deref(), *revision, *failure)?,
        })),
        // One grant the inventory authority decided. The block and the count are
        // echoed for correlation, and the reason a refusal carries is the
        // authority's own.
        ScriptEventKind::LoaderItemGrantResult {
            request_id,
            player_id,
            block_id,
            count,
            failure,
        } => {
            let outcome = match failure {
                Some(failure) => LoaderItemGrantOutcome::Refused(item_grant_failure(*failure)?),
                None => LoaderItemGrantOutcome::Granted,
            };
            Some(Event::LoaderItemGrantResult(LoaderItemGrantResult {
                request: request_id.clone(),
                session: session_of(*player_id),
                block: block_id.clone(),
                count: *count,
                outcome,
            }))
        }
        _ => None,
    }
}

/// The connection one Loader event was produced on.
///
/// The runtime player id *is* the live connection's id on this boundary, exactly
/// as it is for the world observations the Loader and gameplay owners publish,
/// and it is taken from the event itself rather than resolved from a stable
/// identity - the server keeps no such map here, and a reconnect must not move a
/// report onto a connection that never asked.
fn session_of(player_id: ScriptPlayerId) -> u64 {
    player_id.value()
}

/// The client's key-driven view kind, in the contract's own vocabulary.
fn view_request_kind(kind: ScriptClientViewRequestKind) -> Option<ViewRequestKind> {
    match kind {
        ScriptClientViewRequestKind::Settlement => Some(ViewRequestKind::Settlement),
        ScriptClientViewRequestKind::Army => Some(ViewRequestKind::Army),
        // The server's enum is `non_exhaustive`: a kind this contract does not
        // name yet has no member here, and the event is dropped rather than
        // reported as one of the two it does.
        _ => None,
    }
}

/// The owner's own reason a view lifecycle request was refused.
fn view_failure(failure: ScriptClientViewFailure) -> Option<ViewFailure> {
    match failure {
        ScriptClientViewFailure::Refused => Some(ViewFailure::Refused),
        ScriptClientViewFailure::PlayerUnavailable => Some(ViewFailure::PlayerUnavailable),
        ScriptClientViewFailure::UnknownInstance => Some(ViewFailure::UnknownInstance),
        ScriptClientViewFailure::StaleRevision => Some(ViewFailure::StaleRevision),
        ScriptClientViewFailure::TooLarge => Some(ViewFailure::TooLarge),
        _ => None,
    }
}

/// The inventory authority's own reason a grant did not commit.
fn item_grant_failure(failure: ScriptLoaderItemGrantFailure) -> Option<LoaderItemGrantFailure> {
    match failure {
        ScriptLoaderItemGrantFailure::LoaderUnavailable => {
            Some(LoaderItemGrantFailure::LoaderUnavailable)
        }
        ScriptLoaderItemGrantFailure::NotOwned => Some(LoaderItemGrantFailure::NotOwned),
        ScriptLoaderItemGrantFailure::PlayerUnavailable => {
            Some(LoaderItemGrantFailure::PlayerUnavailable)
        }
        ScriptLoaderItemGrantFailure::InventoryFull => Some(LoaderItemGrantFailure::InventoryFull),
        ScriptLoaderItemGrantFailure::RuntimeUnavailable => {
            Some(LoaderItemGrantFailure::RuntimeUnavailable)
        }
        ScriptLoaderItemGrantFailure::Rejected => Some(LoaderItemGrantFailure::Rejected),
        _ => None,
    }
}

/// One typed field the owner accepted, in the contract's own sum shape.
fn view_field(field: &mc_script::ScriptClientViewField) -> Option<ViewField> {
    let value = match field.value() {
        ScriptClientViewFieldValue::Number(number) => ViewFieldValue::Number(*number),
        ScriptClientViewFieldValue::Text(text) => ViewFieldValue::Text(text.clone()),
        ScriptClientViewFieldValue::Selected(selected) => {
            ViewFieldValue::Selected(selected.clone())
        }
        // The server's enum is `non_exhaustive`; a value shape this contract does
        // not carry drops the whole action rather than reporting the field with a
        // value the owner never sent.
        _ => return None,
    };
    Some(ViewField {
        id: field.id().to_owned(),
        value,
    })
}

/// The two shapes one open ended in, exactly as the owner decided them.
fn client_view_outcome(
    view_instance_id: Option<&str>,
    revision: Option<u64>,
    failure: Option<ScriptClientViewFailure>,
) -> Option<ClientViewOutcome> {
    match (view_instance_id, revision, failure) {
        (Some(view_instance_id), Some(revision), None) => {
            Some(ClientViewOutcome::Opened(ViewOpened {
                view_instance_id: view_instance_id.to_owned(),
                revision,
            }))
        }
        (None, None, Some(failure)) => Some(ClientViewOutcome::Refused(view_failure(failure)?)),
        // The owner reports an instance and a revision together or a refusal with
        // neither; a result that is neither shape is one this contract cannot
        // carry truthfully, so it is dropped rather than guessed at.
        _ => None,
    }
}

#[cfg(test)]
#[path = "client_presentation_tests.rs"]
mod client_presentation_tests;
