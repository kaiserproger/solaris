use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use arc_swap::ArcSwap;
use mc_entity::EntityId;

use super::outbound::{
    OrderedDispatchState, OutboundCommand, OutboundPressureMetrics, SessionRecipient,
};
use super::{PlaySession, SessionId};

#[derive(Debug)]
pub(super) struct PublishedEntityVisibility {
    current: Arc<HashSet<EntityId>>,
    published: Arc<ArcSwap<HashSet<EntityId>>>,
}

impl PublishedEntityVisibility {
    pub(super) fn new() -> Self {
        let current = Arc::new(HashSet::new());
        Self {
            published: Arc::new(ArcSwap::from(Arc::clone(&current))),
            current,
        }
    }

    pub(super) fn insert(&mut self, entity_id: EntityId) -> bool {
        Arc::make_mut(&mut self.current).insert(entity_id)
    }

    pub(super) fn remove(&mut self, entity_id: &EntityId) -> bool {
        Arc::make_mut(&mut self.current).remove(entity_id)
    }

    pub(super) fn replace(&mut self, entities: HashSet<EntityId>) {
        self.current = Arc::new(entities);
    }

    pub(super) fn snapshot(&self) -> Arc<HashSet<EntityId>> {
        Arc::clone(&self.current)
    }

    pub(super) fn publish(&self) {
        self.published.store(Arc::clone(&self.current));
    }

    pub(super) fn replace_and_publish(&mut self, entities: HashSet<EntityId>) {
        self.replace(entities);
        self.publish();
    }

    fn publication(&self) -> Arc<ArcSwap<HashSet<EntityId>>> {
        Arc::clone(&self.published)
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
    tx: tokio::sync::mpsc::Sender<OutboundCommand>,
    pressure: Arc<OutboundPressureMetrics>,
    ordered_dispatch: Arc<OrderedDispatchState>,
    visible_entities: Arc<ArcSwap<HashSet<EntityId>>>,
}

impl MovementRecipientPublication {
    fn from_session(id: SessionId, session: &PlaySession) -> Self {
        Self {
            id,
            tx: session.tx.clone(),
            pressure: Arc::clone(&session.pressure),
            ordered_dispatch: Arc::clone(&session.ordered_dispatch),
            visible_entities: session.visible_entities.publication(),
        }
    }

    pub(super) fn id(&self) -> SessionId {
        self.id
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
