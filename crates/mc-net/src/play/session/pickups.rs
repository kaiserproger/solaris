use super::entity_lifecycle::{
    clear_removed_entity_tracking_locked, nearby_entity_candidate_ids_locked,
    track_entity_chunk_locked,
};
use super::interaction_geometry::{distance_sq, entity_aabb};
use super::outbound::{OutboundCommand, SessionRecipient, VisibilityDispatch};
use super::visibility::{despawn_entity_visibility_locked, ordered_session_recipient};
use super::{
    ServerEntitySnapshot, SessionEntityGuards, SessionId, SessionRegistry, SessionRegistryInner,
    initialize_entity_wire_state_locked, schedule_item_despawn_locked, server_entity_snapshot_from,
    session_recipients, spawn_entity_visibility_from_snapshot_locked,
    spawn_entity_visibility_locked, spawned_xp_observer_ids, visible_entity_observers_locked,
};
use crate::play::GameMode;
use crate::play::campfire::PendingCampfireOutput;
use crate::play::inventory::{PlayerInventory, can_stack, item_max_stack};
use crate::play::persistence::XpState;
use crate::play::simulation::SimulationAuthority;
use mc_entity::{
    EntityId, EntityItemPickupOwnerBlock, EntityItemStack, EntityLifecycle, EntitySnapshot,
    SpawnEntity, Vec3,
};
use mc_protocol::packets::play::ItemStack;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

pub(in crate::play) const ENTITY_PICKUP_RADIUS: f64 = 2.25;
pub(in crate::play) const ITEM_PICKUP_DELAY_TICKS: u64 = 4;
const ITEM_MERGE_RADIUS: f64 = 0.5;
const PLAYER_ITEM_OWNER_PICKUP_BLOCK_TICKS: u64 = 100;

#[cfg(test)]
#[derive(Debug)]
pub(in crate::play) struct ClaimedPickup {
    pub(in crate::play) stack: EntityItemStack,
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug)]
pub(in crate::play) struct CreditedItemPickup {
    pub(in crate::play) credited: EntityItemStack,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) changed_slots: Vec<(usize, ItemStack)>,
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
}

struct ClaimedPickupParts {
    picked: EntityItemStack,
    update_entity: bool,
    collector_entity_id: i32,
    snapshot: ServerEntitySnapshot,
    recipients: Vec<SessionRecipient>,
}

#[cfg(test)]
#[derive(Debug)]
pub(in crate::play) struct ClaimedExperience {
    pub(in crate::play) value: i32,
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug)]
pub(in crate::play) struct CreditedExperiencePickup {
    pub(in crate::play) value: i32,
    pub(in crate::play) xp: XpState,
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug)]
pub(in crate::play) struct CreditedArrowPickup {
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) changed_slots: Vec<(usize, ItemStack)>,
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
}

struct ClaimedExperienceParts {
    value: i32,
    collector_entity_id: i32,
    snapshot: ServerEntitySnapshot,
    recipients: Vec<SessionRecipient>,
}

struct ClaimedArrowParts {
    collector_entity_id: i32,
    snapshot: ServerEntitySnapshot,
    recipients: Vec<SessionRecipient>,
}

impl SessionRegistry {
    pub(super) fn pickup_candidate_dispatch(
        &self,
        session_id: SessionId,
    ) -> Option<VisibilityDispatch> {
        self.pickup_candidate_dispatches(vec![session_id])
            .into_iter()
            .next()
    }

    pub(super) fn pickup_candidate_dispatches(
        &self,
        mut session_ids: Vec<SessionId>,
    ) -> Vec<VisibilityDispatch> {
        if session_ids.is_empty() {
            return Vec::new();
        }
        session_ids.sort_unstable();
        session_ids.dedup();
        let plans = {
            let inner = self.lock_inner("plan pickup candidates");
            session_ids
                .into_iter()
                .filter_map(|session_id| {
                    let session = inner.sessions.get(&session_id)?;
                    let position = Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                    let candidate_ids =
                        nearby_entity_candidate_ids_locked(&inner, position, ENTITY_PICKUP_RADIUS);
                    Some((session_id, position, candidate_ids))
                })
                .collect::<Vec<_>>()
        };
        if plans.is_empty() {
            return Vec::new();
        }
        let radius_sq = ENTITY_PICKUP_RADIUS * ENTITY_PICKUP_RADIUS;
        let lifecycle_tick = self.simulation_tick();
        let snapshots = {
            let entities = self.lock_entities("snapshot pickup candidates");
            #[cfg(test)]
            self.pause_during_pickup_snapshot_for_test();
            plans
                .into_iter()
                .map(|(session_id, position, candidate_ids)| {
                    let candidates = candidate_ids
                        .into_iter()
                        .filter_map(|id| entities.snapshot(id))
                        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
                        .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
                        .filter(|entity| {
                            entity.item_stack.is_some()
                                || entity.experience_value.is_some_and(|value| value > 0)
                                || (entity.type_name == "minecraft:arrow"
                                    && entity.on_ground
                                    && entity.velocity == Vec3::ZERO)
                        })
                        .filter(|entity| {
                            if entity.item_stack.is_none() {
                                return true;
                            }
                            let blocked =
                                entity
                                    .retained
                                    .item_pickup_owner_block
                                    .is_some_and(|block| {
                                        block.owner_session == session_id
                                            && lifecycle_tick < block.expires_tick
                                    });
                            entity
                                .retained
                                .item_pickup_ready_tick
                                .is_none_or(|ready_tick| lifecycle_tick >= ready_tick)
                                && !blocked
                        })
                        .map(server_entity_snapshot_from)
                        .collect::<Vec<_>>();
                    (session_id, candidates)
                })
                .collect::<Vec<_>>()
        };
        let inner = self.lock_inner("publish pickup candidates");
        snapshots
            .into_iter()
            .filter_map(|(session_id, candidates)| {
                let session = inner.sessions.get(&session_id)?;
                let current_position = Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                let recipient = SessionRecipient::unordered(
                    session_id,
                    session.tx.clone(),
                    Arc::clone(&session.pressure),
                );
                let candidates = candidates
                    .into_iter()
                    .filter(|entity| distance_sq(entity.position, current_position) <= radius_sq)
                    .collect::<Vec<_>>();
                (!candidates.is_empty()).then_some(VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::PickupCandidates(candidates),
                })
            })
            .collect()
    }

    pub(super) fn append_spawned_xp_pickup_candidates(
        &self,
        dispatches: &mut Vec<VisibilityDispatch>,
    ) {
        let session_ids = spawned_xp_observer_ids(dispatches);
        dispatches.extend(self.pickup_candidate_dispatches(session_ids));
    }

    pub(in crate::play) fn item_pickup_ready_dispatches_owned(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
    ) -> Vec<VisibilityDispatch> {
        let (session_ids, mut dispatches) = {
            let mut inner = self.lock_session_entities("publish item pickup readiness");
            let due_ticks = inner
                .item_pickup_ready
                .range(..=tick)
                .map(|(&ready_tick, _)| ready_tick)
                .collect::<Vec<_>>();
            let mut entity_ids = due_ticks
                .into_iter()
                .flat_map(|ready_tick| {
                    inner
                        .item_pickup_ready
                        .remove(&ready_tick)
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            entity_ids.sort_unstable();
            entity_ids.dedup();
            let dispatches = merge_item_entities_locked(&mut inner, &entity_ids);
            let positions = entity_ids
                .into_iter()
                .filter_map(|entity_id| inner.entities.snapshot(entity_id))
                .filter(|entity| {
                    entity.lifecycle == EntityLifecycle::Alive
                        && entity.item_stack.is_some()
                        && entity
                            .retained
                            .item_pickup_ready_tick
                            .is_some_and(|ready_tick| ready_tick <= tick)
                })
                .map(|entity| entity.position)
                .collect::<Vec<_>>();
            let session_ids = if positions.is_empty() {
                Vec::new()
            } else {
                let radius_sq = ENTITY_PICKUP_RADIUS * ENTITY_PICKUP_RADIUS;
                inner
                    .sessions
                    .iter()
                    .filter_map(|(&session_id, session)| {
                        let player = Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                        positions
                            .iter()
                            .any(|position| distance_sq(*position, player) <= radius_sq)
                            .then_some(session_id)
                    })
                    .collect::<Vec<_>>()
            };
            (session_ids, dispatches)
        };
        dispatches.extend(self.pickup_candidate_dispatches(session_ids));
        dispatches
    }

    pub(in crate::play) fn spawn_item_drop_owned(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: Vec3,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn item drop");
        spawn_item_drop_locked(&mut inner, entity_type_id, position, stack)
    }

    pub(in crate::play) fn try_spawn_item_drop_batch_owned(
        &self,
        _authority: &SimulationAuthority,
        drops: impl IntoIterator<Item = (i32, Vec3, EntityItemStack)>,
    ) -> Result<Vec<VisibilityDispatch>, mc_entity::RegionOwnerLaneError> {
        let drops = drops.into_iter().collect::<Vec<_>>();
        if drops.is_empty() {
            return Ok(Vec::new());
        }
        if drops.iter().any(|(entity_type_id, position, stack)| {
            *entity_type_id < 0 || !position.is_finite() || stack.is_empty()
        }) {
            return Err(mc_entity::RegionOwnerLaneError::InvalidMutation);
        }

        let mut inner = self.lock_session_entities("spawn item drop batch");
        let lifecycle_tick = inner.entity_lifecycle_tick;
        let ready_tick = lifecycle_tick.saturating_add(ITEM_PICKUP_DELAY_TICKS);
        let entities = drops.iter().map(|(entity_type_id, position, stack)| {
            let mut entity = SpawnEntity::new(*entity_type_id, "minecraft:item", *position);
            entity.velocity = item_drop_velocity(*position, stack, lifecycle_tick);
            entity.item_stack = Some(stack.clone());
            entity.retained.spawn_tick = lifecycle_tick;
            entity.retained.item_pickup_ready_tick = Some(ready_tick);
            entity
        });
        let ids = inner.entities.try_spawn_batch(entities)?;
        assert_eq!(
            ids.len(),
            drops.len(),
            "regional entity owner returned a partial successful spawn batch"
        );

        let mut dispatches = Vec::new();
        for (id, (entity_type_id, position, _)) in ids.into_iter().zip(drops) {
            schedule_item_despawn_locked(&mut inner, id, lifecycle_tick);
            inner
                .item_pickup_ready
                .entry(ready_tick)
                .or_default()
                .push(id);
            inner
                .entity_type_aabbs
                .entry(entity_type_id)
                .or_insert_with(|| entity_aabb("minecraft:item"));
            track_entity_chunk_locked(&mut inner, id, position);
            initialize_entity_wire_state_locked(&mut inner, id);
            dispatches.extend(spawn_entity_visibility_locked(&mut inner, id));
        }
        Ok(dispatches)
    }

    pub(in crate::play) fn materialize_pending_campfire_outputs_owned(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: mc_world::BlockPos,
        outputs: &[PendingCampfireOutput],
    ) -> Vec<EntitySnapshot> {
        if outputs.is_empty() {
            return Vec::new();
        }
        let drop_position = Vec3::new(
            f64::from(position.x) + 0.5,
            f64::from(position.y) + 1.0,
            f64::from(position.z) + 0.5,
        );
        let mut inner = self.lock_session_entities("materialize pending campfire outputs");
        let lifecycle_tick = inner.entity_lifecycle_tick;
        let ready_tick = lifecycle_tick.saturating_add(ITEM_PICKUP_DELAY_TICKS);
        let candidates = outputs.iter().map(|output| {
            let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", drop_position);
            entity.uuid = Some(output.uuid);
            entity.velocity = item_drop_velocity(drop_position, &output.stack, lifecycle_tick);
            entity.item_stack = Some(output.stack.clone());
            entity.retained.spawn_tick = lifecycle_tick;
            entity.retained.item_pickup_ready_tick = Some(ready_tick);
            entity
        });
        inner.entities.spawn_unique_batch(candidates);

        let expected_uuids = outputs
            .iter()
            .map(|output| output.uuid)
            .collect::<HashSet<_>>();
        let mut snapshots = inner
            .entities
            .snapshots_vec()
            .into_iter()
            .filter(|snapshot| expected_uuids.contains(&snapshot.uuid))
            .collect::<Vec<_>>();
        snapshots.sort_unstable_by_key(|snapshot| snapshot.uuid.as_u128());
        for snapshot in &snapshots {
            schedule_item_despawn_locked(&mut inner, snapshot.id, snapshot.retained.spawn_tick);
            inner
                .entity_type_aabbs
                .entry(snapshot.type_id)
                .or_insert_with(|| entity_aabb("minecraft:item"));
            track_entity_chunk_locked(&mut inner, snapshot.id, snapshot.position);
            initialize_entity_wire_state_locked(&mut inner, snapshot.id);
        }
        snapshots
    }

    pub(in crate::play) fn publish_materialized_campfire_outputs_owned(
        &self,
        _authority: &SimulationAuthority,
        snapshots: &[EntitySnapshot],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("publish materialized campfire outputs");
        let mut dispatches = Vec::new();
        let mut ready_sessions = Vec::new();
        let lifecycle_tick = inner.entity_lifecycle_tick;
        for snapshot in snapshots {
            let published = spawn_entity_visibility_from_snapshot_locked(
                &mut inner,
                server_entity_snapshot_from(snapshot.clone()),
            );
            if snapshot
                .retained
                .item_pickup_ready_tick
                .is_some_and(|ready_tick| ready_tick <= lifecycle_tick)
            {
                ready_sessions.extend(published.iter().map(|dispatch| dispatch.recipient.id));
            } else if let Some(ready_tick) = snapshot.retained.item_pickup_ready_tick {
                inner
                    .item_pickup_ready
                    .entry(ready_tick)
                    .or_default()
                    .push(snapshot.id);
            }
            dispatches.extend(published);
        }
        drop(inner);
        dispatches.extend(self.pickup_candidate_dispatches(ready_sessions));
        dispatches
    }

    pub(in crate::play) fn spawn_item_drop_checkpoint_only_owned(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: Vec3,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn checkpoint-only item drop");
        spawn_item_drop_checkpoint_only_locked(&mut inner, entity_type_id, position, stack)
    }

    #[cfg(test)]
    pub(in crate::play) fn spawn_item_drop(
        &self,
        entity_type_id: i32,
        position: Vec3,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn item drop");
        spawn_item_drop_locked(&mut inner, entity_type_id, position, stack)
    }

    #[cfg(test)]
    pub(in crate::play) fn spawn_xp_orb(
        &self,
        entity_type_id: i32,
        position: Vec3,
        value: i32,
    ) -> Vec<VisibilityDispatch> {
        let mut dispatches = {
            let mut inner = self.lock_session_entities("spawn xp orb");
            spawn_xp_orb_locked(&mut inner, entity_type_id, position, value)
        };
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        dispatches
    }

    pub(in crate::play) fn spawn_xp_orb_owned(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: Vec3,
        value: i32,
    ) -> Vec<VisibilityDispatch> {
        let mut dispatches = {
            let mut inner = self.lock_session_entities("spawn xp orb");
            spawn_xp_orb_locked(&mut inner, entity_type_id, position, value)
        };
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        dispatches
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn pickup_item_into_inventory(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
        expected_item_id: u32,
        expected_damage: Option<i32>,
        expected_enchantments: &[mc_data::ItemEnchantment],
        max_stack: i32,
    ) -> Option<CreditedItemPickup> {
        if max_stack <= 0 {
            return None;
        }
        let (parts, inventory, changed_slots) = {
            let mut inner = self.lock_session_entities("credit item pickup");
            let snapshot = inner.entities.snapshot(entity_id)?;
            if snapshot.lifecycle != EntityLifecycle::Alive {
                return None;
            }
            let stack = snapshot.item_stack.clone()?;
            if stack.count <= 0
                || stack.item_id != expected_item_id
                || stack.damage != expected_damage
                || stack.enchantments != expected_enchantments
            {
                return None;
            }
            let player_state = inner.player_persistence.get(&collector_session).cloned()?;
            let wait_started = Instant::now();
            let guard = player_state.lock().unwrap_or_else(|poisoned| {
                warn!(
                    session_id = collector_session,
                    "player persistence mutex was poisoned during item pickup; recovering state"
                );
                poisoned.into_inner()
            });
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "credit item pickup",
                wait_started,
                guard,
            );
            if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
                return None;
            }
            let mut inventory = player_state.inventory.clone();
            let probe = ItemStack {
                item_id: stack.item_id,
                count: stack.count,
                damage: stack.damage,
                enchantments: stack.enchantments.clone(),
                custom_name: stack.custom_name.as_deref().cloned(),
                item_model: stack.item_model.as_deref().cloned().map(Arc::new),
            };
            let (remaining, changed_slots) = inventory.merge_pickup_stack(
                probe,
                max_stack,
                player_state.selected_hotbar_slot,
            )?;
            let remaining_count = if remaining.is_empty() {
                0
            } else {
                remaining.count
            };
            let credited_count = stack.count.checked_sub(remaining_count)?;
            if credited_count <= 0 || changed_slots.is_empty() {
                return None;
            }
            #[cfg(test)]
            self.pause_after_item_pickup_plan_for_test();
            let parts =
                claim_item_pickup_locked(&mut inner, snapshot, collector_session, credited_count)?;
            debug_assert_eq!(parts.picked.count, credited_count);
            player_state.replace_inventory(inventory.clone());
            (parts, inventory, changed_slots)
        };
        let (credited, dispatches) = into_claimed_pickup(entity_id, parts);
        Some(CreditedItemPickup {
            credited,
            inventory,
            changed_slots,
            dispatches,
        })
    }

    #[cfg(test)]
    pub(in crate::play) fn claim_item_pickup_for_test(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        max_count: i32,
    ) -> Option<ClaimedPickup> {
        self.claim_item_pickup_owned(entity_id, collector_session, max_count)
    }

    #[cfg(test)]
    pub(in crate::play) fn install_item_pickup_plan_probe_for_test(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .item_pickup_plan_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(super::ItemPickupPlanProbe { reached, resume });
    }

    #[cfg(test)]
    fn pause_after_item_pickup_plan_for_test(&self) {
        let probe = self
            .item_pickup_plan_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("item pickup plan probe receiver");
            probe.resume.recv().expect("item pickup plan probe release");
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn replace_item_stack_after_pickup_plan_for_test(
        &self,
        entity_id: EntityId,
        stack: EntityItemStack,
    ) -> bool {
        let Some(snapshot) =
            super::entity_owner::owner_result(self.entities.handle.snapshot(entity_id))
        else {
            return false;
        };
        super::entity_owner::owner_result(
            self.entities
                .handle
                .set_item_stack_if_current(snapshot, Some(stack)),
        )
    }

    #[cfg(test)]
    fn claim_item_pickup_owned(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        max_count: i32,
    ) -> Option<ClaimedPickup> {
        if max_count <= 0 {
            return None;
        }
        let parts = {
            let mut inner = self.lock_session_entities("remove dead entity");
            let snapshot = inner.entities.snapshot(entity_id)?;
            claim_item_pickup_locked(&mut inner, snapshot, collector_session, max_count)?
        };
        let (stack, dispatches) = into_claimed_pickup(entity_id, parts);
        Some(ClaimedPickup { stack, dispatches })
    }

    pub(in crate::play) fn pickup_arrow_into_inventory(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
        arrow_item_id: u32,
        max_stack: i32,
    ) -> Option<CreditedArrowPickup> {
        if max_stack <= 0 {
            return None;
        }
        let (parts, inventory, changed_slots) = {
            let mut inner = self.lock_session_entities("credit arrow pickup");
            let snapshot = inner.entities.snapshot(entity_id)?;
            if snapshot.lifecycle != EntityLifecycle::Alive
                || snapshot.type_name != "minecraft:arrow"
                || !snapshot.on_ground
                || snapshot.velocity != Vec3::ZERO
            {
                return None;
            }
            let player_state = inner.player_persistence.get(&collector_session).cloned()?;
            let wait_started = Instant::now();
            let guard = player_state.lock().unwrap_or_else(|poisoned| {
                warn!(
                    session_id = collector_session,
                    "player persistence mutex was poisoned during arrow pickup; recovering state"
                );
                poisoned.into_inner()
            });
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "credit arrow pickup",
                wait_started,
                guard,
            );
            if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
                return None;
            }
            let mut inventory = player_state.inventory.clone();
            let (remaining, changed_slots) = inventory.merge_pickup_stack(
                ItemStack::new(arrow_item_id, 1),
                max_stack,
                player_state.selected_hotbar_slot,
            )?;
            if !remaining.is_empty() || changed_slots.is_empty() {
                return None;
            }
            let parts = claim_arrow_pickup_locked(&mut inner, entity_id, collector_session)?;
            player_state.replace_inventory(inventory.clone());
            (parts, inventory, changed_slots)
        };
        let dispatches = into_claimed_arrow(entity_id, parts);
        Some(CreditedArrowPickup {
            inventory,
            changed_slots,
            dispatches,
        })
    }

    #[cfg(test)]
    pub(in crate::play) fn claim_arrow_pickup_for_test(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<Vec<VisibilityDispatch>> {
        self.claim_arrow_pickup_owned(entity_id, collector_session)
    }

    #[cfg(test)]
    fn claim_arrow_pickup_owned(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<Vec<VisibilityDispatch>> {
        let parts = {
            let mut inner = self.lock_session_entities("claim arrow pickup");
            claim_arrow_pickup_locked(&mut inner, entity_id, collector_session)?
        };
        Some(into_claimed_arrow(entity_id, parts))
    }

    pub(in crate::play) fn pickup_experience_into_player(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<CreditedExperiencePickup> {
        let (parts, xp) = {
            let mut inner = self.lock_session_entities("credit experience pickup");
            let current = inner.entities.snapshot(entity_id)?;
            if current.lifecycle != EntityLifecycle::Alive {
                return None;
            }
            let value = current.experience_value?;
            if value <= 0 {
                return None;
            }
            let player_state = inner.player_persistence.get(&collector_session).cloned()?;
            let wait_started = Instant::now();
            let guard = player_state.lock().unwrap_or_else(|poisoned| {
                warn!(
                    session_id = collector_session,
                    "player persistence mutex was poisoned during XP pickup; recovering state"
                );
                poisoned.into_inner()
            });
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "credit experience pickup",
                wait_started,
                guard,
            );
            if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
                return None;
            }
            let mut xp = player_state.xp.clone();
            if !xp.add_points(value) {
                return None;
            }
            let parts = claim_experience_pickup_locked(&mut inner, entity_id, collector_session)?;
            debug_assert_eq!(parts.value, value);
            player_state.replace_xp(xp.clone());
            (parts, xp)
        };
        let (value, dispatches) = into_claimed_experience(entity_id, parts);
        Some(CreditedExperiencePickup {
            value,
            xp,
            dispatches,
        })
    }

    #[cfg(test)]
    pub(in crate::play) fn claim_experience_pickup(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<ClaimedExperience> {
        self.claim_experience_pickup_owned(entity_id, collector_session)
    }

    #[cfg(test)]
    pub(in crate::play) fn claim_experience_pickup_for_test(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<ClaimedExperience> {
        self.claim_experience_pickup_owned(entity_id, collector_session)
    }

    #[cfg(test)]
    fn claim_experience_pickup_owned(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<ClaimedExperience> {
        let parts = {
            let mut inner = self.lock_session_entities("claim experience pickup");
            claim_experience_pickup_locked(&mut inner, entity_id, collector_session)?
        };
        let (value, dispatches) = into_claimed_experience(entity_id, parts);
        Some(ClaimedExperience { value, dispatches })
    }
}

pub(super) fn block_item_pickup_for_owner_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    owner_session: SessionId,
) {
    let expires_tick = inner
        .entity_lifecycle_tick
        .saturating_add(PLAYER_ITEM_OWNER_PICKUP_BLOCK_TICKS);
    let Some(expected) = inner.entities.snapshot(entity_id) else {
        return;
    };
    let mut next = expected.clone();
    next.retained.item_pickup_owner_block = Some(EntityItemPickupOwnerBlock {
        owner_session,
        expires_tick,
    });
    let _ = inner.entities.replace_snapshot_if_current(expected, next);
}

fn item_stack_probe(stack: &EntityItemStack) -> ItemStack {
    ItemStack {
        item_id: stack.item_id,
        count: stack.count,
        damage: stack.damage,
        enchantments: stack.enchantments.clone(),
        custom_name: stack.custom_name.as_deref().cloned(),
        item_model: stack.item_model.as_deref().cloned().map(Arc::new),
    }
}

fn merged_item_owner_block(
    left: Option<EntityItemPickupOwnerBlock>,
    right: Option<EntityItemPickupOwnerBlock>,
    lifecycle_tick: u64,
) -> Option<Option<EntityItemPickupOwnerBlock>> {
    let left = left.filter(|block| lifecycle_tick < block.expires_tick);
    let right = right.filter(|block| lifecycle_tick < block.expires_tick);
    match (left, right) {
        (None, None) => Some(None),
        (Some(left), Some(right)) if left.owner_session == right.owner_session => {
            Some(Some(EntityItemPickupOwnerBlock {
                owner_session: left.owner_session,
                expires_tick: left.expires_tick.max(right.expires_tick),
            }))
        }
        _ => None,
    }
}

pub(super) fn merge_item_entities_locked(
    inner: &mut SessionEntityGuards<'_>,
    ready_ids: &[EntityId],
) -> Vec<VisibilityDispatch> {
    let items = Arc::clone(&inner.player_combat.items);
    let item_facts = Arc::clone(&inner.player_combat.item_facts);
    let mut consumed_ids = HashSet::new();
    let mut dispatches = Vec::new();
    let radius_sq = ITEM_MERGE_RADIUS * ITEM_MERGE_RADIUS;

    for &ready_id in ready_ids {
        if consumed_ids.contains(&ready_id) {
            continue;
        }
        let Some(ready) = inner.entities.snapshot(ready_id) else {
            continue;
        };
        if ready.lifecycle != EntityLifecycle::Alive || ready.type_name != "minecraft:item" {
            continue;
        }
        let Some(ready_stack) = ready.item_stack.as_ref() else {
            continue;
        };
        if ready_stack.count <= 0 {
            continue;
        }

        let mut candidates =
            nearby_entity_candidate_ids_locked(inner, ready.position, ITEM_MERGE_RADIUS);
        candidates.sort_unstable();
        for candidate_id in candidates {
            if candidate_id == ready_id || consumed_ids.contains(&candidate_id) {
                continue;
            }
            let Some(candidate) = inner.entities.snapshot(candidate_id) else {
                continue;
            };
            if candidate.lifecycle != EntityLifecycle::Alive
                || candidate.type_name != "minecraft:item"
                || distance_sq(ready.position, candidate.position) > radius_sq
            {
                continue;
            }
            let Some(merged_owner_block) = merged_item_owner_block(
                ready.retained.item_pickup_owner_block,
                candidate.retained.item_pickup_owner_block,
                inner.entity_lifecycle_tick,
            ) else {
                continue;
            };
            let Some(candidate_stack) = candidate.item_stack.as_ref() else {
                continue;
            };
            if candidate_stack.count <= 0
                || !can_stack(
                    &item_stack_probe(ready_stack),
                    &item_stack_probe(candidate_stack),
                )
            {
                continue;
            }

            let (survivor_expected, consumed_expected) = if ready_stack.count
                > candidate_stack.count
                || (ready_stack.count == candidate_stack.count && ready.id < candidate.id)
            {
                (ready.clone(), candidate.clone())
            } else {
                (candidate.clone(), ready.clone())
            };
            let Some(survivor_stack) = survivor_expected.item_stack.as_ref() else {
                continue;
            };
            let Some(consumed_stack) = consumed_expected.item_stack.as_ref() else {
                continue;
            };
            let Some(merged_count) = survivor_stack.count.checked_add(consumed_stack.count) else {
                continue;
            };
            if items.name_of(survivor_stack.item_id).is_none() {
                continue;
            }
            let max_stack = item_max_stack(&item_facts, &items, &item_stack_probe(survivor_stack));
            if merged_count > max_stack {
                continue;
            }

            let mut survivor_next = survivor_expected.clone();
            let mut merged_stack = survivor_stack.clone();
            merged_stack.count = merged_count;
            survivor_next.item_stack = Some(merged_stack);
            survivor_next.retained.spawn_tick = survivor_expected
                .retained
                .spawn_tick
                .max(consumed_expected.retained.spawn_tick);
            survivor_next.retained.item_pickup_ready_tick = survivor_expected
                .retained
                .item_pickup_ready_tick
                .max(consumed_expected.retained.item_pickup_ready_tick);
            survivor_next.retained.item_pickup_owner_block = merged_owner_block;

            if !inner.entities.merge_item_snapshots_if_current(
                survivor_expected.clone(),
                survivor_next.clone(),
                consumed_expected.clone(),
            ) {
                continue;
            }

            let survivor_id = survivor_next.id;
            let consumed_id = consumed_expected.id;
            let survivor_published = server_entity_snapshot_from(survivor_next.clone());
            let consumed_published = server_entity_snapshot_from(consumed_expected);
            inner
                .published_entity_snapshots
                .insert(survivor_id, survivor_published.clone());
            schedule_item_despawn_locked(inner, survivor_id, survivor_next.retained.spawn_tick);
            if survivor_next
                .retained
                .item_pickup_ready_tick
                .is_some_and(|ready_tick| ready_tick > inner.entity_lifecycle_tick)
            {
                inner
                    .item_pickup_ready
                    .entry(
                        survivor_next
                            .retained
                            .item_pickup_ready_tick
                            .expect("future pickup readiness was checked"),
                    )
                    .or_default()
                    .push(survivor_id);
            }

            let update_recipients =
                session_recipients(inner, visible_entity_observers_locked(inner, survivor_id));
            inner.entity_dispatches.data += update_recipients.len() as u64;
            dispatches.extend(visibility_dispatches(update_recipients, || {
                OutboundCommand::UpdateEntityData(survivor_published.clone())
            }));
            dispatches.extend(despawn_entity_visibility_locked(inner, &consumed_published));
            clear_removed_entity_tracking_locked(inner, consumed_id);
            consumed_ids.insert(consumed_id);
            break;
        }
    }
    dispatches
}

pub(super) fn spawn_item_drop_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    stack: EntityItemStack,
) -> Vec<VisibilityDispatch> {
    let Some(id) = spawn_item_drop_entity_locked(inner, entity_type_id, position, stack) else {
        return Vec::new();
    };
    spawn_entity_visibility_locked(inner, id)
}

pub(super) fn spawn_item_drop_checkpoint_only_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    stack: EntityItemStack,
) -> Vec<VisibilityDispatch> {
    let Some(id) =
        spawn_item_drop_entity_locked_inner(inner, entity_type_id, position, stack, false)
    else {
        return Vec::new();
    };
    spawn_entity_visibility_locked(inner, id)
}

pub(super) fn spawn_item_drop_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    stack: EntityItemStack,
) -> Option<EntityId> {
    spawn_item_drop_entity_locked_inner(inner, entity_type_id, position, stack, true)
}

fn spawn_item_drop_entity_locked_inner(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    stack: EntityItemStack,
    journal_commit: bool,
) -> Option<EntityId> {
    if stack.is_empty() {
        return None;
    }
    let spawn_tick = inner.entity_lifecycle_tick;
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", position);
    entity.velocity = item_drop_velocity(position, &stack, spawn_tick);
    entity.item_stack = Some(stack);
    entity.retained.spawn_tick = spawn_tick;
    let ready_tick = spawn_tick.saturating_add(ITEM_PICKUP_DELAY_TICKS);
    entity.retained.item_pickup_ready_tick = Some(ready_tick);
    let id = if journal_commit {
        inner.entities.spawn(entity)
    } else {
        inner.entities.spawn_deferred_journal(entity)
    };
    schedule_item_despawn_locked(inner, id, spawn_tick);
    inner
        .item_pickup_ready
        .entry(ready_tick)
        .or_default()
        .push(id);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert_with(|| entity_aabb("minecraft:item"));
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    Some(id)
}

fn item_drop_velocity(position: Vec3, stack: &EntityItemStack, simulation_tick: u64) -> Vec3 {
    let seed = position.x.to_bits()
        ^ position.y.to_bits().rotate_left(21)
        ^ position.z.to_bits().rotate_left(42)
        ^ u64::from(stack.item_id).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ simulation_tick.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let component = |value: u8| f64::from(value) / 255.0 * 0.2 - 0.1;
    Vec3::new(
        component(seed as u8),
        0.2,
        component(seed.rotate_left(29) as u8),
    )
}

pub(super) fn spawn_xp_orb_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    value: i32,
) -> Vec<VisibilityDispatch> {
    if value <= 0 {
        return Vec::new();
    }
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:experience_orb", position);
    entity.experience_value = Some(value);
    entity.velocity = Vec3::new(0.0, 0.08, 0.0);
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    let id = inner.entities.spawn(entity);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert_with(|| entity_aabb("minecraft:experience_orb"));
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    spawn_entity_visibility_locked(inner, id)
}

fn collector_within_pickup_radius_locked(
    inner: &SessionRegistryInner,
    collector_session: SessionId,
    entity_position: Vec3,
) -> bool {
    let Some(session) = inner.sessions.get(&collector_session) else {
        return false;
    };
    let player_position = Vec3::new(session.pose.x, session.pose.y, session.pose.z);
    distance_sq(entity_position, player_position) <= ENTITY_PICKUP_RADIUS * ENTITY_PICKUP_RADIUS
}

fn claim_item_pickup_locked(
    inner: &mut SessionEntityGuards<'_>,
    snapshot: EntitySnapshot,
    collector_session: SessionId,
    max_count: i32,
) -> Option<ClaimedPickupParts> {
    if max_count <= 0 {
        return None;
    }
    let entity_id = snapshot.id;
    if snapshot.lifecycle != EntityLifecycle::Alive {
        return None;
    }
    if !collector_within_pickup_radius_locked(inner, collector_session, snapshot.position) {
        return None;
    }
    if !item_pickup_ready_locked(inner, entity_id, inner.entity_lifecycle_tick) {
        return None;
    }
    if item_pickup_owner_blocked_locked(inner, entity_id, collector_session) {
        return None;
    }
    let mut stack = snapshot.item_stack.clone()?;
    if stack.count <= 0 {
        return None;
    }
    let picked_count = stack.count.min(max_count);
    let picked = EntityItemStack {
        item_id: stack.item_id,
        count: picked_count,
        damage: stack.damage,
        enchantments: stack.enchantments.clone(),
        custom_name: stack.custom_name.clone(),
        item_model: stack.item_model.clone(),
    };
    stack.count -= picked_count;

    if stack.count <= 0 {
        let snapshot = inner
            .entities
            .remove_if_current(snapshot)
            .map(server_entity_snapshot_from)?;
        clear_removed_entity_tracking_locked(inner, entity_id);
        let (collector_entity_id, recipients) =
            picked_entity_recipients_locked(inner, entity_id, collector_session);
        Some(ClaimedPickupParts {
            picked,
            update_entity: false,
            collector_entity_id,
            snapshot,
            recipients,
        })
    } else {
        let mut published = server_entity_snapshot_from(snapshot.clone());
        published.item_stack = Some(stack.clone());
        if !inner
            .entities
            .set_item_stack_if_current(snapshot, Some(stack.clone()))
        {
            return None;
        }
        inner
            .published_entity_snapshots
            .insert(entity_id, published.clone());
        let recipients =
            session_recipients(inner, visible_entity_observers_locked(inner, entity_id));
        inner.entity_dispatches.data += recipients.len() as u64;
        Some(ClaimedPickupParts {
            picked,
            update_entity: true,
            collector_entity_id: 0,
            snapshot: published,
            recipients,
        })
    }
}

fn into_claimed_pickup(
    entity_id: EntityId,
    parts: ClaimedPickupParts,
) -> (EntityItemStack, Vec<VisibilityDispatch>) {
    let dispatches = if parts.update_entity {
        visibility_dispatches(parts.recipients, || {
            OutboundCommand::UpdateEntityData(parts.snapshot.clone())
        })
    } else {
        picked_entity_dispatches(
            entity_id,
            parts.collector_entity_id,
            parts.picked.count,
            parts.snapshot,
            parts.recipients,
        )
    };
    (parts.picked, dispatches)
}

fn claim_experience_pickup_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    collector_session: SessionId,
) -> Option<ClaimedExperienceParts> {
    let current = inner.entities.snapshot(entity_id)?;
    if current.lifecycle != EntityLifecycle::Alive {
        return None;
    }
    if !collector_within_pickup_radius_locked(inner, collector_session, current.position) {
        return None;
    }
    let value = current.experience_value?;
    if value <= 0 {
        return None;
    }
    let snapshot = inner
        .entities
        .remove_if_current(current)
        .map(server_entity_snapshot_from)?;
    clear_removed_entity_tracking_locked(inner, entity_id);
    let (collector_entity_id, recipients) =
        picked_entity_recipients_locked(inner, entity_id, collector_session);
    Some(ClaimedExperienceParts {
        value,
        collector_entity_id,
        snapshot,
        recipients,
    })
}

fn into_claimed_experience(
    entity_id: EntityId,
    parts: ClaimedExperienceParts,
) -> (i32, Vec<VisibilityDispatch>) {
    let dispatches = picked_entity_dispatches(
        entity_id,
        parts.collector_entity_id,
        parts.value,
        parts.snapshot,
        parts.recipients,
    );
    (parts.value, dispatches)
}

fn claim_arrow_pickup_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    collector_session: SessionId,
) -> Option<ClaimedArrowParts> {
    let snapshot = inner.entities.snapshot(entity_id)?;
    if snapshot.lifecycle != EntityLifecycle::Alive
        || snapshot.type_name != "minecraft:arrow"
        || !snapshot.on_ground
        || snapshot.velocity != Vec3::ZERO
    {
        return None;
    }
    if !collector_within_pickup_radius_locked(inner, collector_session, snapshot.position) {
        return None;
    }
    let snapshot = inner
        .entities
        .remove_if_current(snapshot)
        .map(server_entity_snapshot_from)?;
    clear_removed_entity_tracking_locked(inner, entity_id);
    let (collector_entity_id, recipients) =
        picked_entity_recipients_locked(inner, entity_id, collector_session);
    Some(ClaimedArrowParts {
        collector_entity_id,
        snapshot,
        recipients,
    })
}

fn into_claimed_arrow(entity_id: EntityId, parts: ClaimedArrowParts) -> Vec<VisibilityDispatch> {
    picked_entity_dispatches(
        entity_id,
        parts.collector_entity_id,
        1,
        parts.snapshot,
        parts.recipients,
    )
}

pub(super) fn item_pickup_ready_locked(
    inner: &SessionEntityGuards<'_>,
    entity_id: EntityId,
    lifecycle_tick: u64,
) -> bool {
    inner
        .entities
        .snapshot(entity_id)
        .and_then(|entity| entity.retained.item_pickup_ready_tick)
        .is_none_or(|ready_tick| lifecycle_tick >= ready_tick)
}

fn item_pickup_owner_blocked_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    collector_session: SessionId,
) -> bool {
    let Some(expected) = inner.entities.snapshot(entity_id) else {
        return false;
    };
    let Some(block) = expected.retained.item_pickup_owner_block else {
        return false;
    };
    if inner.entity_lifecycle_tick >= block.expires_tick {
        let mut next = expected.clone();
        next.retained.item_pickup_owner_block = None;
        let _ = inner.entities.replace_snapshot_if_current(expected, next);
        return false;
    }
    block.owner_session == collector_session
}

fn picked_entity_recipients_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    collector_session: SessionId,
) -> (i32, Vec<SessionRecipient>) {
    let collector_entity_id = inner
        .sessions
        .get(&collector_session)
        .map(|session| session.entity_id)
        .unwrap_or_default();
    let mut recipients = Vec::new();
    for (&observer_id, observer) in &mut inner.sessions {
        if observer.visible_entities.remove(&entity_id) {
            recipients.push(ordered_session_recipient(observer_id, observer));
            observer.visible_entities.publish();
        }
    }
    inner.entity_dispatches.take += recipients.len() as u64;
    inner.entity_dispatches.remove += recipients.len() as u64;
    (collector_entity_id, recipients)
}

fn picked_entity_dispatches(
    entity_id: EntityId,
    collector_entity_id: i32,
    amount: i32,
    snapshot: ServerEntitySnapshot,
    recipients: Vec<SessionRecipient>,
) -> Vec<VisibilityDispatch> {
    let mut dispatches = Vec::with_capacity(recipients.len() * 2);
    for recipient in recipients {
        dispatches.push(VisibilityDispatch {
            recipient: recipient.clone(),
            command: OutboundCommand::TakeItemEntity {
                item_entity_id: entity_id.0,
                player_entity_id: collector_entity_id,
                amount,
            },
        });
        dispatches.push(VisibilityDispatch {
            recipient,
            command: OutboundCommand::DespawnEntity(snapshot.clone()),
        });
    }
    dispatches
}

fn visibility_dispatches(
    recipients: Vec<SessionRecipient>,
    command: impl Fn() -> OutboundCommand,
) -> Vec<VisibilityDispatch> {
    recipients
        .into_iter()
        .map(|recipient| VisibilityDispatch {
            recipient,
            command: command(),
        })
        .collect()
}
