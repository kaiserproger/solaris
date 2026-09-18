use std::sync::Arc;
use std::time::Instant;

use mc_data::ItemStack;

use crate::lock_policy::lock_authoritative_mutex;
use crate::play::inventory::PlayerInventory;
use crate::play::simulation::SimulationAuthority;
use mc_protocol::packets::play::{ClientboundBlockEvent, pack_block_pos};

use super::container_state::{
    ContainerCommitContext, ContainerStateCommitError, ContainerViewer, chest_recipients,
    furnace_recipients, furnace_recipients_except,
};
use super::outbound::{OutboundCommand, SessionRecipient, VisibilityDispatch};
use super::transactions::{ChestTransaction, FurnaceTransaction};
use super::visibility::visibility_dispatches;
use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    pub(in crate::play) fn register_furnace_viewer(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) -> i32 {
        let inner = self.lock_inner("register furnace viewer");
        let endpoint = inner
            .sessions
            .get(&id)
            .map(|session| (session.tx.clone(), Arc::clone(&session.pressure)));
        let mut containers = self.lock_containers(position, "register furnace viewer");
        let state_id = *containers.furnace_state_ids.entry(position).or_insert(1);
        let Some((tx, pressure)) = endpoint else {
            return state_id;
        };
        containers
            .furnace_viewers
            .entry(position)
            .or_default()
            .insert(id, ContainerViewer { tx, pressure });
        state_id
    }

    pub(in crate::play) fn unregister_furnace_viewer(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) {
        let mut containers = self.lock_containers(position, "unregister furnace viewer");
        if let Some(viewers) = containers.furnace_viewers.get_mut(&position) {
            viewers.remove(&id);
            if viewers.is_empty() {
                containers.furnace_viewers.remove(&position);
                containers.furnace_state_ids.remove(&position);
            }
        }
    }

    pub(in crate::play) fn furnace_state_id(&self, position: mc_world::BlockPos) -> i32 {
        let containers = self.lock_containers(position, "furnace state id");
        containers
            .furnace_state_ids
            .get(&position)
            .copied()
            .unwrap_or(1)
    }

    #[cfg(test)]
    pub(in crate::play) fn register_chest_viewer(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) -> i32 {
        self.register_chest_viewer_with_lid_transition(id, position)
            .0
    }

    pub(in crate::play) fn register_chest_viewer_with_lid_transition(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) -> (i32, bool) {
        let inner = self.lock_inner("register chest viewer");
        let endpoint = inner
            .sessions
            .get(&id)
            .map(|session| (session.tx.clone(), Arc::clone(&session.pressure)));
        let mut containers = self.lock_containers(position, "register chest viewer");
        let state_id = *containers.chest_state_ids.entry(position).or_insert(1);
        let Some((tx, pressure)) = endpoint else {
            return (state_id, false);
        };
        let viewers = containers.chest_viewers.entry(position).or_default();
        let lid_opened = viewers.is_empty();
        viewers.insert(id, ContainerViewer { tx, pressure });
        (state_id, lid_opened)
    }

    #[cfg(test)]
    pub(in crate::play) fn unregister_chest_viewer(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) {
        let _ = self.unregister_chest_viewer_with_lid_transition(id, position);
    }

    pub(in crate::play) fn unregister_chest_viewer_with_lid_transition(
        &self,
        id: SessionId,
        position: mc_world::BlockPos,
    ) -> bool {
        let mut containers = self.lock_containers(position, "unregister chest viewer");
        let Some(viewers) = containers.chest_viewers.get_mut(&position) else {
            return false;
        };
        if viewers.remove(&id).is_none() || !viewers.is_empty() {
            return false;
        }
        containers.chest_viewers.remove(&position);
        containers.chest_state_ids.remove(&position);
        true
    }

    /// The container's canonical menu generation, the fence a caller observes
    /// before a transfer of its slots. Widened to the crate because the
    /// server-owned warehouse deposit observes it on the plugin side, which is
    /// the caller of that composite.
    pub(crate) fn chest_state_id(&self, position: mc_world::BlockPos) -> i32 {
        let containers = self.lock_containers(position, "chest state id");
        containers
            .chest_state_ids
            .get(&position)
            .copied()
            .unwrap_or(1)
    }

    pub(in crate::play) fn chest_lid_event_dispatches(
        &self,
        position: mc_world::BlockPos,
        block_type: i32,
        opener_count: u8,
    ) -> Vec<VisibilityDispatch> {
        let chunk = (position.x.div_euclid(16), position.z.div_euclid(16));
        let recipients = {
            let inner = self.lock_inner("chest lid event recipients");
            inner
                .sessions
                .iter()
                .filter(|(_, session)| session.loaded.contains(&chunk))
                .map(|(&id, session)| {
                    SessionRecipient::unordered(
                        id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    )
                })
                .collect()
        };
        visibility_dispatches(recipients, || {
            OutboundCommand::BlockEvent(ClientboundBlockEvent {
                position: pack_block_pos(position.x, position.y, position.z),
                action: 1,
                parameter: opener_count,
                block_type,
            })
        })
    }

    #[cfg(test)]
    pub(in crate::play) fn try_chest_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        expected_state_id: i32,
        state_id_increment: i32,
        except: SessionId,
        slots: Vec<ItemStack>,
    ) -> Result<(i32, Vec<VisibilityDispatch>), i32> {
        let (state_id, recipients) = {
            let mut containers = self.lock_containers(position, "chest slot dispatches");
            let current_state_id = containers
                .chest_state_ids
                .get(&position)
                .copied()
                .unwrap_or(1);
            if current_state_id != expected_state_id {
                return Err(current_state_id);
            }
            let state_id = current_state_id.wrapping_add(state_id_increment.max(1));
            containers.chest_state_ids.insert(position, state_id);
            let recipients = chest_recipients(&containers, position, Some(except));
            (state_id, recipients)
        };
        Ok((
            state_id,
            visibility_dispatches(recipients, || OutboundCommand::ChestSlots {
                position,
                state_id,
                slots: slots.clone(),
            }),
        ))
    }

    pub(in crate::play) fn commit_chest_slots<E>(
        &self,
        authority: &SimulationAuthority,
        context: ContainerCommitContext<'_>,
        state_id_increment: i32,
        slots: Vec<ItemStack>,
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<
        (i32, PlayerInventory, ItemStack, Vec<VisibilityDispatch>),
        ContainerStateCommitError<E>,
    > {
        let ContainerCommitContext {
            position,
            expected_state_id,
            actor_session,
            player,
        } = context;
        let player_state = {
            let inner = self.lock_inner("lookup chest player state");
            if !inner.sessions.contains_key(&actor_session) {
                return Err(ContainerStateCommitError::MissingPlayer);
            }
            let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
                return Err(ContainerStateCommitError::MissingPlayer);
            };
            player_state
        };
        let (state_id, inventory, carried_item, recipients) = {
            let mut containers = self.lock_containers(position, "commit chest slots");
            let wait_started = Instant::now();
            let guard = crate::lock_policy::lock_authoritative_mutex(
                &player_state,
                "play.player_persistence",
            );
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "commit chest player state",
                wait_started,
                guard,
            );
            let current_state_id = containers
                .chest_state_ids
                .get(&position)
                .copied()
                .unwrap_or(1);
            let actor_has_open_view = containers
                .chest_viewers
                .get(&position)
                .is_some_and(|viewers| viewers.contains_key(&actor_session));
            if player_state.inventory_recovery_required
                || !actor_has_open_view
                || current_state_id != expected_state_id
                || player_state.inventory.slots != player.expected_inventory.slots
                || player_state.carried_item != player.expected_carried_item
            {
                return Err(ContainerStateCommitError::Rejected {
                    state_id: current_state_id,
                    inventory: Box::new(player_state.inventory.clone()),
                    carried_item: player_state.carried_item.clone(),
                });
            }
            commit().map_err(ContainerStateCommitError::Commit)?;
            player_state.replace_container(
                player.updated_inventory.clone(),
                player.updated_carried_item.clone(),
            );
            let state_id = current_state_id.wrapping_add(state_id_increment.max(1));
            containers.chest_state_ids.insert(position, state_id);
            let recipients = chest_recipients(&containers, position, Some(actor_session));
            (
                state_id,
                player_state.inventory.clone(),
                player_state.carried_item.clone(),
                recipients,
            )
        };
        let mut dispatches = visibility_dispatches(recipients, || OutboundCommand::ChestSlots {
            position,
            state_id,
            slots: slots.clone(),
        });
        for drop in &player.drops {
            dispatches.extend(self.spawn_item_drop_owned(
                authority,
                drop.entity_type_id,
                drop.position,
                drop.stack.clone(),
            ));
        }
        Ok((state_id, inventory, carried_item, dispatches))
    }

    pub(in crate::play) fn server_chest_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        slots: Vec<ItemStack>,
    ) -> (i32, Vec<VisibilityDispatch>) {
        #[cfg(test)]
        self.pause_before_server_container_dispatch_for_test();
        let (state_id, recipients) = {
            let mut containers = self.lock_containers(position, "server chest slot dispatches");
            let recipients = chest_recipients(&containers, position, None);
            if recipients.is_empty() {
                (
                    containers
                        .chest_state_ids
                        .get(&position)
                        .copied()
                        .unwrap_or(1),
                    recipients,
                )
            } else {
                let state_id = containers
                    .chest_state_ids
                    .entry(position)
                    .and_modify(|state_id| *state_id = state_id.wrapping_add(1))
                    .or_insert(2);
                (*state_id, recipients)
            }
        };
        if recipients.is_empty() {
            return (state_id, Vec::new());
        }
        (
            state_id,
            visibility_dispatches(recipients, || OutboundCommand::ChestSlots {
                position,
                state_id,
                slots: slots.clone(),
            }),
        )
    }

    #[cfg(test)]
    pub(in crate::play) fn try_furnace_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        expected_state_id: i32,
        except: SessionId,
        slots: [ItemStack; 3],
    ) -> Result<(i32, Vec<VisibilityDispatch>), i32> {
        let (state_id, recipients) = {
            let mut containers = self.lock_containers(position, "furnace slot dispatches");
            let current_state_id = containers
                .furnace_state_ids
                .get(&position)
                .copied()
                .unwrap_or(1);
            if current_state_id != expected_state_id {
                return Err(current_state_id);
            }
            let state_id = current_state_id.wrapping_add(1);
            containers.furnace_state_ids.insert(position, state_id);
            let recipients = furnace_recipients_except(&containers, position, except);
            (state_id, recipients)
        };
        Ok((
            state_id,
            visibility_dispatches(recipients, || OutboundCommand::FurnaceSlots {
                position,
                state_id,
                slots: slots.clone(),
            }),
        ))
    }

    pub(in crate::play) fn commit_furnace_slots<E>(
        &self,
        authority: &SimulationAuthority,
        context: ContainerCommitContext<'_>,
        slots: [ItemStack; 3],
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<
        (i32, PlayerInventory, ItemStack, Vec<VisibilityDispatch>),
        ContainerStateCommitError<E>,
    > {
        let ContainerCommitContext {
            position,
            expected_state_id,
            actor_session,
            player,
        } = context;
        let player_state = {
            let inner = self.lock_inner("lookup furnace player state");
            if !inner.sessions.contains_key(&actor_session) {
                return Err(ContainerStateCommitError::MissingPlayer);
            }
            let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
                return Err(ContainerStateCommitError::MissingPlayer);
            };
            player_state
        };
        let (state_id, inventory, carried_item, recipients) = {
            let mut containers = self.lock_containers(position, "commit furnace slots");
            let wait_started = Instant::now();
            let guard = crate::lock_policy::lock_authoritative_mutex(
                &player_state,
                "play.player_persistence",
            );
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "commit furnace player state",
                wait_started,
                guard,
            );
            let current_state_id = containers
                .furnace_state_ids
                .get(&position)
                .copied()
                .unwrap_or(1);
            let actor_has_open_view = containers
                .furnace_viewers
                .get(&position)
                .is_some_and(|viewers| viewers.contains_key(&actor_session));
            if player_state.inventory_recovery_required
                || !actor_has_open_view
                || current_state_id != expected_state_id
                || player_state.inventory.slots != player.expected_inventory.slots
                || player_state.carried_item != player.expected_carried_item
            {
                return Err(ContainerStateCommitError::Rejected {
                    state_id: current_state_id,
                    inventory: Box::new(player_state.inventory.clone()),
                    carried_item: player_state.carried_item.clone(),
                });
            }
            commit().map_err(ContainerStateCommitError::Commit)?;
            player_state.replace_container(
                player.updated_inventory.clone(),
                player.updated_carried_item.clone(),
            );
            let state_id = current_state_id.wrapping_add(1);
            containers.furnace_state_ids.insert(position, state_id);
            let recipients = furnace_recipients_except(&containers, position, actor_session);
            (
                state_id,
                player_state.inventory.clone(),
                player_state.carried_item.clone(),
                recipients,
            )
        };
        let mut dispatches = visibility_dispatches(recipients, || OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots: slots.clone(),
        });
        for drop in &player.drops {
            dispatches.extend(self.spawn_item_drop_owned(
                authority,
                drop.entity_type_id,
                drop.position,
                drop.stack.clone(),
            ));
        }
        if let Some(xp_orb) = player.xp_orb {
            dispatches.extend(self.spawn_xp_orb_owned(
                authority,
                xp_orb.entity_type_id,
                xp_orb.position,
                xp_orb.value,
            ));
        }
        Ok((state_id, inventory, carried_item, dispatches))
    }

    pub(in crate::play) fn server_furnace_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        slots: [ItemStack; 3],
    ) -> (i32, Vec<VisibilityDispatch>) {
        #[cfg(test)]
        self.pause_before_server_container_dispatch_for_test();
        let (state_id, recipients) = {
            let mut containers = self.lock_containers(position, "server furnace slot dispatches");
            let recipients = furnace_recipients(&containers, position);
            if recipients.is_empty() {
                (
                    containers
                        .furnace_state_ids
                        .get(&position)
                        .copied()
                        .unwrap_or(1),
                    recipients,
                )
            } else {
                let state_id = containers
                    .furnace_state_ids
                    .entry(position)
                    .and_modify(|state_id| *state_id = state_id.wrapping_add(1))
                    .or_insert(2);
                (*state_id, recipients)
            }
        };
        if recipients.is_empty() {
            return (state_id, Vec::new());
        }
        (
            state_id,
            visibility_dispatches(recipients, || OutboundCommand::FurnaceSlots {
                position,
                state_id,
                slots: slots.clone(),
            }),
        )
    }

    #[cfg(test)]
    pub(in crate::play) fn server_furnace_slot_dispatches_except(
        &self,
        position: mc_world::BlockPos,
        except: SessionId,
        slots: [ItemStack; 3],
    ) -> (i32, Vec<VisibilityDispatch>) {
        let (state_id, recipients) = {
            let mut containers =
                self.lock_containers(position, "server furnace slot dispatches except viewer");
            let state_id = containers
                .furnace_state_ids
                .entry(position)
                .and_modify(|state_id| *state_id = state_id.wrapping_add(1))
                .or_insert(2);
            let state_id = *state_id;
            let recipients = furnace_recipients_except(&containers, position, except);
            (state_id, recipients)
        };
        (
            state_id,
            visibility_dispatches(recipients, || OutboundCommand::FurnaceSlots {
                position,
                state_id,
                slots: slots.clone(),
            }),
        )
    }

    pub(in crate::play) fn server_furnace_data_dispatches(
        &self,
        position: mc_world::BlockPos,
        changed: Vec<(i16, i16)>,
    ) -> Vec<VisibilityDispatch> {
        let recipients = {
            let containers = self.lock_containers(position, "server furnace data dispatches");
            furnace_recipients(&containers, position)
        };
        visibility_dispatches(recipients, || OutboundCommand::FurnaceData {
            position,
            changed: changed.clone(),
        })
    }

    pub(in crate::play) fn prepare_chest_transaction(
        &self,
        actor_session: Option<SessionId>,
        position: mc_world::BlockPos,
    ) -> Option<ChestTransaction> {
        #[cfg(test)]
        let commit_probe = self
            .container_commit_probe
            .lock()
            .expect("test lock poisoned")
            .clone();
        let warehouse_reservation_floors = lock_authoritative_mutex(
            &self.warehouse_reservation_floors,
            "play.warehouse_reservation_floors",
        )
        .clone();
        let warehouse_reservation_admission = self.warehouse_reservation_admission_for(position);
        let inner = self.lock_inner("prepare regional chest commit");
        let player_state = match actor_session {
            Some(actor_session) => {
                if !inner.sessions.contains_key(&actor_session) {
                    return None;
                }
                match inner.player_persistence.get(&actor_session).cloned() {
                    Some(player_state) => Some(player_state),
                    None => return None,
                }
            }
            // A server-owned composite has no session to look up and no player
            // to fence: the container half is all it needs, and its second
            // participant rides the encoded plugin receipt.
            None => None,
        };
        Some(ChestTransaction {
            actor_session,
            containers: self.containers.shard_arc(position),
            player_state,
            warehouse_reservation_floors,
            warehouse_reservation_admission,
            #[cfg(test)]
            commit_probe,
        })
    }

    pub(in crate::play) fn prepare_furnace_transaction(
        &self,
        actor_session: SessionId,
        position: mc_world::BlockPos,
    ) -> Option<FurnaceTransaction> {
        let inner = self.lock_inner("prepare regional furnace commit");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        inner
            .player_persistence
            .get(&actor_session)
            .cloned()
            .map(|player_state| FurnaceTransaction {
                actor_session,
                containers: self.containers.shard_arc(position),
                player_state,
            })
    }
}
