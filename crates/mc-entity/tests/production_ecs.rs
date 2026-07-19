use mc_entity::{
    AttributeKind, AttributeSet, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot,
    EntityStore, GoalState, Rotation, ShadowEntityRuntime, ShadowInputCommand, ShadowPhysicsResult,
    ShadowStage, SpawnEntity, Vec3,
};
use uuid::Uuid;

#[test]
fn production_entity_store_routes_ordinary_api_through_ecs() {
    let mut store = EntityStore::with_next_id(40);
    let mut entity = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    entity.uuid = Some(Uuid::from_u128(41));
    entity.velocity = Vec3::new(2.0, 0.0, -1.0);
    entity.item_stack = Some(EntityItemStack::new(42, 3));

    let id = store.spawn(entity);

    assert_eq!(id, EntityId(41));
    let view = store.view(id).expect("ordinary production reads use ECS");
    assert_eq!(view.uuid, Uuid::from_u128(41));
    assert_eq!(view.item_stack, Some(EntityItemStack::new(42, 3)));

    assert!(store.set_item_stack(id, Some(EntityItemStack::new(42, 2))));
    store.tick_positions(0.5);
    let snapshot = store.snapshot(id).expect("spawned entity remains in ECS");
    assert_eq!(snapshot.position, Vec3::new(1.5, 64.0, 0.0));
    assert_eq!(snapshot.item_stack, Some(EntityItemStack::new(42, 2)));
    assert_eq!(store.snapshots().collect::<Vec<_>>(), vec![snapshot]);
}

#[test]
fn ordinary_views_enumerate_ecs_entities_with_or_without_shadow_comparison() {
    let mut store = EntityStore::new();
    let first = store.spawn(SpawnEntity::new(
        1,
        "minecraft:cow",
        Vec3::new(1.0, 64.0, 1.0),
    ));
    let second = store.spawn(SpawnEntity::new(
        2,
        "minecraft:sheep",
        Vec3::new(2.0, 64.0, 2.0),
    ));

    let mut ids = store.views().map(|entity| entity.id).collect::<Vec<_>>();
    ids.sort_unstable();

    assert_eq!(ids, vec![first, second]);
}

#[test]
fn ordinary_ranged_position_tick_uses_ecs_order_with_or_without_shadow_comparison() {
    let mut store = EntityStore::new();
    let first = store.spawn(SpawnEntity::new(
        1,
        "minecraft:cow",
        Vec3::new(1.0, 64.0, 1.0),
    ));
    let mut moving = SpawnEntity::new(2, "minecraft:sheep", Vec3::new(2.0, 64.0, 2.0));
    moving.velocity = Vec3::new(4.0, 0.0, -2.0);
    let second = store.spawn(moving);

    store.tick_positions_in_range(1..2, 0.5);

    assert_eq!(
        store.snapshot(first).unwrap().position,
        Vec3::new(1.0, 64.0, 1.0)
    );
    assert_eq!(
        store.snapshot(second).unwrap().position,
        Vec3::new(4.0, 64.0, 1.0)
    );
}

#[test]
fn ordinary_attribute_mutation_updates_ecs_with_or_without_shadow_comparison() {
    let mut store = EntityStore::new();
    let id = store.spawn(SpawnEntity::new(
        1,
        "minecraft:cow",
        Vec3::new(1.0, 64.0, 1.0),
    ));

    store
        .attributes_mut(id)
        .expect("ordinary entity has ECS-backed attributes")
        .set_base(AttributeKind::MovementSpeed, 0.4);

    assert_eq!(
        store
            .snapshot(id)
            .unwrap()
            .attributes
            .base(&AttributeKind::MovementSpeed),
        Some(0.4)
    );
}

#[test]
fn production_ecs_mutations_preserve_state_without_semantic_event_stage() {
    let id = EntityId(1);
    let mut expected = EntitySnapshot {
        id,
        uuid: Uuid::from_u128(1),
        type_id: 1,
        type_name: "minecraft:item".to_owned(),
        position: Vec3::new(0.5, 64.0, 0.5),
        rotation: Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: Some(EntityItemStack::new(42, 1)),
        experience_value: None,
        block_state: None,
        lifecycle: EntityLifecycle::Alive,
        health: 20.0,
        attributes: AttributeSet::new(),
        goal: GoalState::Idle,
        vehicle: None,
        animal: None,
    };
    let mut runtime = ShadowEntityRuntime::new();
    runtime.queue_input(ShadowInputCommand::InsertAuthoritative(expected.clone()));
    runtime.run_stage(ShadowStage::InputAi);

    for step in 1..=256 {
        expected.goal = GoalState::FollowPosition {
            target: Vec3::new(f64::from(step), 65.0, -f64::from(step)),
            speed: 0.25,
        };
        expected.item_stack = Some(EntityItemStack::new(42, step));
        expected.position = Vec3::new(f64::from(step), 64.0, 0.5);
        expected.rotation = Rotation {
            yaw: step as f32,
            pitch: 0.0,
            head_yaw: step as f32,
        };
        expected.velocity = Vec3::new(0.1, 0.0, -0.1);
        expected.on_ground = step % 2 == 0;

        runtime.queue_input(ShadowInputCommand::SetGoal {
            id,
            goal: expected.goal.clone(),
        });
        runtime.queue_input(ShadowInputCommand::SetItemStack {
            id,
            stack: expected.item_stack.clone(),
        });
        runtime.run_stage(ShadowStage::InputAi);
        runtime.queue_physics(ShadowPhysicsResult {
            id,
            position: expected.position,
            rotation: expected.rotation,
            velocity: expected.velocity,
            on_ground: expected.on_ground,
        });
        runtime.run_stage(ShadowStage::PhysicsApply);
    }

    assert_eq!(runtime.snapshot(id), Some(expected));
}
