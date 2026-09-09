use super::*;

fn assert_vehicle_group_cross_lane_migration(journal_commit: bool) {
    let mut source = RegionalEntityStore::new();
    let source_lease = source
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("source region");
    let phase = source.begin_phase().expect("source phase");
    let passenger = source
        .spawn(phase, source_lease, cow(Vec3::new(127.0, 64.0, 0.5)))
        .expect("passenger");
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(passenger);
    let boat = source.spawn(phase, source_lease, boat).expect("boat");
    let snapshots = source.snapshots().collect::<Vec<_>>();
    let mut coordinator =
        super::super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("owner coordinator");
    coordinator
        .insert_snapshots_batch(snapshots)
        .expect("restore vehicle group");
    let expected_boat = coordinator
        .snapshot(boat)
        .expect("boat read")
        .expect("boat snapshot");
    let mut expected_passenger = coordinator
        .snapshot(passenger)
        .expect("passenger read")
        .expect("passenger snapshot");
    let before_turn = expected_passenger.clone();
    expected_passenger.rotation.yaw = 45.0;
    assert!(
        coordinator
            .replace_snapshot_if_current(before_turn, expected_passenger.clone())
            .expect("in-place passenger turn")
    );
    let states = [
        (
            expected_passenger,
            movement(passenger, Vec3::new(130.0, 64.0, 0.5)),
        ),
        (expected_boat, movement(boat, Vec3::new(128.5, 64.0, 0.5))),
    ];

    let applied = if journal_commit {
        coordinator.apply_kinematics_if_current(states)
    } else {
        coordinator.apply_kinematics_if_current_inner(states, false)
    };
    assert!(applied.expect("vehicle migration"));
    let moved_boat = coordinator
        .snapshot(boat)
        .expect("moved boat read")
        .expect("moved boat");
    let moved_passenger = coordinator
        .snapshot(passenger)
        .expect("moved passenger read")
        .expect("moved passenger");
    assert_eq!(moved_boat.position, Vec3::new(128.5, 64.0, 0.5));
    assert_eq!(moved_passenger.position, Vec3::new(128.0, 64.0, 0.5));
    assert_eq!(
        moved_boat.vehicle.and_then(|vehicle| vehicle.passenger),
        Some(passenger)
    );
}

#[test]
fn owner_coordinator_moves_vehicle_group_across_lanes_with_leader_delta() {
    assert_vehicle_group_cross_lane_migration(true);
}

#[test]
fn deferred_owner_commit_moves_vehicle_group_across_lanes_atomically() {
    assert_vehicle_group_cross_lane_migration(false);
}
