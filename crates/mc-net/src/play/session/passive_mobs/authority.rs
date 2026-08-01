use super::super::entity_lifecycle::{
    track_entity_chunk_locked, update_breeding_tick_tracking_locked,
};
use super::super::interaction_geometry::{entity_aabb, entity_geometry, within_entity_reach};
use super::super::{
    OutboundCommand, SessionId, SessionRegistry, VisibilityDispatch, apply_entity_facts,
    entity_event_dispatches_locked, initialize_entity_wire_state_from_snapshot_locked,
    passive_ground_wander_speed, record_entity_dispatches_locked, server_entity_snapshot_from,
    session_recipients, spawn_entity_visibility_from_snapshot_locked, spawn_item_drop_locked,
    visibility_dispatches, visible_entity_observers_locked,
};
use super::{
    BreedingAnimal, GrazingSheep, SHEEP_GRAZING_ACTION_TICK, SHEEP_GRAZING_ANIMATION_TICKS,
    SheepGrazingCandidate, SheepGrazingPlan, advance_sheep_grazing, plan_breeding,
};
use crate::play::inventory::PlayerInventory;
use crate::play::simulation::{
    AnimalFeedPlan, CommittedAnimalFeed, CommittedSheepShear, SheepShearPlan, SimulationAuthority,
};
use mc_entity::{EntityId, EntityItemStack, EntityLifecycle, GoalState, SpawnEntity, Vec3};
use mc_protocol::packets::play::{GameMode, ItemStack};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::Instant;

impl SessionRegistry {
    #[cfg(test)]
    pub(crate) fn set_sheep_grazing_ticks_for_test(
        &self,
        entity_id: EntityId,
        remaining_ticks: Option<u8>,
    ) -> bool {
        let mut entities = self.lock_entities("set sheep grazing timer for test");
        let Some(expected) = entities.snapshot(entity_id) else {
            return false;
        };
        let mut next = expected.clone();
        next.retained.sheep_grazing_ticks = remaining_ticks;
        entities.replace_snapshot_if_current(expected, next)
    }

    pub(in crate::play) fn commit_animal_feed(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &AnimalFeedPlan,
    ) -> Option<CommittedAnimalFeed> {
        let mut inner = self.lock_session_entities("commit animal feed");
        let player_pose = inner.sessions.get(&actor_session)?.pose;
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit animal feed",
            wait_started,
            guard,
        );
        if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
            return None;
        }
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if plan.held_slot != selected_slot && plan.held_slot != 45 {
            return None;
        }
        if player_state.inventory.slots[plan.held_slot] != plan.expected_held
            || plan.expected_held.item_id != plan.food_item_id
        {
            return None;
        }

        let entity = inner.entities.snapshot(plan.entity_id)?;
        if entity.lifecycle != EntityLifecycle::Alive
            || !plan.targets.accepts(&entity.type_name)
            || !within_entity_reach(
                player_pose,
                entity.position,
                entity_geometry(&entity.type_name, entity.animal).aabb,
                player_state.game_mode,
            )
        {
            return None;
        }
        let mut animal = entity.animal?;
        if !animal.can_fall_in_love() {
            return None;
        }

        let mut inventory = player_state.inventory.clone();
        let mut changed_slots = Vec::new();
        if player_state.game_mode != GameMode::Creative {
            let held = &mut inventory.slots[plan.held_slot];
            held.count = held.count.saturating_sub(1);
            if held.count <= 0 {
                *held = ItemStack::EMPTY;
            }
            changed_slots.push((plan.held_slot, held.clone()));
        }

        animal.love_ticks = mc_entity::ANIMAL_LOVE_DURATION_TICKS;
        if !inner.entities.set_animal_state(plan.entity_id, animal) {
            return None;
        }
        if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&plan.entity_id) {
            snapshot.animal = Some(animal);
        }
        update_breeding_tick_tracking_locked(&mut inner, plan.entity_id, Some(animal));
        player_state.replace_inventory(inventory.clone());
        let dispatches = entity_event_dispatches_locked(&inner, plan.entity_id, 18);
        Some(CommittedAnimalFeed {
            inventory,
            changed_slots,
            dispatches,
        })
    }

    pub(in crate::play) fn commit_sheep_shear(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &SheepShearPlan,
    ) -> Option<CommittedSheepShear> {
        let mut inner = self.lock_session_entities("commit sheep shear");
        let player_pose = inner.sessions.get(&actor_session)?.pose;
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit sheep shear",
            wait_started,
            guard,
        );
        if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
            return None;
        }
        let selected_slot =
            PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot);
        if plan.held_slot != selected_slot && plan.held_slot != PlayerInventory::OFFHAND_SLOT {
            return None;
        }
        if player_state.inventory.slots[plan.held_slot] != plan.expected_held
            || plan.expected_held.item_id != plan.shears_item_id
        {
            return None;
        }

        let entity = inner.entities.snapshot(plan.entity_id)?;
        if entity.lifecycle != EntityLifecycle::Alive
            || entity.type_name != "minecraft:sheep"
            || !within_entity_reach(
                player_pose,
                entity.position,
                entity_geometry(&entity.type_name, entity.animal).aabb,
                player_state.game_mode,
            )
            || !inner
                .published_entity_snapshots
                .contains_key(&plan.entity_id)
        {
            return None;
        }
        let mut animal = entity.animal?;
        let mut wool = animal.sheep_wool?;
        if animal.is_baby() || wool.sheared {
            return None;
        }

        let mut inventory = player_state.inventory.clone();
        let mut changed_slots = Vec::new();
        if player_state.game_mode != GameMode::Creative {
            let held = &mut inventory.slots[plan.held_slot];
            let new_damage = held.damage.unwrap_or(0).saturating_add(1);
            if new_damage >= plan.shears_max_damage {
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(new_damage);
            }
            changed_slots.push((plan.held_slot, held.clone()));
        }

        wool.sheared = true;
        animal.sheep_wool = Some(wool);
        if !inner.entities.set_animal_state(plan.entity_id, animal) {
            return None;
        }
        let snapshot = {
            let snapshot = inner.published_entity_snapshots.get_mut(&plan.entity_id)?;
            snapshot.animal = Some(animal);
            snapshot.clone()
        };
        player_state.replace_inventory(inventory.clone());

        let recipients = session_recipients(
            &inner,
            visible_entity_observers_locked(&inner, plan.entity_id),
        );
        let mut dispatches = visibility_dispatches(recipients, || {
            OutboundCommand::UpdateEntityData(snapshot.clone())
        });
        record_entity_dispatches_locked(&mut inner, &dispatches);

        let drop_count = sheep_shear_drop_count(plan.entity_id, inner.entity_lifecycle_tick);
        let wool_item_id = plan.wool_item_ids[usize::from(wool.color.id())];
        let drop_position = Vec3::new(
            entity.position.x,
            entity.position.y + 1.0,
            entity.position.z,
        );
        for _ in 0..drop_count {
            dispatches.extend(spawn_item_drop_locked(
                &mut inner,
                plan.item_entity_type_id,
                drop_position,
                EntityItemStack::new(wool_item_id, 1),
            ));
        }

        Some(CommittedSheepShear {
            inventory,
            changed_slots,
            #[cfg(test)]
            drop_count,
            dispatches,
        })
    }

    pub(in crate::play) fn tick_animal_breeding(
        &self,
        _authority: &SimulationAuthority,
        elapsed_ticks: u16,
    ) -> (usize, Vec<VisibilityDispatch>) {
        if !self.has_live_sessions() {
            return (0, Vec::new());
        }
        let breeding_tick = self.simulation_tick();
        let active_entity_ids = self.active_simulation_entities.load_full();
        if active_entity_ids.is_empty() {
            return (0, Vec::new());
        }
        let breeding_tick_entities = self.simulation_inputs.breeding_tick_entities();
        let active_entity_ids: HashSet<EntityId> = {
            if breeding_tick_entities.len() < active_entity_ids.len() {
                breeding_tick_entities
                    .intersection(&active_entity_ids)
                    .copied()
                    .collect()
            } else {
                active_entity_ids
                    .intersection(&breeding_tick_entities)
                    .copied()
                    .collect()
            }
        };
        if active_entity_ids.is_empty() {
            return (0, Vec::new());
        }
        let entities = self.lock_entities("snapshot animal breeding");
        let mut animals = Vec::new();
        entities.visit_simulation_entities_for_ids(&active_entity_ids, |entity| {
            let Some(animal) = entity.animal.filter(|animal| animal.needs_breeding_tick()) else {
                return;
            };
            #[cfg(test)]
            self.breeding_entity_scan_visits
                .fetch_add(1, Ordering::Relaxed);
            if entity.lifecycle == EntityLifecycle::Alive {
                animals.push(BreedingAnimal {
                    id: entity.id,
                    type_id: entity.type_id,
                    type_name: entity.type_name.to_owned(),
                    position: entity.position,
                    state: animal,
                });
            }
        });
        let expected_animals = animals
            .iter()
            .map(|entity| {
                entities
                    .snapshot(entity.id)
                    .expect("visited breeding entity remains in the locked authority")
            })
            .collect::<Vec<_>>();
        drop(entities);
        if animals.is_empty() {
            return (0, Vec::new());
        }

        #[cfg(test)]
        self.pause_during_breeding_plan_for_test();
        let breeding_plan = plan_breeding(breeding_tick, &animals, elapsed_ticks);

        if animals
            .iter()
            .zip(&expected_animals)
            .zip(&breeding_plan.updates)
            .all(|((animal, expected), update)| {
                debug_assert_eq!(animal.id, update.entity_id);
                expected.animal == Some(update.state)
            })
        {
            return (0, Vec::new());
        }

        let changed_animals = animals
            .iter()
            .zip(&expected_animals)
            .zip(&breeding_plan.updates)
            .filter(|((_, expected), update)| expected.animal != Some(update.state))
            .map(|((_, expected), update)| (expected.clone(), update.state))
            .collect::<Vec<_>>();
        let courtship_goals = breeding_plan
            .courtships
            .iter()
            .flat_map(|courtship| {
                [
                    (courtship.first_id, courtship.first_target),
                    (courtship.second_id, courtship.second_target),
                ]
                .into_iter()
                .filter_map(|(entity_id, target)| {
                    let expected = expected_animals
                        .iter()
                        .find(|snapshot| snapshot.id == entity_id)?;
                    let speed = expected
                        .attributes
                        .base(&mc_entity::AttributeKind::MovementSpeed)
                        .unwrap_or(0.2)
                        * 10.0;
                    let goal = if courtship.completed {
                        GoalState::Wander {
                            speed,
                            period_ticks: 80,
                        }
                    } else {
                        GoalState::FollowPosition { target, speed }
                    };
                    (expected.goal != goal).then_some((entity_id, goal))
                })
            })
            .collect::<Vec<_>>();
        let lifecycle_tick = self.simulation_tick();
        let mut committed_animals = Vec::new();
        let mut children = Vec::new();
        {
            let mut entities = self.lock_entities("commit animal breeding ECS");
            let states_applied = if breeding_plan.births.is_empty() {
                entities
                    .set_animal_states_if_current_deferred_journal(changed_animals.iter().cloned())
            } else {
                entities.set_animal_states_if_current(changed_animals.iter().cloned())
            };
            if !states_applied {
                return (0, Vec::new());
            }
            let applied_goals =
                entities.set_goals_deferred_journal(courtship_goals.iter().cloned());
            debug_assert_eq!(applied_goals, courtship_goals.len());
            #[cfg(test)]
            self.breeding_commits.fetch_add(1, Ordering::Relaxed);
            #[cfg(test)]
            self.breeding_state_updates
                .fetch_add(changed_animals.len() as u64, Ordering::Relaxed);
            let changed_ids = changed_animals
                .iter()
                .map(|(expected, _)| expected.id)
                .collect::<HashSet<_>>();
            entities.prefetch(&changed_ids);
            committed_animals.extend(changed_animals.iter().filter_map(|(expected, animal)| {
                entities
                    .snapshot(expected.id)
                    .filter(|entity| entity.animal == Some(*animal))
            }));

            if !breeding_plan.births.is_empty() {
                let child_entities = breeding_plan
                    .births
                    .into_iter()
                    .map(|birth| {
                        let mut child =
                            SpawnEntity::new(birth.type_id, birth.type_name, birth.position);
                        child.retained.spawn_tick = lifecycle_tick;
                        apply_entity_facts(&mut child);
                        if let Some(color) = birth.sheep_color {
                            child.animal = Some(mc_entity::AnimalBreedingState::adult_sheep(color));
                        }
                        if let Some(animal) = child.animal.as_mut() {
                            animal.age_ticks = mc_entity::BABY_START_AGE_TICKS;
                            animal.love_ticks = 0;
                        }
                        child.goal = GoalState::Wander {
                            speed: passive_ground_wander_speed(&child),
                            period_ticks: 80,
                        };
                        child
                    })
                    .collect::<Vec<_>>();
                let child_ids = entities.spawn_batch(child_entities);
                let child_id_set = child_ids.iter().copied().collect::<HashSet<_>>();
                entities.prefetch(&child_id_set);
                children.extend(child_ids.into_iter().filter_map(|id| entities.snapshot(id)));
            }
        }
        #[cfg(test)]
        self.pause_between_breeding_entity_and_session_commit_for_test();
        let child_ids = children
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        let current =
            self.current_expected_entity_snapshots(committed_animals.into_iter().chain(children));
        let (children, committed_animals): (Vec<_>, Vec<_>) = current
            .into_iter()
            .partition(|snapshot| child_ids.contains(&snapshot.id));
        let committed_animal_ids = committed_animals
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        let mut inner = self.lock_inner("publish animal breeding");
        for committed in committed_animals {
            let entity_id = committed.id;
            update_breeding_tick_tracking_locked(&mut inner, entity_id, committed.animal);
            if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&entity_id) {
                snapshot.animal = committed.animal;
            }
        }

        let mut dispatches = Vec::new();
        for entity_id in breeding_plan.became_adults {
            if !committed_animal_ids.contains(&entity_id) {
                continue;
            }
            let Some(snapshot) = inner.published_entity_snapshots.get(&entity_id).cloned() else {
                continue;
            };
            let recipients =
                session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
            let data_dispatches = visibility_dispatches(recipients, || {
                OutboundCommand::UpdateEntityData(snapshot.clone())
            });
            record_entity_dispatches_locked(&mut inner, &data_dispatches);
            dispatches.extend(data_dispatches);
        }

        let children = children
            .into_iter()
            .map(server_entity_snapshot_from)
            .collect::<Vec<_>>();
        let birth_count = children.len();
        for child in children {
            let child_id = child.id;
            update_breeding_tick_tracking_locked(&mut inner, child_id, child.animal);
            if child.type_name == "minecraft:sheep" {
                inner.sheep_entities.insert(child_id);
            }
            inner
                .entity_type_aabbs
                .entry(child.type_id)
                .or_insert_with(|| entity_aabb(&child.type_name));
            track_entity_chunk_locked(&mut inner, child_id, child.position);
            initialize_entity_wire_state_from_snapshot_locked(&mut inner, &child);
            dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                &mut inner, child,
            ));
        }
        (birth_count, dispatches)
    }

    pub(in crate::play) fn plan_sheep_grazing(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
    ) -> SheepGrazingPlan {
        let (_, loaded_entity_ids) = self.simulation_inputs.active_entity_candidates();
        let loaded_sheep_ids = {
            let inner = self.lock_inner("snapshot loaded sheep index");
            if inner.sheep_entities.len() < loaded_entity_ids.len() {
                inner
                    .sheep_entities
                    .intersection(&loaded_entity_ids)
                    .copied()
                    .collect()
            } else {
                loaded_entity_ids
                    .intersection(&inner.sheep_entities)
                    .copied()
                    .collect()
            }
        };
        let mut entities = self.lock_entities("snapshot sheep grazing candidates");
        let mut sheep_ids = Vec::new();
        entities.visit_sheep_entities_for_ids(&loaded_sheep_ids, |entity| {
            #[cfg(test)]
            self.sheep_grazing_entity_visits
                .fetch_add(1, Ordering::Relaxed);
            if let Some(animal) = entity.animal {
                sheep_ids.push(GrazingSheep {
                    expected: entity.clone(),
                    is_baby: animal.is_baby(),
                });
            }
        });
        #[cfg(test)]
        self.pause_during_sheep_grazing_plan_for_test();
        let mut advance = advance_sheep_grazing(tick, &sheep_ids);
        let updates = advance
            .timer_updates
            .into_iter()
            .map(|update| {
                let mut next = update.expected.clone();
                next.retained.sheep_grazing_ticks = update.remaining;
                (update.expected, next)
            })
            .collect::<Vec<_>>();
        let applied_updates = updates
            .iter()
            .map(|(_, next)| next.id)
            .collect::<HashSet<_>>();
        if !updates.is_empty() && !entities.replace_snapshots_if_current(updates) {
            advance.plan.actions.clear();
        } else {
            advance
                .plan
                .actions
                .retain(|candidate| applied_updates.contains(&candidate.entity_id));
        }
        advance.plan
    }

    pub(in crate::play) fn start_sheep_grazing(
        &self,
        _authority: &SimulationAuthority,
        candidates: &[SheepGrazingCandidate],
    ) -> (usize, Vec<VisibilityDispatch>) {
        let candidates = candidates.to_vec();
        let mut started_entities = Vec::new();
        {
            let mut entities = self.lock_entities("start sheep grazing ECS");
            let candidate_ids = candidates
                .iter()
                .map(|candidate| candidate.entity_id)
                .collect::<HashSet<_>>();
            entities.prefetch(&candidate_ids);
            let mut seen = HashSet::new();
            let mut stopped_states = Vec::new();
            for candidate in candidates {
                if !seen.insert(candidate.entity_id) {
                    continue;
                }
                let Some(entity) = entities.snapshot(candidate.entity_id) else {
                    continue;
                };
                if entity.lifecycle != EntityLifecycle::Alive
                    || entity.type_name != "minecraft:sheep"
                    || entity.animal.and_then(|animal| animal.sheep_wool).is_none()
                    || entity.retained.sheep_grazing_ticks.is_some()
                {
                    continue;
                }
                let current_position = mc_world::BlockPos {
                    x: entity.position.x.floor() as i32,
                    y: entity.position.y.floor() as i32,
                    z: entity.position.z.floor() as i32,
                };
                if current_position != candidate.block_position {
                    continue;
                }

                let stopped_velocity = Vec3::new(0.0, entity.velocity.y, 0.0);
                let mut next = entity.clone();
                next.velocity = stopped_velocity;
                next.retained.sheep_grazing_ticks = Some(SHEEP_GRAZING_ANIMATION_TICKS);
                stopped_states.push((entity, next));
            }
            for (expected, next) in stopped_states {
                if entities.replace_snapshot_if_current(expected, next.clone()) {
                    started_entities.push(next);
                }
            }
        }
        #[cfg(test)]
        self.pause_between_sheep_grazing_entity_and_session_commit_for_test();
        #[cfg(test)]
        self.pause_before_sheep_grazing_owner_read_for_test();
        let committed_entities = self.current_expected_entity_snapshots(started_entities);
        let mut inner = self.lock_inner("publish sheep grazing start");
        let mut started = 0;
        let mut dispatches = Vec::new();
        for committed in committed_entities {
            let entity_id = committed.id;
            if committed.retained.sheep_grazing_ticks != Some(SHEEP_GRAZING_ANIMATION_TICKS) {
                continue;
            }
            if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&entity_id) {
                snapshot.velocity = committed.velocity;
            }
            dispatches.extend(entity_event_dispatches_locked(&inner, entity_id, 10));
            started += 1;
        }
        (started, dispatches)
    }

    pub(in crate::play) fn finish_sheep_grazing(
        &self,
        _authority: &SimulationAuthority,
        entity_ids: &[EntityId],
    ) -> (usize, Vec<VisibilityDispatch>) {
        let entity_ids = entity_ids.to_vec();
        let mut finished_entities = Vec::new();
        {
            let mut entities = self.lock_entities("finish sheep grazing ECS");
            let candidate_ids = entity_ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&candidate_ids);
            let mut seen = HashSet::new();
            let mut planned = Vec::new();
            let mut changed_states = Vec::new();
            for entity_id in entity_ids {
                if !seen.insert(entity_id) {
                    continue;
                }
                let Some(entity) = entities.snapshot(entity_id) else {
                    continue;
                };
                if entity.lifecycle != EntityLifecycle::Alive
                    || entity.type_name != "minecraft:sheep"
                    || entity.retained.sheep_grazing_ticks != Some(SHEEP_GRAZING_ACTION_TICK)
                {
                    continue;
                }
                let Some(mut animal) = entity.animal else {
                    continue;
                };
                let Some(mut wool) = animal.sheep_wool else {
                    continue;
                };
                let previous = animal;
                wool.sheared = false;
                animal.sheep_wool = Some(wool);
                if animal.age_ticks < 0 {
                    animal.age_ticks = animal.age_ticks.saturating_add(1_200).min(0);
                }
                let changed = (animal != previous).then_some(animal);
                if changed.is_some() {
                    changed_states.push((entity.clone(), animal));
                }
                planned.push((entity, changed));
            }
            if changed_states.is_empty()
                || entities.set_animal_states_if_current(changed_states.iter().cloned())
            {
                let changed_ids = changed_states
                    .iter()
                    .map(|(snapshot, _)| snapshot.id)
                    .collect::<HashSet<_>>();
                entities.prefetch(&changed_ids);
                for (expected, animal) in planned {
                    if let Some(committed) = entities.snapshot(expected.id)
                        && animal.is_none_or(|animal| committed.animal == Some(animal))
                    {
                        finished_entities.push((committed, animal));
                    }
                }
            }
        }
        #[cfg(test)]
        self.pause_between_sheep_grazing_entity_and_session_commit_for_test();
        let expected_animals = finished_entities
            .iter()
            .map(|(snapshot, animal)| (snapshot.id, *animal))
            .collect::<HashMap<_, _>>();
        let expected_snapshots = finished_entities
            .into_iter()
            .map(|(snapshot, _)| snapshot)
            .collect::<Vec<_>>();
        let committed_entities = self.current_expected_entity_snapshots(expected_snapshots);
        let mut inner = self.lock_inner("publish sheep grazing finish");
        let mut ate = 0;
        let mut dispatches = Vec::new();
        for committed in committed_entities {
            let entity_id = committed.id;
            let animal = expected_animals[&entity_id];
            if committed.retained.sheep_grazing_ticks != Some(SHEEP_GRAZING_ACTION_TICK) {
                continue;
            }
            if let Some(animal) = animal {
                update_breeding_tick_tracking_locked(&mut inner, entity_id, Some(animal));
                let Some(snapshot) = inner.published_entity_snapshots.get_mut(&entity_id) else {
                    continue;
                };
                snapshot.animal = Some(animal);
                let snapshot = snapshot.clone();
                let recipients =
                    session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
                let data_dispatches = visibility_dispatches(recipients, || {
                    OutboundCommand::UpdateEntityData(snapshot.clone())
                });
                record_entity_dispatches_locked(&mut inner, &data_dispatches);
                dispatches.extend(data_dispatches);
            }
            ate += 1;
        }
        (ate, dispatches)
    }
}

fn sheep_shear_drop_count(entity_id: EntityId, simulation_tick: u64) -> usize {
    let seed = (entity_id.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ simulation_tick.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    1 + (seed.rotate_left(23) % 3) as usize
}
