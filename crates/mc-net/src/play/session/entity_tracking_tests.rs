use mc_entity::{EntityId, Rotation, Vec3};

use super::entity_tracking::{EntityMovementTrackers, LastSentEntityState};

fn state(position_x: f64) -> LastSentEntityState {
    LastSentEntityState {
        position: Vec3::new(position_x, 64.0, 0.5),
        velocity: Vec3::ZERO,
        rotation: Rotation::ZERO,
        on_ground: true,
        tracking_update_count: 0,
        teleport_delay: 0,
    }
}

#[test]
fn tracker_batch_accepts_only_current_states() {
    let trackers = EntityMovementTrackers::default();
    let current = state(0.5);
    let next = state(0.75);
    trackers.insert(EntityId(1), current);
    trackers.insert(EntityId(2), current);

    let accepted = trackers.compare_exchange_many(vec![
        (EntityId(1), current, next),
        (EntityId(2), state(8.0), next),
        (EntityId(3), current, next),
    ]);

    assert_eq!(accepted, [EntityId(1)].into_iter().collect());
    assert_eq!(trackers.get(EntityId(1)), Some(next));
    assert_eq!(trackers.get(EntityId(2)), Some(current));
    assert_eq!(trackers.get(EntityId(3)), None);
}

#[test]
fn removing_tracker_rejects_delayed_commit() {
    let trackers = EntityMovementTrackers::default();
    let current = state(0.5);
    trackers.insert(EntityId(7), current);
    trackers.remove(EntityId(7));

    assert!(!trackers.compare_exchange(EntityId(7), current, state(0.75)));
    assert!(trackers.is_empty());
}
