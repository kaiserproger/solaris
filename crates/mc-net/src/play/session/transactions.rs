use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use mc_data::ItemStack;
use mc_data::block_light::BlockLightTable;
use tracing::warn;

use crate::lock_policy::lock_authoritative_mutex;
use crate::play::VisibilityDispatch;
use crate::play::bucket_interactions::plan_bucket_replacement;
use crate::play::campfire::CampfireCookingState;
use crate::play::containers::{ChestView, chest_slot_stacks};
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::PlayerPersistedState;
use crate::play::simulation::{
    BucketUsePlan, CampfireUsePlan, CommittedBucketUse, CommittedCampfireUse,
    CommittedSurvivalBreak, CommittedSurvivalPlacement, SimulationRequestError, SurvivalBreakPlan,
    SurvivalPlacementPlan, placement_inventory_debit, resident_block_edit_outcome,
};
use crate::play::{
    ChestCommitOutcome, ContainerPlayerPlan, FurnaceCommitOutcome, SharedContainerCommit,
    chest_menu_state_change_count, furnace_slot_stacks,
};

use super::SessionId;
#[cfg(test)]
use super::container_state::ContainerCommitProbe;
use super::container_state::{ContainerRegistry, chest_recipients, furnace_recipients_except};
use super::outbound::OutboundCommand;
use super::visibility::visibility_dispatches;

#[derive(Clone)]
pub(in crate::play) struct SurvivalPlacementTransaction {
    pub(super) player_state: Arc<Mutex<PlayerPersistedState>>,
}

#[derive(Clone)]
pub(in crate::play) struct SurvivalBreakTransaction {
    pub(super) player_state: Arc<Mutex<PlayerPersistedState>>,
}

#[derive(Clone)]
pub(in crate::play) struct BucketUseTransaction {
    pub(super) player_state: Arc<Mutex<PlayerPersistedState>>,
}

#[derive(Clone)]
pub(in crate::play) struct CampfireUseTransaction {
    pub(super) player_state: Arc<Mutex<PlayerPersistedState>>,
    pub(super) campfire_cooking: Arc<Mutex<HashMap<mc_world::BlockPos, CampfireCookingState>>>,
}

#[derive(Clone)]
pub(in crate::play) struct ChestTransaction {
    /// The session this composite acts for, or `None` for a server-owned
    /// deposit whose second participant is a plugin-owned record rather than a
    /// player: there is no menu and no player fence to take.
    pub(super) actor_session: Option<SessionId>,
    pub(super) containers: Arc<Mutex<ContainerRegistry>>,
    /// The acting player's durable state, or `None` when no player's inventory
    /// moves with this container.
    pub(super) player_state: Option<Arc<Mutex<PlayerPersistedState>>>,
    #[cfg(test)]
    pub(super) commit_probe: Option<ContainerCommitProbe>,
}

pub(in crate::play) struct ChestTransactionRequest<'a> {
    pub(in crate::play) primary_position: mc_world::BlockPos,
    pub(in crate::play) positions: &'a [mc_world::BlockPos],
    pub(in crate::play) expected_state_id: i32,
    pub(in crate::play) expected: &'a [mc_world::ChestBlockEntity],
    pub(in crate::play) updated: &'a [mc_world::ChestBlockEntity],
    /// The player participant, or `None` when the container moves on its own
    /// and the second participant rides the plugin receipt.
    pub(in crate::play) player: Option<&'a ContainerPlayerPlan>,
}

#[derive(Clone)]
pub(in crate::play) struct FurnaceTransaction {
    pub(super) actor_session: SessionId,
    pub(super) containers: Arc<Mutex<ContainerRegistry>>,
    pub(super) player_state: Arc<Mutex<PlayerPersistedState>>,
}

pub(in crate::play) struct FurnaceTransactionRequest<'a> {
    pub(in crate::play) position: mc_world::BlockPos,
    pub(in crate::play) expected_state_id: i32,
    pub(in crate::play) expected: &'a mc_world::FurnaceBlockEntity,
    pub(in crate::play) updated: &'a mc_world::FurnaceBlockEntity,
    pub(in crate::play) player: &'a ContainerPlayerPlan,
}

/// The container half of one server-owned deposit, as the transaction left it.
///
/// The composite commits the container and the actor's inventory together; this
/// carries no decision identity of its own, because the run pairs a committed
/// container with the world-journal decision that carries its after-image.
pub(in crate::play) enum ServerOwnedChestCommit {
    /// The container's viewers' post-commit slot view, published only after the
    /// decision that carries the after-image is durable.
    Committed(Vec<VisibilityDispatch>),
    /// The player fence moved: recovery pending, a different inventory or a
    /// different carried item.
    StalePlayer,
    /// The container fence moved: its state id, or its authoritative slots.
    StaleContainer,
    /// The container is not a loaded container at the expected position.
    MissingContainer,
}

/// The container half of one chest composite, as the shared helper left it.
///
/// The menu path and the server-owned path differ only in which actor the
/// post-commit publication excludes and in how each names a refusal, so the
/// state-id fence, the conditional commit, the state-id bump and the
/// publication order live here once and both call this.
enum ContainerCommitHalf {
    /// The container committed and its post-commit slot view is published to
    /// every remaining recipient.
    Committed {
        state_id: i32,
        dispatches: Vec<VisibilityDispatch>,
    },
    /// The container fence moved: its state id or its authoritative slots.
    Stale { state_id: i32 },
    /// The container is not a loaded container at the expected position.
    Missing,
    /// The container's chunks belong to more than one owner region.
    CrossRegion,
}

/// The container half of one chest composite: the state-id fence, the
/// conditional block-entity commit, the state-id bump and the `ChestSlots`
/// publication, in that order.
fn commit_container_half(
    containers: &mut ContainerRegistry,
    mutation: &mc_world::WorldMutationView,
    request: &ChestTransactionRequest<'_>,
    exclude_actor: Option<SessionId>,
    state_id_increment: i32,
) -> ContainerCommitHalf {
    let primary_position = request.primary_position;
    let current_state_id = containers
        .chest_state_ids
        .get(&primary_position)
        .copied()
        .unwrap_or(1);
    if current_state_id != request.expected_state_id {
        return ContainerCommitHalf::Stale {
            state_id: current_state_id,
        };
    }
    match mutation.commit_chests_conditionally(request.positions, request.expected, request.updated)
    {
        mc_world::ResidentChestCommitResult::Applied => {}
        mc_world::ResidentChestCommitResult::Rejected(_) => {
            return ContainerCommitHalf::Stale {
                state_id: current_state_id,
            };
        }
        mc_world::ResidentChestCommitResult::Missing => return ContainerCommitHalf::Missing,
        mc_world::ResidentChestCommitResult::CrossRegion => {
            return ContainerCommitHalf::CrossRegion;
        }
    }
    let state_id = current_state_id.wrapping_add(state_id_increment.max(1));
    containers
        .chest_state_ids
        .insert(primary_position, state_id);
    let recipients = chest_recipients(containers, primary_position, exclude_actor);
    ContainerCommitHalf::Committed {
        state_id,
        dispatches: visibility_dispatches(recipients, || OutboundCommand::ChestSlots {
            position: primary_position,
            state_id,
            slots: chest_slot_stacks(&ChestView {
                chests: request.updated.to_vec(),
            }),
        }),
    }
}

/// The state-id increment of one chest composite.
///
/// A player participant's predicted slots and carried item count with the
/// container's own changed slots; a deposit with no player participant has no
/// prediction to invalidate, so an empty plan contributes nothing and the
/// container's slot changes are the whole count.
fn chest_state_id_increment(request: &ChestTransactionRequest<'_>) -> i32 {
    let before = ChestView {
        chests: request.expected.to_vec(),
    };
    let after = ChestView {
        chests: request.updated.to_vec(),
    };
    match request.player {
        Some(player) => chest_menu_state_change_count(
            &before,
            &after,
            &player.expected_inventory,
            &player.updated_inventory,
            &player.expected_carried_item,
            &player.updated_carried_item,
        ),
        None => {
            let empty = PlayerInventory::empty();
            chest_menu_state_change_count(
                &before,
                &after,
                &empty,
                &empty,
                &ItemStack::EMPTY,
                &ItemStack::EMPTY,
            )
        }
    }
}

impl ChestTransaction {
    /// The server-owned entry point beside [`Self::commit`]: the same container
    /// composite without the `actor_has_open_view` fence, because a plugin
    /// deposit has no menu, and publishing the container's slots to every
    /// viewer including the actor, which receives no menu acknowledgement of
    /// its own. Every other fence is unchanged: the container's state id, its
    /// authoritative block entity, and - when a player participates - the
    /// expected inventory and the expected carried item.
    pub(in crate::play) fn commit_server_owned(
        &self,
        mutation: &mc_world::WorldMutationView,
        request: ChestTransactionRequest<'_>,
    ) -> Result<ServerOwnedChestCommit, SimulationRequestError> {
        let state_id_increment = chest_state_id_increment(&request);
        let container_wait_started = Instant::now();
        let container_guard = lock_authoritative_mutex(&self.containers, "play.container_registry");
        let mut containers = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ContainerRegistry,
            "commit server-owned chest slots",
            container_wait_started,
            container_guard,
        );
        let mut player_state = match self.player_state.as_ref() {
            Some(state) => {
                let player_wait_started = Instant::now();
                let player_guard = lock_authoritative_mutex(state, "play.player_persistence");
                let player_state = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "commit server-owned chest player state",
                    player_wait_started,
                    player_guard,
                );
                if let Some(player) = request.player
                    && (player_state.inventory_recovery_required
                        || player_state.inventory.slots != player.expected_inventory.slots
                        || player_state.carried_item != player.expected_carried_item)
                {
                    return Ok(ServerOwnedChestCommit::StalePlayer);
                }
                Some(player_state)
            }
            // No player participates: the container is the only fenced
            // authority, and the plugin receipt carries the other participant.
            None => None,
        };
        let dispatches = match commit_container_half(
            &mut containers,
            mutation,
            &request,
            // A server-owned deposit publishes to every viewer *including* the
            // acting session: it has no menu acknowledgement of its own, so
            // excluding it would leave its client on the pre-deposit slots.
            None,
            state_id_increment,
        ) {
            ContainerCommitHalf::Committed { dispatches, .. } => dispatches,
            ContainerCommitHalf::Stale { .. } => {
                return Ok(ServerOwnedChestCommit::StaleContainer);
            }
            ContainerCommitHalf::Missing => {
                return Ok(ServerOwnedChestCommit::MissingContainer);
            }
            ContainerCommitHalf::CrossRegion => {
                return Err(SimulationRequestError::CrossRegion);
            }
        };
        if let (Some(player_state), Some(player)) = (player_state.as_mut(), request.player) {
            player_state.replace_container(
                player.updated_inventory.clone(),
                player.updated_carried_item.clone(),
            );
        }
        Ok(ServerOwnedChestCommit::Committed(dispatches))
    }

    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        request: ChestTransactionRequest<'_>,
    ) -> Result<ChestCommitOutcome, SimulationRequestError> {
        let (Some(actor_session), Some(player_state), Some(player)) = (
            self.actor_session,
            self.player_state.as_ref(),
            request.player,
        ) else {
            // The menu path is session-authored by construction: a chest
            // transaction without an actor and a player plan is not a menu
            // commit, and the validator refuses one before it gets here.
            return Err(SimulationRequestError::InvalidCommand);
        };
        let state_id_increment = chest_state_id_increment(&request);
        let container_wait_started = Instant::now();
        let container_guard = lock_authoritative_mutex(&self.containers, "play.container_registry");
        let mut containers = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ContainerRegistry,
            "commit regional chest slots",
            container_wait_started,
            container_guard,
        );
        #[cfg(test)]
        if let Some(probe) = self.commit_probe.as_ref() {
            probe.enter(request.primary_position);
        }
        let player_wait_started = Instant::now();
        let player_guard = lock_authoritative_mutex(player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional chest player state",
            player_wait_started,
            player_guard,
        );
        let current_state_id = containers
            .chest_state_ids
            .get(&request.primary_position)
            .copied()
            .unwrap_or(1);
        let actor_has_open_view = containers
            .chest_viewers
            .get(&request.primary_position)
            .is_some_and(|viewers| viewers.contains_key(&actor_session));
        if player_state.inventory_recovery_required
            || !actor_has_open_view
            || current_state_id != request.expected_state_id
            || player_state.inventory.slots != player.expected_inventory.slots
            || player_state.carried_item != player.expected_carried_item
        {
            let authoritative = mutation
                .chest_block_entities(request.positions)
                .ok_or(SimulationRequestError::WorldUnavailable)?;
            return Ok(SharedContainerCommit::Rejected {
                state_id: current_state_id,
                authoritative,
                inventory: player_state.inventory.clone(),
                carried_item: player_state.carried_item.clone(),
            });
        }

        let (state_id, dispatches) = match commit_container_half(
            &mut containers,
            mutation,
            &request,
            Some(actor_session),
            state_id_increment,
        ) {
            ContainerCommitHalf::Committed {
                state_id,
                dispatches,
            } => (state_id, dispatches),
            ContainerCommitHalf::Stale { state_id } => {
                let authoritative = mutation
                    .chest_block_entities(request.positions)
                    .ok_or(SimulationRequestError::WorldUnavailable)?;
                return Ok(SharedContainerCommit::Rejected {
                    state_id,
                    authoritative,
                    inventory: player_state.inventory.clone(),
                    carried_item: player_state.carried_item.clone(),
                });
            }
            ContainerCommitHalf::Missing => {
                return Err(SimulationRequestError::WorldUnavailable);
            }
            ContainerCommitHalf::CrossRegion => {
                return Err(SimulationRequestError::InvalidCommand);
            }
        };

        player_state.replace_container(
            player.updated_inventory.clone(),
            player.updated_carried_item.clone(),
        );
        Ok(SharedContainerCommit::Committed {
            state_id,
            inventory: player_state.inventory.clone(),
            carried_item: player_state.carried_item.clone(),
            dispatches,
        })
    }
}

impl FurnaceTransaction {
    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        request: FurnaceTransactionRequest<'_>,
    ) -> Result<FurnaceCommitOutcome, SimulationRequestError> {
        let FurnaceTransactionRequest {
            position,
            expected_state_id,
            expected,
            updated,
            player,
        } = request;
        let container_wait_started = Instant::now();
        let container_guard = lock_authoritative_mutex(&self.containers, "play.container_registry");
        let mut containers = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ContainerRegistry,
            "commit regional furnace slots",
            container_wait_started,
            container_guard,
        );
        let player_wait_started = Instant::now();
        let player_guard = lock_authoritative_mutex(&self.player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional furnace player state",
            player_wait_started,
            player_guard,
        );
        let current_state_id = containers
            .furnace_state_ids
            .get(&position)
            .copied()
            .unwrap_or(1);
        let actor_has_open_view = containers
            .furnace_viewers
            .get(&position)
            .is_some_and(|viewers| viewers.contains_key(&self.actor_session));
        if player_state.inventory_recovery_required
            || !actor_has_open_view
            || current_state_id != expected_state_id
            || player_state.inventory.slots != player.expected_inventory.slots
            || player_state.carried_item != player.expected_carried_item
        {
            let authoritative = mutation
                .furnace_block_entity(position)
                .ok_or(SimulationRequestError::WorldUnavailable)?;
            return Ok(SharedContainerCommit::Rejected {
                state_id: current_state_id,
                authoritative,
                inventory: player_state.inventory.clone(),
                carried_item: player_state.carried_item.clone(),
            });
        }

        match mutation.commit_furnace_conditionally(position, expected, updated) {
            mc_world::ResidentFurnaceCommitResult::Applied => {}
            mc_world::ResidentFurnaceCommitResult::Rejected(authoritative) => {
                return Ok(SharedContainerCommit::Rejected {
                    state_id: current_state_id,
                    authoritative: *authoritative,
                    inventory: player_state.inventory.clone(),
                    carried_item: player_state.carried_item.clone(),
                });
            }
            mc_world::ResidentFurnaceCommitResult::Missing => {
                return Err(SimulationRequestError::WorldUnavailable);
            }
        }

        player_state.replace_container(
            player.updated_inventory.clone(),
            player.updated_carried_item.clone(),
        );
        let state_id = current_state_id.wrapping_add(1);
        containers.furnace_state_ids.insert(position, state_id);
        let recipients = furnace_recipients_except(&containers, position, self.actor_session);
        Ok(SharedContainerCommit::Committed {
            state_id,
            inventory: player_state.inventory.clone(),
            carried_item: player_state.carried_item.clone(),
            dispatches: visibility_dispatches(recipients, || OutboundCommand::FurnaceSlots {
                position,
                state_id,
                slots: furnace_slot_stacks(updated),
            }),
        })
    }
}

impl BucketUseTransaction {
    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        block_light: Option<&BlockLightTable>,
        world_tick: u64,
        plan: &BucketUsePlan,
    ) -> Result<Option<CommittedBucketUse>, SimulationRequestError> {
        let wait_started = Instant::now();
        let guard = lock_authoritative_mutex(&self.player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional bucket use",
            wait_started,
            guard,
        );
        if player_state.inventory_recovery_required {
            return Ok(None);
        }
        let (inventory, changed_slots) = if let Some(change) = &plan.inventory {
            if player_state.inventory.slots[change.held_slot] != change.expected_held {
                return Ok(None);
            }
            let Some((inventory, changed_slots)) = plan_bucket_replacement(
                &player_state.inventory,
                change.held_slot,
                change.replacement_item,
                change.replacement_max_stack,
            ) else {
                return Ok(None);
            };
            (Some(inventory), changed_slots)
        } else {
            (None, Vec::new())
        };

        let Some(block) = resident_block_edit_outcome(
            mutation,
            block_light,
            world_tick,
            std::slice::from_ref(&plan.edit),
            std::slice::from_ref(&plan.precondition),
            &[],
        ) else {
            return Ok(None);
        };
        if block.applied.len() != 1 {
            return Err(SimulationRequestError::WorldMutationFailed);
        }
        if let Some(inventory) = &inventory {
            player_state.replace_inventory(inventory.clone());
        }
        Ok(Some(CommittedBucketUse {
            block,
            inventory,
            changed_slots,
        }))
    }
}

impl CampfireUseTransaction {
    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        plan: &CampfireUsePlan,
    ) -> Result<Option<CommittedCampfireUse>, SimulationRequestError> {
        let wait_started = Instant::now();
        let guard = lock_authoritative_mutex(&self.player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional campfire use",
            wait_started,
            guard,
        );
        if player_state.inventory_recovery_required
            || player_state.inventory.slots[plan.held_slot] != plan.expected_held
        {
            return Ok(None);
        }

        let mut campfires =
            lock_authoritative_mutex(&self.campfire_cooking, "play.campfire_cooking");
        let authoritative = campfires.get(&plan.position).cloned().unwrap_or_default();
        if authoritative != plan.expected_cooking {
            return Ok(None);
        }

        let mut inventory = player_state.inventory.clone();
        let held = &mut inventory.slots[plan.held_slot];
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
        let changed_slots = vec![(plan.held_slot, held.clone())];

        match mutation.commit_opaque_block_entity_conditionally(
            plan.position,
            plan.expected_state,
            plan.expected_token,
            plan.persistent_bytes.clone(),
        ) {
            mc_world::ResidentOpaqueBlockEntityCommitResult::Applied => {}
            mc_world::ResidentOpaqueBlockEntityCommitResult::Stale => return Ok(None),
            mc_world::ResidentOpaqueBlockEntityCommitResult::Missing => {
                return Err(SimulationRequestError::WorldUnavailable);
            }
        }

        if plan.updated_cooking.is_empty() {
            campfires.remove(&plan.position);
        } else {
            campfires.insert(plan.position, plan.updated_cooking.clone());
        }
        player_state.replace_inventory(inventory.clone());
        Ok(Some(CommittedCampfireUse {
            inventory,
            changed_slots,
        }))
    }
}

impl SurvivalBreakTransaction {
    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        block_light: Option<&BlockLightTable>,
        world_tick: u64,
        plan: &SurvivalBreakPlan,
    ) -> Result<Option<CommittedSurvivalBreak>, SimulationRequestError> {
        let wait_started = Instant::now();
        let guard = lock_authoritative_mutex(&self.player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional survival break",
            wait_started,
            guard,
        );
        if player_state.inventory_recovery_required
            || player_state.selected_hotbar_slot != plan.held.hotbar_slot
        {
            return Ok(None);
        }
        let tool_slot = PlayerInventory::HOTBAR_BASE + usize::from(plan.held.hotbar_slot);
        if player_state.inventory.slots[tool_slot] != plan.held.expected {
            return Ok(None);
        }
        let mut inventory = player_state.inventory.clone();
        let mut changed_slots = Vec::new();
        if let Some(max_damage) = plan.held.max_damage {
            if plan.held.expected.is_empty() {
                return Err(SimulationRequestError::InvalidCommand);
            }
            let held = &mut inventory.slots[tool_slot];
            let new_damage = held.damage.unwrap_or(0).saturating_add(1);
            if new_damage >= max_damage {
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(new_damage);
            }
            changed_slots.push((tool_slot, held.clone()));
        }

        let Some(block) = resident_block_edit_outcome(
            mutation,
            block_light,
            world_tick,
            &plan.edits,
            &plan.preconditions,
            &[],
        ) else {
            return Ok(None);
        };
        if block.applied.len() != plan.edits.len() {
            warn!(
                expected = plan.edits.len(),
                applied = block.applied.len(),
                "regional survival break preflight produced a partial world edit"
            );
            return Err(SimulationRequestError::WorldMutationFailed);
        }

        player_state.replace_inventory(inventory.clone());
        Ok(Some(CommittedSurvivalBreak {
            block,
            inventory,
            changed_slots,
            dispatches: Vec::new(),
        }))
    }
}

impl SurvivalPlacementTransaction {
    pub(in crate::play) fn commit(
        &self,
        mutation: &mc_world::WorldMutationView,
        block_light: Option<&BlockLightTable>,
        world_tick: u64,
        plan: &SurvivalPlacementPlan,
    ) -> Result<Option<CommittedSurvivalPlacement>, SimulationRequestError> {
        let wait_started = Instant::now();
        let guard = lock_authoritative_mutex(&self.player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit regional survival placement",
            wait_started,
            guard,
        );
        let Some(consume_held_item) =
            placement_inventory_debit(player_state.game_mode, plan.expected_game_mode)
        else {
            return Ok(None);
        };
        let held_slot = plan.held.inventory_slot;
        if held_slot != PlayerInventory::OFFHAND_SLOT
            && held_slot
                != PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot)
        {
            return Ok(None);
        }
        if player_state.inventory_recovery_required
            || player_state.inventory.slots[held_slot] != plan.held.expected
        {
            return Ok(None);
        }
        let mut inventory = player_state.inventory.clone();
        let changed_slots = if consume_held_item {
            let held = &mut inventory.slots[held_slot];
            held.count = held.count.saturating_sub(1);
            if held.count <= 0 {
                *held = ItemStack::EMPTY;
            }
            vec![(held_slot, held.clone())]
        } else {
            Vec::new()
        };

        let Some(block) = resident_block_edit_outcome(
            mutation,
            block_light,
            world_tick,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        ) else {
            return Ok(None);
        };
        if block.applied.len() != plan.edits.len() {
            warn!(
                expected = plan.edits.len(),
                applied = block.applied.len(),
                "regional survival placement preflight produced a partial world edit"
            );
            return Err(SimulationRequestError::WorldMutationFailed);
        }

        if consume_held_item {
            player_state.replace_inventory(inventory.clone());
        }
        Ok(Some(CommittedSurvivalPlacement {
            block,
            inventory,
            changed_slots,
        }))
    }
}
