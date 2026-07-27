use mc_entity::effects_26_1_2::{
    EffectFlags, EffectId, EffectInstance, EffectKind, TargetEffectContext,
};
use mc_entity::living_26_1_2::DamageContext;
use mc_entity::runtime_26_1_2::{PublicationFact, TargetKind};
use mc_entity::{
    AttributeKind, AttributeSet, EntityDamageRequest, EntityEffectOperation, EntityEffectRejection,
    EntityEffectRequest, EntityEffectResult, EntityId, EntityInputCommand, EntityItemStack,
    EntityLifecycle, EntityPhysicsResult, EntityRuntime, EntitySnapshot, EntityStage, EntityStore,
    GoalState, Rotation, SpawnEntity, Vec3,
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
fn production_spawn_installs_required_item_retained_state_atomically() {
    let mut store = EntityStore::new();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    item.item_stack = Some(EntityItemStack::new(42, 3));
    item.retained.spawn_tick = 17;
    item.retained.item_pickup_ready_tick = Some(27);

    let id = store.spawn(item);
    let snapshot = store.snapshot(id).expect("spawned item is authoritative");

    assert_eq!(snapshot.retained.spawn_tick, 17);
    assert_eq!(snapshot.retained.item_pickup_ready_tick, Some(27));
}

#[test]
fn ordinary_views_enumerate_entities_from_the_sole_ecs_authority() {
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
fn ordinary_ranged_position_tick_uses_ecs_order() {
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
fn ordinary_attribute_mutation_updates_ecs() {
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
fn staged_ecs_runtime_mutations_preserve_state() {
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
        retained: mc_entity::EntityRetainedState::default(),
    };
    let mut runtime = EntityRuntime::new();
    runtime.queue_input(EntityInputCommand::Insert(Box::new(expected.clone())));
    runtime.run_stage(EntityStage::InputAi);

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

        runtime.queue_input(EntityInputCommand::SetGoal {
            id,
            goal: expected.goal.clone(),
        });
        runtime.queue_input(EntityInputCommand::SetItemStack {
            id,
            stack: expected.item_stack.clone(),
        });
        runtime.run_stage(EntityStage::InputAi);
        runtime.queue_physics(EntityPhysicsResult {
            id,
            position: expected.position,
            rotation: expected.rotation,
            velocity: expected.velocity,
            on_ground: expected.on_ground,
        });
        runtime.run_stage(EntityStage::PhysicsApply);
    }

    assert_eq!(runtime.snapshot(id), Some(expected));
}

#[test]
fn effect_tick_mutates_authoritative_living_state_and_expires_in_ecs() {
    let mut store = EntityStore::new();
    let id = store.spawn(SpawnEntity::new(
        1,
        "minecraft:cow",
        Vec3::new(0.5, 64.0, 0.5),
    ));
    store
        .damage(
            id,
            EntityDamageRequest {
                amount: 12.0,
                tick: 1,
                death_remove_tick: 21,
                villager_gossip_event: None,
            },
        )
        .expect("seed damaged ECS living state");
    let effect = EffectInstance::new(
        EffectId::new(6),
        EffectKind::InstantHealth,
        1,
        0,
        EffectFlags::default(),
    );
    assert!(matches!(
        store.apply_effect(
            id,
            EntityEffectRequest {
                operation: EntityEffectOperation::Add(effect),
                target_kind: TargetKind::NonPlayer,
                death_remove_tick: 21,
            },
        ),
        EntityEffectResult::Applied(_)
    ));

    let applied = store.apply_effect(
        id,
        EntityEffectRequest {
            operation: EntityEffectOperation::Tick {
                entity_tick_count: 2,
                target_context: TargetEffectContext::LIVING,
                damage_context: DamageContext::default(),
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 22,
        },
    );
    let EntityEffectResult::Applied(applied) = applied else {
        panic!("effect tick must commit");
    };
    assert_eq!(applied.snapshot.health, 12.0);
    assert!(
        matches!(
            applied.publications.as_slice(),
            [PublicationFact::HealthChanged { before: 8.0, after: 12.0, .. },
             PublicationFact::EffectRemoved { effect: removed }]
                if removed.id == effect.id
                    && removed.kind == effect.kind
                    && removed.duration == 0
        ),
        "unexpected publications: {:?}",
        applied.publications
    );
    assert_eq!(store.snapshot(id).unwrap().health, 12.0);
    assert_eq!(
        store.apply_effect(
            id,
            EntityEffectRequest {
                operation: EntityEffectOperation::Tick {
                    entity_tick_count: 3,
                    target_context: TargetEffectContext::LIVING,
                    damage_context: DamageContext::default(),
                },
                target_kind: TargetKind::NonPlayer,
                death_remove_tick: 23,
            },
        ),
        EntityEffectResult::Rejected(EntityEffectRejection::NoActiveEffects)
    );
}

#[test]
fn lethal_effect_damage_commits_death_lifecycle_and_projection_together() {
    let mut store = EntityStore::new();
    let id = store.spawn(SpawnEntity::new(
        1,
        "minecraft:cow",
        Vec3::new(0.5, 64.0, 0.5),
    ));
    let result = store.apply_effect(
        id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(7),
                action: mc_entity::runtime_26_1_2::EffectAction::Damage {
                    amount: 20.0,
                    source: mc_entity::effects_26_1_2::EffectDamageSource::Magic,
                },
                damage_context: Some(DamageContext::default()),
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 42,
        },
    );
    let EntityEffectResult::Applied(applied) = result else {
        panic!("lethal effect transaction must commit");
    };
    assert_eq!(applied.snapshot.health, 0.0);
    assert_eq!(applied.snapshot.lifecycle, EntityLifecycle::Despawning);
    assert_eq!(applied.snapshot.retained.death_remove_tick, Some(42));
    assert!(matches!(
        applied.publications.as_slice(),
        [
            PublicationFact::DamageApplied { .. },
            PublicationFact::HurtEvent { .. },
            PublicationFact::DeathStarted { .. }
        ]
    ));
    assert_eq!(store.snapshot(id), Some(applied.snapshot));
}
