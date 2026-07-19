use super::entity_lifecycle::{
    clear_removed_entity_tracking_locked, nearby_entity_candidate_ids_locked,
    track_entity_chunk_locked,
};
use super::interaction_geometry::{distance_sq, entity_aabb};
use super::outbound::{OutboundCommand, SessionRecipient, VisibilityDispatch};
use super::visibility::ordered_session_recipient;
use super::{
    ServerEntitySnapshot, SessionEntityGuards, SessionId, SessionRegistry, SessionRegistryInner,
    initialize_entity_wire_state_locked, server_entity_snapshot_from, session_recipients,
    spawn_entity_visibility_from_snapshot_locked, spawn_entity_visibility_locked,
    spawned_xp_observer_ids, visible_entity_observers_locked,
};
use crate::play::GameMode;
use crate::play::campfire::PendingCampfireOutput;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::XpState;
use crate::play::simulation::SimulationAuthority;
use mc_entity::{EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot, SpawnEntity, Vec3};
use mc_protocol::packets::play::ItemStack;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

pub(in crate::play) const ENTITY_PICKUP_RADIUS: f64 = 2.25;
pub(in crate::play) const ITEM_PICKUP_DELAY_TICKS: u64 = 4;
const PLAYER_ITEM_OWNER_PICKUP_BLOCK_TICKS: u64 = 100;

#[derive(Debug, Clone, Copy)]
pub(super) struct ItemPickupOwnerBlock {
    pub(super) owner_session: SessionId,
    pub(super) expires_tick: u64,
}

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
                        .map(server_entity_snapshot_from)
                        .collect::<Vec<_>>();
                    (session_id, candidates)
                })
                .collect::<Vec<_>>()
        };
        let lifecycle_tick = self.simulation_tick();
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
                    .filter(|entity| {
                        if entity.item_stack.is_none() {
                            return true;
                        }
                        let item_is_blocked_for_session = inner
                            .item_pickup_owner_blocks
                            .get(&entity.id)
                            .is_some_and(|block| {
                                block.owner_session == session_id
                                    && lifecycle_tick < block.expires_tick
                            });
                        item_pickup_ready_locked(&inner, entity.id, lifecycle_tick)
                            && !item_is_blocked_for_session
                    })
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
        let candidates = outputs.iter().map(|output| {
            let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", drop_position);
            entity.uuid = Some(output.uuid);
            entity.velocity = item_drop_velocity(drop_position, &output.stack, lifecycle_tick);
            entity.item_stack = Some(output.stack.clone());
            entity
        });
        inner.entities.spawn_unique_authoritative_batch(candidates);

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
            inner
                .entity_spawn_ticks
                .entry(snapshot.id)
                .or_insert(lifecycle_tick);
            inner
                .item_spawn_ticks
                .entry(snapshot.id)
                .or_insert(lifecycle_tick);
            inner
                .entity_type_aabbs
                .entry(snapshot.type_id)
                .or_insert_with(|| entity_aabb("minecraft:item"));
            track_entity_chunk_locked(&mut inner, snapshot.id, snapshot.position);
            initialize_entity_wire_state_locked(&mut inner, snapshot.id);
            inner
                .item_pickup_ready_ticks
                .entry(snapshot.id)
                .or_insert(lifecycle_tick.saturating_add(ITEM_PICKUP_DELAY_TICKS));
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
        for snapshot in snapshots {
            dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                &mut inner,
                server_entity_snapshot_from(snapshot.clone()),
            ));
        }
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

    #[cfg(test)]
    pub(in crate::play) fn claim_item_pickup(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
        max_count: i32,
    ) -> Option<ClaimedPickup> {
        self.claim_item_pickup_owned(entity_id, collector_session, max_count)
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
            let stack = snapshot.item_stack?;
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
            };
            let (remaining, changed_slots) = inventory.merge_pickup_stack(probe, max_stack);
            let credited_count = stack.count.saturating_sub(remaining.count);
            if credited_count <= 0 || changed_slots.is_empty() {
                return None;
            }
            let parts =
                claim_item_pickup_locked(&mut inner, entity_id, collector_session, credited_count)?;
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
    pub(in crate::play) fn claim_item_pickup_legacy_for_test(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        max_count: i32,
    ) -> Option<ClaimedPickup> {
        self.claim_item_pickup_owned(entity_id, collector_session, max_count)
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
            claim_item_pickup_locked(&mut inner, entity_id, collector_session, max_count)?
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
            let (remaining, changed_slots) =
                inventory.merge_pickup_stack(ItemStack::new(arrow_item_id, 1), max_stack);
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
    pub(in crate::play) fn claim_arrow_pickup(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> Option<Vec<VisibilityDispatch>> {
        self.claim_arrow_pickup_owned(entity_id, collector_session)
    }

    #[cfg(test)]
    pub(in crate::play) fn claim_arrow_pickup_legacy_for_test(
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
    pub(in crate::play) fn claim_experience_pickup_legacy_for_test(
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
    inner.item_pickup_owner_blocks.insert(
        entity_id,
        ItemPickupOwnerBlock {
            owner_session,
            expires_tick,
        },
    );
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
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", position);
    entity.velocity = item_drop_velocity(position, &stack, inner.entity_lifecycle_tick);
    entity.item_stack = Some(stack);
    let id = if journal_commit {
        inner.entities.spawn_authoritative(entity)
    } else {
        inner.entities.spawn_authoritative_deferred_journal(entity)
    };
    let lifecycle_tick = inner.entity_lifecycle_tick;
    inner.entity_spawn_ticks.insert(id, lifecycle_tick);
    inner.item_spawn_ticks.insert(id, lifecycle_tick);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert_with(|| entity_aabb("minecraft:item"));
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    let ready_tick = inner
        .entity_lifecycle_tick
        .saturating_add(ITEM_PICKUP_DELAY_TICKS);
    inner.item_pickup_ready_ticks.insert(id, ready_tick);
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
    let id = inner.entities.spawn_authoritative(entity);
    let lifecycle_tick = inner.entity_lifecycle_tick;
    inner.entity_spawn_ticks.insert(id, lifecycle_tick);
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
    entity_id: EntityId,
    collector_session: SessionId,
    max_count: i32,
) -> Option<ClaimedPickupParts> {
    if max_count <= 0 {
        return None;
    }
    let snapshot = inner.entities.snapshot(entity_id)?;
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
        if !inner
            .entities
            .set_item_stack_if_current(snapshot, Some(stack.clone()))
        {
            return None;
        }
        let snapshot = inner.published_entity_snapshots.get_mut(&entity_id)?;
        snapshot.item_stack = Some(stack);
        let snapshot = snapshot.clone();
        let recipients =
            session_recipients(inner, visible_entity_observers_locked(inner, entity_id));
        inner.entity_dispatches.data += recipients.len() as u64;
        Some(ClaimedPickupParts {
            picked,
            update_entity: true,
            collector_entity_id: 0,
            snapshot,
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
    inner: &SessionRegistryInner,
    entity_id: EntityId,
    lifecycle_tick: u64,
) -> bool {
    inner
        .item_pickup_ready_ticks
        .get(&entity_id)
        .is_none_or(|ready_tick| lifecycle_tick >= *ready_tick)
}

fn item_pickup_owner_blocked_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    collector_session: SessionId,
) -> bool {
    let Some(block) = inner.item_pickup_owner_blocks.get(&entity_id).copied() else {
        return false;
    };
    if inner.entity_lifecycle_tick >= block.expires_tick {
        inner.item_pickup_owner_blocks.remove(&entity_id);
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
        if Arc::make_mut(&mut observer.visible_entities).remove(&entity_id) {
            recipients.push(ordered_session_recipient(observer_id, observer));
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
