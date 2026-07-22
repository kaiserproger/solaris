use std::collections::HashSet;

use crate::{
    AttributeKind, AttributeSet, EntityId, EntityScale26_1_2, EntityStore, RegionalEntityStore,
    RegionalOwnerHandle, RegionalOwnerRuntime, SpawnEntity, Vec3,
};

fn cow(position: Vec3) -> SpawnEntity {
    SpawnEntity::new(17, "minecraft:cow", position)
}

fn apply_scale(handle: &RegionalOwnerHandle, id: EntityId, scale: EntityScale26_1_2) -> bool {
    let selected = handle
        .snapshots_for_ids_versioned(&HashSet::from([id]))
        .expect("versioned scale input");
    let expected = selected.snapshots()[0].clone();
    let mut next = expected.clone();
    next.set_scale_26_1_2(scale);
    handle
        .replace_snapshot_if_current(expected, next)
        .expect("scale snapshot CAS")
}

#[test]
fn scale_contract_accepts_default_and_exact_vanilla_bounds() {
    assert_eq!(
        EntityScale26_1_2::try_new(1.0).unwrap(),
        EntityScale26_1_2::DEFAULT
    );
    assert_eq!(
        EntityScale26_1_2::try_new(0.0625).unwrap(),
        EntityScale26_1_2::MIN
    );
    assert_eq!(
        EntityScale26_1_2::try_new(16.0).unwrap(),
        EntityScale26_1_2::MAX
    );
    assert_eq!(EntityScale26_1_2::try_new(2.5).unwrap().factor(), 2.5);
}

#[test]
fn scale_contract_rejects_out_of_range_and_nonfinite_values() {
    for invalid in [
        0.0,
        0.0625_f64.next_down(),
        16.0_f64.next_up(),
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert!(
            EntityScale26_1_2::try_new(invalid).is_err(),
            "accepted invalid scale {invalid:?}"
        );
    }
}

#[test]
fn missing_snapshot_scale_projects_the_vanilla_default() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
    entity.attributes = AttributeSet::new();
    let id = store.spawn(entity);
    let snapshot = store.snapshot(id).expect("spawned cow snapshot");

    assert_eq!(snapshot.scale_26_1_2(), EntityScale26_1_2::DEFAULT);
}

#[test]
fn malformed_snapshot_scale_projects_the_vanilla_ranged_attribute_sanitization() {
    for (stored, expected) in [
        (f64::NAN, EntityScale26_1_2::MIN),
        (f64::NEG_INFINITY, EntityScale26_1_2::MIN),
        (0.0, EntityScale26_1_2::MIN),
        (17.0, EntityScale26_1_2::MAX),
        (f64::INFINITY, EntityScale26_1_2::MAX),
    ] {
        let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
        entity.attributes.set_base(AttributeKind::Scale, stored);

        let mut store = EntityStore::new();
        let id = store.spawn(entity);

        assert_eq!(
            store
                .snapshot(id)
                .expect("malformed-scale cow snapshot")
                .scale_26_1_2(),
            expected
        );
    }
}

#[test]
fn regional_scale_apply_from_versioned_snapshot_rejects_stale_cas_and_persists() {
    let runtime = RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("regional owner runtime");
    let handle = runtime.handle();
    let id = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("spawn cow");
    let selected = handle
        .snapshots_for_ids_versioned(&HashSet::from([id]))
        .expect("versioned cow snapshot");
    let stale = selected.clone();
    let expected = selected.snapshots()[0].clone();
    let mut scaled = expected.clone();
    scaled.set_scale_26_1_2(EntityScale26_1_2::try_new(2.0).unwrap());

    assert!(
        handle
            .replace_snapshot_if_current(expected, scaled)
            .expect("apply live scale")
    );
    assert_eq!(
        handle
            .snapshot(id)
            .expect("read scaled cow")
            .expect("scaled cow exists")
            .scale_26_1_2(),
        EntityScale26_1_2::try_new(2.0).unwrap()
    );
    let stale_expected = stale.snapshots()[0].clone();
    let mut stale_next = stale_expected.clone();
    stale_next.set_scale_26_1_2(EntityScale26_1_2::MAX);
    assert!(
        !handle
            .replace_snapshot_if_current(stale_expected, stale_next)
            .expect("reject stale scale CAS")
    );

    assert!(apply_scale(&handle, id, EntityScale26_1_2::MIN));
    assert_eq!(
        handle
            .snapshot(id)
            .expect("read minimum-scale cow")
            .expect("minimum-scale cow exists")
            .scale_26_1_2(),
        EntityScale26_1_2::MIN
    );
    assert!(apply_scale(&handle, id, EntityScale26_1_2::MAX));

    let saved = handle.save_barrier().expect("save scaled cow");
    let saved_snapshot = saved
        .snapshots()
        .first()
        .expect("saved cow snapshot")
        .clone();
    assert_eq!(saved_snapshot.scale_26_1_2(), EntityScale26_1_2::MAX);

    drop(handle);
    runtime.shutdown().expect("shutdown regional owner");

    let mut restored = EntityStore::new();
    assert!(restored.insert_snapshot(saved_snapshot));
    assert_eq!(
        restored
            .snapshot(id)
            .expect("restored cow snapshot")
            .scale_26_1_2(),
        EntityScale26_1_2::MAX
    );
}
