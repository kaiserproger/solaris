use super::*;
use crate::lock_policy::lock_authoritative_mutex;

pub(super) struct SessionEntityOwners {
    _runtime: RegionalOwnerRuntime,
    pub(super) handle: RegionalOwnerHandle,
    observation: Arc<SessionPressureObservation>,
    failure: Arc<EntityOwnerFailureState>,
    journal_failures: Arc<EntityJournalFailureTracker>,
    #[cfg(test)]
    owner_requests: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntityOwnerFatal {
    pub(crate) error: mc_entity::RegionOwnerLaneError,
}

#[derive(Debug)]
struct EntityOwnerFatalPanic(EntityOwnerFatal);

impl std::fmt::Display for EntityOwnerFatalPanic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "regional entity owner failed: {:?}",
            self.0.error
        )
    }
}

impl std::error::Error for EntityOwnerFatalPanic {}

pub(crate) fn entity_owner_fatal_from_panic(
    payload: &(dyn std::any::Any + Send),
) -> Option<mc_entity::RegionOwnerLaneError> {
    payload
        .downcast_ref::<EntityOwnerFatalPanic>()
        .map(|panic| panic.0.error)
}

#[derive(Debug)]
struct EntityOwnerFailureState {
    sender: tokio::sync::watch::Sender<Option<EntityOwnerFatal>>,
}

impl Default for EntityOwnerFailureState {
    fn default() -> Self {
        Self {
            sender: tokio::sync::watch::channel(None).0,
        }
    }
}

impl EntityOwnerFailureState {
    fn current(&self) -> Option<EntityOwnerFatal> {
        *self.sender.borrow()
    }

    fn report(&self, error: mc_entity::RegionOwnerLaneError) -> EntityOwnerFatal {
        let fatal = EntityOwnerFatal { error };
        self.sender.send_if_modified(|current| {
            if current.is_some() {
                false
            } else {
                *current = Some(fatal);
                true
            }
        });
        self.current().unwrap_or(fatal)
    }

    fn fail(&self, error: mc_entity::RegionOwnerLaneError) -> ! {
        let fatal = self.report(error);
        std::panic::panic_any(EntityOwnerFatalPanic(fatal));
    }

    fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<EntityOwnerFatal>> {
        self.sender.subscribe()
    }
}

fn is_runtime_owner_fatal(error: mc_entity::RegionOwnerLaneError) -> bool {
    matches!(
        error,
        mc_entity::RegionOwnerLaneError::Closed
            | mc_entity::RegionOwnerLaneError::WorkerPanicked
            | mc_entity::RegionOwnerLaneError::OutcomeUnknown
    )
}

fn canonical_owner_error(
    handle: &RegionalOwnerHandle,
    error: mc_entity::RegionOwnerLaneError,
) -> mc_entity::RegionOwnerLaneError {
    if error == mc_entity::RegionOwnerLaneError::Closed {
        handle.fatal_error().unwrap_or(error)
    } else {
        error
    }
}

fn owner_call<T>(
    failure: &EntityOwnerFailureState,
    handle: &RegionalOwnerHandle,
    call: impl FnOnce(&RegionalOwnerHandle) -> Result<T, mc_entity::RegionOwnerLaneError>,
) -> T {
    if let Some(fatal) = failure.current() {
        failure.fail(fatal.error);
    }
    match call(handle) {
        Ok(value) => value,
        Err(error) => failure.fail(canonical_owner_error(handle, error)),
    }
}

fn resolve_owner_result<T>(
    failure: &EntityOwnerFailureState,
    handle: &RegionalOwnerHandle,
    result: Result<T, mc_entity::RegionOwnerLaneError>,
) -> T {
    if let Some(fatal) = failure.current() {
        failure.fail(fatal.error);
    }
    match result {
        Ok(value) => value,
        Err(error) => failure.fail(canonical_owner_error(handle, error)),
    }
}

fn try_owner_result<T>(
    failure: &EntityOwnerFailureState,
    handle: &RegionalOwnerHandle,
    result: Result<T, mc_entity::RegionOwnerLaneError>,
) -> Result<T, mc_entity::RegionOwnerLaneError> {
    if let Some(fatal) = failure.current() {
        return Err(fatal.error);
    }
    result.map_err(|error| {
        let error = canonical_owner_error(handle, error);
        if is_runtime_owner_fatal(error) {
            failure.report(error);
        }
        error
    })
}

fn try_owner_call<T>(
    failure: &EntityOwnerFailureState,
    handle: &RegionalOwnerHandle,
    call: impl FnOnce(&RegionalOwnerHandle) -> Result<T, mc_entity::RegionOwnerLaneError>,
) -> Result<T, mc_entity::RegionOwnerLaneError> {
    if let Some(fatal) = failure.current() {
        return Err(fatal.error);
    }
    let result = call(handle);
    try_owner_result(failure, handle, result)
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
        let mut failures = lock_authoritative_mutex(
            &self.failures.by_uuid,
            "persistence.entity_journal_failures",
        );
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

    fn clear_commit_identities(
        &mut self,
        identities: &[(mc_entity::RegionPhase, u64, u64)],
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.inner.clear_commit_identities(identities)
    }

    fn pending_phases(&self) -> Vec<mc_entity::RegionPhase> {
        self.inner.pending_phases()
    }

    fn pending_commit_identities(&self) -> Vec<(mc_entity::RegionPhase, u64, u64)> {
        self.inner.pending_commit_identities()
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
    pub(super) fn try_new(
        observation: Arc<SessionPressureObservation>,
        lane_count: usize,
        journal: Option<Box<dyn mc_entity::RegionalDecisionJournal>>,
    ) -> Result<Self, mc_entity::RegionalOwnerCutoverError> {
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
        }?;
        let handle = runtime.handle();
        let failure = Arc::new(EntityOwnerFailureState::default());
        #[cfg(test)]
        let owner_requests = Arc::new(AtomicU64::new(0));
        Ok(Self {
            _runtime: runtime,
            handle,
            observation,
            failure,
            journal_failures,
            #[cfg(test)]
            owner_requests,
        })
    }

    pub(super) fn access(&self) -> EntityOwnerAccess {
        EntityOwnerAccess {
            handle: self.handle.clone(),
            observation: Arc::clone(&self.observation),
            failure: Arc::clone(&self.failure),
            snapshots: RefCell::new(HashMap::new()),
            selected_snapshots: RefCell::new(None),
            pending_goal_kinematics: RefCell::new(None),
            #[cfg(test)]
            owner_requests: Arc::clone(&self.owner_requests),
        }
    }

    fn resolve<T>(&self, result: Result<T, mc_entity::RegionOwnerLaneError>) -> T {
        resolve_owner_result(&self.failure, &self.handle, result)
    }

    pub(super) fn try_resolve<T>(
        &self,
        result: Result<T, mc_entity::RegionOwnerLaneError>,
    ) -> Result<T, mc_entity::RegionOwnerLaneError> {
        try_owner_result(&self.failure, &self.handle, result)
    }

    fn call<T>(
        &self,
        call: impl FnOnce(&RegionalOwnerHandle) -> Result<T, mc_entity::RegionOwnerLaneError>,
    ) -> T {
        owner_call(&self.failure, &self.handle, call)
    }

    pub(super) fn subscribe_failure(
        &self,
    ) -> tokio::sync::watch::Receiver<Option<EntityOwnerFatal>> {
        self.failure.subscribe()
    }

    #[cfg(test)]
    pub(super) fn report_failure(
        &self,
        error: mc_entity::RegionOwnerLaneError,
    ) -> EntityOwnerFatal {
        self.failure
            .report(canonical_owner_error(&self.handle, error))
    }

    pub(super) fn status(&self) -> RegionalOwnerStatus {
        self.call(RegionalOwnerHandle::status)
    }

    pub(super) fn reconfigure_lanes(&self, lane_count: usize) -> usize {
        self.call(|handle| handle.reconfigure_lanes(lane_count.max(1)))
    }

    pub(super) fn advance_lifecycle_epoch(&self, lifecycle_epoch: u64) {
        self.call(|handle| handle.advance_lifecycle_epoch(lifecycle_epoch));
    }

    pub(super) fn restore_checkpoint_boundary(
        &self,
        lifecycle_epoch: u64,
        sequence_watermark: u64,
    ) {
        self.call(|handle| handle.restore_checkpoint_boundary(lifecycle_epoch, sequence_watermark));
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
        let mut failures = lock_authoritative_mutex(
            &self.journal_failures.by_uuid,
            "persistence.entity_journal_failures",
        );
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
    failure: Arc<EntityOwnerFailureState>,
    snapshots: RefCell<HashMap<EntityId, Option<EntitySnapshot>>>,
    selected_snapshots: RefCell<Option<VersionedEntitySnapshots>>,
    pending_goal_kinematics: RefCell<Option<mc_entity::VersionedEntityKinematics>>,
    #[cfg(test)]
    owner_requests: Arc<AtomicU64>,
}

pub(super) fn owner_result<T>(
    owners: &SessionEntityOwners,
    result: Result<T, mc_entity::RegionOwnerLaneError>,
) -> T {
    owners.resolve(result)
}

impl EntityOwnerAccess {
    fn resolve<T>(&self, result: Result<T, mc_entity::RegionOwnerLaneError>) -> T {
        resolve_owner_result(&self.failure, &self.handle, result)
    }

    fn try_call<T>(
        &self,
        call: impl FnOnce(&RegionalOwnerHandle) -> Result<T, mc_entity::RegionOwnerLaneError>,
    ) -> Result<T, mc_entity::RegionOwnerLaneError> {
        try_owner_call(&self.failure, &self.handle, call)
    }
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
        let selected = self.resolve(self.handle.snapshots_for_ids_versioned(&missing));
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
        self.pending_goal_kinematics.borrow_mut().take();
    }

    /// Refreshes the cached snapshots for `ids` from a versioned owner batch
    /// fetched without holding the session publication lock, and retains and
    /// returns the opaque versioned fence for that batch.
    pub(super) fn refresh_publication_snapshots(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Option<VersionedEntitySnapshots> {
        if ids.is_empty() {
            return None;
        }
        {
            let mut cached = self.snapshots.borrow_mut();
            for id in ids {
                cached.remove(id);
            }
        }
        self.selected_snapshots.borrow_mut().take();
        let selected = self.resolve(self.handle.snapshots_for_ids_versioned(ids));
        #[cfg(test)]
        self.record_owner_request();
        let mut cached = self.snapshots.borrow_mut();
        for id in ids {
            cached.insert(*id, None);
        }
        for snapshot in selected.snapshots() {
            cached.insert(snapshot.id, Some(snapshot.clone()));
        }
        drop(cached);
        self.selected_snapshots.replace(Some(selected.clone()));
        Some(selected)
    }

    /// Checks only the captured routing/version fence and never fetches entity
    /// snapshots, so this remains safe while the session publication lock is held.
    pub(super) fn versioned_snapshots_are_current(
        &self,
        selected: &VersionedEntitySnapshots,
    ) -> bool {
        matches!(
            self.try_call(|handle| handle.versioned_snapshots_are_current(selected)),
            Ok(true)
        )
    }

    pub(super) fn snapshots_vec(&self) -> Vec<EntitySnapshot> {
        #[cfg(test)]
        self.record_owner_request();
        self.resolve(self.handle.snapshots())
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.record_owner_request();
        self.resolve(self.handle.status()).entity_count
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
        let snapshot = self.resolve(self.handle.snapshot(id));
        self.snapshots.borrow_mut().insert(id, snapshot.clone());
        snapshot
    }

    pub(super) fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        {
            let cached = self.snapshots.borrow();
            if let Some(snapshot) = cached.get(&id).and_then(Option::as_ref) {
                return Some(motion_state_from_snapshot(snapshot));
            }
        }
        #[cfg(test)]
        self.record_owner_request();
        let snapshot = self.resolve(self.handle.snapshot(id));
        self.snapshots.borrow_mut().insert(id, snapshot.clone());
        Some(motion_state_from_snapshot(snapshot.as_ref()?))
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

    /// Mirrors the store-side parallel kinematics admission gate in
    /// `mc-entity`: only batches this large justify per-tick worker fan-out.
    const PARALLEL_KINEMATICS_MIN_STATES: usize = 257;

    /// Cheap local admission check for parallel kinematics commits. It never
    /// queries the owner: batch size and id uniqueness decide, while upstream
    /// CPU permit acquisition and the owner's cached-route fallbacks cap the
    /// actual concurrency.
    pub(super) fn parallel_kinematics_batch_count(&self, states: &[EntityKinematics]) -> usize {
        if states.len() < Self::PARALLEL_KINEMATICS_MIN_STATES {
            return 0;
        }
        let mut ids = HashSet::with_capacity(states.len());
        if states.iter().any(|state| !ids.insert(state.id)) {
            return 0;
        }
        states.len()
    }

    pub(super) fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        #[cfg(test)]
        self.record_owner_request();
        let id = self.resolve(self.handle.spawn(entity));
        self.observation.record_entity_inserts(1);
        id
    }

    pub(super) fn spawn_deferred_journal(&mut self, entity: SpawnEntity) -> EntityId {
        #[cfg(test)]
        self.record_owner_request();
        let id = self.resolve(self.handle.spawn_deferred_journal(entity));
        self.observation.record_entity_inserts(1);
        id
    }

    pub(super) fn spawn_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntityId> {
        let result = self.try_spawn_batch(entities);
        self.resolve(result)
    }

    pub(super) fn try_spawn_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, mc_entity::RegionOwnerLaneError> {
        #[cfg(test)]
        self.record_owner_request();
        let ids = self.try_call(|handle| handle.spawn_batch(entities))?;
        self.observation.record_entity_inserts(ids.len());
        Ok(ids)
    }

    pub(super) fn spawn_unique_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntitySnapshot> {
        let result = self.try_spawn_unique_batch(entities);
        self.resolve(result)
    }

    pub(super) fn try_spawn_unique_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, mc_entity::RegionOwnerLaneError> {
        #[cfg(test)]
        self.record_owner_request();
        let snapshots = self.try_call(|handle| handle.spawn_unique_batch(entities))?;
        self.observation.record_entity_inserts(snapshots.len());
        Ok(snapshots)
    }

    pub(super) fn insert_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        let expected = snapshots.len();
        let inserted = self.resolve(self.handle.insert_snapshots_batch(snapshots));
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
        let applied = self.resolve(
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
        let applied = self.resolve(self.handle.set_animal_states_if_current(states));
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
        let applied = self.resolve(
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
        let applied = self.resolve(self.handle.set_goal(id, goal));
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
        let applied = self.resolve(self.handle.set_goals_deferred_journal(goals));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    #[cfg(test)]
    pub(super) fn set_item_stack_if_current(
        &mut self,
        expected: EntitySnapshot,
        stack: Option<EntityItemStack>,
    ) -> bool {
        let id = expected.id;
        let applied = self.resolve(self.handle.set_item_stack_if_current(expected, stack));
        self.invalidate(id);
        applied
    }

    pub(super) fn resolve_item_pickup_claim(
        &mut self,
        entity: EntityId,
        claim: u64,
        stack: Option<EntityItemStack>,
    ) -> Option<mc_entity::ItemPickupClaimResolution> {
        let resolved = self.resolve(
            self.handle
                .resolve_item_pickup_claim_deferred_journal(entity, claim, stack),
        );
        self.invalidate(entity);
        resolved
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
        let applied = self.resolve(self.handle.apply_kinematics_if_current(states));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn damage_if_current(
        &mut self,
        expected: EntitySnapshot,
        request: EntityDamageRequest,
    ) -> Option<mc_entity::EntityDamage> {
        let id = expected.id;
        let damage = self.resolve(self.handle.damage_if_current(expected, request));
        self.invalidate(id);
        damage
    }

    pub(super) fn damage_batch_if_current(
        &mut self,
        requests: impl IntoIterator<Item = (EntitySnapshot, EntityDamageRequest)>,
    ) -> Vec<mc_entity::EntityDamage> {
        let requests = requests.into_iter().collect::<Vec<_>>();
        let ids = requests
            .iter()
            .map(|(snapshot, _)| snapshot.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let damages = self.resolve(self.handle.damage_batch_if_current(requests));
        for id in ids {
            self.invalidate(id);
        }
        damages
    }

    pub(super) fn apply_effect_if_current(
        &mut self,
        expected: EntitySnapshot,
        request: mc_entity::EntityEffectRequest,
    ) -> mc_entity::EntityEffectResult {
        let id = expected.id;
        let result = self.resolve(self.handle.apply_effect_if_current(expected, request));
        self.invalidate(id);
        result
    }

    pub(super) fn remove_if_current(&mut self, expected: EntitySnapshot) -> Option<EntitySnapshot> {
        let id = expected.id;
        let removed = self.resolve(self.handle.remove_if_current(expected));
        if removed.is_some() {
            self.observation.record_entity_remove();
            self.snapshots.borrow_mut().insert(id, None);
        } else {
            self.invalidate(id);
        }
        removed
    }

    pub(super) fn replace_snapshot_if_current(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> bool {
        let id = expected.id;
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.replace_snapshot_if_current(expected, next));
        self.invalidate(id);
        applied
    }

    pub(super) fn convert_snapshot_if_current(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> bool {
        let id = expected.id;
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.convert_snapshot_if_current(expected, next));
        self.invalidate(id);
        applied
    }

    pub(super) fn replace_snapshot_if_current_deferred_journal(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> bool {
        let id = expected.id;
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(
            self.handle
                .replace_snapshot_if_current_deferred_journal(expected, next),
        );
        self.invalidate(id);
        applied
    }

    pub(super) fn replace_snapshots_if_current(
        &mut self,
        snapshots: impl IntoIterator<Item = (EntitySnapshot, EntitySnapshot)>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        let ids = snapshots
            .iter()
            .map(|(expected, _)| expected.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.replace_snapshots_if_current(snapshots));
        for id in ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn merge_item_snapshots_if_current(
        &mut self,
        survivor_expected: EntitySnapshot,
        survivor_next: EntitySnapshot,
        consumed_expected: EntitySnapshot,
    ) -> bool {
        let survivor_id = survivor_expected.id;
        let consumed_id = consumed_expected.id;
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.merge_item_snapshots_if_current(
            survivor_expected,
            survivor_next,
            consumed_expected,
        ));
        self.invalidate(survivor_id);
        if applied {
            self.observation.record_entity_remove();
            self.snapshots.borrow_mut().insert(consumed_id, None);
        } else {
            self.invalidate(consumed_id);
        }
        applied
    }

    pub(super) fn commit_villager_inventory_pickup_if_current(
        &mut self,
        commit: mc_entity::VillagerInventoryPickupCommit,
    ) -> bool {
        let villager_id = commit.villager.0.id;
        let item_id = commit.item.0.id;
        let item_removed = commit.item.1.is_none();
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(
            self.handle
                .commit_villager_inventory_pickup_if_current(commit),
        );
        self.invalidate(villager_id);
        self.invalidate(item_id);
        if applied && item_removed {
            self.observation.record_entity_remove();
            self.snapshots.borrow_mut().insert(item_id, None);
        }
        applied
    }

    pub(super) fn commit_villager_food_share_if_current(
        &mut self,
        commit: mc_entity::VillagerFoodShareCommit,
    ) -> Option<EntitySnapshot> {
        let donor_id = commit.donor.0.id;
        let recipient_id = commit.recipient.id;
        #[cfg(test)]
        self.record_owner_request();
        let thrown = self.resolve(self.handle.commit_villager_food_share_if_current(commit));
        self.invalidate(donor_id);
        self.invalidate(recipient_id);
        if let Some(thrown) = &thrown {
            self.observation.record_entity_inserts(1);
            self.invalidate(thrown.id);
        }
        thrown
    }

    pub(super) fn commit_villager_courtship_if_current(
        &mut self,
        commit: mc_entity::VillagerCourtshipCommit,
    ) -> bool {
        let parent_ids = commit
            .parents
            .iter()
            .map(|(expected, _)| expected.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.commit_villager_courtship_if_current(commit));
        for id in parent_ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn commit_villager_no_bed_if_current(
        &mut self,
        commit: mc_entity::VillagerNoBedCommit,
    ) -> bool {
        let parent_ids = commit
            .parents
            .iter()
            .map(|(expected, _)| expected.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let applied = self.resolve(self.handle.commit_villager_no_bed_if_current(commit));
        for id in parent_ids {
            self.invalidate(id);
        }
        applied
    }

    pub(super) fn commit_villager_birth_if_current(
        &mut self,
        commit: mc_entity::VillagerBirthCommit,
    ) -> Option<EntitySnapshot> {
        let parent_ids = commit
            .parents
            .iter()
            .map(|(expected, _)| expected.id)
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.record_owner_request();
        let child = self.resolve(self.handle.commit_villager_birth_if_current(commit));
        for id in parent_ids {
            self.invalidate(id);
        }
        if let Some(child) = &child {
            self.observation.record_entity_inserts(1);
            self.invalidate(child.id);
        }
        child
    }

    fn apply_kinematics_conditionally(
        &mut self,
        states: Vec<EntityKinematics>,
        commit: impl FnOnce(
            &RegionalOwnerHandle,
            Vec<(EntitySnapshot, EntityKinematics)>,
        ) -> Result<
            (Vec<EntityKinematics>, Option<VersionedEntitySnapshots>),
            mc_entity::RegionOwnerLaneError,
        >,
    ) -> Vec<EntityKinematics> {
        #[cfg(feature = "load-bench")]
        let mut profile = Some(KinematicsProfile::started());
        let ids = states.iter().map(|state| state.id).collect::<HashSet<_>>();
        #[cfg(feature = "load-bench")]
        {
            if states.len() <= 256 {
                profile = None;
            } else if let Some(profile) = profile.as_mut() {
                let collect_ids_us = profile.end_phase();
                profile.collect_ids_us = collect_ids_us;
            }
        }
        self.prefetch(&ids);
        #[cfg(feature = "load-bench")]
        {
            if let Some(profile) = profile.as_mut() {
                let prefetch_us = profile.end_phase();
                profile.prefetch_us = prefetch_us;
            }
        }
        // `prefetch` left a cache entry for every requested id: `Some` for
        // entities the owner still holds and `None` for absent ones. Clone
        // each present snapshot exactly once, straight into `conditional`.
        let conditional = {
            let cache = self.snapshots.borrow();
            states
                .iter()
                .filter_map(|state| {
                    cache
                        .get(&state.id)
                        .and_then(Option::as_ref)
                        .map(|snapshot| (snapshot.clone(), *state))
                })
                .collect::<Vec<_>>()
        };
        #[cfg(feature = "load-bench")]
        {
            if let Some(profile) = profile.as_mut() {
                let conditional_build_us = profile.end_phase();
                profile.conditional_build_us = conditional_build_us;
            }
        }
        let conditional_count = conditional.len();
        if conditional.is_empty() {
            #[cfg(feature = "load-bench")]
            if let Some(profile) = profile.as_ref() {
                profile.emit(states.len(), conditional_count, conditional_count, 0, 0);
            }
            return Vec::new();
        }
        #[cfg(test)]
        self.record_owner_request();
        let (committed, committed_snapshots) = self.resolve(commit(&self.handle, conditional));
        #[cfg(feature = "load-bench")]
        {
            if let Some(profile) = profile.as_mut() {
                let regional_commit_us = profile.end_phase();
                profile.regional_commit_us = regional_commit_us;
            }
        }
        if committed.len() != conditional_count {
            #[cfg(feature = "load-bench")]
            if let Some(profile) = profile.as_ref() {
                profile.emit(
                    states.len(),
                    conditional_count,
                    conditional_count,
                    committed.len(),
                    0,
                );
            }
            return Vec::new();
        }
        // Every committed id was cloned from a `Some` cache entry above and
        // nothing else can touch this private cache while this method runs,
        // so the entry must still be present here.
        let mut cache = self.snapshots.borrow_mut();
        for state in &committed {
            let snapshot = cache
                .get_mut(&state.id)
                .and_then(Option::as_mut)
                .expect("prefetch cached a present snapshot for every committed id");
            snapshot.position = state.position;
            snapshot.rotation = state.rotation;
            snapshot.velocity = state.velocity;
            snapshot.on_ground = state.on_ground;
        }
        drop(cache);
        if let Some(selected) = committed_snapshots {
            self.selected_snapshots.replace(Some(selected));
        }
        #[cfg(feature = "load-bench")]
        {
            if let Some(profile) = profile.as_mut() {
                let cache_update_us = profile.end_phase();
                profile.emit(
                    states.len(),
                    conditional_count,
                    conditional_count,
                    committed.len(),
                    cache_update_us,
                );
            }
        }
        committed
    }

    pub(super) fn apply_kinematics_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> Vec<EntityKinematics> {
        let states = states.into_iter().collect::<Vec<_>>();
        if let Some(expected) = self.pending_goal_kinematics.borrow_mut().take() {
            #[cfg(test)]
            self.record_owner_request();
            if let Some(committed) = self.resolve(
                self.handle
                    .try_apply_kinematics_if_versioned_deferred_journal(expected, states.clone()),
            ) {
                return committed;
            }
        }
        self.apply_kinematics_conditionally(states, |handle, conditional| {
            handle
                .apply_kinematics_if_current_deferred_journal_committed(conditional)
                .map(|committed| (committed, None))
        })
    }

    pub(super) fn apply_kinematics_parallel_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
    ) -> Vec<EntityKinematics> {
        let states = states.into_iter().collect::<Vec<_>>();
        if let Some(expected) = self.pending_goal_kinematics.borrow_mut().take() {
            #[cfg(test)]
            self.record_owner_request();
            if let Some(committed) = self.resolve(
                self.handle
                    .try_apply_kinematics_if_versioned_deferred_journal(expected, states.clone()),
            ) {
                return committed;
            }
        }
        self.apply_kinematics_conditionally(states, |handle, conditional| {
            if let Some(selected) = handle
                .try_apply_kinematics_if_current_parallel_deferred_journal_versioned(
                    &conditional,
                    max_workers,
                )?
            {
                let committed = selected
                    .snapshots()
                    .iter()
                    .map(|snapshot| EntityKinematics {
                        id: snapshot.id,
                        position: snapshot.position,
                        rotation: snapshot.rotation,
                        velocity: snapshot.velocity,
                        on_ground: snapshot.on_ground,
                    })
                    .collect();
                return Ok((committed, Some(selected)));
            }
            handle
                .apply_kinematics_if_current_deferred_journal_committed(conditional)
                .map(|committed| (committed, None))
        })
    }

    #[cfg(test)]
    pub(super) fn apply_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        self.apply_kinematics_authoritative(states).len()
    }

    pub(super) fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> mc_entity::RegionalPreparedGoalTick {
        self.pending_goal_kinematics.borrow_mut().take();
        #[cfg(test)]
        self.record_owner_request();
        let selected = self.selected_snapshots.borrow_mut().take();
        self.resolve(match selected {
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
        let result = self.resolve(
            self.handle
                .apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(resolved, ids),
        );
        let mut cache = self.snapshots.borrow_mut();
        for id in ids {
            cache.remove(id);
        }
        self.selected_snapshots.borrow_mut().take();
        result.map(|(stats, states, versioned)| {
            self.pending_goal_kinematics.replace(versioned);
            (stats, states)
        })
    }

    pub(super) fn visit_simulation_entities(
        &self,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        for snapshot in self.snapshots_vec() {
            visitor(entity_snapshot_view(&snapshot));
        }
    }

    pub(super) fn simulation_projections_for_ids(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Vec<mc_entity::EntitySimulationProjection> {
        #[cfg(test)]
        self.record_owner_request();
        self.resolve(self.handle.simulation_projections_for_ids(ids))
    }

    pub(super) fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(mc_entity::EntityView<'_>),
    ) {
        self.prefetch(ids);
        let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
        ordered_ids.sort_unstable();
        let cache = self.snapshots.borrow();
        for id in ordered_ids {
            if let Some(Some(snapshot)) = cache.get(&id) {
                visitor(entity_snapshot_view(snapshot));
            }
        }
    }

    pub(super) fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(&EntitySnapshot),
    ) {
        #[cfg(test)]
        self.record_owner_request();
        for snapshot in self
            .resolve(self.handle.snapshots_for_ids(ids))
            .into_iter()
            .filter(|snapshot| {
                snapshot.lifecycle == EntityLifecycle::Alive
                    && snapshot.type_name == "minecraft:sheep"
                    && snapshot
                        .animal
                        .is_some_and(|animal| animal.sheep_wool.is_some())
            })
        {
            visitor(&snapshot);
        }
    }
}

fn motion_state_from_snapshot(snapshot: &EntitySnapshot) -> EntityMotionState {
    EntityMotionState {
        arrow_revision: snapshot
            .retained
            .arrow_state
            .as_ref()
            .map(|state| state.projectile.revision),
        arrow_embedded_block: snapshot
            .retained
            .arrow_state
            .as_ref()
            .filter(|state| state.in_ground)
            .and_then(|state| state.last_block_position),
        id: snapshot.id,
        position: snapshot.position,
        rotation: snapshot.rotation,
        velocity: snapshot.velocity,
        on_ground: snapshot.on_ground,
        fall_distance: snapshot.retained.fall_distance,
        goal_fence: mc_entity::EntityGoalFence::from_goal(&snapshot.goal),
        is_item: snapshot.type_name == "minecraft:item",
        is_experience: snapshot.type_name == "minecraft:experience_orb",
        is_arrow: snapshot.type_name == "minecraft:arrow",
        is_hurting_projectile: snapshot.retained.hurting_projectile_state.is_some(),
        hurting_projectile_revision: snapshot
            .retained
            .hurting_projectile_state
            .as_ref()
            .map(|state| state.projectile.revision),
        is_throwable_projectile: snapshot.retained.throwable_projectile_state.is_some(),
        throwable_projectile_revision: snapshot
            .retained
            .throwable_projectile_state
            .as_ref()
            .map(|state| state.projectile.revision),
        sends_velocity: !matches!(
            snapshot.type_name.as_str(),
            "minecraft:item" | "minecraft:experience_orb"
        ),
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
        retained: snapshot.retained.clone(),
    }
}

#[cfg(feature = "load-bench")]
struct KinematicsProfile {
    started_at: std::time::Instant,
    phase_started_at: std::time::Instant,
    collect_ids_us: u64,
    prefetch_us: u64,
    conditional_build_us: u64,
    regional_commit_us: u64,
}

#[cfg(feature = "load-bench")]
impl KinematicsProfile {
    fn started() -> Self {
        let now = std::time::Instant::now();
        Self {
            started_at: now,
            phase_started_at: now,
            collect_ids_us: 0,
            prefetch_us: 0,
            conditional_build_us: 0,
            regional_commit_us: 0,
        }
    }

    fn end_phase(&mut self) -> u64 {
        let now = std::time::Instant::now();
        let us = now.duration_since(self.phase_started_at).as_micros() as u64;
        self.phase_started_at = now;
        us
    }

    fn emit(
        &self,
        states_count: usize,
        expected_count: usize,
        conditional_count: usize,
        committed_count: usize,
        cache_update_us: u64,
    ) {
        let total_us = self.started_at.elapsed().as_micros() as u64;
        eprintln!(
            "OWNER_KINEMATICS_PROFILE states={states_count} expected={expected_count} \
             conditional={conditional_count} committed={committed_count} \
             collect_ids_us={} prefetch_us={} conditional_build_us={} \
             regional_commit_us={} cache_update_us={cache_update_us} \
             total_us={total_us}",
            self.collect_ids_us,
            self.prefetch_us,
            self.conditional_build_us,
            self.regional_commit_us,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_owner_result_reports_runtime_fatal_and_returns_typed_errors() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let mut failure = owners.subscribe_failure();

        assert_eq!(
            owners.try_resolve::<()>(Err(mc_entity::RegionOwnerLaneError::OutcomeUnknown)),
            Err(mc_entity::RegionOwnerLaneError::OutcomeUnknown)
        );
        assert_eq!(
            failure
                .borrow_and_update()
                .as_ref()
                .map(|fatal| fatal.error),
            Some(mc_entity::RegionOwnerLaneError::OutcomeUnknown)
        );
        assert_eq!(
            owners.try_resolve(Ok(7_u8)),
            Err(mc_entity::RegionOwnerLaneError::OutcomeUnknown),
            "the first fatal state blocks later owner calls before dispatch"
        );
    }

    #[test]
    fn first_owner_fatal_is_published_before_typed_unwind_and_blocks_future_calls() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let mut failure = owners.subscribe_failure();
        let first = owners.report_failure(mc_entity::RegionOwnerLaneError::WorkerPanicked);
        let repeated = owners.report_failure(mc_entity::RegionOwnerLaneError::OutcomeUnknown);

        assert_eq!(
            repeated, first,
            "the first fatal state remains authoritative"
        );
        assert_eq!(*failure.borrow_and_update(), Some(first));

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owners.status()))
            .expect_err("future owner calls must unwind after publishing fatal state");
        let typed = panic
            .downcast_ref::<EntityOwnerFatalPanic>()
            .expect("owner failure uses the typed panic payload");
        assert_eq!(typed.0, first);
        assert_eq!(*failure.borrow(), Some(first));
    }

    fn kinematics_for(id: EntityId, position: Vec3, source: &EntitySnapshot) -> EntityKinematics {
        EntityKinematics {
            id,
            position,
            rotation: source.rotation,
            velocity: Vec3::ZERO,
            on_ground: true,
        }
    }

    struct WalkableGoalProbe;

    impl mc_entity::PathingProbe for WalkableGoalProbe {
        fn can_stand_at(&self, _position: Vec3) -> mc_entity::PathingProbeResult {
            mc_entity::PathingProbeResult::Walkable
        }
    }

    #[test]
    fn post_goal_dense_kinematics_uses_one_fenced_owner_commit() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let mut access = owners.access();
        let ids = (0..32)
            .map(|index| {
                access.spawn(SpawnEntity::new(
                    1,
                    "minecraft:zombie",
                    Vec3::new(0.5 + f64::from(index), 64.0, 0.5),
                ))
            })
            .collect::<HashSet<_>>();
        let prepared = access.prepare_goal_tick_with_pathing_for_ids(1, &ids);
        let resolved = prepared.resolve(&WalkableGoalProbe, mc_entity::PathingBudget::DEFAULT);
        let (_, post_goal) = access
            .apply_prepared_goal_tick_and_alive_kinematics(resolved, &ids)
            .expect("goal apply");
        owners.reset_owner_requests_for_test();
        let targets = post_goal
            .iter()
            .map(|state| EntityKinematics {
                position: Vec3::new(state.position.x + 0.25, state.position.y, state.position.z),
                ..*state
            })
            .collect::<Vec<_>>();

        assert_eq!(
            access.apply_kinematics_authoritative(targets.clone()),
            targets
        );
        assert_eq!(
            owners.owner_requests_for_test(),
            1,
            "the fenced commit must avoid the snapshot prefetch plus full-snapshot CAS"
        );
    }

    #[test]
    fn post_goal_region_crossing_uses_existing_snapshot_cas_fallback() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let mut access = owners.access();
        let id = access.spawn(SpawnEntity::new(
            1,
            "minecraft:zombie",
            Vec3::new(0.5, 64.0, 0.5),
        ));
        let ids = HashSet::from([id]);
        let resolved = access
            .prepare_goal_tick_with_pathing_for_ids(1, &ids)
            .resolve(&WalkableGoalProbe, mc_entity::PathingBudget::DEFAULT);
        access
            .apply_prepared_goal_tick_and_alive_kinematics(resolved, &ids)
            .expect("goal apply");
        owners.reset_owner_requests_for_test();
        let crossing = EntityKinematics {
            id,
            position: Vec3::new(128.5, 64.0, 0.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
        };

        assert_eq!(
            access.apply_kinematics_authoritative([crossing]),
            vec![crossing]
        );
        assert!(
            owners.owner_requests_for_test() > 1,
            "crossing must reject the fence and use the snapshot CAS requests"
        );
        assert_eq!(
            access.snapshot(id).expect("crossed snapshot").position,
            crossing.position
        );
    }

    #[test]
    fn parallel_kinematics_batch_count_admits_only_large_unique_batches() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let access = owners.access();
        let kinematics = (0..257)
            .map(|index| EntityKinematics {
                id: EntityId(index),
                position: Vec3::new(0.5, 64.0, 0.5),
                rotation: mc_entity::Rotation::ZERO,
                velocity: Vec3::ZERO,
                on_ground: true,
            })
            .collect::<Vec<_>>();
        assert_eq!(access.parallel_kinematics_batch_count(&kinematics), 257);

        let mut duplicated = kinematics;
        duplicated[256].id = duplicated[0].id;
        assert_eq!(access.parallel_kinematics_batch_count(&duplicated), 0);
        assert_eq!(
            access.parallel_kinematics_batch_count(&duplicated[..256]),
            0
        );
    }

    #[test]
    fn parallel_kinematics_commits_current_and_stale_batch_keeps_cache() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 2, None)
                .expect("test entity owners");
        let mut access = owners.access();
        let id = access.spawn(SpawnEntity::new(
            1,
            "minecraft:zombie",
            Vec3::new(0.5, 64.0, 0.5),
        ));
        let expected = access.snapshot(id).expect("spawned snapshot");
        let moved = kinematics_for(id, Vec3::new(0.75, 64.0, 0.5), &expected);

        assert_eq!(
            access.apply_kinematics_parallel_authoritative([moved], 2),
            vec![moved]
        );
        assert_eq!(
            access.snapshot(id).expect("cached snapshot").position,
            moved.position,
            "the commit updates the session cache in place"
        );
        access.invalidate(id);
        assert_eq!(
            access.snapshot(id).expect("committed snapshot").position,
            moved.position
        );

        let current = access.snapshot(id).expect("current snapshot");
        let newer = kinematics_for(id, Vec3::new(1.5, 64.0, 0.5), &current);
        owners
            .handle
            .apply_kinematics_if_current_deferred_journal_committed([(current, newer)])
            .expect("underlying owner commit");

        let stale = kinematics_for(id, Vec3::new(1.25, 64.0, 0.5), &expected);
        assert!(
            access
                .apply_kinematics_parallel_authoritative([stale], 2)
                .is_empty(),
            "all-or-none: a stale expected snapshot commits nothing"
        );
        assert_eq!(
            access.snapshot(id).expect("cached snapshot").position,
            moved.position,
            "an all-or-none miss must not overwrite the session cache"
        );
    }

    #[test]
    fn parallel_kinematics_retains_current_versioned_snapshots() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 2, None)
                .expect("test entity owners");
        let mut access = owners.access();
        let first = access.spawn(SpawnEntity::new(
            1,
            "minecraft:zombie",
            Vec3::new(0.5, 64.0, 0.5),
        ));
        let second = access.spawn(SpawnEntity::new(
            2,
            "minecraft:zombie",
            Vec3::new(128.5, 64.0, 0.5),
        ));
        let first_snapshot = access.snapshot(first).expect("first snapshot");
        let second_snapshot = access.snapshot(second).expect("second snapshot");
        let first_move = kinematics_for(first, Vec3::new(0.75, 64.0, 0.5), &first_snapshot);
        let second_move = kinematics_for(second, Vec3::new(128.75, 64.0, 0.5), &second_snapshot);

        let committed =
            access.apply_kinematics_parallel_authoritative([first_move, second_move], 2);
        assert_eq!(committed.len(), 2);
        let selected = access
            .selected_snapshots
            .borrow()
            .clone()
            .expect("parallel fast path retains committed versioned snapshots");
        assert_eq!(selected.snapshots().len(), 2);
        assert!(
            access.versioned_snapshots_are_current(&selected),
            "the retained fence must describe the post-commit owner state"
        );
    }

    #[test]
    fn kinematics_batch_skips_absent_ids_and_stale_commit_keeps_cache() {
        let owners =
            SessionEntityOwners::try_new(Arc::new(SessionPressureObservation::default()), 1, None)
                .expect("test entity owners");
        let mut access = owners.access();
        let id = access.spawn(SpawnEntity::new(
            1,
            "minecraft:zombie",
            Vec3::new(0.5, 64.0, 0.5),
        ));
        let expected = access.snapshot(id).expect("spawned snapshot");
        let absent = EntityId(i32::MAX);

        assert!(
            access
                .apply_kinematics_authoritative([kinematics_for(
                    absent,
                    Vec3::new(9.5, 64.0, 9.5),
                    &expected
                )])
                .is_empty(),
            "a batch of only absent ids commits nothing"
        );
        assert!(
            access.snapshot(absent).is_none(),
            "absent ids stay absent from the session cache"
        );

        let moved = kinematics_for(id, Vec3::new(0.75, 64.0, 0.5), &expected);
        let committed = access.apply_kinematics_authoritative([
            moved,
            kinematics_for(absent, Vec3::new(8.5, 64.0, 8.5), &expected),
        ]);
        assert_eq!(
            committed,
            vec![moved],
            "absent ids are skipped; present ids commit in states order"
        );
        assert_eq!(
            access.snapshot(id).expect("cached snapshot").position,
            moved.position,
            "the present id updates the session cache in place"
        );

        let current = access.snapshot(id).expect("cached snapshot");
        let newer = kinematics_for(id, Vec3::new(1.5, 64.0, 0.5), &current);
        owners
            .handle
            .apply_kinematics_if_current_deferred_journal_committed([(current, newer)])
            .expect("underlying owner commit");

        let stale = kinematics_for(id, Vec3::new(1.25, 64.0, 0.5), &expected);
        assert!(
            access.apply_kinematics_authoritative([stale]).is_empty(),
            "all-or-none: a stale expected snapshot commits nothing"
        );
        assert_eq!(
            access.snapshot(id).expect("cached snapshot").position,
            moved.position,
            "an all-or-none miss must not overwrite the session cache"
        );
    }
}
