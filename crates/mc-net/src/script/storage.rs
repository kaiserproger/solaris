use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mc_script::{
    AdmittedScriptCommand, ScriptCommand, ScriptEvent, ScriptPluginStorageCompareAndSwapRequest,
    ScriptPluginStorageDeleteRequest, ScriptPluginStorageFailure, ScriptStorageMutation,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::events::{
    TargetedEventDelivery, deliver_required_targeted_event, deliver_targeted_event,
};
use crate::play::{ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare, SessionRegistry};
use crate::server::{ScriptEventSink, ShutdownHandle};

const STORAGE_DIRECTORY: &str = "solaris/plugin-storage-v1";
const JOURNAL_FILE: &str = "journal-v1.bin";
const JOURNAL_TEMP_FILE: &str = "journal-v1.tmp";
const STORAGE_QUEUE_CAPACITY: usize = 256;
const MAX_RECORDS_PER_PLUGIN: usize = 4_096;
const MAX_LIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 8 * 1024;
const MAX_TRANSACTION_FRAME_BYTES: usize = 80 * 1024;
const OP_SNAPSHOT_RESET: u8 = 3;
const OP_SNAPSHOT_SET: u8 = 4;
const OP_DURABLE_CAS: u8 = 5;
const OP_DURABLE_DELETE: u8 = 6;
const OP_RESULT_DELIVERED: u8 = 7;
const OP_SNAPSHOT_RESULT: u8 = 8;
const OP_STORAGE_BATCH: u8 = 9;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginStorageStartError {
    #[error("plugin storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin storage journal is malformed: {0}")]
    Malformed(&'static str),
    #[error("plugin storage journal exceeds 128 MiB")]
    JournalTooLarge,
    #[error("plugin storage live data exceeds configured limits")]
    LiveQuotaExceeded,
}

#[derive(Debug, Error)]
pub(crate) enum PluginStorageMutationError {
    #[error("plugin storage revision overflow")]
    RevisionOverflow,
    #[error("plugin storage quota exceeded")]
    QuotaExceeded,
    #[error("plugin storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin storage synchronization outcome is unknown: {0}")]
    DurabilityUnknown(std::io::Error),
    #[error("plugin storage request identity was reused with different content")]
    RequestIdentityConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredRecord {
    value: String,
    version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DurableMutationKind {
    CompareAndSwap {
        expected_version: Option<u64>,
        value: String,
    },
    Delete {
        expected_version: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableMutationResult {
    transaction_id: u64,
    plugin_id: String,
    request_id: String,
    key: String,
    mutation: DurableMutationKind,
    delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DurableStorageBatchMutation {
    CompareAndSwap {
        key: String,
        expected_version: Option<u64>,
        value: String,
    },
    Delete {
        key: String,
        expected_version: Option<u64>,
    },
}

impl DurableStorageBatchMutation {
    fn key(&self) -> &str {
        match self {
            Self::CompareAndSwap { key, .. } | Self::Delete { key, .. } => key,
        }
    }

    const fn expected_version(&self) -> Option<u64> {
        match self {
            Self::CompareAndSwap {
                expected_version, ..
            }
            | Self::Delete {
                expected_version, ..
            } => *expected_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedStorageBatch {
    transaction_id: u64,
    plugin_id: String,
    mutations: Vec<DurableStorageBatchMutation>,
}

impl DurableMutationResult {
    fn event(&self) -> Result<ScriptEvent, mc_script::ScriptDtoError> {
        match &self.mutation {
            DurableMutationKind::CompareAndSwap {
                expected_version,
                value,
            } => {
                let request = ScriptPluginStorageCompareAndSwapRequest::try_new(
                    &self.request_id,
                    &self.key,
                    *expected_version,
                    value,
                )?;
                ScriptEvent::plugin_storage_cas_result(
                    &self.plugin_id,
                    &request,
                    true,
                    Some(self.transaction_id),
                )
            }
            DurableMutationKind::Delete { expected_version } => {
                let request = ScriptPluginStorageDeleteRequest::try_new(
                    &self.request_id,
                    &self.key,
                    *expected_version,
                )?;
                ScriptEvent::plugin_storage_delete_result(
                    &self.plugin_id,
                    &request,
                    true,
                    Some(self.transaction_id),
                )
            }
        }
    }
}

enum DurableMutationCommit {
    NotApplied,
    Committed(DurableMutationResult),
    Replay(DurableMutationResult),
}

pub(crate) struct PluginStorage {
    directory: PathBuf,
    journal_path: PathBuf,
    journal_bytes: u64,
    revision: u64,
    live_bytes: usize,
    records: BTreeMap<(String, String), StoredRecord>,
    mutation_results: BTreeMap<(String, String), DurableMutationResult>,
    unknown_result: Option<DurableMutationResult>,
    #[cfg(test)]
    fault: Option<StorageFaultPoint>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageFaultPoint {
    Write,
    Sync,
    ResultSync,
    Rename,
}

impl PluginStorage {
    pub(crate) fn open(world_root: &Path) -> Result<Self, PluginStorageStartError> {
        let directory = world_root.join(STORAGE_DIRECTORY);
        fs::create_dir_all(&directory)?;
        let journal_path = directory.join(JOURNAL_FILE);
        if !journal_path.exists() {
            let journal = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&journal_path)?;
            journal.sync_all()?;
            sync_parent(&directory)?;
        }
        let mut storage = Self {
            directory,
            journal_path,
            journal_bytes: 0,
            revision: 0,
            live_bytes: 0,
            records: BTreeMap::new(),
            mutation_results: BTreeMap::new(),
            unknown_result: None,
            #[cfg(test)]
            fault: None,
        };
        storage.load_journal()?;
        Ok(storage)
    }

    #[cfg(test)]
    pub(crate) fn get(&self, plugin_id: &str, key: &str) -> Option<(String, u64)> {
        self.records
            .get(&(plugin_id.to_owned(), key.to_owned()))
            .map(|record| (record.value.clone(), record.version))
    }

    #[cfg(test)]
    pub(crate) fn compare_and_swap(
        &mut self,
        plugin_id: &str,
        key: &str,
        expected_version: Option<u64>,
        value: &str,
    ) -> Result<Option<u64>, PluginStorageMutationError> {
        let request_id = format!("test-{}", self.revision.saturating_add(1));
        match self.compare_and_swap_durable(plugin_id, &request_id, key, expected_version, value)? {
            DurableMutationCommit::NotApplied => Ok(None),
            DurableMutationCommit::Committed(result) | DurableMutationCommit::Replay(result) => {
                let version = result.transaction_id;
                self.acknowledge_result(version)?;
                Ok(Some(version))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn delete(
        &mut self,
        plugin_id: &str,
        key: &str,
        expected_version: Option<u64>,
    ) -> Result<Option<u64>, PluginStorageMutationError> {
        let request_id = format!("test-{}", self.revision.saturating_add(1));
        match self.delete_durable(plugin_id, &request_id, key, expected_version)? {
            DurableMutationCommit::NotApplied => Ok(None),
            DurableMutationCommit::Committed(result) | DurableMutationCommit::Replay(result) => {
                let version = result.transaction_id;
                self.acknowledge_result(version)?;
                Ok(Some(version))
            }
        }
    }

    #[cfg(test)]
    pub(super) fn fill_plugin_record_quota_for_test(&mut self, plugin_id: &str) {
        for index in 0..MAX_RECORDS_PER_PLUGIN {
            self.records.insert(
                (plugin_id.to_owned(), format!("existing-{index}")),
                StoredRecord {
                    value: "x".to_owned(),
                    version: 1,
                },
            );
        }
    }

    #[cfg(test)]
    pub(super) fn set_live_bytes_for_test(&mut self, live_bytes: usize) {
        self.live_bytes = live_bytes;
    }

    #[cfg(test)]
    pub(super) fn pending_result_count_for_test(&self) -> usize {
        self.mutation_results
            .values()
            .filter(|result| !result.delivered)
            .count()
    }

    #[cfg(test)]
    pub(super) fn compare_and_swap_request_for_test(
        &mut self,
        plugin_id: &str,
        request_id: &str,
        key: &str,
        expected_version: Option<u64>,
        value: &str,
    ) -> Result<Option<u64>, PluginStorageMutationError> {
        match self.compare_and_swap_durable(plugin_id, request_id, key, expected_version, value)? {
            DurableMutationCommit::NotApplied => Ok(None),
            DurableMutationCommit::Committed(result) | DurableMutationCommit::Replay(result) => {
                self.acknowledge_result(result.transaction_id)?;
                Ok(Some(result.transaction_id))
            }
        }
    }

    #[cfg(test)]
    pub(super) fn storage_batch_for_test(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
    ) -> Result<bool, PluginStorageMutationError> {
        match self.prepare_batch(plugin_id, mutations)? {
            ScriptStoragePrepareOutcome::Prepared(batch) => {
                self.commit_batch(batch)?;
                Ok(true)
            }
            ScriptStoragePrepareOutcome::Rejected => Ok(false),
        }
    }

    #[cfg(test)]
    pub(super) fn inject_fault_for_test(&mut self, fault: StorageFaultPoint) {
        self.fault = Some(fault);
    }

    #[cfg(test)]
    pub(super) fn force_compact_for_test(&mut self) -> Result<(), PluginStorageMutationError> {
        self.compact()
    }

    fn get_inner(&self, plugin_id: &str, key: &str) -> Option<(&str, u64)> {
        self.records
            .get(&(plugin_id.to_owned(), key.to_owned()))
            .map(|record| (record.value.as_str(), record.version))
    }

    fn compare_and_swap_durable(
        &mut self,
        plugin_id: &str,
        request_id: &str,
        key: &str,
        expected_version: Option<u64>,
        value: &str,
    ) -> Result<DurableMutationCommit, PluginStorageMutationError> {
        let mutation = DurableMutationKind::CompareAndSwap {
            expected_version,
            value: value.to_owned(),
        };
        if let Some(result) = self.existing_result(plugin_id, request_id, key, &mutation)? {
            return Ok(DurableMutationCommit::Replay(result));
        }
        let entry = (plugin_id.to_owned(), key.to_owned());
        if self.records.get(&entry).map(|record| record.version) != expected_version {
            return Ok(DurableMutationCommit::NotApplied);
        }
        self.ensure_cas_quota(plugin_id, key, value)?;
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let result = DurableMutationResult {
            transaction_id,
            plugin_id: plugin_id.to_owned(),
            request_id: request_id.to_owned(),
            key: key.to_owned(),
            mutation,
            delivered: false,
        };
        validate_durable_result(&result).map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        if let Err(error) = self.append_durable_mutation(OP_DURABLE_CAS, &result) {
            if matches!(error, PluginStorageMutationError::DurabilityUnknown(_)) {
                self.unknown_result = Some(result);
            }
            return Err(error);
        }
        self.install_durable_mutation(result.clone())
            .expect("validated durable compare-and-swap must install");
        Ok(DurableMutationCommit::Committed(result))
    }

    fn delete_durable(
        &mut self,
        plugin_id: &str,
        request_id: &str,
        key: &str,
        expected_version: Option<u64>,
    ) -> Result<DurableMutationCommit, PluginStorageMutationError> {
        let mutation = DurableMutationKind::Delete { expected_version };
        if let Some(result) = self.existing_result(plugin_id, request_id, key, &mutation)? {
            return Ok(DurableMutationCommit::Replay(result));
        }
        let entry = (plugin_id.to_owned(), key.to_owned());
        let Some(record) = self.records.get(&entry) else {
            return Ok(DurableMutationCommit::NotApplied);
        };
        if Some(record.version) != expected_version {
            return Ok(DurableMutationCommit::NotApplied);
        }
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let result = DurableMutationResult {
            transaction_id,
            plugin_id: plugin_id.to_owned(),
            request_id: request_id.to_owned(),
            key: key.to_owned(),
            mutation,
            delivered: false,
        };
        validate_durable_result(&result).map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        if let Err(error) = self.append_durable_mutation(OP_DURABLE_DELETE, &result) {
            if matches!(error, PluginStorageMutationError::DurabilityUnknown(_)) {
                self.unknown_result = Some(result);
            }
            return Err(error);
        }
        self.install_durable_mutation(result.clone())
            .expect("validated durable delete must install");
        Ok(DurableMutationCommit::Committed(result))
    }

    fn prepare_batch(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<PreparedStorageBatch>, PluginStorageMutationError> {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let mutations = mutations
            .iter()
            .map(|mutation| match mutation {
                ScriptStorageMutation::CompareAndSwap {
                    key,
                    expected_version,
                    value,
                } => DurableStorageBatchMutation::CompareAndSwap {
                    key: key.clone(),
                    expected_version: *expected_version,
                    value: value.clone(),
                },
                ScriptStorageMutation::Delete {
                    key,
                    expected_version,
                } => DurableStorageBatchMutation::Delete {
                    key: key.clone(),
                    expected_version: *expected_version,
                },
                _ => unreachable!("validated storage mutation variant"),
            })
            .collect::<Vec<_>>();
        let batch = PreparedStorageBatch {
            transaction_id,
            plugin_id: plugin_id.to_owned(),
            mutations,
        };
        if !self.batch_preconditions_match(&batch) {
            return Ok(ScriptStoragePrepareOutcome::Rejected);
        }
        self.ensure_batch_quota(&batch)?;
        let payload = encode_storage_batch(&batch)?;
        let frame = frame(&payload);
        self.compact_before_append_if_needed(frame.len())?;
        Ok(ScriptStoragePrepareOutcome::Prepared(batch))
    }

    fn commit_batch(
        &mut self,
        batch: PreparedStorageBatch,
    ) -> Result<(), PluginStorageMutationError> {
        let payload = encode_storage_batch(&batch)?;
        self.append_frame(&frame(&payload), false)?;
        self.install_storage_batch(batch)
            .expect("prepared storage batch must install after durable append");
        Ok(())
    }

    fn batch_preconditions_match(&self, batch: &PreparedStorageBatch) -> bool {
        batch.mutations.iter().all(|mutation| {
            let current_version = self
                .records
                .get(&(batch.plugin_id.clone(), mutation.key().to_owned()))
                .map(|record| record.version);
            match mutation {
                DurableStorageBatchMutation::CompareAndSwap { .. } => {
                    current_version == mutation.expected_version()
                }
                DurableStorageBatchMutation::Delete { .. } => {
                    current_version.is_some() && current_version == mutation.expected_version()
                }
            }
        })
    }

    fn ensure_batch_quota(
        &self,
        batch: &PreparedStorageBatch,
    ) -> Result<(), PluginStorageMutationError> {
        let mut record_count = self
            .records
            .keys()
            .filter(|(owner, _)| owner == &batch.plugin_id)
            .count();
        let mut live_bytes = self.live_bytes;
        for mutation in &batch.mutations {
            let entry = (batch.plugin_id.clone(), mutation.key().to_owned());
            let old_value_bytes = self
                .records
                .get(&entry)
                .map_or(0, |record| record.value.len());
            match mutation {
                DurableStorageBatchMutation::CompareAndSwap { value, .. } => {
                    if !self.records.contains_key(&entry) {
                        record_count = record_count
                            .checked_add(1)
                            .ok_or(PluginStorageMutationError::QuotaExceeded)?;
                    }
                    live_bytes = live_bytes
                        .checked_sub(old_value_bytes)
                        .and_then(|bytes| bytes.checked_add(value.len()))
                        .ok_or(PluginStorageMutationError::QuotaExceeded)?;
                }
                DurableStorageBatchMutation::Delete { .. } => {
                    record_count = record_count
                        .checked_sub(1)
                        .ok_or(PluginStorageMutationError::QuotaExceeded)?;
                    live_bytes = live_bytes
                        .checked_sub(old_value_bytes)
                        .ok_or(PluginStorageMutationError::QuotaExceeded)?;
                }
            }
        }
        if record_count > MAX_RECORDS_PER_PLUGIN || live_bytes > MAX_LIVE_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        Ok(())
    }

    fn install_storage_batch(
        &mut self,
        batch: PreparedStorageBatch,
    ) -> Result<(), PluginStorageStartError> {
        validate_storage_batch(&batch)?;
        if batch.transaction_id
            != self
                .revision
                .checked_add(1)
                .ok_or(PluginStorageStartError::Malformed("revision overflow"))?
            || !self.batch_preconditions_match(&batch)
        {
            return Err(PluginStorageStartError::Malformed("stale storage batch"));
        }
        self.ensure_batch_quota(&batch)
            .map_err(|_| PluginStorageStartError::LiveQuotaExceeded)?;
        for mutation in batch.mutations {
            let entry = (batch.plugin_id.clone(), mutation.key().to_owned());
            match mutation {
                DurableStorageBatchMutation::CompareAndSwap { value, .. } => {
                    let old_value_bytes = self
                        .records
                        .get(&entry)
                        .map_or(0, |record| record.value.len());
                    self.live_bytes = self.live_bytes - old_value_bytes + value.len();
                    self.records.insert(
                        entry,
                        StoredRecord {
                            value,
                            version: batch.transaction_id,
                        },
                    );
                }
                DurableStorageBatchMutation::Delete { .. } => {
                    let removed = self
                        .records
                        .remove(&entry)
                        .expect("batch delete was validated");
                    self.live_bytes -= removed.value.len();
                }
            }
        }
        self.revision = batch.transaction_id;
        Ok(())
    }

    fn existing_result(
        &self,
        plugin_id: &str,
        request_id: &str,
        key: &str,
        mutation: &DurableMutationKind,
    ) -> Result<Option<DurableMutationResult>, PluginStorageMutationError> {
        let identity = (plugin_id.to_owned(), request_id.to_owned());
        let Some(result) = self.mutation_results.get(&identity) else {
            return Ok(None);
        };
        if result.key != key || &result.mutation != mutation {
            return Err(PluginStorageMutationError::RequestIdentityConflict);
        }
        Ok(Some(result.clone()))
    }

    fn append_durable_mutation(
        &mut self,
        operation: u8,
        result: &DurableMutationResult,
    ) -> Result<(), PluginStorageMutationError> {
        let mut payload = vec![operation];
        payload.extend_from_slice(
            &serde_json::to_vec(result)
                .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?,
        );
        if payload.len() > MAX_FRAME_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        let frame = frame(&payload);
        self.compact_before_append_if_needed(frame.len())?;
        self.append_frame(&frame, false)
    }

    fn acknowledge_result(
        &mut self,
        transaction_id: u64,
    ) -> Result<(), PluginStorageMutationError> {
        let Some(result) = self
            .mutation_results
            .values()
            .find(|result| result.transaction_id == transaction_id)
        else {
            return Err(PluginStorageMutationError::RequestIdentityConflict);
        };
        if result.delivered {
            return Ok(());
        }
        let mut payload = Vec::with_capacity(9);
        payload.push(OP_RESULT_DELIVERED);
        payload.extend_from_slice(&transaction_id.to_le_bytes());
        let frame = frame(&payload);
        self.compact_before_append_if_needed(frame.len())?;
        self.append_frame(&frame, true)?;
        self.mutation_results
            .values_mut()
            .find(|result| result.transaction_id == transaction_id)
            .expect("result was checked above")
            .delivered = true;
        Ok(())
    }

    fn pending_results(&self) -> Vec<DurableMutationResult> {
        let mut pending = self
            .mutation_results
            .values()
            .filter(|result| !result.delivered)
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by_key(|result| result.transaction_id);
        pending
    }

    fn take_unknown_result(&mut self) -> Option<DurableMutationResult> {
        self.unknown_result.take()
    }

    fn ensure_cas_quota(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), PluginStorageMutationError> {
        let entry = (plugin_id.to_owned(), key.to_owned());
        let old_value_bytes = self
            .records
            .get(&entry)
            .map_or(0, |record| record.value.len());
        let records_for_plugin = self
            .records
            .keys()
            .filter(|(owner, _)| owner == plugin_id)
            .count();
        if !self.records.contains_key(&entry) && records_for_plugin >= MAX_RECORDS_PER_PLUGIN {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        let live_bytes = self.live_bytes - old_value_bytes + value.len();
        if live_bytes > MAX_LIVE_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        Ok(())
    }

    fn load_journal(&mut self) -> Result<(), PluginStorageStartError> {
        let metadata = fs::metadata(&self.journal_path)?;
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(PluginStorageStartError::JournalTooLarge);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(&self.journal_path)?.read_to_end(&mut bytes)?;
        let mut offset = 0usize;
        let mut snapshot_mode = false;
        while offset < bytes.len() {
            let prefix = offset;
            if bytes.len() - offset < 4 {
                self.truncate_incomplete_prefix(prefix)?;
                break;
            }
            let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let length = usize::try_from(length)
                .map_err(|_| PluginStorageStartError::Malformed("frame length"))?;
            if length == 0
                || length > MAX_TRANSACTION_FRAME_BYTES
                || (length > MAX_FRAME_BYTES
                    && bytes.get(offset + 4).copied() != Some(OP_STORAGE_BATCH))
            {
                return Err(PluginStorageStartError::Malformed("frame length"));
            }
            let frame_end = offset
                .checked_add(4 + length + 4)
                .ok_or(PluginStorageStartError::Malformed("frame length"))?;
            if frame_end > bytes.len() {
                self.truncate_incomplete_prefix(prefix)?;
                break;
            }
            let payload = &bytes[offset + 4..offset + 4 + length];
            let expected_crc =
                u32::from_le_bytes(bytes[offset + 4 + length..frame_end].try_into().unwrap());
            if crc32fast::hash(payload) != expected_crc {
                return Err(PluginStorageStartError::Malformed("frame checksum"));
            }
            snapshot_mode = self.apply_frame(payload, snapshot_mode)?;
            offset = frame_end;
        }
        self.journal_bytes =
            u64::try_from(offset).map_err(|_| PluginStorageStartError::JournalTooLarge)?;
        Ok(())
    }

    fn truncate_incomplete_prefix(&self, prefix: usize) -> Result<(), PluginStorageStartError> {
        let journal = OpenOptions::new().write(true).open(&self.journal_path)?;
        journal.set_len(
            u64::try_from(prefix).map_err(|_| PluginStorageStartError::JournalTooLarge)?,
        )?;
        journal.sync_all()?;
        sync_parent(&self.directory)?;
        Ok(())
    }

    fn apply_frame(
        &mut self,
        payload: &[u8],
        snapshot_mode: bool,
    ) -> Result<bool, PluginStorageStartError> {
        let Some((&operation, rest)) = payload.split_first() else {
            return Err(PluginStorageStartError::Malformed("empty frame"));
        };
        match operation {
            OP_SNAPSHOT_RESET => {
                if rest.len() != 8 {
                    return Err(PluginStorageStartError::Malformed("snapshot reset"));
                }
                self.records.clear();
                self.mutation_results.clear();
                self.live_bytes = 0;
                self.revision = u64::from_le_bytes(rest.try_into().unwrap());
                Ok(true)
            }
            OP_SNAPSHOT_SET => {
                let (revision, plugin_id, key, value) = decode_record_frame(rest)?;
                if !snapshot_mode || revision == 0 || revision > self.revision || value.is_empty() {
                    return Err(PluginStorageStartError::Malformed("snapshot record"));
                }
                self.install_record(plugin_id, key, value, revision)?;
                Ok(true)
            }
            OP_DURABLE_CAS | OP_DURABLE_DELETE => {
                let result = decode_durable_result(rest)?;
                if result.delivered
                    || matches!(
                        (&result.mutation, operation),
                        (
                            DurableMutationKind::CompareAndSwap { .. },
                            OP_DURABLE_DELETE
                        ) | (DurableMutationKind::Delete { .. }, OP_DURABLE_CAS)
                    )
                {
                    return Err(PluginStorageStartError::Malformed("durable mutation"));
                }
                self.install_durable_mutation(result)?;
                Ok(false)
            }
            OP_RESULT_DELIVERED => {
                if rest.len() != 8 {
                    return Err(PluginStorageStartError::Malformed("result delivery"));
                }
                let transaction_id = u64::from_le_bytes(rest.try_into().unwrap());
                let Some(result) = self
                    .mutation_results
                    .values_mut()
                    .find(|result| result.transaction_id == transaction_id)
                else {
                    return Err(PluginStorageStartError::Malformed(
                        "unknown result delivery",
                    ));
                };
                if result.delivered {
                    return Err(PluginStorageStartError::Malformed(
                        "duplicate result delivery",
                    ));
                }
                result.delivered = true;
                Ok(false)
            }
            OP_SNAPSHOT_RESULT => {
                if !snapshot_mode {
                    return Err(PluginStorageStartError::Malformed("snapshot result"));
                }
                let result = decode_durable_result(rest)?;
                if result.transaction_id == 0 || result.transaction_id > self.revision {
                    return Err(PluginStorageStartError::Malformed(
                        "snapshot result revision",
                    ));
                }
                validate_durable_result(&result)?;
                let identity = (result.plugin_id.clone(), result.request_id.clone());
                if self.mutation_results.insert(identity, result).is_some() {
                    return Err(PluginStorageStartError::Malformed(
                        "duplicate snapshot result",
                    ));
                }
                Ok(true)
            }
            OP_STORAGE_BATCH => {
                let batch = decode_storage_batch(rest)?;
                self.install_storage_batch(batch)?;
                Ok(false)
            }
            _ => Err(PluginStorageStartError::Malformed("frame operation")),
        }
    }

    fn install_durable_mutation(
        &mut self,
        result: DurableMutationResult,
    ) -> Result<(), PluginStorageStartError> {
        validate_durable_result(&result)?;
        if result.transaction_id
            != self
                .revision
                .checked_add(1)
                .ok_or(PluginStorageStartError::Malformed("revision overflow"))?
        {
            return Err(PluginStorageStartError::Malformed("non-monotonic revision"));
        }
        let entry = (result.plugin_id.clone(), result.key.clone());
        match &result.mutation {
            DurableMutationKind::CompareAndSwap {
                expected_version,
                value,
            } => {
                if self.records.get(&entry).map(|record| record.version) != *expected_version {
                    return Err(PluginStorageStartError::Malformed("stale durable mutation"));
                }
                self.install_record(&result.plugin_id, &result.key, value, result.transaction_id)?;
            }
            DurableMutationKind::Delete { expected_version } => {
                let Some(record) = self.records.get(&entry) else {
                    return Err(PluginStorageStartError::Malformed("absent durable delete"));
                };
                if Some(record.version) != *expected_version {
                    return Err(PluginStorageStartError::Malformed("stale durable delete"));
                }
                let removed = self.records.remove(&entry).expect("record checked above");
                self.live_bytes -= removed.value.len();
            }
        }
        let identity = (result.plugin_id.clone(), result.request_id.clone());
        if self
            .mutation_results
            .insert(identity, result.clone())
            .is_some()
        {
            return Err(PluginStorageStartError::Malformed(
                "duplicate request identity",
            ));
        }
        self.revision = result.transaction_id;
        Ok(())
    }

    fn install_record(
        &mut self,
        plugin_id: &str,
        key: &str,
        value: &str,
        version: u64,
    ) -> Result<(), PluginStorageStartError> {
        validate_record_fields(plugin_id, key, value)?;
        let entry = (plugin_id.to_owned(), key.to_owned());
        let old_value_bytes = self
            .records
            .get(&entry)
            .map_or(0, |record| record.value.len());
        if !self.records.contains_key(&entry)
            && self
                .records
                .keys()
                .filter(|(owner, _)| owner == plugin_id)
                .count()
                >= MAX_RECORDS_PER_PLUGIN
        {
            return Err(PluginStorageStartError::LiveQuotaExceeded);
        }
        self.live_bytes = self.live_bytes - old_value_bytes + value.len();
        if self.live_bytes > MAX_LIVE_BYTES {
            return Err(PluginStorageStartError::LiveQuotaExceeded);
        }
        self.records.insert(
            entry,
            StoredRecord {
                value: value.to_owned(),
                version,
            },
        );
        Ok(())
    }

    fn compact_before_append_if_needed(
        &mut self,
        next_frame_bytes: usize,
    ) -> Result<(), PluginStorageMutationError> {
        let next_journal_bytes = self
            .journal_bytes
            .checked_add(u64::try_from(next_frame_bytes).expect("frame size fits u64"))
            .ok_or(PluginStorageMutationError::QuotaExceeded)?;
        if next_journal_bytes <= MAX_JOURNAL_BYTES {
            return Ok(());
        }
        self.compact()?;
        if self.journal_bytes + u64::try_from(next_frame_bytes).expect("frame size fits u64")
            > MAX_JOURNAL_BYTES
        {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        Ok(())
    }

    fn compact(&mut self) -> Result<(), PluginStorageMutationError> {
        let temporary_path = self.directory.join(JOURNAL_TEMP_FILE);
        let mut temporary = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        let mut reset_payload = Vec::with_capacity(9);
        reset_payload.push(OP_SNAPSHOT_RESET);
        reset_payload.extend_from_slice(&self.revision.to_le_bytes());
        let reset = frame(&reset_payload);
        #[cfg(test)]
        self.maybe_fail(StorageFaultPoint::Write)?;
        temporary.write_all(&reset)?;
        for ((plugin_id, key), record) in &self.records {
            let payload = encode_record_payload(
                OP_SNAPSHOT_SET,
                record.version,
                plugin_id,
                key,
                &record.value,
            )?;
            temporary.write_all(&frame(&payload))?;
        }
        for result in self.mutation_results.values() {
            let mut payload = vec![OP_SNAPSHOT_RESULT];
            payload.extend_from_slice(
                &serde_json::to_vec(result).map_err(|error| {
                    PluginStorageMutationError::Io(std::io::Error::other(error))
                })?,
            );
            if payload.len() > MAX_FRAME_BYTES {
                return Err(PluginStorageMutationError::QuotaExceeded);
            }
            temporary.write_all(&frame(&payload))?;
        }
        if temporary.metadata()?.len() > MAX_JOURNAL_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        #[cfg(test)]
        self.maybe_fail(StorageFaultPoint::Sync)?;
        temporary.sync_all()?;
        #[cfg(test)]
        self.maybe_fail(StorageFaultPoint::Rename)?;
        fs::rename(&temporary_path, &self.journal_path)?;
        sync_parent(&self.directory)?;
        self.journal_bytes = fs::metadata(&self.journal_path)?.len();
        Ok(())
    }

    fn append_frame(
        &mut self,
        frame: &[u8],
        result_ack: bool,
    ) -> Result<(), PluginStorageMutationError> {
        #[cfg(not(test))]
        let _ = result_ack;
        let mut journal = OpenOptions::new().append(true).open(&self.journal_path)?;
        #[cfg(test)]
        self.maybe_fail(StorageFaultPoint::Write)?;
        journal
            .write_all(frame)
            .map_err(PluginStorageMutationError::DurabilityUnknown)?;
        #[cfg(test)]
        self.maybe_fail_unknown(if result_ack {
            StorageFaultPoint::ResultSync
        } else {
            StorageFaultPoint::Sync
        })?;
        journal
            .sync_all()
            .map_err(PluginStorageMutationError::DurabilityUnknown)?;
        self.journal_bytes += u64::try_from(frame.len()).expect("frame size fits u64");
        Ok(())
    }

    #[cfg(test)]
    fn maybe_fail(&mut self, point: StorageFaultPoint) -> Result<(), PluginStorageMutationError> {
        if self.fault == Some(point) {
            self.fault = None;
            return Err(PluginStorageMutationError::Io(std::io::Error::other(
                "injected storage failure",
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    fn maybe_fail_unknown(
        &mut self,
        point: StorageFaultPoint,
    ) -> Result<(), PluginStorageMutationError> {
        if self.fault == Some(point) {
            self.fault = None;
            return Err(PluginStorageMutationError::DurabilityUnknown(
                std::io::Error::other("injected storage synchronization failure"),
            ));
        }
        Ok(())
    }
}

impl ScriptStorageTransactionPrepare for PluginStorage {
    type Prepared = PreparedStorageBatch;
    type Error = PluginStorageMutationError;

    fn prepare(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        self.prepare_batch(plugin_id, mutations)
    }

    fn commit(&mut self, prepared: Self::Prepared) -> Result<(), Self::Error> {
        self.commit_batch(prepared)
    }
}

fn encode_storage_batch(
    batch: &PreparedStorageBatch,
) -> Result<Vec<u8>, PluginStorageMutationError> {
    validate_storage_batch(batch).map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
    let mut payload = vec![OP_STORAGE_BATCH];
    payload.extend_from_slice(
        &serde_json::to_vec(batch)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?,
    );
    if payload.len() > MAX_TRANSACTION_FRAME_BYTES {
        return Err(PluginStorageMutationError::QuotaExceeded);
    }
    Ok(payload)
}

fn decode_storage_batch(payload: &[u8]) -> Result<PreparedStorageBatch, PluginStorageStartError> {
    let batch = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("storage batch"))?;
    validate_storage_batch(&batch)?;
    Ok(batch)
}

fn validate_storage_batch(batch: &PreparedStorageBatch) -> Result<(), PluginStorageStartError> {
    validate_delete_fields(&batch.plugin_id, "batch")?;
    if batch.transaction_id == 0 || batch.mutations.is_empty() || batch.mutations.len() > 16 {
        return Err(PluginStorageStartError::Malformed("storage batch identity"));
    }
    let mut keys = std::collections::BTreeSet::new();
    for mutation in &batch.mutations {
        if !keys.insert(mutation.key()) {
            return Err(PluginStorageStartError::Malformed(
                "duplicate storage batch key",
            ));
        }
        match mutation {
            DurableStorageBatchMutation::CompareAndSwap { key, value, .. } => {
                validate_record_fields(&batch.plugin_id, key, value)?;
            }
            DurableStorageBatchMutation::Delete { key, .. } => {
                validate_delete_fields(&batch.plugin_id, key)?;
            }
        }
    }
    Ok(())
}

fn decode_durable_result(payload: &[u8]) -> Result<DurableMutationResult, PluginStorageStartError> {
    let result = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("durable result"))?;
    validate_durable_result(&result)?;
    Ok(result)
}

fn validate_durable_result(result: &DurableMutationResult) -> Result<(), PluginStorageStartError> {
    validate_delete_fields(&result.plugin_id, &result.key)?;
    if result.transaction_id == 0
        || result.request_id.is_empty()
        || result.request_id.len() > 64
        || !result.request_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(PluginStorageStartError::Malformed(
            "durable result identity",
        ));
    }
    if let DurableMutationKind::CompareAndSwap { value, .. } = &result.mutation {
        validate_record_fields(&result.plugin_id, &result.key, value)?;
    }
    Ok(())
}

fn encode_record_payload(
    operation: u8,
    revision: u64,
    plugin_id: &str,
    key: &str,
    value: &str,
) -> Result<Vec<u8>, PluginStorageMutationError> {
    if plugin_id.len() > 64 || key.is_empty() || key.len() > 128 || value.len() > 4_096 {
        return Err(PluginStorageMutationError::QuotaExceeded);
    }
    let mut payload = Vec::with_capacity(12 + plugin_id.len() + key.len() + value.len());
    payload.push(operation);
    payload.extend_from_slice(&revision.to_le_bytes());
    payload.push(u8::try_from(plugin_id.len()).expect("plugin id length checked"));
    payload.push(u8::try_from(key.len()).expect("key length checked"));
    payload.extend_from_slice(
        &u16::try_from(value.len())
            .expect("value length checked")
            .to_le_bytes(),
    );
    payload.extend_from_slice(plugin_id.as_bytes());
    payload.extend_from_slice(key.as_bytes());
    payload.extend_from_slice(value.as_bytes());
    Ok(payload)
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(payload.len()).expect("storage frame length fits u32");
    let mut frame = Vec::with_capacity(4 + payload.len() + 4);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    frame
}

fn decode_record_frame(payload: &[u8]) -> Result<(u64, &str, &str, &str), PluginStorageStartError> {
    if payload.len() < 12 {
        return Err(PluginStorageStartError::Malformed("record frame"));
    }
    let revision = u64::from_le_bytes(payload[..8].try_into().unwrap());
    let plugin_len = usize::from(payload[8]);
    let key_len = usize::from(payload[9]);
    let value_len = usize::from(u16::from_le_bytes(payload[10..12].try_into().unwrap()));
    let expected = 12usize
        .checked_add(plugin_len)
        .and_then(|length| length.checked_add(key_len))
        .and_then(|length| length.checked_add(value_len))
        .ok_or(PluginStorageStartError::Malformed("record lengths"))?;
    if expected != payload.len() {
        return Err(PluginStorageStartError::Malformed("record lengths"));
    }
    let plugin_end = 12 + plugin_len;
    let key_end = plugin_end + key_len;
    let plugin_id = std::str::from_utf8(&payload[12..plugin_end])
        .map_err(|_| PluginStorageStartError::Malformed("plugin id"))?;
    let key = std::str::from_utf8(&payload[plugin_end..key_end])
        .map_err(|_| PluginStorageStartError::Malformed("key"))?;
    let value = std::str::from_utf8(&payload[key_end..])
        .map_err(|_| PluginStorageStartError::Malformed("value"))?;
    Ok((revision, plugin_id, key, value))
}

fn validate_record_fields(
    plugin_id: &str,
    key: &str,
    value: &str,
) -> Result<(), PluginStorageStartError> {
    if plugin_id.is_empty()
        || plugin_id.len() > 64
        || !plugin_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || key.is_empty()
        || key.len() > 128
        || value.is_empty()
        || value.len() > 4_096
    {
        return Err(PluginStorageStartError::Malformed("record fields"));
    }
    Ok(())
}

fn validate_delete_fields(plugin_id: &str, key: &str) -> Result<(), PluginStorageStartError> {
    if plugin_id.is_empty()
        || plugin_id.len() > 64
        || !plugin_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || key.is_empty()
        || key.len() > 128
    {
        return Err(PluginStorageStartError::Malformed("delete fields"));
    }
    Ok(())
}

fn sync_parent(directory: &Path) -> std::io::Result<()> {
    File::open(directory)?.sync_all()
}

#[derive(Clone)]
pub(crate) struct PluginStorageHandle {
    commands: mpsc::Sender<AdmittedScriptCommand>,
    stopped: Arc<StorageActorStop>,
}

struct StorageActorStop {
    failed: AtomicBool,
    stopped: AtomicBool,
    notify: tokio::sync::Notify,
}

impl StorageActorStop {
    fn mark_failed(&self) {
        self.failed.store(true, Ordering::Release);
    }

    fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

struct StorageActorStopGuard(Arc<StorageActorStop>);

impl Drop for StorageActorStopGuard {
    fn drop(&mut self) {
        self.0.mark_stopped();
    }
}

struct StorageActorContext {
    events: ScriptEventSink,
    shutdown: ShutdownHandle,
    sessions: Arc<SessionRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    item_facts: Arc<mc_data::item_components::ItemFactsTable>,
    stopped: Arc<StorageActorStop>,
}

impl PluginStorageHandle {
    pub(crate) fn start(
        world_root: &Path,
        events: ScriptEventSink,
        shutdown: ShutdownHandle,
        sessions: Arc<SessionRegistry>,
        items: Arc<mc_data::items::ItemRegistry>,
        item_facts: Arc<mc_data::item_components::ItemFactsTable>,
    ) -> Result<Self, PluginStorageStartError> {
        let storage = PluginStorage::open(world_root)?;
        let (commands, receiver) = mpsc::channel(STORAGE_QUEUE_CAPACITY);
        let stopped = Arc::new(StorageActorStop {
            failed: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        });
        tokio::spawn(run_storage_actor(
            storage,
            receiver,
            StorageActorContext {
                events,
                shutdown,
                sessions,
                items,
                item_facts,
                stopped: Arc::clone(&stopped),
            },
        ));
        Ok(Self { commands, stopped })
    }

    pub(crate) async fn enqueue(
        &self,
        command: AdmittedScriptCommand,
        shutdown: &ShutdownHandle,
    ) -> Result<(), AdmittedScriptCommand> {
        if self.stopped.failed.load(Ordering::Acquire) {
            return Err(command);
        }
        let reserve = tokio::select! {
            biased;
            () = shutdown.notified() => return Err(command),
            result = self.commands.reserve() => result,
        };
        let permit = match reserve {
            Ok(permit) => permit,
            Err(_) => return Err(command),
        };
        permit.send(command);
        Ok(())
    }

    pub(crate) async fn wait_stopped(&self) {
        loop {
            let notified = self.stopped.notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            if self.stopped.stopped.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.stopped.failed.load(Ordering::Acquire)
    }
}

async fn run_storage_actor(
    mut storage: PluginStorage,
    mut commands: mpsc::Receiver<AdmittedScriptCommand>,
    context: StorageActorContext,
) {
    let StorageActorContext {
        events,
        shutdown,
        sessions,
        items,
        item_facts,
        stopped,
    } = context;
    let _stopped = StorageActorStopGuard(Arc::clone(&stopped));
    if !replay_pending_results(&mut storage, &events, &shutdown).await {
        stopped.mark_failed();
        fail_queued_storage_commands(&mut commands, &events).await;
        return;
    }
    loop {
        let command = tokio::select! {
            biased;
            () = shutdown.notified() => return,
            command = commands.recv() => match command {
                Some(command) => command,
                None => return,
            },
        };
        let delivery = match command.request() {
            ScriptCommand::PluginStorageGet { request } => {
                let value = storage.get_inner(command.plugin_id(), request.key());
                match command.plugin_storage_get_result(
                    value.map(|(value, _)| value),
                    value.map(|(_, version)| version),
                ) {
                    Ok(event) => deliver_targeted_event(&events, event, &shutdown).await,
                    Err(error) => {
                        warn!(?error, "script storage result construction rejected");
                        return;
                    }
                }
            }
            ScriptCommand::PluginStorageCompareAndSwap { request } => {
                let outcome = storage.compare_and_swap_durable(
                    command.plugin_id(),
                    request.request_id(),
                    request.key(),
                    request.expected_version(),
                    request.value(),
                );
                let (event, durable_result) = match outcome {
                    Ok(DurableMutationCommit::NotApplied) => {
                        (command.plugin_storage_cas_result(false, None), None)
                    }
                    Ok(DurableMutationCommit::Committed(result))
                    | Ok(DurableMutationCommit::Replay(result)) => {
                        let event =
                            command.plugin_storage_cas_result(true, Some(result.transaction_id));
                        (event, Some(result))
                    }
                    Err(
                        error @ (PluginStorageMutationError::RevisionOverflow
                        | PluginStorageMutationError::QuotaExceeded
                        | PluginStorageMutationError::RequestIdentityConflict),
                    ) => {
                        debug!(?error, "script storage mutation rejected before durability");
                        (command.plugin_storage_cas_result(false, None), None)
                    }
                    Err(error @ PluginStorageMutationError::Io(_)) => {
                        warn!(
                            ?error,
                            "script storage durability failed; stopping storage actor"
                        );
                        fail_storage_actor(command, &mut commands, &events, &stopped).await;
                        return;
                    }
                    Err(error @ PluginStorageMutationError::DurabilityUnknown(_)) => {
                        warn!(
                            ?error,
                            "script storage synchronization outcome is unknown; deferring durable result to restart"
                        );
                        let result = storage
                            .take_unknown_result()
                            .expect("durable mutation sync failure retains its result identity");
                        let _deferred =
                            command.plugin_storage_cas_result(true, Some(result.transaction_id));
                        stopped.mark_failed();
                        fail_queued_storage_commands(&mut commands, &events).await;
                        return;
                    }
                };
                let delivery = match event {
                    Ok(event) => deliver_targeted_event(&events, event, &shutdown).await,
                    Err(error) => {
                        warn!(?error, "script storage result construction rejected");
                        return;
                    }
                };
                if !matches!(delivery, TargetedEventDelivery::Delivered) {
                    stopped.mark_failed();
                    fail_queued_storage_commands(&mut commands, &events).await;
                    return;
                }
                if let Some(result) = durable_result
                    && let Err(error) = storage.acknowledge_result(result.transaction_id)
                {
                    warn!(?error, "script storage result acknowledgement failed");
                    stopped.mark_failed();
                    fail_queued_storage_commands(&mut commands, &events).await;
                    return;
                }
                TargetedEventDelivery::Delivered
            }
            ScriptCommand::PluginStorageDelete { request } => {
                let outcome = storage.delete_durable(
                    command.plugin_id(),
                    request.request_id(),
                    request.key(),
                    request.expected_version(),
                );
                let (event, durable_result) = match outcome {
                    Ok(DurableMutationCommit::NotApplied) => {
                        (command.plugin_storage_delete_result(false, None), None)
                    }
                    Ok(DurableMutationCommit::Committed(result))
                    | Ok(DurableMutationCommit::Replay(result)) => {
                        let event =
                            command.plugin_storage_delete_result(true, Some(result.transaction_id));
                        (event, Some(result))
                    }
                    Err(
                        error @ (PluginStorageMutationError::RevisionOverflow
                        | PluginStorageMutationError::QuotaExceeded
                        | PluginStorageMutationError::RequestIdentityConflict),
                    ) => {
                        debug!(?error, "script storage mutation rejected before durability");
                        (command.plugin_storage_delete_result(false, None), None)
                    }
                    Err(error @ PluginStorageMutationError::Io(_)) => {
                        warn!(
                            ?error,
                            "script storage durability failed; stopping storage actor"
                        );
                        fail_storage_actor(command, &mut commands, &events, &stopped).await;
                        return;
                    }
                    Err(error @ PluginStorageMutationError::DurabilityUnknown(_)) => {
                        warn!(
                            ?error,
                            "script storage synchronization outcome is unknown; deferring durable result to restart"
                        );
                        let result = storage
                            .take_unknown_result()
                            .expect("durable mutation sync failure retains its result identity");
                        let _deferred =
                            command.plugin_storage_delete_result(true, Some(result.transaction_id));
                        stopped.mark_failed();
                        fail_queued_storage_commands(&mut commands, &events).await;
                        return;
                    }
                };
                let delivery = match event {
                    Ok(event) => deliver_targeted_event(&events, event, &shutdown).await,
                    Err(error) => {
                        warn!(?error, "script storage result construction rejected");
                        return;
                    }
                };
                if !matches!(delivery, TargetedEventDelivery::Delivered) {
                    stopped.mark_failed();
                    fail_queued_storage_commands(&mut commands, &events).await;
                    return;
                }
                if let Some(result) = durable_result
                    && let Err(error) = storage.acknowledge_result(result.transaction_id)
                {
                    warn!(?error, "script storage result acknowledgement failed");
                    stopped.mark_failed();
                    fail_queued_storage_commands(&mut commands, &events).await;
                    return;
                }
                TargetedEventDelivery::Delivered
            }
            ScriptCommand::InventoryStorageTransaction { transaction } => {
                let outcome = sessions.commit_script_inventory_storage_transaction(
                    command.plugin_id(),
                    transaction,
                    &items,
                    &item_facts,
                    &mut storage,
                );
                let event = match outcome {
                    Ok(committed) => command.inventory_storage_transaction_result(committed),
                    Err(
                        error @ (PluginStorageMutationError::RevisionOverflow
                        | PluginStorageMutationError::QuotaExceeded
                        | PluginStorageMutationError::RequestIdentityConflict),
                    ) => {
                        debug!(?error, "script inventory-storage transaction rejected");
                        command.inventory_storage_transaction_result(false)
                    }
                    Err(error @ PluginStorageMutationError::Io(_)) => {
                        warn!(
                            ?error,
                            "script transaction durability failed; stopping storage actor"
                        );
                        fail_storage_actor(command, &mut commands, &events, &stopped).await;
                        return;
                    }
                    Err(error @ PluginStorageMutationError::DurabilityUnknown(_)) => {
                        warn!(
                            ?error,
                            "script transaction durability outcome is unknown; stopping storage actor"
                        );
                        stopped.mark_failed();
                        fail_queued_storage_commands(&mut commands, &events).await;
                        return;
                    }
                };
                match event {
                    Ok(event) => deliver_targeted_event(&events, event, &shutdown).await,
                    Err(error) => {
                        warn!(
                            ?error,
                            "script inventory-storage result construction rejected"
                        );
                        return;
                    }
                }
            }
            _ => {
                debug!("non-storage admitted command reached plugin storage actor");
                return;
            }
        };
        if !matches!(delivery, TargetedEventDelivery::Delivered) {
            return;
        }
    }
}

async fn replay_pending_results(
    storage: &mut PluginStorage,
    events: &ScriptEventSink,
    shutdown: &ShutdownHandle,
) -> bool {
    for result in storage.pending_results() {
        let event = match result.event() {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "durable storage result reconstruction rejected");
                return false;
            }
        };
        if !matches!(
            deliver_targeted_event(events, event, shutdown).await,
            TargetedEventDelivery::Delivered
        ) {
            return false;
        }
        if let Err(error) = storage.acknowledge_result(result.transaction_id) {
            warn!(
                ?error,
                "durable storage result replay acknowledgement failed"
            );
            return false;
        }
    }
    true
}

async fn fail_storage_actor(
    failed_command: AdmittedScriptCommand,
    commands: &mut mpsc::Receiver<AdmittedScriptCommand>,
    events: &ScriptEventSink,
    stopped: &StorageActorStop,
) {
    stopped.mark_failed();
    commands.close();

    let mut delivery_open = deliver_storage_failure(events, failed_command).await;
    while let Some(command) = commands.recv().await {
        let event =
            match storage_failure_event(command, ScriptPluginStorageFailure::DurabilityFailed) {
                Ok(event) => event,
                Err(error) => {
                    warn!(
                        ?error,
                        "admitted storage failure result construction rejected"
                    );
                    continue;
                }
            };
        if delivery_open {
            delivery_open = matches!(
                deliver_required_targeted_event(events, event).await,
                TargetedEventDelivery::Delivered
            );
        }
    }
}

async fn fail_queued_storage_commands(
    commands: &mut mpsc::Receiver<AdmittedScriptCommand>,
    events: &ScriptEventSink,
) {
    commands.close();
    let mut delivery_open = true;
    while let Some(command) = commands.recv().await {
        let event =
            match storage_failure_event(command, ScriptPluginStorageFailure::DurabilityFailed) {
                Ok(event) => event,
                Err(error) => {
                    warn!(
                        ?error,
                        "admitted storage failure result construction rejected"
                    );
                    continue;
                }
            };
        if delivery_open {
            delivery_open = matches!(
                deliver_required_targeted_event(events, event).await,
                TargetedEventDelivery::Delivered
            );
        }
    }
}

async fn deliver_storage_failure(events: &ScriptEventSink, command: AdmittedScriptCommand) -> bool {
    let event = match storage_failure_event(command, ScriptPluginStorageFailure::DurabilityFailed) {
        Ok(event) => event,
        Err(error) => {
            warn!(
                ?error,
                "admitted storage failure result construction rejected"
            );
            return false;
        }
    };
    matches!(
        deliver_required_targeted_event(events, event).await,
        TargetedEventDelivery::Delivered
    )
}

pub(super) fn storage_failure_event(
    command: AdmittedScriptCommand,
    failure: ScriptPluginStorageFailure,
) -> Result<ScriptEvent, mc_script::ScriptDtoError> {
    if matches!(
        command.request(),
        ScriptCommand::InventoryStorageTransaction { .. }
    ) {
        command.inventory_storage_transaction_result(false)
    } else {
        command.plugin_storage_failure_result(failure)
    }
}

#[cfg(test)]
pub(super) async fn run_storage_actor_for_test(
    storage: PluginStorage,
    commands: Vec<AdmittedScriptCommand>,
    events: ScriptEventSink,
    shutdown: ShutdownHandle,
) {
    let (sender, receiver) = mpsc::channel(STORAGE_QUEUE_CAPACITY);
    for command in commands {
        sender
            .try_send(command)
            .expect("test storage command queue has capacity");
    }
    drop(sender);
    let stopped = Arc::new(StorageActorStop {
        failed: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        notify: tokio::sync::Notify::new(),
    });
    run_storage_actor(
        storage,
        receiver,
        StorageActorContext {
            events,
            shutdown,
            sessions: Arc::new(SessionRegistry::new()),
            items: Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            stopped,
        },
    )
    .await;
}
