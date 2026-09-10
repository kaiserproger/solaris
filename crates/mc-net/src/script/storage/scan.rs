use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mc_script::{
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptStorageEntry,
};
use uuid::Uuid;

use super::StoredRecord;

const MAX_SCANS: usize = 64;
const MAX_SCANS_PER_PLUGIN: usize = 8;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const SCAN_LIFETIME: Duration = Duration::from_secs(60);
// Charges retained values even when shared, plus keys and worst-case page cursor metadata.
const ENTRY_OVERHEAD: usize = 256;

struct SnapshotEntry {
    key: String,
    record: Arc<StoredRecord>,
}

struct Snapshot {
    owner: String,
    prefix: String,
    revision: u64,
    limit: u8,
    expires: Instant,
    charged_bytes: usize,
    entries: Vec<SnapshotEntry>,
    cursors: Vec<Uuid>,
}

#[derive(Default)]
pub(super) struct StorageScans {
    snapshots: BTreeMap<Uuid, Snapshot>,
    cursors: BTreeMap<Uuid, (Uuid, usize)>,
    charged_bytes: usize,
}

impl StorageScans {
    pub(super) fn scan(
        &mut self,
        owner: &str,
        revision: u64,
        records: &BTreeMap<(String, String), Arc<StoredRecord>>,
        prefix: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> ScriptOperationOutcome {
        match self.page(owner, revision, records, prefix, cursor, limit) {
            Ok((revision, payload)) => ScriptOperationOutcome::committed(revision, payload)
                .expect("validated storage snapshot obeys script bounds"),
            Err(failure) => ScriptOperationOutcome::rejected(failure),
        }
    }

    fn page(
        &mut self,
        owner: &str,
        revision: u64,
        records: &BTreeMap<(String, String), Arc<StoredRecord>>,
        prefix: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> Result<(u64, ScriptOperationPayload), ScriptOperationFailure> {
        if limit == 0
            || usize::from(limit) > mc_script::MAX_STORAGE_SCAN_PAGE
            || revision > mc_script::MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        let now = Instant::now();
        let (snapshot_id, offset) = if let Some(cursor) = cursor {
            let cursor =
                Uuid::parse_str(cursor).map_err(|_| ScriptOperationFailure::CursorExpired)?;
            let (id, offset) = self
                .cursors
                .get(&cursor)
                .copied()
                .ok_or(ScriptOperationFailure::CursorExpired)?;
            (id, offset)
        } else {
            for (_, snapshot) in self
                .snapshots
                .extract_if(.., |_, snapshot| snapshot.expires <= now)
            {
                self.charged_bytes -= snapshot.charged_bytes;
                for cursor in snapshot.cursors {
                    self.cursors.remove(&cursor);
                }
            }
            if self.snapshots.len() >= MAX_SCANS
                || self
                    .snapshots
                    .values()
                    .filter(|snapshot| snapshot.owner == owner)
                    .count()
                    >= MAX_SCANS_PER_PLUGIN
            {
                return Err(ScriptOperationFailure::Capacity);
            }
            let mut entries = Vec::new();
            let mut charged_bytes = 0;
            for ((record_owner, key), record) in
                records.range((owner.to_owned(), prefix.to_owned())..)
            {
                if record_owner != owner || !key.starts_with(prefix) {
                    break;
                }
                charged_bytes += ENTRY_OVERHEAD + key.len() + record.value.len();
                if charged_bytes > MAX_SNAPSHOT_BYTES - self.charged_bytes {
                    return Err(ScriptOperationFailure::Capacity);
                }
                entries.push(SnapshotEntry {
                    key: key.clone(),
                    record: Arc::clone(record),
                });
            }
            if entries.len() <= usize::from(limit) {
                return Ok((
                    revision,
                    ScriptOperationPayload::StoragePage {
                        entries: entries
                            .into_iter()
                            .map(|entry| {
                                ScriptStorageEntry::new(
                                    entry.key,
                                    entry.record.value.clone(),
                                    entry.record.version,
                                )
                            })
                            .collect(),
                        cursor: None,
                    },
                ));
            }
            let id = Uuid::new_v4();
            self.snapshots.insert(
                id,
                Snapshot {
                    owner: owner.to_owned(),
                    prefix: prefix.to_owned(),
                    revision,
                    limit,
                    expires: now + SCAN_LIFETIME,
                    charged_bytes,
                    entries,
                    cursors: vec![id],
                },
            );
            self.cursors.insert(id, (id, 0));
            self.charged_bytes += charged_bytes;
            (id, 0)
        };

        let snapshot = self
            .snapshots
            .get_mut(&snapshot_id)
            .ok_or(ScriptOperationFailure::CursorExpired)?;
        if snapshot.owner != owner {
            return Err(ScriptOperationFailure::Forbidden);
        }
        if snapshot.expires <= now {
            return Err(ScriptOperationFailure::CursorExpired);
        }
        if snapshot.prefix != prefix || snapshot.limit != limit {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        let end = (offset + usize::from(limit)).min(snapshot.entries.len());
        let next = if end < snapshot.entries.len() {
            let page = offset / usize::from(limit);
            if snapshot.cursors.len() == page + 1 {
                let cursor = Uuid::new_v4();
                snapshot.cursors.push(cursor);
                self.cursors.insert(cursor, (snapshot_id, end));
            }
            Some(snapshot.cursors[page + 1].to_string())
        } else {
            None
        };
        let entries = snapshot.entries[offset..end]
            .iter()
            .map(|entry| {
                ScriptStorageEntry::new(
                    entry.key.clone(),
                    entry.record.value.clone(),
                    entry.record.version,
                )
            })
            .collect();
        Ok((
            snapshot.revision,
            ScriptOperationPayload::StoragePage {
                entries,
                cursor: next,
            },
        ))
    }
}
