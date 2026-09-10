use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{ScriptInventoryStorageTransaction, ScriptStorageMutation};

use crate::play::SessionRegistry;
use crate::play::persistence::inventory_recovery::PlayerInventoryRecovery;
use crate::play::world_journal::{WorldChunkJournal, WorldChunkJournalError};
use crate::play::{
    ScriptStorageCommitError, ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
use crate::server::ShutdownHandle;

use super::{
    MAX_TRANSACTION_FRAME_BYTES, OP_STORAGE_BATCH, PluginStorage, PluginStorageMutationError,
    PluginStorageStartError, PreparedStorageBatch, decode_storage_batch, encode_storage_batch,
    frame,
};

impl PreparedStorageBatch {
    pub(crate) const MAX_ENCODED_BYTES: usize = MAX_TRANSACTION_FRAME_BYTES;

    pub(crate) fn encode_world_inventory(&self) -> Result<Vec<u8>, PluginStorageMutationError> {
        self.validate_inventory_participant()
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        encode_storage_batch(self)
    }

    pub(crate) fn decode_world_inventory(payload: &[u8]) -> Result<Self, PluginStorageStartError> {
        let Some((&OP_STORAGE_BATCH, body)) = payload.split_first() else {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision batch",
            ));
        };
        if payload.len() > MAX_TRANSACTION_FRAME_BYTES {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision size",
            ));
        }
        let batch = decode_storage_batch(body)?;
        batch.validate_inventory_participant()?;
        Ok(batch)
    }

    fn validate_inventory_participant(&self) -> Result<(), PluginStorageStartError> {
        if self.inventory.is_none()
            && !self.operation.as_ref().is_some_and(|receipt| {
                matches!(
                    receipt.outcome.payload(),
                    mc_script::ScriptOperationPayload::OwnedInventory { .. }
                )
            })
        {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision has no inventory participant",
            ));
        }
        Ok(())
    }
}

pub(crate) struct InventoryRuntime {
    world: Option<(PathBuf, WorldChunkJournal)>,
    sessions: Arc<SessionRegistry>,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
    save_coordinator: Arc<tokio::sync::Mutex<()>>,
}

impl InventoryRuntime {
    pub(crate) fn new(
        world_root: Option<&Path>,
        shutdown: &ShutdownHandle,
        sessions: Arc<SessionRegistry>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        let world = world_root
            .zip(sessions.world_chunk_journal())
            .map(|(root, journal)| (root.to_owned(), journal));
        Self {
            world,
            sessions,
            items,
            item_facts,
            save_coordinator: shutdown.save_coordinator(),
        }
    }

    /// World chunk images have already replayed; no live actor or save is admitted.
    pub(crate) fn recover(
        &self,
        storage: &mut PluginStorage,
    ) -> Result<(), PluginStorageStartError> {
        let Some((root, journal)) = &self.world else {
            return Ok(());
        };
        journal
            .recover_inventory_decisions(|id, mut batch| {
                let player = batch.inventory.take();
                if batch.transaction_id > storage.revision {
                    if storage.revision.checked_add(1) != Some(batch.transaction_id)
                        || !storage.batch_preconditions_match(&batch)
                    {
                        return Err(WorldChunkJournalError::InventoryDecision(
                            "stale inventory storage projection".to_owned(),
                        ));
                    }
                    storage.commit_batch(batch).map_err(|error| {
                        WorldChunkJournalError::InventoryDecision(error.to_string())
                    })?;
                }
                if let Some(player) = player {
                    player.recover(root, id).map_err(|error| {
                        WorldChunkJournalError::InventoryDecision(error.to_string())
                    })?;
                }
                Ok(())
            })
            .map_err(|error| std::io::Error::other(error).into())
    }

    pub(crate) async fn commit_storage(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        transaction: &ScriptInventoryStorageTransaction,
    ) -> Result<bool, PluginStorageMutationError> {
        let Some((root, journal)) = &self.world else {
            return Ok(false);
        };
        // Acquire save admission before reserving a WAL turn or participant locks.
        // This reuses the existing file-writer authority, not a second player lock.
        let _save_guard = self.save_coordinator.lock().await;
        let id = journal.reserve_decision_ids(1).map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?[0];
        journal.wait_for_append_turn(id).await.map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?;
        let current_tick = self.sessions.simulation_tick();
        let mut commit = WorldInventoryCommit {
            storage,
            journal,
            root,
            id,
            current_tick,
            decided: false,
        };
        let result = self.sessions.commit_script_inventory_storage_transaction(
            plugin_id,
            transaction,
            &self.items,
            &self.item_facts,
            &mut commit,
        );
        if !commit.decided {
            journal
                .record_reserved_snapshot_groups(current_tick, vec![(id, Vec::new())])
                .map_err(|error| {
                    self.sessions.report_world_chunk_journal_failure();
                    std::io::Error::other(error)
                })?;
        } else if matches!(result, Ok(true)) {
            journal
                .mark_inventory_projected(id)
                .expect("unacknowledged inventory decision remains retained");
        } else if matches!(
            result,
            Err(PluginStorageMutationError::DurabilityUnknown(_))
        ) {
            // A durable decision with incomplete participants cannot admit more
            // world work indefinitely while its checkpoint cutoff is retained.
            self.sessions.report_world_chunk_journal_failure();
        }
        result
    }
}

struct WorldInventoryCommit<'a> {
    storage: &'a mut PluginStorage,
    journal: &'a WorldChunkJournal,
    root: &'a Path,
    id: u64,
    current_tick: u64,
    decided: bool,
}

impl ScriptStorageTransactionPrepare for WorldInventoryCommit<'_> {
    type Prepared = PreparedStorageBatch;
    type Error = PluginStorageMutationError;

    fn prepare(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
        inventory: PlayerInventoryRecovery,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        self.storage
            .prepare_batch(plugin_id, mutations, Some(inventory))
    }

    fn commit(
        &mut self,
        mut batch: Self::Prepared,
    ) -> Result<u64, ScriptStorageCommitError<Self::Error>> {
        let player = batch.inventory.take();
        let payload = encode_storage_batch(&batch);
        batch.inventory = player;
        let projection = frame(&payload.map_err(ScriptStorageCommitError::NotCommitted)?);
        self.storage
            .compact_before_append_if_needed(projection.len())
            .map_err(ScriptStorageCommitError::NotCommitted)?;
        if let Err(error) = self.journal.record_reserved_inventory_decision(
            self.current_tick,
            self.id,
            Vec::new(),
            &batch,
        ) {
            if error.outcome_unknown() {
                self.decided = true;
                return Err(ScriptStorageCommitError::DurabilityUnknown(
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error)),
                ));
            }
            let error = match error {
                WorldChunkJournalError::InventoryEncoding(error) => error,
                error => PluginStorageMutationError::Io(std::io::Error::other(error)),
            };
            return Err(ScriptStorageCommitError::NotCommitted(error));
        }
        self.decided = true;
        self.project(batch, &projection).map_err(|error| {
            ScriptStorageCommitError::DurabilityUnknown(match error {
                error @ PluginStorageMutationError::DurabilityUnknown(_) => error,
                error => {
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error))
                }
            })
        })?;
        Ok(self.id)
    }
}

impl WorldInventoryCommit<'_> {
    fn project(
        &mut self,
        mut batch: PreparedStorageBatch,
        frame: &[u8],
    ) -> Result<(), PluginStorageMutationError> {
        let player = batch.inventory.take();
        self.storage.append_frame(frame, false)?;
        self.storage
            .install_storage_batch(batch)
            .expect("prepared inventory projection matches the actor-owned ledger");
        if let Some(player) = player {
            player
                .recover(self.root, self.id)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl InventoryRuntime {
    pub(crate) fn player_only_for_test(
        root: &Path,
        sessions: Arc<SessionRegistry>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let (journal, pending) =
            WorldChunkJournal::open_for_test(root, blocks, Arc::clone(&items)).unwrap();
        assert!(journal.decode_pending(&pending).unwrap().is_empty());
        sessions.install_world_chunk_journal(journal);
        Self::new(
            Some(root),
            &ShutdownHandle::default(),
            sessions,
            items,
            item_facts,
        )
    }
}
