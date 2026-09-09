use crate::{EntityId, EntityStore, SpawnEntity, Vec3, VehicleKind};
use uuid::Uuid;

#[test]
fn batch_vehicle_links_accept_shared_chain_tails_and_reject_cycles_atomically() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(SpawnEntity::new(144, "minecraft:cow", Vec3::ZERO));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::ZERO,
    ));
    store.mount_vehicle(boat, passenger).unwrap();
    let template = store.snapshot(boat).unwrap();
    let incoming = |id, passenger| {
        let mut snapshot = template.clone();
        snapshot.id = EntityId(id);
        snapshot.uuid = Uuid::from_u128(id as u128);
        snapshot.vehicle.as_mut().unwrap().passenger = passenger;
        snapshot
    };

    assert!(store.insert_snapshots_batch([
        incoming(100, Some(EntityId(101))),
        incoming(101, Some(boat)),
    ]));
    assert_eq!(
        store.vehicle_for_passenger(EntityId(101)),
        Some(EntityId(100))
    );
    assert_eq!(store.vehicle_for_passenger(boat), Some(EntityId(101)));
    assert_eq!(store.vehicle_for_passenger(passenger), Some(boat));

    assert!(!store.insert_snapshots_batch([
        incoming(202, None),
        incoming(200, Some(EntityId(201))),
        incoming(201, Some(EntityId(200))),
    ]));
    assert!(
        [200, 201, 202]
            .into_iter()
            .all(|id| store.snapshot(EntityId(id)).is_none())
    );
    assert_eq!(store.snapshot(boat), Some(template));
    assert_eq!(store.vehicle_for_passenger(boat), Some(EntityId(101)));
}
