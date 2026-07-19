use super::*;

pub(super) struct SessionEntityOwners {
    _runtime: RegionalOwnerRuntime,
    pub(super) handle: RegionalOwnerHandle,
    observation: Arc<SessionPressureObservation>,
    journal_failures: Arc<EntityJournalFailureTracker>,
    #[cfg(test)]
    owner_requests: Arc<AtomicU64>,
}

#[derive(Default)]
struct EntityJournalFailureTracker {
    by_uuid: Mutex<HashMap<uuid::Uuid, mc_entity::RegionalDecisionJournalError>>,
}

struct TrackedEntityDecisionJournal {
    inner: Box<dyn mc_entity::RegionalDecisionJournal>,
    failures: Arc<EntityJournalFailureTracker>,
}

impl TrackedEntityDecisionJournal {
    fn record_failure(
        &self,
        decisions: &[mc_entity::RegionalCommitDecision],
        error: mc_entity::RegionalDecisionJournalError,
    ) {
        let mut failures = self
            .failures
            .by_uuid
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for snapshot in decisions
            .iter()
            .flat_map(mc_entity::RegionalCommitDecision::upserts)
        {
            failures.insert(snapshot.uuid, error);
        }
    }
}

impl mc_entity::RegionalDecisionJournal for TrackedEntityDecisionJournal {
    fn enabled(&self) -> bool {
        self.inner.enabled()
    }

    fn record_commit(
        &mut self,
        decision: &mc_entity::RegionalCommitDecision,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.inner.record_commit(decision).inspect_err(|error| {
            self.record_failure(std::slice::from_ref(decision), *error);
        })
    }

    fn record_commits(
        &mut self,
        decisions: &[mc_entity::RegionalCommitDecision],
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.inner.record_commits(decisions).inspect_err(|error| {
            self.record_failure(decisions, *error);
        })
    }

    fn clear_commit(
        &mut self,
        phase: mc_entity::RegionPhase,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.inner.clear_commit(phase)
    }

    fn clear_commits(
        &mut self,
        phases: &[mc_entity::RegionPhase],
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.inner.clear_commits(phases)
    }

    fn pending_phases(&self) -> Vec<mc_entity::RegionPhase> {
        self.inner.pending_phases()
    }

    fn recovery_watermark(&self) -> (mc_entity::RegionPhase, u64) {
        self.inner.recovery_watermark()
    }
}

impl std::fmt::Debug for SessionEntityOwners {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionEntityOwners")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl SessionEntityOwners {
    pub(super) fn new(
        observation: Arc<SessionPressureObservation>,
        lane_count: usize,
        journal: Option<Box<dyn mc_entity::RegionalDecisionJournal>>,
    ) -> Self {
        let lane_count = lane_count.max(1);
        let store = mc_entity::RegionalEntityStore::with_next_id(SERVER_ENTITY_ID_START - 1);
        let journal_failures = Arc::new(EntityJournalFailureTracker::default());
        let journal = journal.map(|inner| {
            Box::new(TrackedEntityDecisionJournal {
                inner,
                failures: Arc::clone(&journal_failures),
            }) as Box<dyn mc_entity::RegionalDecisionJournal>
        });
        let runtime = match journal {
            Some(journal) => {
                RegionalOwnerRuntime::from_store_with_journal(store, lane_count, journal)
            }
            None => RegionalOwnerRuntime::from_store(store, lane_count),
        }
        .unwrap_or_else(|error| {
            panic!(
                "failed to start {lane_count} persistent regional entity owners: {:?}",
                error.error
            )
        });
        let handle = runtime.handle();
        #[cfg(test)]
        let owner_requests = Arc::new(AtomicU64::new(0));
        Self {
            _runtime: runtime,
            handle,
            observation,
            journal_failures,
            #[cfg(test)]
            owner_requests,
        }
    }

    pub(super) fn access(&self) -> EntityOwnerAccess {
        EntityOwnerAccess {
            handle: self.handle.clone(),
            observation: Arc::clone(&self.observation),
            snapshots: RefCell::new(HashMap::new()),
            selected_snapshots: RefCell::new(None),
            #[cfg(test)]
            owner_requests: Arc::clone(&self.owner_requests),
        }
    }

    pub(super) fn status(&self) -> RegionalOwnerStatus {
        owner_result(self.handle.status())
    }

    pub(super) fn reconfigure_lanes(&self, lane_count: usize) -> usize {
        owner_result(self.handle.reconfigure_lanes(lane_count.max(1)))
    }

    #[cfg(test)]
    pub(super) fn access_for_test(&self) -> EntityOwnerAccess {
        self.access()
    }

    #[cfg(test)]
    pub(super) fn owner_responsive_for_test(&self) -> bool {
        self.handle.status().is_ok()
    }

    #[cfg(test)]
    pub(super) fn reset_owner_requests_for_test(&self) {
        self.owner_requests.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn owner_requests_for_test(&self) -> u64 {
        self.owner_requests.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn record_owner_request_for_test(&self) {
        self.owner_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn take_journal_failure(
        &self,
        uuids: impl IntoIterator<Item = uuid::Uuid>,
    ) -> Option<mc_entity::RegionalDecisionJournalError> {
        let mut failures = self
            .journal_failures
            .by_uuid
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut outcome = None;
        for uuid in uuids {
            if let Some(error) = failures.remove(&uuid) {
                debug_assert!(
                    outcome.is_none_or(|current: mc_entity::RegionalDecisionJournalError| {
                        current.outcome_unknown() == error.outcome_unknown()
                    }),
                    "one entity batch cannot have mixed journal outcomes"
                );
                outcome = Some(error);
            }
        }
        outcome
    }
}

pub(super) struct EntityOwnerAccess {
    handle: RegionalOwnerHandle,
    observation: Arc<SessionPressureObservation>,
    snapshots: RefCell<HashMap<EntityId, Option<EntitySnapshot>>>,
    selected_snapshots: RefCell<Option<VersionedEntitySnapshots>>,
    #[cfg(test)]
    owner_requests: Arc<AtomicU64>,
}

pub(super) fn owner_result<T>(result: Result<T, mc_entity::RegionOwnerLaneError>) -> T {
    result.unwrap_or_else(|error| panic!("regional entity owner failed: {error:?}"))
}

impl EntityOwnerAccess {
    #[cfg(test)]
    pub(super) fn record_owner_request(&self) {
        self.owner_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn prefetch(&self, ids: &HashSet<EntityId>) {
        let missing = ids
            .iter()
            .filter(|id| !self.snapshots.borrow().contains_key(id))
            .copied()
            .collect::<HashSet<_>>();
        if missing.is_empty() {
            return;
        }
        #[cfg(test)]
        self.record_owner_request();
        let selected = owner_result(self.handle.snapshots_for_ids_versioned(&missing));
        let mut snapshots = self.snapshots.borrow_mut();
        for id in missing {
            snapshots.insert(id, None);
        }
        for snapshot in selected.snapshots() {
            snapshots.insert(snapshot.id, Some(snapshot.clone()));
        }
        self.selected_snapshots.replace(Some(selected));
    }

    pub(super) fn invalidate(&self, id: EntityId) {
        self.snapshots.borrow_mut().remove(&id);
        self.selected_snapshots.borrow_mut().take();
    }

    pub(super) fn snapshots_vec(&self) -> Vec<EntitySnapshot> {
        #[cfg(test)]
        self.record_owner_request();
        owner_result(self.handle.snapshots())
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.record_owner_request();
        owner_result(self.handle.status()).entity_count
    }

    pub(super) fn contains(&self, id: EntityId) -> bool {
        self.snapshot(id).is_some()
    }

    pub(super) fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        if let Some(snapshot) = self.snapshots.borrow().get(&id) {
            return snapshot.clone();
        }
        #[cfg(test)]
        self.record_owner_request();
        let snapshot = owner_result(self.handle.snapshot(id));
        self.snapshots.borrow_mut().insert(id, snapshot.clone());
        snapshot
    }

    pub(super) fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        self.snapshot(id).map(|snapshot| EntityMotionState {
            id: snapshot.id,
            position: snapshot.position,
            rotation: snapshot.rotation,
            velocity: snapshot.velocity,
            on_ground: snapshot.on_ground,
            is_item: snapshot.type_name == "minecraft:item",
            is_experience: snapshot.type_name == "minecraft:experience_orb",
            is_arrow: snapshot.type_name == "minecraft:arrow",
            sends_velocity: !matches!(
                snapshot.type_name.as_str(),
                "minecraft:item" | "minecraft:experience_orb"
            ),
        })
    }

    #[cfg(test)]
    pub(super) fn snapshots(&self) -> std::vec::IntoIter<EntitySnapshot> {
        self.snapshots_vec().into_iter()
    }

    #[cfg(test)]
    pub(super) fn region_len(&self, key: RegionKey) -> usize {
        self.snapshots_vec()
            .iter()
            .filter(|snapshot| RegionKey::from_position(snapshot.position) == Some(key))
            .count()
    }

    pub(super) fn parallel_kinematics_batch_count(&self, _states: &[EntityKinematics]) -> usize {
        0
    }

    #[cfg(test)]
    pub(super) fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        self.spawn_authoritative(entity)
    }

    pub(super) fn spawn_authoritative(&mut self, entity: SpawnEntity) -> EntityId {
        #[cfg(test)]
        self.record_owner_request();
        let id = owner_result(self.handle.spawn_authoritative(entity));
        self.observation.record_entity_inserts(1);
        id
    }

    pub(super) fn spawn_authoritative_deferred_journal(&mut self, entity: SpawnEntity) -> EntityId {
        #[cfg(test)]
        self.record_owner_request();
        let id = owner_result(self.handle.spawn_authoritative_deferred_journal(entity));
        self.observation.record_entity_inserts(1);
        id
    }

    pub(super) fn spawn_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntityId> {
        #[cfg(test)]
        self.record_owner_request();
        let ids = owner_result(self.handle.spawn_authoritative_batch(entities));
        self.observation.record_entity_inserts(ids.len());
        ids
    }

    pub(super) fn spawn_unique_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntitySnapshot> {
        owner_result(self.try_spawn_unique_authoritative_batch(entities))
    }

    pub(super) fn try_spawn_unique_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, mc_entity::RegionOwnerLaneError> {
        #[cfg(test)]
        self.record_owner_request();
        let snapshots = self.handle.spawn_unique_authoritative_batch(entities)?;
        self.observation.record_entity_inserts(snapshots.len());
        Ok(snapshots)
    }

    pub(super) fn insert_authoritative_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        let expected = snapshots.len();
        let inserted = owner_result(self.handle.insert_authoritative_snapshots_batch(snapshots));
        self.observation.record_entity_inserts(inserted);
        inserted == expected
    }

    pub(super) fn set_animal_state(
        &mut self,
        id: EntityId,
        animal: mc_entity::AnimalBreedingState,
    ) -> bool {
        let Some(expected) = self.snapshot(id) else {
            return false;
        };
        #[cfg(test)]
        self.record_owner_request();
        let applied = owner_result(
            self.handle
                .set_animal_states_if_current([(expected, animal)]),
        );
        self.invalidate(id);
        applied
    }

    pub(super) fn set_animal_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, mc_entity::AnimalBreedingState)>,
    ) -> bool {
        let states = states.into_iter().collect::<Vec<_>>();
        let ids = states
            .iter()
            .map(|(snapshot, _)| snapshot.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = owner_result(self.handle.set_animal_states_if_current(states));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn set_animal_states_if_current_deferred_journal(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, mc_entity::AnimalBreedingState)>,
    ) -> bool {
        let states = states.into_iter().collect::<Vec<_>>();
        let ids = states
            .iter()
            .map(|(snapshot, _)| snapshot.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = owner_result(
            self.handle
                .set_animal_states_if_current_deferred_journal(states),
        );
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    #[cfg(test)]
    pub(super) fn set_goal(&mut self, id: EntityId, goal: GoalState) -> bool {
        let applied = owner_result(self.handle.set_goal(id, goal));
        self.invalidate(id);
        applied
    }

    pub(super) fn set_goals_deferred_journal(
        &mut self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> usize {
        let goals = goals.into_iter().collect::<Vec<_>>();
        let ids = goals.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = owner_result(self.handle.set_goals_deferred_journal(goals));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn set_item_stack_if_current(
        &mut self,
        expected: EntitySnapshot,
        stack: Option<EntityItemStack>,
    ) -> bool {
        let id = expected.id;
        let applied = owner_result(self.handle.set_item_stack_if_current(expected, stack));
        self.invalidate(id);
        applied
    }

    pub(super) fn set_velocity(&mut self, id: EntityId, velocity: Vec3) -> bool {
        let Some(expected) = self.snapshot(id) else {
            return false;
        };
        let state = EntityKinematics {
            id,
            position: expected.position,
            rotation: expected.rotation,
            velocity,
            on_ground: expected.on_ground,
        };
        self.apply_kinematics_states_if_current([(expected, state)])
    }

    pub(super) fn apply_kinematics_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> bool {
        let states = states.into_iter().collect::<Vec<_>>();
        let ids = states
            .iter()
            .map(|(snapshot, _)| snapshot.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = owner_result(self.handle.apply_kinematics_if_current(states));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn set_position(&mut self, id: EntityId, position: Vec3) -> bool {
        let applied = owner_result(self.handle.set_position(id, position));
        self.invalidate(id);
        applied
    }

    pub(super) fn damage(&mut self, id: EntityId, amount: f32) -> Option<mc_entity::EntityDamage> {
        let expected = self.snapshot(id)?;
        let damage = owner_result(self.handle.damage_if_current(expected, amount));
        self.invalidate(id);
        damage
    }

    pub(super) fn remove_if_current(&mut self, expected: EntitySnapshot) -> Option<EntitySnapshot> {
        let id = expected.id;
        let removed = owner_result(self.handle.remove_if_current(expected));
        if removed.is_some() {
            self.observation.record_entity_remove();
            self.snapshots.borrow_mut().insert(id, None);
        } else {
            self.invalidate(id);
        }
        removed
    }

    pub(super) fn apply_kinematics_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> Vec<EntityKinematics> {
        let states = states.into_iter().collect::<Vec<_>>();
        let ids = states.iter().map(|state| state.id).collect::<HashSet<_>>();
        self.prefetch(&ids);
        let expected = ids
            .iter()
            .filter_map(|id| self.snapshot(*id).map(|snapshot| (*id, snapshot)))
            .collect::<HashMap<_, _>>();
        let conditional = states
            .iter()
            .filter_map(|state| {
                expected
                    .get(&state.id)
                    .cloned()
                    .map(|snapshot| (snapshot, *state))
            })
            .collect::<Vec<_>>();
        if conditional.is_empty()
            || !owner_result(
                self.handle
                    .apply_kinematics_if_current_deferred_journal(conditional),
            )
        {
            return Vec::new();
        }
        let applied = owner_result(self.handle.snapshots_for_ids(&ids));
        {
            let mut cache = self.snapshots.borrow_mut();
            for snapshot in &applied {
                cache.insert(snapshot.id, Some(snapshot.clone()));
            }
        }
        applied
            .into_iter()
            .map(|snapshot| EntityKinematics {
                id: snapshot.id,
                position: snapshot.position,
                rotation: snapshot.rotation,
                velocity: snapshot.velocity,
                on_ground: snapshot.on_ground,
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn apply_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        self.apply_kinematics_authoritative(states).len()
    }

    pub(super) fn apply_kinematics_parallel_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        _max_workers: usize,
    ) -> Vec<EntityKinematics> {
        self.apply_kinematics_authoritative(states)
    }

    pub(super) fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> mc_entity::RegionalPreparedGoalTick {
        #[cfg(test)]
        self.record_owner_request();
        let selected = self.selected_snapshots.borrow_mut().take();
        owner_result(match selected {
            Some(selected) => self
                .handle
                .prepare_goal_tick_with_pathing_for_versioned_snapshots(tick, active_ids, selected),
            None => self
                .handle
                .prepare_goal_tick_with_pathing_for_ids(tick, active_ids),
        })
    }

    pub(super) fn apply_prepared_goal_tick_and_alive_kinematics(
        &mut self,
        resolved: mc_entity::RegionalResolvedGoalTick,
        ids: &HashSet<EntityId>,
    ) -> Option<(mc_entity::GoalTickStats, Vec<EntityKinematics>)> {
        #[cfg(test)]
        self.record_owner_request();
        let result = owner_result(
            self.handle
                .apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(resolved, ids),
        );
        let mut cache = self.snapshots.borrow_mut();
        for id in ids {
            cache.remove(id);
        }
        self.selected_snapshots.borrow_mut().take();
        result
    }

    pub(super) fn visit_simulation_entities(
        &self,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        for snapshot in self.snapshots_vec() {
            visitor(entity_snapshot_view(&snapshot));
        }
    }

    pub(super) fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        self.prefetch(ids);
        let mut snapshots = {
            let cache = self.snapshots.borrow();
            ids.iter()
                .filter_map(|id| cache.get(id).and_then(Clone::clone))
                .collect::<Vec<_>>()
        };
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        for snapshot in snapshots {
            visitor(entity_snapshot_view(&snapshot));
        }
    }

    pub(super) fn visit_breeding_tick_entities(
        &self,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        #[cfg(test)]
        self.record_owner_request();
        let snapshots = owner_result(self.handle.breeding_tick_snapshots());
        {
            let mut cache = self.snapshots.borrow_mut();
            for snapshot in &snapshots {
                cache.insert(snapshot.id, Some(snapshot.clone()));
            }
        }
        for snapshot in snapshots {
            visitor(entity_snapshot_view(&snapshot));
        }
    }

    pub(super) fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        for snapshot in owner_result(self.handle.snapshots_for_ids(ids))
            .into_iter()
            .filter(|snapshot| {
                snapshot.lifecycle == EntityLifecycle::Alive
                    && snapshot.type_name == "minecraft:sheep"
                    && snapshot
                        .animal
                        .is_some_and(|animal| animal.sheep_wool.is_some())
            })
        {
            visitor(entity_snapshot_view(&snapshot));
        }
    }

    #[cfg(test)]
    pub(super) fn shadow_comparison_stats(&self) -> ShadowComparisonStats {
        owner_result(self.handle.status()).shadow
    }

    #[cfg(test)]
    pub(super) fn compare_shadow(
        &mut self,
        tick: u64,
        stage: mc_entity::ShadowStage,
    ) -> Result<mc_entity::ShadowComparison, Box<mc_entity::ShadowDivergence>> {
        let comparison = owner_result(self.handle.compare_shadow(tick, stage));
        self.observation
            .publish_entities(&owner_result(self.handle.status()));
        comparison
    }
}

fn entity_snapshot_view(snapshot: &EntitySnapshot) -> mc_entity::EntityView<'_> {
    mc_entity::EntityView {
        id: snapshot.id,
        uuid: snapshot.uuid,
        type_id: snapshot.type_id,
        type_name: &snapshot.type_name,
        position: snapshot.position,
        rotation: snapshot.rotation,
        velocity: snapshot.velocity,
        on_ground: snapshot.on_ground,
        item_stack: snapshot.item_stack.clone(),
        experience_value: snapshot.experience_value,
        block_state: snapshot.block_state,
        lifecycle: snapshot.lifecycle,
        health: snapshot.health,
        attributes: &snapshot.attributes,
        goal: &snapshot.goal,
        vehicle: snapshot.vehicle,
        animal: snapshot.animal,
    }
}
