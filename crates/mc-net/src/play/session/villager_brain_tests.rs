use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_entity::villager_26_1_2::{
    VillagerActivity, VillagerBrainProfile, VillagerBrainState, VillagerPoiSet,
    VillagerScheduleEntry, VillagerScheduleKind,
};
use mc_entity::villager_gossip_26_1_2::{VillagerGossipEvent, VillagerGossipState};
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
    apply_villager_brain_transitions, apply_villager_gossip_transfers,
    commit_villager_brain_transitions, commit_villager_gossip_transfer_pair,
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

fn install_gossip_villager(
    registry: &SessionRegistry,
    position: Vec3,
    activity: VillagerActivity,
    gossip: Option<VillagerGossipState>,
) -> EntityId {
    let id = registry.spawn_script_villager_for_test(position);
    let mut entities = registry.lock_entities("install villager gossip fixture");
    let expected = entities.snapshot(id).expect("villager snapshot");
    let mut next = expected.clone();
    next.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::None,
        1,
    ));
    let mut brain = VillagerBrainState::adult(VillagerPoiSet {
        home: Some(position),
        job_site: None,
        meeting_point: Some(position),
    });
    brain.activity = activity;
    next.retained.villager_brain = Some(brain);
    next.retained.villager_gossip = gossip;
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
        let mut gossip = mc_entity::villager_gossip_26_1_2::VillagerGossipState::default();
        gossip.record_event(
            mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::Trade { player: customer },
        );
        merchant.offers[0].uses = 1;
        next.retained.villager_gossip = Some(gossip);
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
    let away_gossip = away.retained.villager_gossip.as_ref().unwrap();
    assert_eq!(away_gossip.trading_value(customer), 2);
    assert_eq!(away_gossip.last_decay_game_time, 2_000);

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
    let next_day_gossip = next_day.retained.villager_gossip.as_ref().unwrap();
    assert_eq!(next_day_gossip.trading_value(customer), 0);
    assert!(next_day_gossip.player_gossips.is_empty());
    assert_eq!(next_day_gossip.last_decay_game_time, 26_000);
}

#[test]
fn nearby_idle_villagers_transfer_gossip_with_mutual_cooldown_and_interaction_target() {
    let registry = SessionRegistry::new();
    let player = uuid::Uuid::from_u128(0xCAFE);
    let receiver = install_gossip_villager(
        &registry,
        Vec3::new(0.5, 64.0, 0.5),
        VillagerActivity::Idle,
        None,
    );
    let mut source_gossip = VillagerGossipState::default();
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    let source = install_gossip_villager(
        &registry,
        Vec3::new(1.5, 64.0, 0.5),
        VillagerActivity::Work,
        Some(source_gossip),
    );
    let initiators = HashSet::from([receiver]);
    let candidates = HashSet::from([receiver, source]);
    let mut entities = registry.lock_entities("apply villager gossip transfer");

    assert_eq!(
        apply_villager_gossip_transfers(&mut entities, &initiators, &candidates, 1_200),
        1
    );
    let received = entities.snapshot(receiver).unwrap();
    let source_after = entities.snapshot(source).unwrap();
    assert_eq!(
        received
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        30
    );
    let receiver_brain = received.retained.villager_brain.as_ref().unwrap();
    assert_eq!(receiver_brain.interaction_target, Some(source));
    assert_eq!(receiver_brain.last_gossip_time, 1_200);
    assert_eq!(
        source_after
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .last_gossip_time,
        1_200
    );

    let expected = source_after;
    let mut next = expected.clone();
    next.retained
        .villager_gossip
        .as_mut()
        .unwrap()
        .record_event(VillagerGossipEvent::HurtByPlayer { player });
    assert!(entities.replace_snapshot_if_current(expected, next));
    assert_eq!(
        apply_villager_gossip_transfers(&mut entities, &initiators, &candidates, 2_399),
        0,
        "both participants must observe the full mutual 1,200-tick cooldown"
    );
    assert_eq!(
        entities
            .snapshot(receiver)
            .unwrap()
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        30
    );
    assert_eq!(
        apply_villager_gossip_transfers(&mut entities, &initiators, &candidates, 2_400),
        1
    );
    let received = entities.snapshot(receiver).unwrap();
    assert_eq!(
        received
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        55,
        "transfer decay is 20 and merge uses max rather than addition"
    );
    assert_eq!(
        received
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .last_gossip_time,
        2_400
    );
}

#[test]
fn production_villager_tick_routes_due_idle_gossip_transfer() {
    let registry = SessionRegistry::new();
    let (_session, _outbound) = register_profession_observer(&registry);
    registry.set_world_time(10);
    let receiver_position = Vec3::new(2.5, 64.0, 0.5);
    let source_position = Vec3::new(3.5, 64.0, 0.5);
    let dispatches = registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[
            HerdSpawn {
                chunk: (0, 0),
                slot: 10,
                entity_type_id: 119,
                entity_type_name: "minecraft:villager".to_owned(),
                position: receiver_position,
                hostile: false,
                sheep_color: None,
            },
            HerdSpawn {
                chunk: (0, 0),
                slot: 11,
                entity_type_id: 119,
                entity_type_name: "minecraft:villager".to_owned(),
                position: source_position,
                hostile: false,
                sheep_color: None,
            },
        ],
    );
    let mut spawned = dispatches
        .iter()
        .filter_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(snapshot)
                if snapshot.type_name == "minecraft:villager" =>
            {
                Some((snapshot.position.x, snapshot.id))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    spawned.sort_by(|left, right| left.0.total_cmp(&right.0));
    let receiver = spawned[0].1;
    let source = spawned[1].1;
    dispatch_visibility_commands(dispatches);

    let player = uuid::Uuid::from_u128(0xFEED);
    let mut source_gossip = VillagerGossipState::default();
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    {
        let mut entities = registry.lock_entities("install tracked villager gossip pair");
        for (id, position, gossip) in [
            (receiver, receiver_position, None),
            (source, source_position, Some(source_gossip)),
        ] {
            let expected = entities.snapshot(id).unwrap();
            let mut next = expected.clone();
            next.retained.villager = Some(VillagerData::new(
                VillagerKind::Plains,
                VillagerProfession::None,
                1,
            ));
            next.retained.villager_brain = Some(VillagerBrainState::adult(VillagerPoiSet {
                home: Some(position),
                job_site: None,
                meeting_point: Some(position),
            }));
            next.retained.villager_gossip = gossip;
            assert!(entities.replace_snapshot_if_current(expected, next));
        }
    }
    let phase = (20 - u64::from(receiver.0.unsigned_abs()) % 20) % 20;
    let due_tick = 1_200 + phase;

    let _ = registry.tick_entities_and_collect_physics_queries(due_tick);

    let receiver = registry
        .lock_entities("read production villager gossip transfer")
        .snapshot(receiver)
        .unwrap();
    assert_eq!(
        receiver
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        30
    );
    assert_eq!(
        receiver
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .interaction_target,
        Some(source)
    );
}

#[test]
fn stale_villager_gossip_pair_rejects_both_snapshots_without_partial_cooldown() {
    let registry = SessionRegistry::new();
    let player = uuid::Uuid::from_u128(0xD00D);
    let receiver = install_gossip_villager(
        &registry,
        Vec3::new(0.5, 64.0, 0.5),
        VillagerActivity::Meet,
        None,
    );
    let mut source_gossip = VillagerGossipState::default();
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    let source = install_gossip_villager(
        &registry,
        Vec3::new(1.5, 64.0, 0.5),
        VillagerActivity::Idle,
        Some(source_gossip),
    );
    let mut entities = registry.lock_entities("reject stale villager gossip pair");
    let receiver_expected = entities.snapshot(receiver).unwrap();
    let source_expected = entities.snapshot(source).unwrap();

    let mut source_current = source_expected.clone();
    source_current
        .retained
        .villager_gossip
        .as_mut()
        .unwrap()
        .record_event(VillagerGossipEvent::HurtByPlayer { player });
    assert!(entities.replace_snapshot_if_current(source_expected.clone(), source_current));
    assert!(!commit_villager_gossip_transfer_pair(
        &mut entities,
        receiver_expected,
        source_expected,
        1_200,
    ));

    let receiver_after = entities.snapshot(receiver).unwrap();
    let receiver_brain = receiver_after.retained.villager_brain.as_ref().unwrap();
    assert_eq!(receiver_brain.interaction_target, None);
    assert_eq!(receiver_brain.last_gossip_time, 0);
    assert!(receiver_after.retained.villager_gossip.is_none());
    let source_after = entities.snapshot(source).unwrap();
    assert_eq!(
        source_after
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        75
    );
    assert_eq!(
        source_after
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .last_gossip_time,
        0
    );
}

#[test]
fn one_source_participates_in_at_most_one_gossip_pair_per_tick() {
    let registry = SessionRegistry::new();
    let player = uuid::Uuid::from_u128(0xFACE);
    let mut source_gossip = VillagerGossipState::default();
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    source_gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    let source = install_gossip_villager(
        &registry,
        Vec3::new(1.5, 64.0, 0.5),
        VillagerActivity::Work,
        Some(source_gossip),
    );
    let first = install_gossip_villager(
        &registry,
        Vec3::new(0.5, 64.0, 0.5),
        VillagerActivity::Idle,
        None,
    );
    let second = install_gossip_villager(
        &registry,
        Vec3::new(2.5, 64.0, 0.5),
        VillagerActivity::Idle,
        None,
    );
    let mut entities = registry.lock_entities("apply disjoint villager gossip pairs");
    assert_eq!(
        apply_villager_gossip_transfers(
            &mut entities,
            &HashSet::from([first, second]),
            &HashSet::from([source, first, second]),
            1_200,
        ),
        1
    );
    assert_eq!(
        entities
            .snapshot(first)
            .unwrap()
            .retained
            .villager_gossip
            .as_ref()
            .unwrap()
            .minor_negative_value(player),
        30
    );
    assert!(
        entities
            .snapshot(second)
            .unwrap()
            .retained
            .villager_gossip
            .is_none()
    );
    assert_eq!(
        entities
            .snapshot(source)
            .unwrap()
            .retained
            .villager_brain
            .as_ref()
            .unwrap()
            .last_gossip_time,
        1_200
    );
}

#[test]
fn gossip_transfer_requires_idle_or_meet_and_distance_squared_at_most_five() {
    let registry = SessionRegistry::new();
    let player = uuid::Uuid::from_u128(0xABCD);
    let mut gossip = VillagerGossipState::default();
    gossip.record_event(VillagerGossipEvent::HurtByPlayer { player });
    let controlled = install_gossip_villager(
        &registry,
        Vec3::new(0.5, 64.0, 0.5),
        VillagerActivity::Controlled,
        None,
    );
    let nearby = install_gossip_villager(
        &registry,
        Vec3::new(1.5, 64.0, 0.5),
        VillagerActivity::Idle,
        Some(gossip.clone()),
    );
    let far = install_gossip_villager(
        &registry,
        Vec3::new(10.5, 64.0, 0.5),
        VillagerActivity::Idle,
        Some(gossip),
    );
    let mut entities = registry.lock_entities("validate villager gossip activity and reach");
    assert_eq!(
        apply_villager_gossip_transfers(
            &mut entities,
            &HashSet::from([controlled]),
            &HashSet::from([controlled, nearby]),
            1_200,
        ),
        0
    );
    assert_eq!(
        apply_villager_gossip_transfers(
            &mut entities,
            &HashSet::from([nearby]),
            &HashSet::from([nearby, far]),
            1_200,
        ),
        0
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
