use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use mc_protocol::packets::play::ItemStack;
use tokio::sync::mpsc;
use tracing::warn;

use crate::play::ContainerPlayerPlan;
use crate::play::inventory::PlayerInventory;

use super::outbound::{
    OutboundCommand, OutboundPressureMetrics, SessionPressureObservation, SessionRecipient,
};
use super::{SessionId, SessionRegistry};

#[derive(Debug)]
pub(in crate::play) enum ContainerStateCommitError<E> {
    Rejected {
        state_id: i32,
        inventory: Box<PlayerInventory>,
        carried_item: ItemStack,
    },
    MissingPlayer,
    Commit(E),
}

pub(in crate::play) struct ContainerCommitContext<'a> {
    pub(in crate::play) position: mc_world::BlockPos,
    pub(in crate::play) expected_state_id: i32,
    pub(in crate::play) actor_session: SessionId,
    pub(in crate::play) player: &'a ContainerPlayerPlan,
}

#[derive(Debug, Clone)]
pub(super) struct ContainerViewer {
    pub(super) tx: mpsc::Sender<OutboundCommand>,
    pub(super) pressure: Arc<OutboundPressureMetrics>,
}

#[derive(Debug, Default)]
pub(super) struct ContainerRegistry {
    pub(super) furnace_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, ContainerViewer>>,
    pub(super) furnace_state_ids: HashMap<mc_world::BlockPos, i32>,
    pub(super) chest_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, ContainerViewer>>,
    pub(super) chest_state_ids: HashMap<mc_world::BlockPos, i32>,
}

pub(super) const CONTAINER_REGISTRY_SHARDS: usize = 64;

#[derive(Debug, Clone)]
pub(super) struct ContainerRegistryShards {
    pub(super) shards: Arc<[Arc<Mutex<ContainerRegistry>>; CONTAINER_REGISTRY_SHARDS]>,
}

impl Default for ContainerRegistryShards {
    fn default() -> Self {
        Self {
            shards: Arc::new(std::array::from_fn(|_| {
                Arc::new(Mutex::new(ContainerRegistry::default()))
            })),
        }
    }
}

impl ContainerRegistryShards {
    pub(super) fn shard_index(position: mc_world::BlockPos) -> usize {
        let region_x = position
            .x
            .div_euclid(16)
            .div_euclid(mc_entity::REGION_SIZE_CHUNKS);
        let region_z = position
            .z
            .div_euclid(16)
            .div_euclid(mc_entity::REGION_SIZE_CHUNKS);
        ((region_x as u32).wrapping_mul(31) ^ region_z as u32) as usize % CONTAINER_REGISTRY_SHARDS
    }

    pub(super) fn shard_arc(&self, position: mc_world::BlockPos) -> Arc<Mutex<ContainerRegistry>> {
        Arc::clone(&self.shards[Self::shard_index(position)])
    }
}

pub(super) struct ContainerRegistryGuard<'a> {
    guard: crate::lock_metrics::TimedGuard<MutexGuard<'a, ContainerRegistry>>,
    observation: &'a SessionPressureObservation,
    furnace_viewer_sets_before: usize,
    chest_viewer_sets_before: usize,
    dirty: bool,
}

impl Deref for ContainerRegistryGuard<'_> {
    type Target = ContainerRegistry;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for ContainerRegistryGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;
        &mut self.guard
    }
}

impl Drop for ContainerRegistryGuard<'_> {
    fn drop(&mut self) {
        if self.dirty {
            self.observation.record_container_viewer_set_change(
                self.furnace_viewer_sets_before,
                self.guard.furnace_viewers.len(),
                self.chest_viewer_sets_before,
                self.guard.chest_viewers.len(),
            );
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(super) struct ContainerCommitProbe {
    entered: std::sync::mpsc::Sender<mc_world::BlockPos>,
    release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

#[cfg(test)]
impl ContainerCommitProbe {
    pub(super) fn enter(&self, position: mc_world::BlockPos) {
        self.entered
            .send(position)
            .expect("container commit probe entry");
        self.release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .expect("container commit probe release");
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct ServerContainerDispatchProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct ServerFurnaceCommitProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

impl SessionRegistry {
    pub(super) fn lock_containers(
        &self,
        position: mc_world::BlockPos,
        operation: &'static str,
    ) -> ContainerRegistryGuard<'_> {
        self.lock_container_shard(ContainerRegistryShards::shard_index(position), operation)
    }

    pub(super) fn lock_container_shard(
        &self,
        shard: usize,
        operation: &'static str,
    ) -> ContainerRegistryGuard<'_> {
        let wait_started = Instant::now();
        let guard = self.containers.shards[shard]
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("container registry mutex was poisoned; recovering state");
                poisoned.into_inner()
            });
        let furnace_viewer_sets_before = guard.furnace_viewers.len();
        let chest_viewer_sets_before = guard.chest_viewers.len();
        ContainerRegistryGuard {
            guard: crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ContainerRegistry,
                operation,
                wait_started,
                guard,
            ),
            observation: &self.pressure_observation,
            furnace_viewer_sets_before,
            chest_viewer_sets_before,
            dirty: false,
        }
    }

    pub(in crate::play) fn player_container_state(
        &self,
        id: SessionId,
    ) -> Option<(PlayerInventory, ItemStack)> {
        let state = {
            let inner = self.lock_inner("find player container state");
            if !inner.sessions.contains_key(&id) {
                return None;
            }
            inner.player_persistence.get(&id).cloned()
        }?;
        let wait_started = Instant::now();
        let guard = state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = id,
                "player persistence mutex was poisoned while reading container state; recovering state"
            );
            poisoned.into_inner()
        });
        let state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "read player container state",
            wait_started,
            guard,
        );
        Some((state.inventory.clone(), state.carried_item.clone()))
    }

    #[cfg(test)]
    pub(in crate::play) fn install_server_container_dispatch_probe(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .server_container_dispatch_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ServerContainerDispatchProbe { reached, resume });
    }

    #[cfg(test)]
    pub(super) fn pause_before_server_container_dispatch_for_test(&self) {
        let probe = self
            .server_container_dispatch_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("server container dispatch probe receiver");
            probe
                .resume
                .recv()
                .expect("server container dispatch probe release");
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn install_server_furnace_commit_probe(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .server_furnace_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ServerFurnaceCommitProbe { reached, resume });
    }

    #[cfg(test)]
    pub(in crate::play) fn pause_after_server_furnace_commit_for_test(&self) {
        let probe = self
            .server_furnace_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("server furnace commit probe receiver");
            probe
                .resume
                .recv()
                .expect("server furnace commit probe release");
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn install_container_commit_probe(
        &self,
        entered: std::sync::mpsc::Sender<mc_world::BlockPos>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .container_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ContainerCommitProbe {
            entered,
            release: Arc::new(Mutex::new(release)),
        });
    }
}

pub(super) fn furnace_recipients_except(
    containers: &ContainerRegistry,
    position: mc_world::BlockPos,
    except: SessionId,
) -> Vec<SessionRecipient> {
    containers
        .furnace_viewers
        .get(&position)
        .into_iter()
        .flat_map(|viewers| viewers.iter())
        .filter(|&(&id, _)| id != except)
        .map(|(&id, viewer)| {
            SessionRecipient::unordered(id, viewer.tx.clone(), Arc::clone(&viewer.pressure))
        })
        .collect()
}

pub(super) fn furnace_recipients(
    containers: &ContainerRegistry,
    position: mc_world::BlockPos,
) -> Vec<SessionRecipient> {
    containers
        .furnace_viewers
        .get(&position)
        .into_iter()
        .flat_map(|viewers| viewers.iter())
        .map(|(&id, viewer)| {
            SessionRecipient::unordered(id, viewer.tx.clone(), Arc::clone(&viewer.pressure))
        })
        .collect()
}

pub(super) fn chest_recipients(
    containers: &ContainerRegistry,
    position: mc_world::BlockPos,
    except: Option<SessionId>,
) -> Vec<SessionRecipient> {
    containers
        .chest_viewers
        .get(&position)
        .into_iter()
        .flat_map(|viewers| viewers.iter())
        .filter(|&(&id, _)| except != Some(id))
        .map(|(&id, viewer)| {
            SessionRecipient::unordered(id, viewer.tx.clone(), Arc::clone(&viewer.pressure))
        })
        .collect()
}
