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
    ScriptAxisAlignedZone, ScriptCommand, ScriptDtoError, ScriptOnlinePlayersRequest,
    ScriptOperation, ScriptOperationRequest, ScriptPlayerId, ScriptPlayerTeleportRequest,
    ScriptPluginStorageCompareAndSwapRequest, ScriptPluginStorageGetRequest, ScriptPosition,
    ScriptStorageMutation, ScriptZoneProtection,
};

use crate::bindings::solaris::plugin::commands::{Command, MessageTarget, StorageMutation};
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
        // A removal names one zone id and nothing else. The DTO carries the id as
        // the id it is - the Lua API's own removal is exactly this - and the
        // server's own command validator is what refuses an id the contract does not
        // admit, exactly as it does for the id inside a box.
        Command::RemoveZone(remove) => Ok(ScriptCommand::RemoveZone {
            zone_id: remove.zone,
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
