use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use mc_entity::EntityId;

use super::outbound::{
    OrderedDispatchState, OutboundCommand, OutboundPressureMetrics, SessionRecipient,
};
use super::{PlaySession, PlayerPose, SessionId};

#[derive(Debug, Clone, Copy)]
pub(super) struct PublishedCombatTargetState {
    pose: PlayerPose,
    alive: bool,
    targetable: bool,
}

#[derive(Debug, Default)]
pub(super) struct SessionPublicationEpoch {
    revision: AtomicU64,
}

impl SessionPublicationEpoch {
    fn begin_update(&self) {
        let previous = self.revision.fetch_add(1, Ordering::AcqRel);
        debug_assert!(previous.is_multiple_of(2));
    }

    fn finish_update(&self) {
        let previous = self.revision.fetch_add(1, Ordering::Release);
        debug_assert!(!previous.is_multiple_of(2));
    }

    fn load(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}

impl PublishedCombatTargetState {
    pub(super) fn pose(self) -> PlayerPose {
        self.pose
    }

    pub(super) fn is_targetable(self) -> bool {
        self.targetable
    }

    pub(super) fn is_alive(self) -> bool {
        self.alive
    }
}

#[derive(Debug)]
pub(super) struct PublishedCombatTarget {
    published: Arc<ArcSwap<PublishedCombatTargetState>>,
    epoch: Arc<SessionPublicationEpoch>,
}

impl PublishedCombatTarget {
    pub(super) fn new(pose: PlayerPose, epoch: Arc<SessionPublicationEpoch>) -> Self {
        Self {
            published: Arc::new(ArcSwap::from_pointee(PublishedCombatTargetState {
                pose,
                alive: true,
                targetable: true,
            })),
            epoch,
        }
    }

    pub(super) fn publish(&self, pose: PlayerPose, alive: bool, targetable: bool) {
        self.epoch.begin_update();
        self.published.store(Arc::new(PublishedCombatTargetState {
            pose,
            alive,
            targetable,
        }));
        self.epoch.finish_update();
    }

    pub(super) fn close(&self, pose: PlayerPose) {
        self.publish(pose, false, false);
    }

    fn publication(&self) -> Arc<ArcSwap<PublishedCombatTargetState>> {
        Arc::clone(&self.published)
    }

    fn epoch(&self) -> Arc<SessionPublicationEpoch> {
        Arc::clone(&self.epoch)
    }
}

#[derive(Debug)]
pub(super) struct PublishedEntityVisibility {
    current: Arc<HashSet<EntityId>>,
    published: Arc<ArcSwap<HashSet<EntityId>>>,
    epoch: Arc<SessionPublicationEpoch>,
    updating: bool,
}

impl PublishedEntityVisibility {
    pub(super) fn new(epoch: Arc<SessionPublicationEpoch>) -> Self {
        let current = Arc::new(HashSet::new());
        Self {
            published: Arc::new(ArcSwap::from(Arc::clone(&current))),
            current,
            epoch,
            updating: false,
        }
    }

    pub(super) fn insert(&mut self, entity_id: EntityId) -> bool {
        if self.current.contains(&entity_id) {
            return false;
        }
        self.begin_update();
        Arc::make_mut(&mut self.current).insert(entity_id)
    }

    pub(super) fn remove(&mut self, entity_id: &EntityId) -> bool {
        if !self.current.contains(entity_id) {
            return false;
        }
        self.begin_update();
        Arc::make_mut(&mut self.current).remove(entity_id)
    }

    pub(super) fn replace(&mut self, entities: HashSet<EntityId>) {
        if *self.current == entities {
            return;
        }
        self.begin_update();
        self.current = Arc::new(entities);
    }

    pub(super) fn snapshot(&self) -> Arc<HashSet<EntityId>> {
        Arc::clone(&self.current)
    }

    pub(super) fn publish(&mut self) {
        if !self.updating {
            return;
        }
        self.published.store(Arc::clone(&self.current));
        self.updating = false;
        self.epoch.finish_update();
    }

    pub(super) fn replace_and_publish(&mut self, entities: HashSet<EntityId>) {
        self.replace(entities);
        self.publish();
    }

    fn publication(&self) -> Arc<ArcSwap<HashSet<EntityId>>> {
        Arc::clone(&self.published)
    }

    fn begin_update(&mut self) {
        if !self.updating {
            self.epoch.begin_update();
            self.updating = true;
        }
    }
}

impl Deref for PublishedEntityVisibility {
    type Target = HashSet<EntityId>;

    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

#[derive(Debug, Clone)]
pub(super) struct MovementRecipientPublication {
    id: SessionId,
    entity_id: i32,
    tx: tokio::sync::mpsc::Sender<OutboundCommand>,
    pressure: Arc<OutboundPressureMetrics>,
    ordered_dispatch: Arc<OrderedDispatchState>,
    visible_entities: Arc<ArcSwap<HashSet<EntityId>>>,
    combat_target: Arc<ArcSwap<PublishedCombatTargetState>>,
    publication_epoch: Arc<SessionPublicationEpoch>,
}

impl MovementRecipientPublication {
    fn from_session(id: SessionId, session: &PlaySession) -> Self {
        Self {
            id,
            entity_id: session.entity_id,
            tx: session.tx.clone(),
            pressure: Arc::clone(&session.pressure),
            ordered_dispatch: Arc::clone(&session.ordered_dispatch),
            visible_entities: session.visible_entities.publication(),
            combat_target: session.combat_target.publication(),
            publication_epoch: session.combat_target.epoch(),
        }
    }

    pub(super) fn id(&self) -> SessionId {
        self.id
    }

    pub(super) fn entity_id(&self) -> i32 {
        self.entity_id
    }

    pub(super) fn recipient(&self) -> SessionRecipient {
        SessionRecipient::ordered(
            self.id,
            self.tx.clone(),
            Arc::clone(&self.pressure),
            &self.ordered_dispatch,
        )
    }

    pub(super) fn visible_entities(&self) -> Arc<HashSet<EntityId>> {
        self.visible_entities.load_full()
    }

    pub(super) fn combat_target(&self) -> Arc<PublishedCombatTargetState> {
        self.combat_target.load_full()
    }

    pub(super) fn combat_target_snapshot(
        &self,
    ) -> Option<(PublishedCombatTargetState, Arc<HashSet<EntityId>>)> {
        let before = self.publication_epoch.load();
        if !before.is_multiple_of(2) {
            return None;
        }
        let target = *self.combat_target();
        let visible_entities = self.visible_entities();
        (self.publication_epoch.load() == before).then_some((target, visible_entities))
    }

    pub(super) fn reserve_combat_recipient_if(
        &self,
        validate: impl FnOnce(PublishedCombatTargetState, &HashSet<EntityId>) -> bool,
    ) -> Option<(PublishedCombatTargetState, SessionRecipient)> {
        let (target, mut recipients) = self.reserve_combat_recipients_if(1, validate)?;
        Some((
            target,
            recipients
                .pop()
                .expect("one requested combat recipient is reserved"),
        ))
    }

    pub(super) fn reserve_combat_recipients_if(
        &self,
        count: usize,
        validate: impl FnOnce(PublishedCombatTargetState, &HashSet<EntityId>) -> bool,
    ) -> Option<(PublishedCombatTargetState, Vec<SessionRecipient>)> {
        let before = self.publication_epoch.load();
        if count == 0 || !before.is_multiple_of(2) {
            return None;
        }
        let target = *self.combat_target();
        let visible_entities = self.visible_entities();
        if !validate(target, &visible_entities) {
            return None;
        }
        let recipients = (0..count).map(|_| self.recipient()).collect();
        if self.publication_epoch.load() != before {
            return None;
        }
        Some((target, recipients))
    }

    pub(super) fn reserve_observer_if_visible(
        &self,
        entity_id: EntityId,
    ) -> Option<SessionRecipient> {
        let before = self.publication_epoch.load();
        if !before.is_multiple_of(2) || !self.visible_entities().contains(&entity_id) {
            return None;
        }
        let recipient = self.recipient();
        if self.publication_epoch.load() != before {
            return None;
        }
        Some(recipient)
    }

    pub(super) fn is_same_session(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.ordered_dispatch, &other.ordered_dispatch)
    }
}

pub(super) type MovementRecipientIndex = HashMap<SessionId, MovementRecipientPublication>;

pub(super) fn build_movement_recipient_index(
    sessions: &HashMap<SessionId, PlaySession>,
) -> MovementRecipientIndex {
    sessions
        .iter()
        .map(|(&id, session)| (id, MovementRecipientPublication::from_session(id, session)))
        .collect()
}
