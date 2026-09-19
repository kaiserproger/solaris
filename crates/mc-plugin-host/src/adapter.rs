//! Conversion from the contract's typed commands to the server's own DTOs.
//!
//! A guest's command is a request about *this* server: it names a stable player
//! identity or a live session, never a runtime id. Turning that into the DTO the
//! admission boundary accepts is a game-side lookup for exactly one case - a
//! stable identity has to be resolved to the session that identity holds right
//! now - so the host asks for it through [`PlayerSessions`] instead of guessing.
//! Everything else is a total, checked conversion: an unknown stub cannot slip
//! through as a `ScriptCommand` that no owner understands.

use std::num::NonZeroUsize;

use mc_script::{
    CommandBatch as ScriptCommandBatch, CommandBatchError, CommandCapabilities,
    ScriptAxisAlignedZone, ScriptClientSound, ScriptClientViewAction, ScriptClientViewField,
    ScriptClientViewFormation, ScriptClientViewMarker, ScriptClientViewModel, ScriptClientViewOpen,
    ScriptClientViewPresent, ScriptClientViewResourceEntry, ScriptClientViewRow,
    ScriptClientViewTab, ScriptCommand, ScriptDtoError, ScriptInventoryMenu,
    ScriptInventoryMenuItem, ScriptInventoryMenuSlot, ScriptInventoryResourceDelta,
    ScriptInventoryStorageTransaction, ScriptLoaderItemGrantRequest, ScriptOnlinePlayersRequest,
    ScriptOperation, ScriptOperationRequest, ScriptPlayerId, ScriptPlayerTeleportRequest,
    ScriptPluginStorageCompareAndSwapRequest, ScriptPluginStorageGetRequest, ScriptPosition,
    ScriptStorageMutation, ScriptZoneProtection,
};

use crate::bindings::solaris::plugin::client_presentation::{
    ClientCommand, ViewField, ViewFieldValue, ViewFormation, ViewModel,
};
use crate::bindings::solaris::plugin::commands::{Command, MessageTarget, StorageMutation};
use crate::bindings::solaris::plugin::domain_operations::DomainOperation;
use crate::bindings::solaris::plugin::types::Position;

/// Where a plugin's stable player identity is looked up.
///
/// The host never invents a session id and never addresses a player by a stale
/// one: a plugin that names a player who is not connected is answered by this
/// lookup failing, and the command is refused rather than delivered to whoever
/// holds that runtime id now.
pub trait PlayerSessions {
    /// The session currently held by `player`, when that player is connected.
    fn session_of(&self, player: &str) -> Option<u64>;
}

/// The [`PlayerSessions`] of a caller that owns no sessions - a deployment check
/// or a unit test - which resolves nobody and is answered accordingly.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSessions;

impl PlayerSessions for NoSessions {
    fn session_of(&self, _player: &str) -> Option<u64> {
        None
    }
}

/// Why one command of a batch could not be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The batch names a player who holds no session right now.
    #[error("the batch addresses a player who is not connected")]
    UnknownPlayer,
    /// The converted batch did not fit the server's own batch bound.
    #[error("the batch exceeds the server's command batch bound")]
    BatchRejected,
    /// One command violated a bound the contract declares, so the server's own
    /// DTO refused it. The plugin answered something malformed.
    #[error("the command is not a valid {field} for the server")]
    InvalidCommand { field: &'static str },
    /// A command needs a capability the package never declared, so it was refused
    /// before anything was written anywhere.
    #[error("the command needs the {capability} capability, which the package does not declare")]
    PermissionDenied { capability: &'static str },
    /// The batch claimed its own provenance, which only the host may attach.
    #[error("the command claimed host provenance, which only the host attaches")]
    ProvenanceRejected,
}

impl From<ScriptDtoError> for AdapterError {
    fn from(error: ScriptDtoError) -> Self {
        let field = match error {
            ScriptDtoError::EmptyValue { field }
            | ScriptDtoError::ValueTooLong { field, .. }
            | ScriptDtoError::InvalidId { field, .. }
            | ScriptDtoError::InvalidResourceId { field, .. }
            | ScriptDtoError::InconsistentResult { field }
            | ScriptDtoError::TooManyEntries { field, .. }
            | ScriptDtoError::DuplicateId { field, .. } => field,
            _ => "command",
        };
        Self::InvalidCommand { field }
    }
}

/// Convert one staged batch into the server's own command batch.
///
/// The whole batch is converted before any of it is returned, so a caller never
/// submits half a callback's answer: the first unconvertible command fails the
/// batch and nothing of it is applied.
pub fn to_script_batch(
    batch: crate::CommandBatch,
    limit: NonZeroUsize,
    sessions: &(impl PlayerSessions + ?Sized),
    capabilities: &CommandCapabilities,
) -> Result<ScriptCommandBatch, AdapterError> {
    let mut converted = ScriptCommandBatch::new(limit);
    for command in batch.into_commands() {
        let command = convert(command, sessions)?;
        // Converted against the package's own grants, so a command the manifest
        // never declared fails here as the plugin's own bug; the boundary checks
        // the same grants again when the batch is submitted.
        converted
            .try_push_authorized(command, capabilities)
            .map_err(|error| match error {
                // Keep the server's own reason: an operator reading "the plugin
                // lost its routes" must learn whether the manifest was short, the
                // batch was too large, or the command was malformed.
                CommandBatchError::PermissionDenied { capability } => {
                    AdapterError::PermissionDenied {
                        capability: capability.code(),
                    }
                }
                CommandBatchError::InvalidCommand { error } => error.into(),
                CommandBatchError::ProvenanceRejected => AdapterError::ProvenanceRejected,
                CommandBatchError::Full { .. } | CommandBatchError::AdmissionUnavailable => {
                    AdapterError::BatchRejected
                }
                // The enum is `non_exhaustive`: a refusal this host does not name
                // yet is the batch not reaching the server, not a licence to drop
                // it silently.
                _ => AdapterError::BatchRejected,
            })?;
    }
    Ok(converted)
}

/// Convert one contract command.
fn convert(
    command: Command,
    sessions: &(impl PlayerSessions + ?Sized),
) -> Result<ScriptCommand, AdapterError> {
    match command {
        Command::SendMessage(message) => {
            let player_id = match &message.target {
                MessageTarget::Session(session) => ScriptPlayerId::new(*session),
                // A stable identity is resolved now, not when the guest ran: the
                // message reaches the session this player holds at admission.
                MessageTarget::Player(player) => {
                    let session = sessions
                        .session_of(player)
                        .ok_or(AdapterError::UnknownPlayer)?;
                    ScriptPlayerId::new(session)
                }
            };
            Ok(ScriptCommand::SendChatMessage {
                player_id,
                message: message.text,
            })
        }
        Command::Broadcast(message) => Ok(ScriptCommand::BroadcastChatMessage { message }),
        Command::DisconnectPlayer(disconnect) => Ok(ScriptCommand::DisconnectPlayer {
            player_id: ScriptPlayerId::new(disconnect.session),
            reason: disconnect.reason,
        }),
        // Storage names no player and needs no lookup: the DTO bounds the key,
        // the value and the correlation id, and the host binds the owner.
        Command::StorageGet(get) => Ok(ScriptCommand::PluginStorageGet {
            request: ScriptPluginStorageGetRequest::try_new(get.request, get.key)?,
        }),
        Command::StorageCas(cas) => Ok(ScriptCommand::PluginStorageCompareAndSwap {
            request: ScriptPluginStorageCompareAndSwapRequest::try_new(
                cas.request,
                cas.key,
                cas.expected_version,
                cas.value,
            )?,
        }),
        // The query carries no player and no authority beyond reading who is
        // connected: the server bounds the limit and answers with its own
        // snapshot, which the host renames rather than re-derives.
        Command::ListOnlinePlayers(query) => Ok(ScriptCommand::ListOnlinePlayers {
            request: ScriptOnlinePlayersRequest::try_new(
                query.request,
                usize::try_from(query.limit).unwrap_or(usize::MAX),
            )?,
        }),
        // A teleport is an effect on one live connection, so the guest names the
        // session it saw and the host looks nothing up: the server's own router
        // refuses a session that is gone instead of moving whoever holds that
        // runtime id now. The coordinates are the guest's own, bounded by the
        // server's position validator - a value it cannot accept is the plugin's
        // malformed answer, never a clamped or defaulted one.
        Command::TeleportPlayer(teleport) => Ok(ScriptCommand::TeleportPlayer {
            request: ScriptPlayerTeleportRequest::try_new(
                teleport.request,
                ScriptPlayerId::new(teleport.session),
                ScriptPosition::try_new(
                    teleport.position.x,
                    teleport.position.y,
                    teleport.position.z,
                )
                .ok_or(AdapterError::InvalidCommand {
                    field: "player teleport position",
                })?,
            )?,
        }),
        // A batch is converted mutation by mutation against nothing but the
        // server's own DTO: no revision, key, value or mutation is invented, added
        // or reordered by the host beyond the canonicalization the DTO itself
        // performs, so a batch that reaches the owner is the plugin's own. The
        // DTO's validator is what refuses one past a bound the contract documents
        // - too many mutations, a key repeated inside the batch, a key or value
        // past its byte bound - and it reports the field it refused.
        Command::StorageBatchCas(batch) => Ok(ScriptCommand::Operation {
            request: ScriptOperationRequest::try_new(
                batch.request,
                ScriptOperation::StorageBatch {
                    operation_id: batch.operation_id,
                    mutations: batch.mutations.into_iter().map(storage_mutation).collect(),
                },
            )?,
        }),
        // A probe carries one durable id and changes nothing: the server reads the
        // outcome it recorded under that id, so the host has nothing to look up and
        // nothing to add.
        Command::OperationStatus(status) => Ok(ScriptCommand::Operation {
            request: ScriptOperationRequest::try_new(
                status.request,
                ScriptOperation::Status {
                    operation_id: status.operation_id,
                },
            )?,
        }),
        Command::Operation(request) => {
            let operation = match request.operation {
                DomainOperation::Settlement(value) => ScriptOperation::Settlement {
                    operation: crate::domain_settlements::decode(value)?,
                },
                DomainOperation::Resident(value) => ScriptOperation::Resident {
                    operation: crate::domain_residents::decode(value)?,
                },
                DomainOperation::ResidentOrder(value) => ScriptOperation::ResidentOrder {
                    operation: crate::domain_residents::decode_order(value)?,
                },
                DomainOperation::Inventory(value) => ScriptOperation::Inventory {
                    operation: crate::domain_inventories::decode(value)?,
                },
            };
            Ok(ScriptCommand::Operation {
                request: ScriptOperationRequest::try_new(request.request, operation)?,
            })
        }
        // A zone is a box of finite coordinates and nothing else, and the DTO the
        // owner applies is built by the validator the owner itself re-validates the
        // box with: the host invents no corner, no dimension and no id. A record the
        // validator cannot accept - an id or a dimension the contract does not
        // admit, a coordinate outside the server's position bound, a box whose
        // minimum corner is past its maximum - is the plugin's own malformed answer
        // and fails the batch; it is never clamped to the bound and never replaced
        // with a box the plugin did not ask for.
        Command::UpsertZone(zone) => Ok(ScriptCommand::UpsertZone {
            zone: ScriptAxisAlignedZone::try_new(
                zone.zone,
                zone.dimension,
                position(zone.minimum, "zone minimum")?,
                position(zone.maximum, "zone maximum")?,
            )?,
        }),
        // The protected variant is the same box with one protection record: the one
        // actor the owner admits inside it. The uuid is the guest's own, normalized
        // by the server's own validator, and a uuid it cannot accept is refused
        // rather than replaced with an actor the plugin never named.
        Command::UpsertProtectedZone(zone) => Ok(ScriptCommand::UpsertZone {
            zone: ScriptAxisAlignedZone::try_new_with_protection(
                zone.zone,
                zone.dimension,
                position(zone.minimum, "zone minimum")?,
                position(zone.maximum, "zone maximum")?,
                Some(ScriptZoneProtection::try_actor_or_operator(
                    zone.allowed_actor_uuid,
                )?),
            )?,
        }),
        // A removal names one zone id and nothing else. The DTO carries that id
        // unchanged, and the server command validator refuses an id the contract
        // does not admit, exactly as it does for the id inside a box.
        Command::RemoveZone(remove) => Ok(ScriptCommand::RemoveZone {
            zone_id: remove.zone,
        }),
        // The transaction's two sides are the guest's own data, so this is the
        // DTO's own constructor and not a policy decision: every delta and every
        // mutation is re-validated by `ScriptInventoryStorageTransaction::try_new`,
        // which refuses an empty side, a repeat, an amount or a key/value the
        // contract does not admit. The session is the DTO's own player id, as for
        // `teleport-player`, so the server's inventory owner refuses a transaction
        // for a connection that has ended instead of reaching whoever holds that
        // runtime id now.
        Command::InventoryStorageTransaction(transaction) => {
            let inventory = transaction
                .inventory
                .into_iter()
                .map(|delta| ScriptInventoryResourceDelta::try_new(delta.resource, delta.delta))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ScriptCommand::InventoryStorageTransaction {
                transaction: ScriptInventoryStorageTransaction::try_new(
                    transaction.request,
                    ScriptPlayerId::new(transaction.session),
                    inventory,
                    transaction
                        .storage
                        .into_iter()
                        .map(storage_mutation)
                        .collect(),
                )?,
            })
        }
        // A menu names one live connection, as a teleport does, so the session the
        // guest saw is the DTO's own player id: the server's menu owner refuses a
        // menu for a connection that has ended instead of showing it to whoever
        // holds that runtime id now. Every slot is re-validated by the server's own
        // constructors - the menu id, the title, the slot indexes, the resources,
        // the counts and the labels - so a definition the contract does not admit is
        // the plugin's own malformed answer and fails the batch: the host invents no
        // slot and clamps no value rather than converting something the plugin did
        // not ask for. The package's own `inventory_menus` grant is what admits the
        // converted command, as it is for every other command, and no answer event
        // follows it - opening a menu is a request the plugin hears about only
        // through a click on one of its slots.
        Command::OpenInventoryMenu(open) => {
            let slots = open
                .menu
                .slots
                .into_iter()
                .map(|slot| {
                    Ok(ScriptInventoryMenuSlot::new(
                        slot.slot,
                        ScriptInventoryMenuItem::try_new(slot.resource, slot.count, slot.label)?,
                    ))
                })
                .collect::<Result<Vec<_>, ScriptDtoError>>()?;
            Ok(ScriptCommand::OpenInventoryMenu {
                player_id: ScriptPlayerId::new(open.session),
                menu: ScriptInventoryMenu::try_new(open.menu.id, open.menu.title, slots)?,
            })
        }
        // A close carries the session and the menu id and nothing else, so the DTO
        // holds the guest's own two values: the server's own command validator
        // refuses an id the contract does not admit, and the menu owner decides
        // whether that session holds that menu - the host neither invents a menu id
        // nor retargets the close to whatever menu the connection has open now.
        Command::CloseInventoryMenu(close) => Ok(ScriptCommand::CloseInventoryMenu {
            player_id: ScriptPlayerId::new(close.session),
            menu_id: close.menu,
        }),
        // Loader-bound client presentation. Every member names one live session
        // rather than a stable identity: a view, a sound and a block-item grant are
        // effects on one acknowledged Loader connection, so the server's own
        // owners refuse a request for a session that is gone instead of delivering
        // it to whoever holds that runtime id now. The host looks nothing up and
        // adds nothing - the session is the DTO's own player id and the model is
        // rebuilt by the DTO's own constructors - and the Loader gate stays where
        // the native path keeps it: the manifest's bundled content and its
        // permission, checked inside the owner that applies the request, which is
        // why this command carries no capability here, exactly as it carries none
        // natively.
        Command::ClientPresentation(request) => match request {
            ClientCommand::OpenClientView(open) => Ok(ScriptCommand::OpenClientView {
                request: ScriptClientViewOpen::try_new(
                    &open.request,
                    ScriptPlayerId::new(open.session),
                    &open.owned_view_id,
                    view_model(open.model)?,
                )?,
            }),
            ClientCommand::PresentClientView(present) => Ok(ScriptCommand::PresentClientView {
                request: ScriptClientViewPresent::try_new(
                    ScriptPlayerId::new(present.session),
                    &present.view_instance_id,
                    present.expected_revision,
                    view_model(present.model)?,
                )?,
            }),
            ClientCommand::CloseClientView(close) => Ok(ScriptCommand::CloseClientView {
                player_id: ScriptPlayerId::new(close.session),
                view_instance_id: close.view_instance_id,
            }),
            ClientCommand::PlayClientSound(play) => Ok(ScriptCommand::ClientSound {
                player_id: ScriptPlayerId::new(play.session),
                sound: ScriptClientSound::play(
                    &play.sound_id,
                    play.volume,
                    play.pitch,
                    play.position
                        .map(|at| position(at, "client sound position"))
                        .transpose()?,
                )?,
            }),
            ClientCommand::StopClientSound(stop) => Ok(ScriptCommand::ClientSound {
                player_id: ScriptPlayerId::new(stop.session),
                sound: ScriptClientSound::stop(&stop.sound_id)?,
            }),
            ClientCommand::GrantLoaderBlockItem(grant) => Ok(ScriptCommand::GrantLoaderBlockItem {
                request: ScriptLoaderItemGrantRequest::try_new(
                    grant.request,
                    ScriptPlayerId::new(grant.session),
                    grant.block,
                    grant.count,
                )?,
            }),
        },
        // A timer is the plugin's own host-side memory, not a server object: the
        // host takes timer commands out of a callback's answer before it converts
        // it, because they are applied to the instance's own schedule and never
        // reach a game owner. One arriving here is refused under the name of what
        // it is rather than converted into a server command that would mean
        // something else - a timer is not a chat line and not a zone.
        Command::ScheduleTimer(_) | Command::CancelTimer(_) => Err(AdapterError::InvalidCommand {
            field: "timer command",
        }),
    }
}

/// One contract position as the server's own.
///
/// The bounds are the server's position validator's, not a copy of them: a
/// coordinate it cannot accept fails the conversion under the name of the corner
/// the plugin got wrong, and the host never clamps one to the edge.
fn position(position: Position, field: &'static str) -> Result<ScriptPosition, AdapterError> {
    ScriptPosition::try_new(position.x, position.y, position.z)
        .ok_or(AdapterError::InvalidCommand { field })
}

/// One contract mutation as the server's own storage mutation.
///
/// The two shapes carry the same fields, so this is a rename and not a
/// translation: the key, the expected revision and the value are the guest's own,
/// and the DTO's validator refuses the ones the contract does not admit.
fn storage_mutation(mutation: StorageMutation) -> ScriptStorageMutation {
    match mutation {
        StorageMutation::Cas(cas) => ScriptStorageMutation::CompareAndSwap {
            key: cas.key,
            expected_version: cas.expected_version,
            value: cas.value,
        },
        StorageMutation::Delete(delete) => ScriptStorageMutation::Delete {
            key: delete.key,
            expected_version: delete.expected_version,
        },
    }
}

/// One contract view model as the server's own DTO.
///
/// Every nested row, field, action, tab, resource entry and marker is rebuilt by
/// the same `mc_script` constructors the native model uses, so a model that
/// reaches the Loader owner is validated at the component boundary: page count,
/// row and per-list element bounds, cell and text byte bounds, id and label byte
/// bounds, finite amounts and radii, duplicate-free ids and selection tokens all
/// refuse the whole batch as the plugin's malformed answer. Nothing is clamped,
/// truncated, defaulted or reordered, and no value is invented for one the
/// plugin omitted.
fn view_model(model: ViewModel) -> Result<ScriptClientViewModel, AdapterError> {
    let rows = model
        .rows
        .into_iter()
        .map(|row| ScriptClientViewRow::try_new(row.cells))
        .collect::<Result<Vec<_>, _>>()?;
    let fields = model
        .fields
        .into_iter()
        .map(|field| {
            let ViewField { id, value } = field;
            match value {
                ViewFieldValue::Number(number) => ScriptClientViewField::try_number(&id, number),
                ViewFieldValue::Text(text) => ScriptClientViewField::try_text(&id, text),
                ViewFieldValue::Selected(selected) => {
                    ScriptClientViewField::try_selected(&id, &selected)
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let actions = model
        .actions
        .into_iter()
        .map(|action| {
            ScriptClientViewAction::try_new(
                &action.action_id,
                action.enabled,
                action.label,
                action.deny_reason,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let tabs = model
        .tabs
        .into_iter()
        .map(|tab| ScriptClientViewTab::try_new(&tab.id, &tab.label))
        .collect::<Result<Vec<_>, _>>()?;
    let resource_entries = model
        .resource_entries
        .into_iter()
        .map(|entry| ScriptClientViewResourceEntry::try_new(&entry.id, entry.have, entry.need))
        .collect::<Result<Vec<_>, _>>()?;
    let markers = model
        .markers
        .into_iter()
        .map(|marker| {
            ScriptClientViewMarker::try_new(
                &marker.marker_id,
                marker.selection_token,
                marker.action_id,
                marker.formation.map(formation),
                marker.radius,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    ScriptClientViewModel::try_new(
        model.page,
        model.page_count,
        rows,
        fields,
        actions,
        tabs,
        resource_entries,
        markers,
        model.reason,
    )
    .map_err(Into::into)
}

/// One contract formation as the server's own. The two vocabularies are the same
/// four values, so this is a rename and the server's own set is what decides
/// which ones exist.
fn formation(formation: ViewFormation) -> ScriptClientViewFormation {
    match formation {
        ViewFormation::Line => ScriptClientViewFormation::Line,
        ViewFormation::Column => ScriptClientViewFormation::Column,
        ViewFormation::Wedge => ScriptClientViewFormation::Wedge,
        ViewFormation::Square => ScriptClientViewFormation::Square,
    }
}
