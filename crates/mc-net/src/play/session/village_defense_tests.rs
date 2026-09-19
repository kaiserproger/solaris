use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use mc_data::Identifier;
use mc_entity::villager_26_1_2::{VillagerBrainState, VillagerPoiSet, VillagerScheduleKind};
use mc_entity::villager_population_26_1_2::VillagerPopulationState;
use mc_entity::{VillagerData, VillagerKind, VillagerProfession};
use mc_script::precommit::{HookDecision, HookFailurePolicy, HookKind, HookRegistration};
use mc_script::{ScriptHostInput, script_boundary_pair};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos};
use tokio::sync::mpsc;

use crate::play::persistence::PersistedEntityCheckpoint;
use crate::play::{LoggedInProfile, PlayerPose};

use super::*;

fn defense_world() -> (WorldReadView, BlockMaterialIds) {
    let block = |name: &str, id: u32| mc_data::blocks::BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    let blocks = Arc::new(
        BlockRegistry::from_report(&[block("minecraft:air", 0), block("minecraft:stone", 1)])
            .unwrap(),
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for x in 0..16 {
        for z in 0..16 {
            let _ = chunk.set_block(x, 63, z, BlockStateId(1));
        }
    }
    world.insert_generated_chunk(chunk_pos, chunk).unwrap();
    (world.read_view(), BlockMaterialIds::new(0, None, None))
}

fn register_observer(registry: &SessionRegistry) -> (u64, mpsc::Receiver<OutboundCommand>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("GolemObserver"),
        name: "GolemObserver".to_owned(),
    };
    let (tx, rx) = mpsc::channel(64);
    let session = registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0;
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    (session, rx)
}

fn spawn_villager(registry: &SessionRegistry, position: Vec3, tick: u64) -> EntityId {
    let mut entity = SpawnEntity::new(139, "minecraft:villager", position);
    entity.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::None,
        1,
    ));
    entity.retained.villager_population = Some(VillagerPopulationState::adult());
    let mut brain = VillagerBrainState::adult(VillagerPoiSet {
        home: Some(position),
        job_site: None,
        meeting_point: Some(position),
    });
    brain.schedule = VillagerScheduleKind::Adult;
    brain.last_slept_tick = Some(tick);
    entity.retained.villager_brain = Some(brain);
    apply_entity_facts(&mut entity);
    registry
        .lock_entities("spawn village defence villager")
        .spawn(entity)
}

fn spawn_mob(
    registry: &SessionRegistry,
    type_id: i32,
    type_name: &str,
    position: Vec3,
) -> EntityId {
    spawn_mob_with_max_health(registry, type_id, type_name, position, None)
}

fn spawn_mob_with_max_health(
    registry: &SessionRegistry,
    type_id: i32,
    type_name: &str,
    position: Vec3,
    max_health: Option<f64>,
) -> EntityId {
    let mut entity = SpawnEntity::new(type_id, type_name, position);
    apply_entity_facts(&mut entity);
    if let Some(max_health) = max_health {
        entity
            .attributes
            .set_base(AttributeKind::MaxHealth, max_health);
    }
    let id = registry
        .lock_entities("spawn village defence mob")
        .spawn(entity);
    if is_hostile_entity(type_name) {
        registry
            .lock_inner("index village defence hostile fixture")
            .hostile_entities
            .insert(id);
    }
    id
}

fn seed_golem_attack(
    registry: &SessionRegistry,
) -> (EntityId, EntityId, u64, mpsc::Receiver<OutboundCommand>) {
    registry.configure_arrow_kill_rewards(
        None,
        None,
        None,
        Arc::new(mc_data::items::ItemRegistry::default()),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let (_observer, outbound) = register_observer(registry);
    let golem = spawn_mob(
        registry,
        70,
        "minecraft:iron_golem",
        Vec3::new(4.5, 64.0, 4.5),
    );
    let ravager = spawn_mob_with_max_health(
        registry,
        109,
        "minecraft:ravager",
        Vec3::new(6.0, 64.0, 4.5),
        Some(100.0),
    );
    registry.publish_active_simulation_entities_for_test([golem, ravager]);
    let mut dispatches = Vec::new();
    {
        let mut inner = registry.lock_session_entities("publish hooked golem combat fixture");
        for entity_id in [golem, ravager] {
            let position = inner.entities.snapshot(entity_id).unwrap().position;
            track_entity_chunk_locked(&mut inner, entity_id, position);
            let visible = spawn_entity_visibility_locked(&mut inner, entity_id);
            assert!(!visible.is_empty());
            dispatches.extend(visible);
        }
    }
    super::super::outbound::dispatch_visibility_commands(dispatches);
    let phase = u64::from(golem.0.unsigned_abs()) % GOLEM_ATTACK_INTERVAL;
    let due = if phase == 0 {
        GOLEM_ATTACK_INTERVAL
    } else {
        GOLEM_ATTACK_INTERVAL - phase
    };
    (golem, ravager, due, outbound)
}

#[test]
fn three_recently_slept_villagers_spawn_one_persisted_golem_and_memory_blocks_duplicate() {
    let registry = SessionRegistry::new();
    let (_observer, _outbound) = register_observer(&registry);
    let (world, materials) = defense_world();
    let villagers = [
        spawn_villager(&registry, Vec3::new(4.5, 64.0, 4.5), 90),
        spawn_villager(&registry, Vec3::new(5.5, 64.0, 4.5), 90),
        spawn_villager(&registry, Vec3::new(4.5, 64.0, 5.5), 90),
    ];
    let zombie = spawn_mob(
        &registry,
        151,
        "minecraft:zombie",
        Vec3::new(7.5, 64.0, 4.5),
    );
    registry.publish_active_simulation_entities_for_test(villagers.into_iter().chain([zombie]));
    registry.synchronize_entity_lifecycle_epoch(100);

    let (report, dispatches) = registry.tick_village_defense(
        &SimulationAuthority::for_test(),
        100,
        70,
        Some(&world),
        Some(&materials),
    );
    assert_eq!(report.spawned_golems, 1);
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::SpawnEntity(entity)
                if entity.type_name == "minecraft:iron_golem"
        ) || matches!(
            &dispatch.command,
            OutboundCommand::SpawnEntities(entities)
                if entities.iter().any(|entity| entity.type_name == "minecraft:iron_golem")
        )
    }));
    let records = registry.persisted_entity_records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
            .count(),
        1
    );
    let golem = &records
        .iter()
        .find(|record| record.snapshot.type_name == "minecraft:iron_golem")
        .expect("persisted iron golem")
        .snapshot;
    for entity_id in villagers.iter().copied().chain([zombie]) {
        let other = registry
            .lock_entities("read village defence spawn neighbour")
            .snapshot(entity_id)
            .unwrap();
        assert!(!aabbs_intersect(
            golem.position,
            entity_aabb(&golem.type_name),
            other.position,
            entity_aabb(&other.type_name),
        ));
    }
    for villager in villagers {
        let brain = registry
            .lock_entities("read golem detection memory")
            .snapshot(villager)
            .unwrap()
            .retained
            .villager_brain
            .unwrap();
        assert_eq!(brain.golem_detected_until_tick, Some(699));
    }

    let active = records.into_iter().map(|record| record.snapshot.id);
    registry.publish_active_simulation_entities_for_test(active);
    registry.synchronize_entity_lifecycle_epoch(200);
    let (repeat, _) = registry.tick_village_defense(
        &SimulationAuthority::for_test(),
        200,
        70,
        Some(&world),
        Some(&materials),
    );
    assert_eq!(repeat.spawned_golems, 0);
    assert_eq!(
        registry
            .persisted_entity_records()
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
            .count(),
        1
    );
}

#[test]
fn restored_golem_and_villager_memory_prevent_duplicate_spawn() {
    let source = SessionRegistry::new();
    let (_observer, _outbound) = register_observer(&source);
    let (world, materials) = defense_world();
    let villagers = [
        spawn_villager(&source, Vec3::new(4.5, 64.0, 4.5), 90),
        spawn_villager(&source, Vec3::new(5.5, 64.0, 4.5), 90),
        spawn_villager(&source, Vec3::new(4.5, 64.0, 5.5), 90),
    ];
    let zombie = spawn_mob(&source, 151, "minecraft:zombie", Vec3::new(7.5, 64.0, 4.5));
    source.publish_active_simulation_entities_for_test(villagers.into_iter().chain([zombie]));
    source.synchronize_entity_lifecycle_epoch(100);
    assert_eq!(
        source
            .tick_village_defense(
                &SimulationAuthority::for_test(),
                100,
                70,
                Some(&world),
                Some(&materials),
            )
            .0
            .spawned_golems,
        1
    );

    let records = source.persisted_entity_records();
    let restored = SessionRegistry::new();
    let (_observer, _outbound) = register_observer(&restored);
    assert_eq!(
        restored.restore_persisted_entities(PersistedEntityCheckpoint::new(100, records.clone())),
        records.len()
    );
    let restored_records = restored.persisted_entity_records();
    assert_eq!(
        restored_records
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
            .count(),
        1
    );
    for record in restored_records
        .iter()
        .filter(|record| record.snapshot.type_name == "minecraft:villager")
    {
        let brain = record
            .snapshot
            .retained
            .villager_brain
            .as_ref()
            .expect("restored villager brain");
        assert_eq!(brain.last_slept_tick, Some(90));
        assert_eq!(brain.golem_detected_until_tick, Some(699));
    }

    restored.publish_active_simulation_entities_for_test(
        restored_records.iter().map(|record| record.snapshot.id),
    );
    restored.synchronize_entity_lifecycle_epoch(800);
    let (restored_world, restored_materials) = defense_world();
    let (report, _) = restored.tick_village_defense(
        &SimulationAuthority::for_test(),
        800,
        70,
        Some(&restored_world),
        Some(&restored_materials),
    );
    assert_eq!(report.spawned_golems, 0);
    assert_eq!(
        restored
            .persisted_entity_records()
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
            .count(),
        1
    );
    for record in restored
        .persisted_entity_records()
        .iter()
        .filter(|record| record.snapshot.type_name == "minecraft:villager")
    {
        assert_eq!(
            record
                .snapshot
                .retained
                .villager_brain
                .as_ref()
                .and_then(|brain| brain.golem_detected_until_tick),
            Some(1_399)
        );
    }
}

#[test]
fn golem_goal_update_uses_deferred_owner_path() {
    let registry = SessionRegistry::new();
    let golem = spawn_mob(
        &registry,
        70,
        "minecraft:iron_golem",
        Vec3::new(4.5, 64.0, 4.5),
    );
    let goal = GoalState::Wander {
        speed: GOLEM_WANDER_SPEED,
        period_ticks: GOLEM_WANDER_PERIOD_TICKS,
    };
    let applied = registry
        .lock_entities("set village defence fixture goal")
        .set_goals_deferred_journal([(golem, goal.clone())]);
    assert_eq!(applied, 1);
    assert_eq!(
        registry
            .lock_entities("read village defence fixture goal")
            .snapshot(golem)
            .unwrap()
            .goal,
        goal
    );
}

#[test]
fn surviving_damage_accepts_followup_velocity_update() {
    let registry = SessionRegistry::new();
    let zombie = spawn_mob(
        &registry,
        151,
        "minecraft:zombie",
        Vec3::new(6.0, 64.0, 4.5),
    );
    let mut inner = registry.lock_session_entities("damage village defence fixture");
    assert!(
        super::super::entity_combat::damage_server_entity_locked(&mut inner, zombie, 16.5, None,)
            .is_some()
    );
    assert!(
        inner
            .entities
            .set_velocity(zombie, Vec3::new(0.0, GOLEM_VERTICAL_KNOCKBACK, 0.0),)
    );
}

#[test]
fn golem_attacks_nearby_ravager_with_event_and_vertical_knockback() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        None,
        None,
        None,
        Arc::new(mc_data::items::ItemRegistry::default()),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let (_observer, _outbound) = register_observer(&registry);
    let golem = spawn_mob(
        &registry,
        70,
        "minecraft:iron_golem",
        Vec3::new(4.5, 64.0, 4.5),
    );
    let ravager = spawn_mob_with_max_health(
        &registry,
        109,
        "minecraft:ravager",
        Vec3::new(6.0, 64.0, 4.5),
        Some(100.0),
    );
    registry.publish_active_simulation_entities_for_test([golem, ravager]);
    {
        let mut inner = registry.lock_session_entities("publish golem combat fixture");
        for entity_id in [golem, ravager] {
            let position = inner.entities.snapshot(entity_id).unwrap().position;
            track_entity_chunk_locked(&mut inner, entity_id, position);
            assert!(!spawn_entity_visibility_locked(&mut inner, entity_id).is_empty());
        }
    }
    let phase = u64::from(golem.0.unsigned_abs()) % GOLEM_ATTACK_INTERVAL;
    let due = if phase == 0 {
        GOLEM_ATTACK_INTERVAL
    } else {
        GOLEM_ATTACK_INTERVAL - phase
    };

    let before = registry
        .lock_entities("read ravager before golem attack")
        .snapshot(ravager)
        .unwrap();
    assert!(before.health > 30.0, "ravager health={}", before.health);
    let (report, dispatches) =
        registry.tick_village_defense(&SimulationAuthority::for_test(), due, 70, None, None);
    assert_eq!(report.golem_attacks, 1);
    let after = registry
        .lock_entities("read ravager after golem attack")
        .snapshot(ravager)
        .unwrap();
    assert!(after.health < before.health);
    assert!(after.velocity.y >= GOLEM_VERTICAL_KNOCKBACK - 1.0e-9);
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::EntityEvent { entity_id, event_id }
                if entity_id == golem.0 && event_id == GOLEM_ATTACK_EVENT
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn before_damage_keep_preserves_golem_knockback_and_animation_and_cancel_commits_nothing() {
    let direct = SessionRegistry::new();
    let (direct_golem, direct_target, due, _) = seed_golem_attack(&direct);
    let (_, direct_dispatches) =
        direct.tick_village_defense(&SimulationAuthority::for_test(), due, 70, None, None);
    let direct_target_snapshot = direct
        .lock_entities("inspect direct golem attack")
        .snapshot(direct_target)
        .expect("direct target remains");
    assert!(direct_dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::EntityEvent { entity_id, event_id }
                if entity_id == direct_golem.0 && event_id == GOLEM_ATTACK_EVENT
        )
    }));

    let kept = SessionRegistry::new();
    let (kept_golem, kept_target, due, mut kept_outbound) = seed_golem_attack(&kept);
    let (handle, mut owner) = crate::play::simulation::simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "golem-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    kept.install_precommit_boundary(boundary);
    kept.install_damage_precommit_handle(handle);
    let (report, dispatches) =
        kept.tick_village_defense(&SimulationAuthority::for_test(), due, 70, None, None);
    assert_eq!(report.golem_attacks, 1);
    assert!(dispatches.is_empty());
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Keep)
            .expect("live golem request"),
        _ => panic!("expected one golem damage request"),
    }
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&kept, 1).processed, 1);
    assert_eq!(
        kept.lock_entities("inspect kept golem attack")
            .snapshot(kept_target)
            .expect("kept target remains"),
        direct_target_snapshot
    );
    assert!(
        std::iter::from_fn(|| kept_outbound.try_recv().ok()).any(|command| {
            matches!(
                command,
                OutboundCommand::EntityEvent { entity_id, event_id }
                    if entity_id == kept_golem.0 && event_id == GOLEM_ATTACK_EVENT
            )
        })
    );

    let cancelled = SessionRegistry::new();
    let (_cancelled_golem, cancelled_target, due, _outbound) = seed_golem_attack(&cancelled);
    let before_cancel = cancelled
        .lock_entities("capture cancelled golem target")
        .snapshot(cancelled_target)
        .expect("cancelled target exists");
    let (handle, mut owner) = crate::play::simulation::simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "golem-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    cancelled.install_precommit_boundary(boundary);
    cancelled.install_damage_precommit_handle(handle);
    cancelled.tick_village_defense(&SimulationAuthority::for_test(), due, 70, None, None);
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Cancel)
            .expect("live golem request"),
        _ => panic!("expected one golem damage request"),
    }
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&cancelled, 1).processed, 1);
    assert_eq!(
        cancelled
            .lock_entities("inspect cancelled golem attack")
            .snapshot(cancelled_target)
            .expect("cancelled target remains"),
        before_cancel
    );
}

#[test]
fn golem_never_targets_creeper() {
    let registry = SessionRegistry::new();
    let (_observer, _outbound) = register_observer(&registry);
    let golem = spawn_mob(
        &registry,
        70,
        "minecraft:iron_golem",
        Vec3::new(4.5, 64.0, 4.5),
    );
    let creeper = spawn_mob(
        &registry,
        20,
        "minecraft:creeper",
        Vec3::new(5.5, 64.0, 4.5),
    );
    registry.publish_active_simulation_entities_for_test([golem, creeper]);

    let (report, _) =
        registry.tick_village_defense(&SimulationAuthority::for_test(), 20, 70, None, None);
    assert_eq!(report.golem_attacks, 0);
    let goal = registry
        .lock_entities("read golem creeper exclusion")
        .snapshot(golem)
        .unwrap()
        .goal;
    assert!(matches!(goal, GoalState::Wander { .. }), "goal={goal:?}");
}
