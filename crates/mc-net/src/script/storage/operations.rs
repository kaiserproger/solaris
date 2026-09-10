use mc_script::{
    ScriptEvent, ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome,
    ScriptOperationPayload, ScriptOperationRequest, ScriptOperationState, ScriptStorageChange,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    DurableStorageBatchMutation, PluginStorage, PluginStorageMutationError,
    PluginStorageStartError, ScriptStoragePrepareOutcome, frame, validate_delete_fields,
};

pub(super) const OP_SNAPSHOT_OPERATION: u8 = 11;
pub(super) const OP_OPERATION_DELIVERED: u8 = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableOperationReceipt {
    pub(super) plugin_id: String,
    pub(super) operation_id: String,
    pub(super) request_id: String,
    pub(super) fingerprint: [u8; 32],
    pub(super) revision: u64,
    pub(super) outcome: ScriptOperationOutcome,
    pub(super) delivered: bool,
}

impl DurableOperationReceipt {
    pub(super) fn event(&self) -> Result<ScriptEvent, mc_script::ScriptDtoError> {
        ScriptEvent::operation_result(
            &self.plugin_id,
            &self.request_id,
            Some(&self.operation_id),
            self.outcome.clone(),
        )
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        validate_delete_fields(&self.plugin_id, "operation")?;
        ScriptOperationRequest::try_new(
            &self.request_id,
            ScriptOperation::Status {
                operation_id: self.operation_id.clone(),
            },
        )
        .map_err(|_| PluginStorageStartError::Malformed("operation identity"))?;
        self.outcome
            .validate()
            .map_err(|_| PluginStorageStartError::Malformed("operation outcome"))?;
        if self.revision == 0
            || self.revision > mc_script::MAX_SCRIPT_WORLD_TIME
            || self.outcome.revision() != Some(self.revision)
            || self.outcome.state() != ScriptOperationState::Committed
        {
            return Err(PluginStorageStartError::Malformed("operation revision"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationDeliveryAck {
    plugin_id: String,
    operation_id: String,
    revision: u64,
}

pub(super) enum OperationExecution {
    Reply(ScriptOperationOutcome),
    Durable(DurableOperationReceipt),
}

impl PluginStorage {
    pub(super) fn execute_operation(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<OperationExecution, PluginStorageMutationError> {
        match request.operation() {
            ScriptOperation::Status { operation_id } => Ok(OperationExecution::Reply(
                self.operation_results
                    .get(&(plugin_id.to_owned(), operation_id.clone()))
                    .map_or_else(
                        || ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                        |receipt| receipt.outcome.clone(),
                    ),
            )),
            ScriptOperation::StorageScan {
                prefix,
                cursor,
                limit,
            } => Ok(OperationExecution::Reply(self.scans.scan(
                plugin_id,
                self.revision,
                &self.records,
                prefix,
                cursor.as_deref(),
                *limit,
            ))),
            ScriptOperation::StorageBatch {
                operation_id,
                mutations,
            } => {
                let mut fingerprint = Sha256::new();
                serde_json::to_writer(&mut fingerprint, request.operation()).map_err(|error| {
                    PluginStorageMutationError::Io(std::io::Error::other(error))
                })?;
                let fingerprint: [u8; 32] = fingerprint.finalize().into();
                let identity = (plugin_id.to_owned(), operation_id.clone());
                if let Some(receipt) = self.operation_results.get(&identity) {
                    return Ok(if receipt.fingerprint == fingerprint {
                        OperationExecution::Durable(receipt.clone())
                    } else {
                        OperationExecution::Reply(ScriptOperationOutcome::rejected(
                            ScriptOperationFailure::OperationConflict,
                        ))
                    });
                }
                let mut batch = match self.prepare_batch(plugin_id, mutations, None)? {
                    ScriptStoragePrepareOutcome::Prepared(batch) => batch,
                    ScriptStoragePrepareOutcome::Rejected => {
                        return Ok(OperationExecution::Reply(ScriptOperationOutcome::rejected(
                            ScriptOperationFailure::StaleRevision,
                        )));
                    }
                };
                let changes = batch
                    .mutations
                    .iter()
                    .map(|mutation| {
                        ScriptStorageChange::new(
                            mutation.key().to_owned(),
                            matches!(mutation, DurableStorageBatchMutation::Delete { .. }),
                        )
                    })
                    .collect();
                let outcome = ScriptOperationOutcome::committed(
                    batch.transaction_id,
                    ScriptOperationPayload::StorageBatch { changes },
                )
                .map_err(|_| PluginStorageMutationError::RevisionOverflow)?;
                let receipt = DurableOperationReceipt {
                    plugin_id: plugin_id.to_owned(),
                    operation_id: operation_id.clone(),
                    request_id: request.request_id().to_owned(),
                    fingerprint,
                    revision: batch.transaction_id,
                    outcome,
                    delivered: false,
                };
                batch.operation = Some(receipt.clone());
                if let Err(error) = self.commit_batch(batch) {
                    if matches!(error, PluginStorageMutationError::DurabilityUnknown(_)) {
                        self.unknown_operation = Some(receipt);
                    }
                    return Err(error);
                }
                Ok(OperationExecution::Durable(receipt))
            }
            _ => Ok(OperationExecution::Reply(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ))),
        }
    }

    pub(super) fn acknowledge_operation(
        &mut self,
        receipt: &DurableOperationReceipt,
    ) -> Result<(), PluginStorageMutationError> {
        let key = (receipt.plugin_id.clone(), receipt.operation_id.clone());
        let Some(current) = self.operation_results.get(&key) else {
            return Err(PluginStorageMutationError::RequestIdentityConflict);
        };
        if current.delivered {
            return Ok(());
        }
        let ack = OperationDeliveryAck {
            plugin_id: receipt.plugin_id.clone(),
            operation_id: receipt.operation_id.clone(),
            revision: receipt.revision,
        };
        let mut payload = vec![OP_OPERATION_DELIVERED];
        payload.extend_from_slice(
            &serde_json::to_vec(&ack)
                .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?,
        );
        let frame = frame(&payload);
        self.compact_before_append_if_needed(frame.len())?;
        self.append_frame(&frame, true)?;
        self.install_operation_ack(ack)
            .expect("checked operation acknowledgement remains current");
        Ok(())
    }

    pub(super) fn install_operation_ack(
        &mut self,
        ack: OperationDeliveryAck,
    ) -> Result<(), PluginStorageStartError> {
        let Some(receipt) = self
            .operation_results
            .get_mut(&(ack.plugin_id, ack.operation_id))
        else {
            return Err(PluginStorageStartError::Malformed(
                "unknown operation acknowledgement",
            ));
        };
        if receipt.delivered || receipt.revision != ack.revision {
            return Err(PluginStorageStartError::Malformed(
                "stale operation acknowledgement",
            ));
        }
        receipt.delivered = true;
        Ok(())
    }
}
