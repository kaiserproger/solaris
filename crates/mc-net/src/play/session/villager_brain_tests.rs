use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_entity::villager_26_1_2::{
    VillagerActivity, VillagerBrainProfile, VillagerBrainState, VillagerPoiSet,
    VillagerScheduleEntry, VillagerScheduleKind,
};
use mc_entity::villager_merchant_26_1_2::{
    VillagerMerchantState, VillagerTradeCost, VillagerTradeOffer,
};
use mc_entity::{
    EntityId, EntityItemStack, GoalState, Vec3, VillagerData, VillagerKind, VillagerProfession,
};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos};
use tokio::sync::mpsc;

use crate::login::LoggedInProfile;
use crate::play::{HerdSpawn, PlayerPose};

use super::entity_simulation::{
    apply_villager_brain_transitions, commit_villager_brain_transitions,
    villager_brain_due_for_tick, villager_brain_probe_ids,
};
use super::{OutboundCommand, SessionRegistry, dispatch_visibility_commands};

fn install_brain(registry: &SessionRegistry) -> mc_entity::EntityId {
    let position = Vec3::new(0.5, 64.0, 0.5);
    let id = registry.spawn_script_villager_for_test(position);
    let mut entities = registry.lock_entities("install villager brain test fixture");
    let expected = entities.snapshot(id).expect("villager snapshot");
    let mut next = expected.clone();
    next.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::Toolsmith,
        1,
    ));
    next.retained.villager_brain = Some(VillagerBrainState::adult(VillagerPoiSet {
        home: Some(Vec3::new(1.5, 64.0, 0.5)),
        job_site: Some(Vec3::new(8.5, 64.0, 0.5)),
        meeting_point: Some(Vec3::new(4.5, 64.0, 4.5)),
    }));
    next.goal = GoalState::Idle;
    assert!(entities.replace_snapshot_if_current(expected, next));
    id
}

fn register_profession_observer(
    registry: &SessionRegistry,
) -> (u64, mpsc::Receiver<OutboundCommand>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ProfessionObserver"),
        name: "ProfessionObserver".to_owned(),
    };
    let (tx, rx) = mpsc::channel(16);
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

fn profession_world(job_site_block: &str) -> (Arc<BlockRegistry>, mc_world::WorldReadView) {
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
        BlockRegistry::from_report(&[
            block("minecraft:air", 0),
            block("minecraft:smithing_table", 1),
            block("minecraft:blast_furnace", 2),
        ])
        .unwrap(),
    );
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let state = match job_site_block {
        "minecraft:smithing_table" => 1,
        "minecraft:blast_furnace" => 2,
        _ => 0,
    };
    let _ = chunk.set_block(8, 64, 0, BlockStateId(state));
    world.insert_generated_chunk(chunk_pos, chunk).unwrap();
    (blocks, world.read_view())
}

fn install_unemployed_villager(registry: &SessionRegistry) -> mc_entity::EntityId {
    let position = Vec3::new(8.5, 64.0, 0.5);
    let dispatches = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 119,
            entity_type_name: "minecraft:villager".to_owned(),
            position,
            hostile: false,
            sheep_color: None,
        }],
    );
    let id = dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
            _ => None,
        })
        .expect("villager spawn dispatch");
    dispatch_visibility_commands(dispatches);
    let mut entities = registry.lock_entities("install unemployed villager fixture");
    let expected = entities.snapshot(id).unwrap();
    let mut next = expected.clone();
    next.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::None,
        1,
    ));
    next.retained.villager_brain = Some(VillagerBrainState::adult(VillagerPoiSet {
        home: Some(position),
        job_site: Some(position),
        meeting_point: Some(position),
    }));
    next.retained.villager_merchant = None;
    assert!(entities.replace_snapshot_if_current(expected, next));
    id
}

fn apply_brain(
    registry: &SessionRegistry,
    id: mc_entity::EntityId,
    lifecycle_tick: u64,
    day_time: i64,
) -> usize {
    let profile = registry.villager_brain_profile();
    let mut entities = registry.lock_entities("apply villager brain test transition");
    apply_villager_brain_transitions(
        &mut entities,
        &HashSet::from([id]),
        lifecycle_tick,
        day_time,
        &profile,
    )
}

#[test]
fn brain_cadence_shards_sixteen_thousand_villagers_and_wakes_exact_boundaries() {
    let profile = VillagerBrainProfile::vanilla_26_1_2();
    let population = (1..=16_000_i32).map(EntityId).collect::<HashSet<_>>();
    let counts = (0..20_u64)
        .map(|tick| {
            villager_brain_probe_ids(&population, &HashSet::new(), tick, 100, &profile).len()
        })
        .collect::<Vec<_>>();
    assert!(counts.iter().all(|count| *count == 800), "{counts:?}");
    assert_eq!(counts.iter().sum::<usize>(), 16_000);

    let mut custom = profile;
    custom.adult_schedule = vec![
        VillagerScheduleEntry {
            day_time: 0,
            activity: VillagerActivity::Rest,
        },
        VillagerScheduleEntry {
            day_time: 123,
            activity: VillagerActivity::Work,
        },
    ];
    custom.validate().unwrap();
    let off_phase = EntityId(1);
    assert_eq!(
        villager_brain_probe_ids(&population, &HashSet::new(), 2, 123, &custom).len(),
        16_000,
        "a custom schedule boundary must bypass the ordinary phase shard"
    );
    let inactive_override = EntityId(20_001);
    let override_probe = villager_brain_probe_ids(
        &population,
        &HashSet::from([off_phase, inactive_override]),
        2,
        122,
        &custom,
    );
    assert!(override_probe.contains(&off_phase));
    assert!(!override_probe.contains(&inactive_override));
    assert!(!villager_brain_due_for_tick(
        off_phase,
        VillagerScheduleKind::Adult,
        None,
        2,
        122,
        &custom,
    ));
    assert!(villager_brain_due_for_tick(
        off_phase,
        VillagerScheduleKind::Adult,
        None,
        2,
        123,
        &custom,
    ));
    assert!(villager_brain_due_for_tick(
        off_phase,
        VillagerScheduleKind::Adult,
        Some(2),
        2,
        122,
        &custom,
    ));
}

#[test]
fn stale_villager_transition_does_not_starve_current_neighbor() {
    let registry = SessionRegistry::new();
    let first = install_brain(&registry);
    let second = install_brain(&registry);
    let mut entities = registry.lock_entities("prepare split villager brain CAS");
    let first_expected = entities.snapshot(first).unwrap();
    let second_expected = entities.snapshot(second).unwrap();

    let concurrent_goal = GoalState::Wander {
        speed: 0.11,
        period_ticks: 81,
    };
    let mut concurrent_first = first_expected.clone();
    concurrent_first.goal = concurrent_goal.clone();
    assert!(entities.replace_snapshot_if_current(first_expected.clone(), concurrent_first));

    let mut first_next = first_expected.clone();
    first_next.goal = GoalState::FollowPosition {
        target: Vec3::new(20.0, 64.0, 0.0),
        speed: 0.3,
    };
    let second_goal = GoalState::FollowPosition {
        target: Vec3::new(30.0, 64.0, 0.0),
        speed: 0.4,
    };
    let mut second_next = second_expected.clone();
    second_next.goal = second_goal.clone();

    assert_eq!(
        commit_villager_brain_transitions(
            &mut entities,
            vec![(first_expected, first_next), (second_expected, second_next)],
        ),
        1
    );
    assert_eq!(entities.snapshot(first).unwrap().goal, concurrent_goal);
    assert_eq!(entities.snapshot(second).unwrap().goal, second_goal);
}

#[test]
fn schedule_transitions_are_data_driven_and_do_not_write_unchanged_ticks() {
    let registry = SessionRegistry::new();
    let id = install_brain(&registry);
    let mut profile = VillagerBrainProfile::vanilla_26_1_2();
    profile.work_speed = 0.75;
    registry
        .configure_villager_brain_profile(profile)
        .expect("custom villager profile validates");

    assert_eq!(apply_brain(&registry, id, 1, 2_000), 1);
    let work = registry
        .lock_entities("read work transition")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        work.goal,
        GoalState::FollowPosition {
            target: Vec3::new(8.5, 64.0, 0.5),
            speed: 0.75,
        }
    );
    assert_eq!(
        work.retained.villager_brain.as_ref().unwrap().activity,
        VillagerActivity::Work
    );

    assert_eq!(
        apply_brain(&registry, id, 2, 2_001),
        0,
        "unchanged schedule interval must not append another owner mutation"
    );

    assert_eq!(apply_brain(&registry, id, 3, 9_000), 1);
    let meeting = registry
        .lock_entities("read meeting transition")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        meeting.goal,
        GoalState::FollowPosition {
            target: Vec3::new(4.5, 64.0, 4.5),
            speed: 0.3,
        }
    );
    assert_eq!(
        meeting.retained.villager_brain.as_ref().unwrap().activity,
        VillagerActivity::Meet
    );
}

#[test]
fn unemployed_adult_claims_smithing_table_and_publishes_toolsmith_metadata_once() {
    let registry = SessionRegistry::new();
    let (_session, mut outbound) = register_profession_observer(&registry);
    let id = install_unemployed_villager(&registry);
    let (blocks, world_read) = profession_world("minecraft:smithing_table");
    let items = mc_data::items::solaris_required_items();
    let due_tick = (20 - u64::from(id.0.unsigned_abs()) % 20) % 20;

    let _ = registry.tick_entities_and_collect_physics_queries_with_profession_context(
        due_tick,
        &world_read,
        &blocks,
        &items,
    );
    let assigned = registry
        .lock_entities("read assigned villager profession")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        assigned.retained.villager.unwrap().profession,
        VillagerProfession::Toolsmith
    );
    let merchant = assigned
        .retained
        .villager_merchant
        .as_ref()
        .expect("toolsmith merchant catalog");
    assert_eq!(merchant.offers.len(), 5);
    assert_eq!(merchant.xp, 0);

    let mut metadata_updates = 0;
    while let Ok(command) = outbound.try_recv() {
        if matches!(
            command,
            OutboundCommand::UpdateEntityData(ref snapshot)
                if snapshot.id == id
                    && snapshot
                        .villager
                        .is_some_and(|villager| villager.profession == VillagerProfession::Toolsmith)
        ) {
            metadata_updates += 1;
        }
    }
    assert_eq!(metadata_updates, 1);

    let _ = registry.tick_entities_and_collect_physics_queries_with_profession_context(
        due_tick + 20,
        &world_read,
        &blocks,
        &items,
    );
    let mut repeated_update = false;
    while let Ok(command) = outbound.try_recv() {
        repeated_update |= matches!(
            command,
            OutboundCommand::UpdateEntityData(snapshot) if snapshot.id == id
        );
    }
    assert!(
        !repeated_update,
        "stable profession must not republish unchanged metadata"
    );
}

#[test]
fn unsupported_job_site_keeps_villager_unemployed_without_merchant_state() {
    let registry = SessionRegistry::new();
    let (_session, mut outbound) = register_profession_observer(&registry);
    let id = install_unemployed_villager(&registry);
    let (blocks, world_read) = profession_world("minecraft:blast_furnace");
    let items = mc_data::items::solaris_required_items();
    let due_tick = (20 - u64::from(id.0.unsigned_abs()) % 20) % 20;

    let _ = registry.tick_entities_and_collect_physics_queries_with_profession_context(
        due_tick,
        &world_read,
        &blocks,
        &items,
    );
    let unchanged = registry
        .lock_entities("read unsupported job-site villager")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        unchanged.retained.villager.unwrap().profession,
        VillagerProfession::None
    );
    assert!(unchanged.retained.villager_merchant.is_none());
    let mut metadata_update = false;
    while let Ok(command) = outbound.try_recv() {
        metadata_update |= matches!(
            command,
            OutboundCommand::UpdateEntityData(snapshot) if snapshot.id == id
        );
    }
    assert!(!metadata_update);
}

#[test]
fn working_villager_restocks_only_at_job_site_with_cooldown_and_daily_limit() {
    let registry = SessionRegistry::new();
    let id = install_brain(&registry);
    let customer = crate::login::offline_uuid("RestockCustomer");
    {
        let mut entities = registry.lock_entities("install villager restock fixture");
        let expected = entities.snapshot(id).unwrap();
        let mut next = expected.clone();
        let mut merchant = VillagerMerchantState::new(vec![VillagerTradeOffer::new(
            VillagerTradeCost::new(17, 1),
            EntityItemStack::new(23, 1),
            12,
            1,
            0.2,
        )])
        .unwrap();
        merchant.record_player_trade(customer, 0).unwrap();
        merchant.offers[0].uses = 1;
        next.retained.villager_merchant = Some(merchant);
        assert!(entities.replace_snapshot_if_current(expected, next));
    }

    assert_eq!(apply_brain(&registry, id, 1, 2_000), 1);
    let away = registry
        .lock_entities("read merchant away from job site")
        .snapshot(id)
        .unwrap();
    let away_merchant = away.retained.villager_merchant.as_ref().unwrap();
    assert_eq!(
        away_merchant.offers[0].uses, 1,
        "work activity alone must not restock away from the claimed job site"
    );
    assert_eq!(away_merchant.trading_reputation(customer), 2);
    assert_eq!(away_merchant.last_reputation_decay_game_time, Some(2_000));

    {
        let mut entities = registry.lock_entities("move merchant to job site");
        let expected = entities.snapshot(id).unwrap();
        let mut next = expected.clone();
        next.position = Vec3::new(8.5, 64.0, 0.5);
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    assert_eq!(apply_brain(&registry, id, 20, 2_020), 1);
    let first = registry
        .lock_entities("read first merchant restock")
        .snapshot(id)
        .unwrap();
    let first_merchant = first.retained.villager_merchant.as_ref().unwrap();
    assert_eq!(first_merchant.offers[0].uses, 0);
    assert_eq!(first_merchant.restocks_today, 1);
    assert_eq!(first_merchant.last_restock_game_time, Some(2_020));

    {
        let mut entities = registry.lock_entities("use merchant offer after first restock");
        let expected = entities.snapshot(id).unwrap();
        let mut next = expected.clone();
        next.retained.villager_merchant.as_mut().unwrap().offers[0].uses = 1;
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    assert_eq!(apply_brain(&registry, id, 40, 2_220), 0);
    assert_eq!(
        registry
            .lock_entities("read merchant cooldown")
            .snapshot(id)
            .unwrap()
            .retained
            .villager_merchant
            .as_ref()
            .unwrap()
            .offers[0]
            .uses,
        1
    );

    assert_eq!(apply_brain(&registry, id, 60, 3_220), 1);
    {
        let mut entities = registry.lock_entities("use merchant offer after second restock");
        let expected = entities.snapshot(id).unwrap();
        let mut next = expected.clone();
        let merchant = next.retained.villager_merchant.as_mut().unwrap();
        assert_eq!(merchant.restocks_today, 2);
        merchant.offers[0].uses = 1;
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    assert_eq!(apply_brain(&registry, id, 80, 4_420), 0);
    assert_eq!(apply_brain(&registry, id, 100, 26_000), 1);
    let next_day = registry
        .lock_entities("read next-day merchant restock")
        .snapshot(id)
        .unwrap();
    let next_day_merchant = next_day.retained.villager_merchant.as_ref().unwrap();
    assert_eq!(next_day_merchant.offers[0].uses, 0);
    assert_eq!(next_day_merchant.restocks_today, 1);
    assert_eq!(next_day_merchant.last_restock_day, 1);
    assert_eq!(next_day_merchant.trading_reputation(customer), 0);
    assert!(next_day_merchant.player_reputations.is_empty());
    assert_eq!(
        next_day_merchant.last_reputation_decay_game_time,
        Some(26_000)
    );
}

#[tokio::test]
async fn plugin_binding_is_a_persisted_override_that_expires_back_to_schedule() {
    let registry = SessionRegistry::new();
    let id = install_brain(&registry);
    let claim = registry
        .claim_script_villager_binding(
            Vec3::new(0.5, 64.0, 0.5),
            2.0,
            "owner.brain-test".to_owned(),
        )
        .await
        .expect("claim request")
        .expect("villager claim");
    assert!(
        registry
            .apply_script_villager_binding_goal(claim.token().to_owned(), GoalState::Idle,)
            .await
            .expect("hold override")
    );

    let controlled = registry
        .lock_entities("read controlled villager")
        .snapshot(id)
        .unwrap();
    assert_eq!(controlled.goal, GoalState::Idle);
    let brain = controlled.retained.villager_brain.as_ref().unwrap();
    assert_eq!(brain.activity, VillagerActivity::Controlled);
    assert!(brain.override_order.is_some());
    assert_eq!(brain.override_expires_tick, Some(claim.expires_at_tick()));
    assert!(registry.overridden_villager_entities().contains(&id));

    assert_eq!(
        apply_brain(&registry, id, claim.expires_at_tick() - 1, 9_000),
        0,
        "active override remains unchanged before exact expiry"
    );
    let controlled = registry
        .lock_entities("read controlled activity before expiry")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        controlled
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .activity,
        VillagerActivity::Controlled
    );
    assert_eq!(controlled.goal, GoalState::Idle);

    registry.synchronize_entity_lifecycle_epoch(claim.expires_at_tick());
    assert_eq!(
        apply_brain(&registry, id, claim.expires_at_tick(), 9_000),
        1
    );
    let resumed = registry
        .lock_entities("read resumed villager schedule")
        .snapshot(id)
        .unwrap();
    assert_eq!(
        resumed.retained.villager_brain.as_ref().unwrap().activity,
        VillagerActivity::Meet
    );
    assert_eq!(
        resumed
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .override_order,
        None
    );
    assert_eq!(
        resumed.goal,
        GoalState::FollowPosition {
            target: Vec3::new(4.5, 64.0, 4.5),
            speed: 0.3,
        }
    );
}
