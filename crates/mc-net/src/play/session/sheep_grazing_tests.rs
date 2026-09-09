use super::tests::register_test_session;
use super::*;

#[test]
fn sheep_grazing_preserves_zero_timer_cleanup_and_loaded_scope() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "GrazingLoadedScope");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    for x in [0.5, 160.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:sheep".to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let records = registry.persisted_entity_records();
    let loaded = records
        .iter()
        .find(|record| record.snapshot.position.x == 0.5)
        .unwrap()
        .snapshot
        .id;
    let unloaded = records
        .iter()
        .find(|record| record.snapshot.position.x == 160.5)
        .unwrap()
        .snapshot
        .id;
    assert!(registry.set_sheep_grazing_ticks_for_test(loaded, Some(0)));
    assert!(registry.set_sheep_grazing_ticks_for_test(unloaded, Some(6)));

    let plan = registry.plan_sheep_grazing(&SimulationAuthority::for_test(), 1);

    assert!(plan.starts.is_empty());
    assert!(plan.actions.is_empty());
    let timers = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| {
            (
                record.snapshot.id,
                record.snapshot.retained.sheep_grazing_ticks,
            )
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(timers, HashMap::from([(loaded, None), (unloaded, Some(6))]));
}

#[test]
fn sheep_grazing_idle_selection_keeps_adult_and_baby_start_phases() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "GrazingStartPhases");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let snapshot = registry.persisted_entity_records().pop().unwrap().snapshot;
    let id = snapshot.id;
    let adult_tick = (0..1_000)
        .find(|&tick| sheep_grazing_starts_on_tick(id, tick, false))
        .unwrap();
    let baby_only_tick = (adult_tick + 50) % 1_000;

    let adult = registry.plan_sheep_grazing(&SimulationAuthority::for_test(), adult_tick);
    assert_eq!(
        adult
            .starts
            .iter()
            .map(|candidate| candidate.entity_id)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert!(
        registry
            .plan_sheep_grazing(&SimulationAuthority::for_test(), baby_only_tick)
            .starts
            .is_empty()
    );

    let mut animal = snapshot.animal.unwrap();
    animal.age_ticks = mc_entity::BABY_START_AGE_TICKS;
    assert!(
        registry
            .lock_entities("set grazing baby age")
            .set_animal_state(id, animal)
    );

    let baby = registry.plan_sheep_grazing(&SimulationAuthority::for_test(), baby_only_tick);
    assert_eq!(
        baby.starts
            .iter()
            .map(|candidate| candidate.entity_id)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert_eq!(
        registry.persisted_entity_records()[0]
            .snapshot
            .retained
            .sheep_grazing_ticks,
        None
    );
}
