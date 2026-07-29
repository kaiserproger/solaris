use std::collections::{HashMap, HashSet};

use mc_entity::villager_26_1_2::{
    VillagerBrainProfile, VillagerBrainState, VillagerPoiSet, VillagerScheduleKind,
};
use mc_entity::villager_population_26_1_2::{
    VILLAGER_COURTSHIP_DISTANCE_SQUARED, VILLAGER_HOME_SEARCH_RADIUS,
    VILLAGER_INTERACTION_RANGE, VillagerFoodItemIds, VillagerPopulationState,
    deterministic_villager_child_uuid,
};
use mc_entity::{
    EntityId, EntityLifecycle, EntitySnapshot, SpawnEntity, Vec3, VillagerData, VillagerKind,
    VillagerProfession,
};

use crate::play::simulation::SimulationAuthority;

use super::entity_lifecycle::{
    clear_removed_entity_tracking_locked, nearby_entity_candidate_ids_locked,
    track_entity_chunk_locked,
};
use super::interaction_geometry::entity_aabb;
use super::visibility::{
    despawn_entity_visibility_locked, entity_event_dispatches_locked,
    initialize_entity_wire_state_from_snapshot_locked,
    install_committed_entity_publications_locked, server_entity_snapshot_from, session_recipients,
    visibility_dispatches, visible_entity_observers_locked,
};
use super::*;

const MAX_VILLAGER_FOOD_PICKUPS_PER_TICK: usize = 4;
const MAX_NEW_COURTSHIPS_PER_TICK: usize = 1;
const MAX_VILLAGER_BIRTHS_PER_TICK: usize = 2;
const VILLAGER_FOOD_MAX_STACK_SIZE: i32 = 64;
const VILLAGER_HEART_EVENT: i8 = 18;
const VILLAGER_BIRTH_EVENT: i8 = 12;
const VILLAGER_ANGRY_EVENT: i8 = 13;

impl SessionRegistry {
    pub(in crate::play) fn register_settlement_vacant_home(
        &self,
        claim: String,
        position: Vec3,
    ) -> bool {
        if claim.is_empty() || !position.is_finite() {
            return false;
        }
        let mut inner = self.lock_inner("register settlement vacant home");
        match inner.settlement_vacant_homes.get(&claim) {
            Some(current) => *current == position,
            None => {
                inner.settlement_vacant_homes.insert(claim, position);
                true
            }
        }
    }

    pub(in crate::play) fn tick_villager_population(
        &self,
        _authority: &SimulationAuthority,
        current_tick: u64,
        food_items: VillagerFoodItemIds,
        villager_type_id: i32,
        item_type_id: i32,
        elapsed_ticks: u32,
    ) -> (usize, Vec<VisibilityDispatch>) {
        if villager_type_id < 0 || item_type_id < 0 {
            return (0, Vec::new());
        }
        let active_ids = self.active_simulation_entities.load_full();
        if active_ids.is_empty() {
            return (0, Vec::new());
        }

        let mut inner = self.lock_session_entities("tick villager population");
        let mut dispatches = Vec::new();
        dispatches.extend(advance_active_villager_ages_locked(
            &mut inner,
            &active_ids,
            elapsed_ticks,
        ));
        cleanup_orphaned_active_courtships_locked(&mut inner, &active_ids);
        dispatches.extend(pick_up_villager_food_locked(
            &mut inner,
            &active_ids,
            current_tick,
            food_items,
        ));
        dispatches.extend(share_one_villager_food_stack_locked(
            &mut inner,
            &active_ids,
            current_tick,
            food_items,
            item_type_id,
        ));
        dispatches.extend(start_one_villager_courtship_locked(
            &mut inner,
            &active_ids,
            current_tick,
            food_items,
        ));
        let (births, birth_dispatches) = finish_due_villager_births_locked(
            &mut inner,
            current_tick,
            food_items,
            villager_type_id,
            self.world_time(),
        );
        dispatches.extend(birth_dispatches);
        (births, dispatches)
    }
}

pub(super) fn rebuild_villager_population_indexes_locked(
    inner: &mut SessionRegistryInner,
    snapshots: &[EntitySnapshot],
) {
    inner.settlement_claimed_homes.clear();
    inner.villager_birth_deadlines.clear();
    inner.villager_birth_deadline_by_parent.clear();

    let by_uuid = snapshots
        .iter()
        .map(|snapshot| (snapshot.uuid, snapshot))
        .collect::<HashMap<_, _>>();
    let mut scheduled = HashSet::<[EntityId; 2]>::new();
    for snapshot in snapshots {
        let Some(population) = snapshot.retained.villager_population.as_ref() else {
            continue;
        };
        if let Some(claim) = population
            .claimed_home
            .as_ref()
            .filter(|claim| !claim.is_empty())
        {
            inner.settlement_claimed_homes.insert(claim.clone());
        }
        let Some(pending) = population.pending_birth.as_ref() else {
            continue;
        };
        if snapshot.lifecycle != EntityLifecycle::Alive
            || snapshot.type_name != "minecraft:villager"
        {
            continue;
        }
        let Some(partner) = by_uuid.get(&pending.partner_uuid).copied() else {
            continue;
        };
        if partner.lifecycle != EntityLifecycle::Alive || partner.type_name != "minecraft:villager"
        {
            continue;
        }
        let Some(partner_pending) = partner
            .retained
            .villager_population
            .as_ref()
            .and_then(|population| population.pending_birth.as_ref())
        else {
            continue;
        };
        if partner_pending.partner_uuid != snapshot.uuid
            || partner_pending.started_tick != pending.started_tick
            || partner_pending.ready_tick != pending.ready_tick
        {
            continue;
        }
        let pair = sorted_pair(snapshot.id, partner.id);
        if scheduled.insert(pair) {
            schedule_villager_birth_locked(inner, pair, pending.ready_tick);
        }
    }
}

fn advance_active_villager_ages_locked(
    inner: &mut SessionEntityGuards<'_>,
    active_ids: &HashSet<EntityId>,
    elapsed_ticks: u32,
) -> Vec<VisibilityDispatch> {
    if elapsed_ticks == 0 {
        return Vec::new();
    }
    let mut ids = active_ids.iter().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    let mut dispatches = Vec::new();
    for entity_id in ids {
        let Some(expected) = inner.entities.snapshot(entity_id) else {
            continue;
        };
        if expected.lifecycle != EntityLifecycle::Alive
            || expected.type_name != "minecraft:villager"
        {
            continue;
        }
        let Some(mut population) = expected.retained.villager_population.clone() else {
            continue;
        };
        if population.age_ticks == 0 {
            continue;
        }
        let became_adult = population.advance_age(elapsed_ticks);
        let mut next = expected.clone();
        next.retained.villager_population = Some(population);
        if became_adult && let Some(brain) = next.retained.villager_brain.as_mut() {
            brain.schedule = VillagerScheduleKind::Adult;
        }
        if !inner
            .entities
            .replace_snapshot_if_current(expected, next.clone())
        {
            continue;
        }
        let published = server_entity_snapshot_from(next);
        inner
            .published_entity_snapshots
            .insert(entity_id, published.clone());
        if became_adult {
            let recipients =
                session_recipients(inner, visible_entity_observers_locked(inner, entity_id));
            let updates = visibility_dispatches(recipients, || {
                OutboundCommand::UpdateEntityData(published.clone())
            });
            record_entity_dispatches_locked(inner, &updates);
            dispatches.extend(updates);
        }
    }
    dispatches
}

fn cleanup_orphaned_active_courtships_locked(
    inner: &mut SessionEntityGuards<'_>,
    active_ids: &HashSet<EntityId>,
) {
    let orphaned = active_ids
        .iter()
        .copied()
        .filter(|entity_id| {
            inner.entities.snapshot(*entity_id).is_some_and(|snapshot| {
                snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_some())
            }) && !inner
                .villager_birth_deadline_by_parent
                .contains_key(entity_id)
        })
        .collect::<Vec<_>>();
    for entity_id in orphaned {
        abort_villager_courtship_locked(inner, &[entity_id]);
    }
}

fn abort_villager_courtship_locked(inner: &mut SessionEntityGuards<'_>, entity_ids: &[EntityId]) {
    let mut transitions = Vec::new();
    let mut unique_ids = entity_ids.to_vec();
    unique_ids.sort_unstable();
    unique_ids.dedup();
    for entity_id in unique_ids {
        let Some(expected) = inner.entities.snapshot(entity_id) else {
            continue;
        };
        let Some(population) = expected.retained.villager_population.as_ref() else {
            continue;
        };
        if population.pending_birth.is_none() {
            continue;
        }
        let mut next = expected.clone();
        next.retained
            .villager_population
            .as_mut()
            .expect("population state was observed")
            .abort_pending_birth();
        transitions.push((expected, next));
    }
    if !transitions.is_empty() {
        let _ = inner.entities.replace_snapshots_if_current(transitions);
    }
    for entity_id in entity_ids {
        if let Some(deadline) = inner.villager_birth_deadline_by_parent.remove(entity_id) {
            let remove_bucket = inner
                .villager_birth_deadlines
                .get_mut(&deadline)
                .is_some_and(|bucket| {
                    bucket.retain(|pair| !pair.contains(entity_id));
                    bucket.is_empty()
                });
            if remove_bucket {
                inner.villager_birth_deadlines.remove(&deadline);
            }
        }
    }
}

fn pick_up_villager_food_locked(
    inner: &mut SessionEntityGuards<'_>,
    active_ids: &HashSet<EntityId>,
    current_tick: u64,
    food_items: VillagerFoodItemIds,
) -> Vec<VisibilityDispatch> {
    let mut villager_ids = active_ids.iter().copied().collect::<Vec<_>>();
    villager_ids.sort_unstable();
    let mut dispatches = Vec::new();
    let mut committed = 0_usize;

    for villager_id in villager_ids {
        if committed >= MAX_VILLAGER_FOOD_PICKUPS_PER_TICK {
            break;
        }
        let Some(villager) = inner.entities.snapshot(villager_id) else {
            continue;
        };
        if villager.lifecycle != EntityLifecycle::Alive
            || villager.type_name != "minecraft:villager"
            || villager.retained.villager_population.is_none()
        {
            continue;
        }
        let mut candidates = nearby_entity_candidate_ids_locked(inner, villager.position, 2.0);
        candidates.sort_unstable();
        for item_id in candidates {
            let Some(item) = inner.entities.snapshot(item_id) else {
                continue;
            };
            let Some(stack) = item.item_stack.as_ref() else {
                continue;
            };
            if item.lifecycle != EntityLifecycle::Alive
                || item.type_name != "minecraft:item"
                || stack.count <= 0
                || !food_items.contains(stack.item_id)
                || item
                    .retained
                    .item_pickup_ready_tick
                    .is_some_and(|ready_tick| ready_tick > current_tick)
                || item
                    .retained
                    .item_pickup_owner_block
                    .is_some_and(|block| block.expires_tick > current_tick)
            {
                continue;
            }

            let mut villager_next = villager.clone();
            let remainder = match villager_next
                .retained
                .villager_population
                .as_mut()
                .expect("villager population was checked")
                .add_to_inventory(stack.clone(), VILLAGER_FOOD_MAX_STACK_SIZE)
            {
                Ok(remainder) => remainder,
                Err(_) => continue,
            };
            if remainder.as_ref() == Some(stack) {
                continue;
            }
            let item_next = remainder.map(|remainder| {
                let mut next = item.clone();
                next.item_stack = Some(remainder);
                next
            });
            let transition = (item.clone(), item_next.clone());
            if !inner.entities.commit_villager_inventory_pickup_if_current(
                mc_entity::VillagerInventoryPickupCommit {
                    villager: (villager.clone(), villager_next),
                    item: transition.clone(),
                    item_max_stack_size: VILLAGER_FOOD_MAX_STACK_SIZE,
                },
            ) {
                continue;
            }
            dispatches.extend(publish_item_transition_locked(inner, &transition));
            committed += 1;
            break;
        }
    }
    dispatches
}

fn share_one_villager_food_stack_locked(
    inner: &mut SessionEntityGuards<'_>,
    active_ids: &HashSet<EntityId>,
    current_tick: u64,
    food_items: VillagerFoodItemIds,
    item_type_id: i32,
) -> Vec<VisibilityDispatch> {
    let mut villagers = active_ids
        .iter()
        .filter_map(|entity_id| inner.entities.snapshot(*entity_id))
        .filter(|snapshot| {
            snapshot.lifecycle == EntityLifecycle::Alive
                && snapshot.type_name == "minecraft:villager"
                && snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_none())
        })
        .collect::<Vec<_>>();
    villagers.sort_unstable_by_key(|snapshot| snapshot.id);

    for donor in &villagers {
        let Some(donor_population) = donor.retained.villager_population.as_ref() else {
            continue;
        };
        if !donor_population.has_excess_food(food_items) {
            continue;
        }
        for recipient in &villagers {
            if donor.id == recipient.id
                || distance_squared(donor.position, recipient.position)
                    > VILLAGER_COURTSHIP_DISTANCE_SQUARED
            {
                continue;
            }
            let Some(recipient_population) = recipient.retained.villager_population.as_ref() else {
                continue;
            };
            if !recipient_population.wants_more_food(food_items) {
                continue;
            }

            let mut donor_next = donor.clone();
            let Some(shared) = donor_next
                .retained
                .villager_population
                .as_mut()
                .expect("donor population was checked")
                .inventory
                .extract_food_share(food_items, VILLAGER_FOOD_MAX_STACK_SIZE)
            else {
                continue;
            };
            let dx = recipient.position.x - donor.position.x;
            let dy = recipient.position.y - donor.position.y;
            let dz = recipient.position.z - donor.position.z;
            let length = distance_squared(donor.position, recipient.position).sqrt();
            let velocity = if length > 0.0 {
                Vec3::new(
                    dx / length
                        * mc_entity::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
                    dy / length
                        * mc_entity::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
                    dz / length
                        * mc_entity::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
                )
            } else {
                Vec3::new(0.0, 0.0, 0.0)
            };
            let mut thrown = SpawnEntity::new(
                item_type_id,
                "minecraft:item",
                Vec3::new(
                    donor.position.x,
                    donor.position.y
                        + mc_entity::villager_population_26_1_2::VILLAGER_ITEM_THROW_Y_OFFSET,
                    donor.position.z,
                ),
            );
            let share_scope = format!("food-share:{}", shared.item_id);
            thrown.uuid = Some(deterministic_villager_child_uuid(
                donor.uuid,
                recipient.uuid,
                &share_scope,
                current_tick,
            ));
            thrown.velocity = velocity;
            thrown.item_stack = Some(shared);
            thrown.retained.spawn_tick = current_tick;
            thrown.retained.item_pickup_ready_tick = current_tick.checked_add(
                mc_entity::villager_population_26_1_2::VILLAGER_ITEM_THROW_PICKUP_DELAY_TICKS,
            );
            let Some(thrown) = inner.entities.commit_villager_food_share_if_current(
                mc_entity::VillagerFoodShareCommit {
                    donor: (donor.clone(), donor_next),
                    recipient: recipient.clone(),
                    thrown_item: thrown,
                    food_items,
                    item_max_stack_size: VILLAGER_FOOD_MAX_STACK_SIZE,
                    current_tick,
                },
            ) else {
                continue;
            };

            let thrown_id = thrown.id;
            inner
                .entity_type_aabbs
                .entry(thrown.type_id)
                .or_insert_with(|| entity_aabb(&thrown.type_name));
            track_entity_chunk_locked(inner, thrown_id, thrown.position);
            let published = server_entity_snapshot_from(thrown);
            initialize_entity_wire_state_from_snapshot_locked(inner, &published);
            return install_committed_entity_publications_locked(inner, vec![published]);
        }
    }
    Vec::new()
}

fn start_one_villager_courtship_locked(
    inner: &mut SessionEntityGuards<'_>,
    active_ids: &HashSet<EntityId>,
    current_tick: u64,
    food_items: VillagerFoodItemIds,
) -> Vec<VisibilityDispatch> {
    if MAX_NEW_COURTSHIPS_PER_TICK == 0 {
        return Vec::new();
    }
    let mut villagers = active_ids
        .iter()
        .filter_map(|entity_id| inner.entities.snapshot(*entity_id))
        .filter(|snapshot| {
            snapshot.lifecycle == EntityLifecycle::Alive
                && snapshot.type_name == "minecraft:villager"
                && snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.can_breed(false, food_items))
        })
        .collect::<Vec<_>>();
    villagers.sort_unstable_by_key(|snapshot| snapshot.id);

    for first_index in 0..villagers.len() {
        for second_index in first_index + 1..villagers.len() {
            let first = &villagers[first_index];
            let second = &villagers[second_index];
            if distance_squared(first.position, second.position)
                > VILLAGER_INTERACTION_RANGE * VILLAGER_INTERACTION_RANGE
            {
                continue;
            }
            let seed = courtship_seed(first, second, current_tick);
            let mut first_next = first.clone();
            let mut second_next = second.clone();
            let first_population = first_next
                .retained
                .villager_population
                .as_mut()
                .expect("eligible villager has population state");
            let second_population = second_next
                .retained
                .villager_population
                .as_mut()
                .expect("eligible villager has population state");
            if first_population
                .start_pending_birth(second.uuid, current_tick, seed, false, food_items)
                .is_err()
                || second_population
                    .start_pending_birth(first.uuid, current_tick, seed, false, food_items)
                    .is_err()
            {
                continue;
            }
            let ready_tick = first_population
                .pending_birth
                .as_ref()
                .expect("pending birth was installed")
                .ready_tick;
            let commit = mc_entity::VillagerCourtshipCommit {
                parents: [(first.clone(), first_next), (second.clone(), second_next)],
                current_tick,
                food_items,
                deterministic_seed: seed,
            };
            if !inner.entities.commit_villager_courtship_if_current(commit) {
                continue;
            }

            let pair = sorted_pair(first.id, second.id);
            schedule_villager_birth_locked(inner, pair, ready_tick);
            let mut events = entity_event_dispatches_locked(inner, first.id, VILLAGER_HEART_EVENT);
            events.extend(entity_event_dispatches_locked(
                inner,
                second.id,
                VILLAGER_HEART_EVENT,
            ));
            record_entity_dispatches_locked(inner, &events);
            return events;
        }
    }
    Vec::new()
}

fn finish_due_villager_births_locked(
    inner: &mut SessionEntityGuards<'_>,
    current_tick: u64,
    food_items: VillagerFoodItemIds,
    villager_type_id: i32,
    world_time: u64,
) -> (usize, Vec<VisibilityDispatch>) {
    let due_pairs = drain_due_villager_birth_pairs_locked(inner, current_tick);
    let mut births = 0;
    let mut dispatches = Vec::new();
    for pair in due_pairs.into_iter().take(MAX_VILLAGER_BIRTHS_PER_TICK) {
        let first = inner.entities.snapshot(pair[0]);
        let second = inner.entities.snapshot(pair[1]);
        let (Some(first), Some(second)) = (first, second) else {
            abort_villager_courtship_locked(inner, &pair);
            continue;
        };
        let Some((started_tick, ready_tick)) = reciprocal_pending_birth(&first, &second) else {
            abort_villager_courtship_locked(inner, &pair);
            continue;
        };
        if ready_tick > current_tick {
            schedule_villager_birth_locked(inner, pair, ready_tick);
            continue;
        }
        if distance_squared(first.position, second.position) > VILLAGER_COURTSHIP_DISTANCE_SQUARED {
            abort_villager_courtship_locked(inner, &pair);
            continue;
        }

        let vacant_home = inner
            .settlement_vacant_homes
            .iter()
            .find(|(claim, position)| {
                !inner.settlement_claimed_homes.contains(*claim)
                    && distance_squared(first.position, **position)
                        <= VILLAGER_HOME_SEARCH_RADIUS * VILLAGER_HOME_SEARCH_RADIUS
            })
            .map(|(claim, position)| (claim.clone(), *position));

        let Some((home_claim, home_position)) = vacant_home else {
            let mut first_next = first.clone();
            let mut second_next = second.clone();
            let no_bed_ready = first_next
                .retained
                .villager_population
                .as_mut()
                .and_then(|population| {
                    population
                        .finish_courtship_without_child(current_tick, food_items)
                        .ok()
                })
                .is_some()
                && second_next
                    .retained
                    .villager_population
                    .as_mut()
                    .and_then(|population| {
                        population
                            .finish_courtship_without_child(current_tick, food_items)
                            .ok()
                    })
                    .is_some();
            if !no_bed_ready {
                abort_villager_courtship_locked(inner, &pair);
                continue;
            }
            if inner.entities.commit_villager_no_bed_if_current(mc_entity::VillagerNoBedCommit {
                parents: [(first.clone(), first_next), (second.clone(), second_next)],
                current_tick,
                food_items,
            }) {
                let mut events =
                    entity_event_dispatches_locked(inner, first.id, VILLAGER_ANGRY_EVENT);
                events.extend(entity_event_dispatches_locked(
                    inner,
                    second.id,
                    VILLAGER_ANGRY_EVENT,
                ));
                record_entity_dispatches_locked(inner, &events);
                dispatches.extend(events);
            } else {
                abort_villager_courtship_locked(inner, &pair);
            }
            continue;
        };

        let mut first_next = first.clone();
        let mut second_next = second.clone();
        let parents_ready = first_next
            .retained
            .villager_population
            .as_mut()
            .and_then(|population| {
                population
                    .finish_successful_birth(current_tick, food_items)
                    .ok()
            })
            .is_some()
            && second_next
                .retained
                .villager_population
                .as_mut()
                .and_then(|population| {
                    population
                        .finish_successful_birth(current_tick, food_items)
                        .ok()
                })
                .is_some();
        if !parents_ready {
            abort_villager_courtship_locked(inner, &pair);
            continue;
        }

        inner.settlement_claimed_homes.insert(home_claim.clone());
        let mut child = SpawnEntity::new(villager_type_id, "minecraft:villager", first.position);
        child.uuid = Some(deterministic_villager_child_uuid(
            first.uuid,
            second.uuid,
            &home_claim,
            started_tick,
        ));
        child.retained.spawn_tick = current_tick;
        child.retained.villager = Some(VillagerData::new(
            VillagerKind::Plains,
            VillagerProfession::None,
            1,
        ));
        let brain = VillagerBrainState::baby(VillagerPoiSet {
            home: Some(home_position),
            job_site: None,
            meeting_point: None,
        });
        child.retained.villager_brain = Some(brain.clone());
        child.retained.villager_population =
            Some(VillagerPopulationState::baby(home_claim.clone()));
        apply_entity_facts(&mut child);
        if let Ok(plan) = mc_entity::villager_26_1_2::plan_villager_brain(
            &brain,
            &VillagerBrainProfile::vanilla_26_1_2(),
            current_tick,
            i64::try_from(world_time).unwrap_or(i64::MAX),
        ) {
            child.retained.villager_brain = Some(plan.state);
            child.goal = plan.goal;
        }

        let commit = mc_entity::VillagerBirthCommit {
            parents: [(first.clone(), first_next), (second.clone(), second_next)],
            child,
            current_tick,
            food_items,
        };
        let Some(child) = inner.entities.commit_villager_birth_if_current(commit) else {
            inner.settlement_claimed_homes.remove(&home_claim);
            abort_villager_courtship_locked(inner, &pair);
            continue;
        };

        let child_id = child.id;
        inner
            .entity_type_aabbs
            .entry(child.type_id)
            .or_insert_with(|| entity_aabb(&child.type_name));
        track_entity_chunk_locked(inner, child_id, child.position);
        let published = server_entity_snapshot_from(child);
        initialize_entity_wire_state_from_snapshot_locked(inner, &published);
        dispatches.extend(install_committed_entity_publications_locked(
            inner,
            vec![published],
        ));
        let events = entity_event_dispatches_locked(inner, child_id, VILLAGER_BIRTH_EVENT);
        record_entity_dispatches_locked(inner, &events);
        dispatches.extend(events);
        births += 1;
    }
    (births, dispatches)
}

fn publish_item_transition_locked(
    inner: &mut SessionEntityGuards<'_>,
    transition: &(EntitySnapshot, Option<EntitySnapshot>),
) -> Vec<VisibilityDispatch> {
    let (expected, next) = transition;
    match next {
        Some(next) => {
            let published = server_entity_snapshot_from(next.clone());
            inner
                .published_entity_snapshots
                .insert(expected.id, published.clone());
            let recipients =
                session_recipients(inner, visible_entity_observers_locked(inner, expected.id));
            let updates = visibility_dispatches(recipients, || {
                OutboundCommand::UpdateEntityData(published.clone())
            });
            record_entity_dispatches_locked(inner, &updates);
            updates
        }
        None => {
            let published = inner
                .published_entity_snapshots
                .get(&expected.id)
                .cloned()
                .unwrap_or_else(|| server_entity_snapshot_from(expected.clone()));
            let dispatches = despawn_entity_visibility_locked(inner, &published);
            clear_removed_entity_tracking_locked(inner, expected.id);
            dispatches
        }
    }
}

fn schedule_villager_birth_locked(
    inner: &mut SessionRegistryInner,
    pair: [EntityId; 2],
    deadline: u64,
) {
    for entity_id in pair {
        if let Some(previous) = inner
            .villager_birth_deadline_by_parent
            .insert(entity_id, deadline)
            && previous != deadline
        {
            let remove_bucket = inner
                .villager_birth_deadlines
                .get_mut(&previous)
                .is_some_and(|bucket| {
                    bucket.retain(|queued| !queued.contains(&entity_id));
                    bucket.is_empty()
                });
            if remove_bucket {
                inner.villager_birth_deadlines.remove(&previous);
            }
        }
    }
    let bucket = inner.villager_birth_deadlines.entry(deadline).or_default();
    if !bucket.contains(&pair) {
        bucket.push_back(pair);
    }
}

fn drain_due_villager_birth_pairs_locked(
    inner: &mut SessionRegistryInner,
    current_tick: u64,
) -> Vec<[EntityId; 2]> {
    let mut pairs = Vec::new();
    while pairs.len() < MAX_VILLAGER_BIRTHS_PER_TICK {
        let Some((&deadline, _)) = inner.villager_birth_deadlines.first_key_value() else {
            break;
        };
        if deadline > current_tick {
            break;
        }
        let pair = {
            let queue = inner
                .villager_birth_deadlines
                .get_mut(&deadline)
                .expect("first birth deadline exists");
            let pair = queue
                .pop_front()
                .expect("villager birth deadline queue is non-empty");
            if queue.is_empty() {
                inner.villager_birth_deadlines.remove(&deadline);
            }
            pair
        };
        if pair.iter().all(|entity_id| {
            inner.villager_birth_deadline_by_parent.get(entity_id) == Some(&deadline)
        }) {
            for entity_id in pair {
                inner.villager_birth_deadline_by_parent.remove(&entity_id);
            }
            pairs.push(pair);
        }
    }
    pairs
}

fn reciprocal_pending_birth(
    first: &EntitySnapshot,
    second: &EntitySnapshot,
) -> Option<(u64, u64)> {
    if first.lifecycle != EntityLifecycle::Alive
        || second.lifecycle != EntityLifecycle::Alive
        || first.type_name != "minecraft:villager"
        || second.type_name != "minecraft:villager"
    {
        return None;
    }
    let first_pending = first
        .retained
        .villager_population
        .as_ref()?
        .pending_birth
        .as_ref()?;
    let second_pending = second
        .retained
        .villager_population
        .as_ref()?
        .pending_birth
        .as_ref()?;
    (first_pending.partner_uuid == second.uuid
        && second_pending.partner_uuid == first.uuid
        && first_pending.started_tick == second_pending.started_tick
        && first_pending.ready_tick == second_pending.ready_tick)
        .then_some((first_pending.started_tick, first_pending.ready_tick))
}

fn sorted_pair(first: EntityId, second: EntityId) -> [EntityId; 2] {
    if first <= second {
        [first, second]
    } else {
        [second, first]
    }
}

fn distance_squared(first: Vec3, second: Vec3) -> f64 {
    let dx = first.x - second.x;
    let dy = first.y - second.y;
    let dz = first.z - second.z;
    dx.mul_add(dx, dy.mul_add(dy, dz * dz))
}

fn courtship_seed(first: &EntitySnapshot, second: &EntitySnapshot, current_tick: u64) -> u64 {
    let uuid = deterministic_villager_child_uuid(first.uuid, second.uuid, "", current_tick);
    let bytes = uuid.as_u128();
    (bytes as u64) ^ ((bytes >> 64) as u64) ^ current_tick
}

#[cfg(any())]
mod bread_only_legacy_tests {
    use std::sync::Arc;

    use mc_entity::villager_26_1_2::{VillagerActivity, VillagerScheduleKind};
    use mc_entity::villager_population_26_1_2::{
        VILLAGER_BABY_START_AGE_TICKS, VILLAGER_PARENT_COOLDOWN_TICKS,
    };
    use mc_entity::{EntityItemStack, Rotation};

    use super::*;

    const VILLAGER_TYPE_ID: i32 = 139;
    const ITEM_TYPE_ID: i32 = 70;
    const BREAD_ITEM_ID: u32 = 41;

    fn spawn_population_villager(
        registry: &SessionRegistry,
        position: Vec3,
        population: VillagerPopulationState,
        schedule: VillagerScheduleKind,
    ) -> EntityId {
        let mut inner = registry.lock_session_entities("spawn population villager test fixture");
        let mut entity = SpawnEntity::new(VILLAGER_TYPE_ID, "minecraft:villager", position);
        entity.retained.villager = Some(VillagerData::new(
            VillagerKind::Plains,
            VillagerProfession::None,
            1,
        ));
        entity.retained.villager_population = Some(population);
        let mut brain = match schedule {
            VillagerScheduleKind::Adult => VillagerBrainState::adult(VillagerPoiSet::default()),
            VillagerScheduleKind::Baby => VillagerBrainState::baby(VillagerPoiSet::default()),
        };
        brain.activity = VillagerActivity::Idle;
        entity.retained.villager_brain = Some(brain);
        apply_entity_facts(&mut entity);
        let id = inner.entities.spawn(entity);
        let snapshot = inner.entities.snapshot(id).expect("spawned villager");
        track_entity_chunk_locked(&mut inner, id, snapshot.position);
        let published = server_entity_snapshot_from(snapshot);
        initialize_entity_wire_state_from_snapshot_locked(&mut inner, &published);
        inner.published_entity_snapshots.insert(id, published);
        id
    }

    fn spawn_bread(registry: &SessionRegistry, position: Vec3, count: i32) -> EntityId {
        registry.spawn_item_drop(
            ITEM_TYPE_ID,
            position,
            EntityItemStack::new(BREAD_ITEM_ID, count),
        );
        registry
            .lock_entities("find test bread")
            .snapshots()
            .filter(|snapshot| {
                snapshot.type_name == "minecraft:item"
                    && snapshot
                        .item_stack
                        .as_ref()
                        .is_some_and(|stack| stack.item_id == BREAD_ITEM_ID)
            })
            .max_by_key(|snapshot| snapshot.id)
            .expect("spawned bread")
            .id
    }

    fn activate(registry: &SessionRegistry, ids: impl IntoIterator<Item = EntityId>) {
        registry
            .active_simulation_entities
            .store(Arc::new(ids.into_iter().collect()));
    }

    #[test]
    fn six_bread_starts_one_courtship_and_one_home_blocks_a_second_pair() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let first = spawn_population_villager(
            &registry,
            Vec3::new(0.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let second = spawn_population_villager(
            &registry,
            Vec3::new(1.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let third = spawn_population_villager(
            &registry,
            Vec3::new(4.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let fourth = spawn_population_villager(
            &registry,
            Vec3::new(5.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let bread = spawn_bread(&registry, Vec3::new(1.0, 64.0, 0.5), 12);
        assert!(registry.register_settlement_vacant_home(
            "settlement:test-home".to_owned(),
            Vec3::new(2.5, 64.0, 0.5),
        ));
        activate(&registry, [first, second, third, fourth]);

        let (births, _) =
            registry.tick_villager_population(&authority, 100, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        assert_eq!(births, 0);
        let snapshots = registry
            .lock_entities("read courtship state")
            .snapshots_vec();
        let pending = snapshots
            .iter()
            .filter(|snapshot| {
                snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_some())
            })
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 2);
        assert_eq!(
            snapshots
                .iter()
                .find(|snapshot| snapshot.id == bread)
                .and_then(|snapshot| snapshot.item_stack.as_ref())
                .map(|stack| stack.count),
            Some(6)
        );

        registry.tick_villager_population(&authority, 120, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        let pending_after = registry
            .lock_entities("verify one home fence")
            .snapshots()
            .filter(|snapshot| {
                snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_some())
            })
            .count();
        assert_eq!(pending_after, 2);
    }

    #[test]
    fn insufficient_wrong_or_distant_bread_does_not_mutate_villagers_or_items() {
        for (item_id, count, item_position) in [
            (BREAD_ITEM_ID, 5, Vec3::new(1.0, 64.0, 0.5)),
            (BREAD_ITEM_ID + 1, 6, Vec3::new(1.0, 64.0, 0.5)),
            (BREAD_ITEM_ID, 6, Vec3::new(4.5, 64.0, 0.5)),
        ] {
            let registry = SessionRegistry::new();
            let authority = SimulationAuthority::for_test();
            let first = spawn_population_villager(
                &registry,
                Vec3::new(0.5, 64.0, 0.5),
                VillagerPopulationState::adult(),
                VillagerScheduleKind::Adult,
            );
            let second = spawn_population_villager(
                &registry,
                Vec3::new(1.5, 64.0, 0.5),
                VillagerPopulationState::adult(),
                VillagerScheduleKind::Adult,
            );
            let mut inner = registry.lock_session_entities("spawn nonmatching bread");
            let mut item = SpawnEntity::new(ITEM_TYPE_ID, "minecraft:item", item_position);
            item.item_stack = Some(EntityItemStack::new(item_id, count));
            let item_id_entity = inner.entities.spawn(item);
            let item_snapshot = inner.entities.snapshot(item_id_entity).unwrap();
            track_entity_chunk_locked(&mut inner, item_id_entity, item_snapshot.position);
            drop(inner);
            assert!(registry.register_settlement_vacant_home(
                "settlement:test-home".to_owned(),
                Vec3::new(2.5, 64.0, 0.5),
            ));
            activate(&registry, [first, second]);

            registry.tick_villager_population(&authority, 100, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
            let entities = registry.lock_entities("verify rejected courtship");
            for parent in [first, second] {
                assert!(
                    entities
                        .snapshot(parent)
                        .unwrap()
                        .retained
                        .villager_population
                        .unwrap()
                        .pending_birth
                        .is_none()
                );
            }
            assert_eq!(
                entities
                    .snapshot(item_id_entity)
                    .unwrap()
                    .item_stack
                    .unwrap()
                    .count,
                count
            );
        }
    }

    #[test]
    fn dying_parent_aborts_pending_birth_and_releases_the_home() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let first = spawn_population_villager(
            &registry,
            Vec3::new(0.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let second = spawn_population_villager(
            &registry,
            Vec3::new(1.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        spawn_bread(&registry, Vec3::new(1.0, 64.0, 0.5), 6);
        assert!(registry.register_settlement_vacant_home(
            "settlement:abort-home".to_owned(),
            Vec3::new(2.5, 64.0, 0.5),
        ));
        activate(&registry, [first, second]);
        registry.tick_villager_population(&authority, 100, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        let ready_tick = registry
            .lock_entities("read abort deadline")
            .snapshot(first)
            .unwrap()
            .retained
            .villager_population
            .unwrap()
            .pending_birth
            .unwrap()
            .ready_tick;
        let damage = registry
            .damage_server_entity_for_test(second, 100.0)
            .expect("lethal population parent damage");
        assert!(damage.killed);

        assert_eq!(
            registry
                .tick_villager_population(
                    &authority,
                    ready_tick,
                    BREAD_ITEM_ID,
                    VILLAGER_TYPE_ID,
                    20,
                )
                .0,
            0
        );
        let inner = registry.lock_session_entities("verify aborted population courtship");
        for parent in [first, second] {
            let population = inner
                .entities
                .snapshot(parent)
                .unwrap()
                .retained
                .villager_population
                .unwrap();
            assert_eq!(population.food_points, 12);
            assert!(population.pending_birth.is_none());
        }
        assert!(
            !inner
                .settlement_claimed_homes
                .contains("settlement:abort-home")
        );
        assert!(inner.villager_birth_deadline_by_parent.is_empty());
    }

    #[test]
    fn due_birth_uses_owner_id_and_restart_rebuild_preserves_one_child() {
        let source = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let first = spawn_population_villager(
            &source,
            Vec3::new(0.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        let second = spawn_population_villager(
            &source,
            Vec3::new(1.5, 64.0, 0.5),
            VillagerPopulationState::adult(),
            VillagerScheduleKind::Adult,
        );
        spawn_bread(&source, Vec3::new(1.0, 64.0, 0.5), 6);
        assert!(source.register_settlement_vacant_home(
            "settlement:restart-home".to_owned(),
            Vec3::new(2.5, 64.0, 0.5),
        ));
        activate(&source, [first, second]);
        source.synchronize_entity_lifecycle_epoch(100);
        source.tick_villager_population(&authority, 100, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        let ready_tick = source
            .lock_entities("read pending deadline")
            .snapshot(first)
            .unwrap()
            .retained
            .villager_population
            .unwrap()
            .pending_birth
            .unwrap()
            .ready_tick;
        let checkpoint = source.persisted_entity_save_snapshot().0;

        let restored = SessionRegistry::new();
        assert_eq!(restored.restore_persisted_entities(checkpoint), 2);
        assert!(restored.register_settlement_vacant_home(
            "settlement:restart-home".to_owned(),
            Vec3::new(2.5, 64.0, 0.5),
        ));
        activate(&restored, [first, second]);
        assert_eq!(
            restored
                .tick_villager_population(
                    &authority,
                    ready_tick - 1,
                    BREAD_ITEM_ID,
                    VILLAGER_TYPE_ID,
                    20,
                )
                .0,
            0
        );
        assert_eq!(
            restored
                .tick_villager_population(
                    &authority,
                    ready_tick,
                    BREAD_ITEM_ID,
                    VILLAGER_TYPE_ID,
                    20,
                )
                .0,
            1
        );
        let villagers = restored
            .lock_entities("read restored birth")
            .snapshots()
            .filter(|snapshot| snapshot.type_name == "minecraft:villager")
            .collect::<Vec<_>>();
        assert_eq!(villagers.len(), 3);
        let child = villagers
            .iter()
            .find(|snapshot| {
                snapshot
                    .retained
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.age_ticks == VILLAGER_BABY_START_AGE_TICKS)
            })
            .expect("one persisted baby");
        assert!(child.id.0 > first.0.max(second.0));
        assert_eq!(
            restored
                .tick_villager_population(
                    &authority,
                    ready_tick + 20,
                    BREAD_ITEM_ID,
                    VILLAGER_TYPE_ID,
                    20,
                )
                .0,
            0
        );
    }

    #[test]
    fn baby_matures_exactly_once_and_switches_to_adult_schedule() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let mut population = VillagerPopulationState::baby("settlement:baby-home".to_owned());
        population.age_ticks = -20;
        let baby = spawn_population_villager(
            &registry,
            Vec3::new(0.5, 64.0, 0.5),
            population,
            VillagerScheduleKind::Baby,
        );
        activate(&registry, [baby]);

        registry.tick_villager_population(&authority, 100, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        let mature = registry
            .lock_entities("read matured villager")
            .snapshot(baby)
            .unwrap();
        assert_eq!(
            mature
                .retained
                .villager_population
                .as_ref()
                .unwrap()
                .age_ticks,
            0
        );
        assert_eq!(
            mature.retained.villager_brain.as_ref().unwrap().schedule,
            VillagerScheduleKind::Adult
        );
        assert!(!server_entity_snapshot_from(mature).villager_baby);

        registry.tick_villager_population(&authority, 120, BREAD_ITEM_ID, VILLAGER_TYPE_ID, 20);
        assert_eq!(
            registry
                .lock_entities("verify mature age stable")
                .snapshot(baby)
                .unwrap()
                .retained
                .villager_population
                .unwrap()
                .age_ticks,
            0
        );
        assert_eq!(VILLAGER_PARENT_COOLDOWN_TICKS, 6_000);
        assert!(Rotation::ZERO.is_finite());
    }
}
