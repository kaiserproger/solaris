use super::*;

#[test]
fn regional_tick_separates_goal_phase_from_owner_physics_publication() {
    let (world_read, materials) = two_block_wall_pathing_world();
    let materials = Arc::new(materials);
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("RegionalTickTarget"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 0.5),
    );
    registry.mark_loaded(player, (0, 0));
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:rabbit".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let rabbit = registry.persisted_entity_records()[0].snapshot.id;
    let before = registry
        .server_entity_snapshot(rabbit)
        .expect("published rabbit before regional tick");
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 1);

    let (queries, owner_fence, _, regional_commit) = registry
        .tick_entities_and_collect_physics_queries_regional(
            &resources,
            20,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: 8,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            EntitySimulationWorldContext::with_pathing_for_test(
                &world_read,
                Arc::clone(&materials),
            ),
        );

    assert!(
        queries.is_empty(),
        "same-chunk physics stays on its owner lane"
    );
    assert!(owner_fence.is_none());
    let unpublished = registry
        .server_entity_snapshot(rabbit)
        .expect("rabbit remains unpublished after regional goal phase");
    assert_eq!(unpublished.position, before.position);
    assert_eq!(unpublished.on_ground, before.on_ground);
    let prepared = regional_commit.expect("regional physics preparation");
    let (queries, owner_fence, regional_commit, _lane_timings) =
        registry.commit_owned_region_physics(prepared);
    assert!(queries.is_empty());
    assert!(owner_fence.is_none());
    let regional_commit = regional_commit.expect("regional movement publication");
    assert_eq!(regional_commit.states.len(), 1);
    let committed = regional_commit.states[0];
    assert_eq!(committed.id, rabbit);
    assert!(
        committed.position != before.position
            || committed.rotation != before.rotation
            || committed.velocity != before.velocity
            || committed.on_ground != before.on_ground,
        "regional tick must return only committed movement changes"
    );

    let accepted = registry.apply_entity_physics_if_current_and_dispatch_regional(
        &resources,
        20,
        &queries,
        &[],
        owner_fence,
        &EntityProjectilePhysicsFacts::default(),
        Some(regional_commit),
    );
    assert!(accepted.is_empty());
    assert_eq!(
        registry
            .server_entity_snapshot(rabbit)
            .expect("published rabbit after regional tick")
            .position,
        committed.position
    );
    assert_eq!(
        registry
            .server_entity_snapshot(rabbit)
            .expect("published rabbit rotation after regional tick")
            .rotation,
        committed.rotation
    );
    assert_eq!(
        registry.simulation_inputs.entity_chunk(rabbit),
        Some((0, 0))
    );

    let (queries, owner_fence, _, stale_commit) = registry
        .tick_entities_and_collect_physics_queries_regional(
            &resources,
            21,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: 8,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            EntitySimulationWorldContext::with_pathing_for_test(
                &world_read,
                Arc::clone(&materials),
            ),
        );
    assert!(queries.is_empty());
    assert!(owner_fence.is_none());
    let prepared = stale_commit.expect("second regional physics preparation");
    let (queries, owner_fence, stale_commit, _lane_timings) =
        registry.commit_owned_region_physics(prepared);
    assert!(queries.is_empty());
    assert!(owner_fence.is_none());
    let stale_commit = stale_commit.expect("second regional movement publication");
    let published_before_stale_apply = registry
        .server_entity_snapshot(rabbit)
        .expect("published rabbit before stale regional apply");
    let newer_velocity = Vec3::new(-0.25, 0.5, 0.125);
    assert_ne!(published_before_stale_apply.velocity, newer_velocity);
    assert!(
        registry
            .lock_entities("invalidate regional publication fence")
            .set_velocity(rabbit, newer_velocity)
    );

    let rejected = registry.apply_entity_physics_if_current_and_dispatch_regional(
        &resources,
        21,
        &queries,
        &[],
        owner_fence,
        &EntityProjectilePhysicsFacts::default(),
        Some(stale_commit),
    );
    assert!(rejected.is_empty());
    let published_after_stale_apply = registry
        .server_entity_snapshot(rabbit)
        .expect("stale regional movement remains unpublished");
    assert_eq!(published_after_stale_apply.velocity, newer_velocity);
    assert_eq!(
        registry
            .lock_entities("inspect newer regional owner state")
            .snapshot(rabbit)
            .expect("regional rabbit")
            .velocity,
        newer_velocity
    );
}
