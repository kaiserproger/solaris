use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    MAX_INVENTORY_STORAGE_MUTATIONS, MAX_PLUGIN_STORAGE_KEY_BYTES, MAX_SCRIPT_ID_BYTES,
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptOwnedInventoryOperation, ScriptStorageMutation,
    validate_bounded_nonempty, validate_bounded_value, validate_plugin_storage_value,
    validate_script_id,
};

pub const MAX_STORAGE_SCAN_PAGE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptOperation {
    StorageBatch {
        operation_id: String,
        mutations: Vec<ScriptStorageMutation>,
    },
    StorageScan {
        prefix: String,
        cursor: Option<String>,
        limit: u8,
    },
    Status {
        operation_id: String,
    },
    Inventory {
        operation: ScriptOwnedInventoryOperation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedOperationRequest")]
pub struct ScriptOperationRequest {
    request_id: String,
    operation: ScriptOperation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedOperationRequest {
    request_id: String,
    operation: ScriptOperation,
}

impl TryFrom<UncheckedOperationRequest> for ScriptOperationRequest {
    type Error = ScriptDtoError;

    fn try_from(value: UncheckedOperationRequest) -> Result<Self, Self::Error> {
        Self::try_new(value.request_id, value.operation)
    }
}

impl ScriptOperationRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        mut operation: ScriptOperation,
    ) -> Result<Self, ScriptDtoError> {
        match &mut operation {
            ScriptOperation::StorageBatch { mutations, .. } => {
                mutations.sort_unstable_by(|left, right| left.key().cmp(right.key()));
            }
            ScriptOperation::Inventory { operation } => operation.canonicalize(),
            _ => {}
        }
        let request = Self {
            request_id: validate_script_id(request_id.as_ref())?,
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn operation(&self) -> &ScriptOperation {
        &self.operation
    }

    pub fn operation_id(&self) -> Option<&str> {
        match &self.operation {
            ScriptOperation::StorageBatch { operation_id, .. }
            | ScriptOperation::Status { operation_id } => Some(operation_id),
            ScriptOperation::StorageScan { .. } => None,
            ScriptOperation::Inventory { operation } => operation.operation_id(),
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_script_id_value(&self.request_id)?;
        match &self.operation {
            ScriptOperation::StorageBatch {
                operation_id,
                mutations,
            } => {
                crate::validate_script_id_value(operation_id)?;
                validate_storage_mutations(mutations)
            }
            ScriptOperation::StorageScan {
                prefix,
                cursor,
                limit,
            } => {
                validate_bounded_value(
                    "storage scan prefix",
                    prefix,
                    MAX_PLUGIN_STORAGE_KEY_BYTES,
                )?;
                if let Some(cursor) = cursor {
                    validate_bounded_nonempty("storage scan cursor", cursor, MAX_SCRIPT_ID_BYTES)?;
                }
                if *limit == 0 || usize::from(*limit) > MAX_STORAGE_SCAN_PAGE {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            ScriptOperation::Status { operation_id } => {
                crate::validate_script_id_value(operation_id)
            }
            ScriptOperation::Inventory { operation } => operation.validate(),
        }
    }
}

pub(crate) fn validate_storage_mutations(
    mutations: &[ScriptStorageMutation],
) -> Result<(), ScriptDtoError> {
    if mutations.is_empty() {
        return Err(ScriptDtoError::EmptyTransaction);
    }
    if mutations.len() > MAX_INVENTORY_STORAGE_MUTATIONS {
        return Err(ScriptDtoError::TooManyEntries {
            field: "storage mutations",
            max: MAX_INVENTORY_STORAGE_MUTATIONS,
        });
    }
    let mut keys = BTreeSet::new();
    for mutation in mutations {
        validate_bounded_nonempty(
            "plugin storage key",
            mutation.key(),
            MAX_PLUGIN_STORAGE_KEY_BYTES,
        )?;
        if !keys.insert(mutation.key()) {
            return Err(ScriptDtoError::DuplicateId {
                field: "plugin storage key",
                actual_bytes: mutation.key().len(),
            });
        }
        let expected_version = match mutation {
            ScriptStorageMutation::CompareAndSwap {
                expected_version,
                value,
                ..
            } => {
                validate_plugin_storage_value(value)?;
                expected_version
            }
            ScriptStorageMutation::Delete {
                expected_version, ..
            } => expected_version,
        };
        if expected_version.is_some_and(|revision| revision > MAX_SCRIPT_WORLD_TIME) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptOperationState {
    Rejected,
    Accepted,
    Running,
    Paused,
    Committed,
    Cancelled,
}

impl ScriptOperationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptOperationFailure {
    InvalidRequest,
    Forbidden,
    StaleRevision,
    NotFound,
    Unloaded,
    Blocked,
    InsufficientItems,
    Capacity,
    Busy,
    RuntimeUnavailable,
    OperationConflict,
    CursorExpired,
}

impl ScriptOperationFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Forbidden => "forbidden",
            Self::StaleRevision => "stale_revision",
            Self::NotFound => "not_found",
            Self::Unloaded => "unloaded",
            Self::Blocked => "blocked",
            Self::InsufficientItems => "insufficient_items",
            Self::Capacity => "capacity",
            Self::Busy => "busy",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::OperationConflict => "operation_conflict",
            Self::CursorExpired => "cursor_expired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStorageEntry {
    pub key: String,
    pub value: String,
    pub revision: u64,
}

impl ScriptStorageEntry {
    #[must_use]
    pub fn new(key: String, value: String, revision: u64) -> Self {
        Self {
            key,
            value,
            revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStorageChange {
    pub key: String,
    pub deleted: bool,
}

impl ScriptStorageChange {
    #[must_use]
    pub fn new(key: String, deleted: bool) -> Self {
        Self { key, deleted }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptOperationPayload {
    None,
    StorageBatch {
        changes: Vec<ScriptStorageChange>,
    },
    StoragePage {
        entries: Vec<ScriptStorageEntry>,
        cursor: Option<String>,
    },
    OwnedInventory {
        result: crate::ScriptOwnedInventoryResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptOperationOutcome {
    state: ScriptOperationState,
    revision: Option<u64>,
    failure: Option<ScriptOperationFailure>,
    payload: ScriptOperationPayload,
}

impl ScriptOperationOutcome {
    pub fn committed(
        revision: u64,
        payload: ScriptOperationPayload,
    ) -> Result<Self, ScriptDtoError> {
        let outcome = Self {
            state: ScriptOperationState::Committed,
            revision: Some(revision),
            failure: None,
            payload,
        };
        outcome.validate()?;
        Ok(outcome)
    }

    pub fn rejected(failure: ScriptOperationFailure) -> Self {
        Self {
            state: ScriptOperationState::Rejected,
            revision: None,
            failure: Some(failure),
            payload: ScriptOperationPayload::None,
        }
    }

    pub const fn state(&self) -> ScriptOperationState {
        self.state
    }

    pub const fn revision(&self) -> Option<u64> {
        self.revision
    }

    pub const fn failure(&self) -> Option<ScriptOperationFailure> {
        self.failure
    }

    pub fn payload(&self) -> &ScriptOperationPayload {
        &self.payload
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self
            .revision
            .is_some_and(|revision| revision > MAX_SCRIPT_WORLD_TIME)
            || (self.state == ScriptOperationState::Rejected) != self.failure.is_some()
            || (self.state != ScriptOperationState::Rejected && self.revision.is_none())
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "operation outcome",
            });
        }
        match &self.payload {
            ScriptOperationPayload::None => Ok(()),
            ScriptOperationPayload::OwnedInventory { result } => result.validate(),
            ScriptOperationPayload::StorageBatch { changes } => {
                if changes.is_empty() || changes.len() > MAX_INVENTORY_STORAGE_MUTATIONS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut keys = BTreeSet::new();
                for change in changes {
                    validate_bounded_nonempty(
                        "plugin storage key",
                        &change.key,
                        MAX_PLUGIN_STORAGE_KEY_BYTES,
                    )?;
                    if !keys.insert(&change.key) {
                        return Err(ScriptDtoError::DuplicateId {
                            field: "plugin storage key",
                            actual_bytes: change.key.len(),
                        });
                    }
                }
                Ok(())
            }
            ScriptOperationPayload::StoragePage { entries, cursor } => {
                if entries.len() > MAX_STORAGE_SCAN_PAGE {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "storage page",
                        max: MAX_STORAGE_SCAN_PAGE,
                    });
                }
                if let Some(cursor) = cursor {
                    validate_bounded_nonempty("storage scan cursor", cursor, MAX_SCRIPT_ID_BYTES)?;
                }
                let mut previous = None;
                for entry in entries {
                    validate_bounded_nonempty(
                        "plugin storage key",
                        &entry.key,
                        MAX_PLUGIN_STORAGE_KEY_BYTES,
                    )?;
                    validate_plugin_storage_value(&entry.value)?;
                    if entry.revision == 0
                        || entry.revision > self.revision.unwrap_or(0)
                        || previous.is_some_and(|key: &str| key >= entry.key.as_str())
                    {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "storage scan entry",
                        });
                    }
                    previous = Some(entry.key.as_str());
                }
                Ok(())
            }
        }
    }
}
