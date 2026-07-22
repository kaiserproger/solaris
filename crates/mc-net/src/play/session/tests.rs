use super::*;
use mc_data::recipes::{Ingredient, IngredientAlternative, RecipeKind};
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::login::GameProfileProperty;
use mc_protocol::packets::play::PlayerInfoUpdate;
use mc_script::{ScriptEventKind, ScriptGameMode};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Barrier, Mutex};
use tokio::io::duplex;
use tokio::sync::mpsc;

use crate::connection::{ConnectionReader, PRE_PLAY_READ_TIMEOUT, read_packet_with_timeout};

#[test]
fn lethal_survival_commit_pushes_immutable_player_death_before_session_cleanup() {
    let registry = SessionRegistry::new();
    let mut deaths = registry.install_script_commit_event_outbox();
    let session = register_test_session(&registry, "CommittedDeath");
    let pose = PlayerPose::new(2.5, 70.0, -4.5);
    let persisted = PlayerPersistedState::new_default(pose);
    let expected_survival = persisted.survival;
    let expected_inventory = persisted.inventory.clone();
    let expected_carried_item = persisted.carried_item.clone();
    let expected_xp = persisted.xp.clone();
    registry.register_player_persistence(session, Arc::new(Mutex::new(persisted)));

    let mut dead = expected_survival;
    dead.apply_damage(SurvivalState::MAX_HEALTH);
    let committed = registry.commit_player_survival(
        &SimulationAuthority::for_test(),
        session,
        &PlayerSurvivalPlan {
            expected_survival,
            updated_survival: dead,
            expected_inventory: expected_inventory.clone(),
            updated_inventory: expected_inventory,
            expected_carried_item,
            expected_xp: expected_xp.clone(),
            updated_xp: expected_xp,
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            position: Vec3::new(pose.x, pose.y, pose.z),
        },
    );
    assert!(matches!(
        committed,
        Some(PlayerSurvivalCommitOutcome::Committed(committed)) if committed.died
    ));
    assert!(
        registry
            .lock_inner("verify dead target projection")
            .dead_sessions
            .contains(&session)
    );

    registry.unregister(session);
    let event = deaths
        .try_recv()
        .expect("authoritative death must survive session cleanup");
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerDied {
            player_id,
            context,
            dimension,
            game_mode: ScriptGameMode::Survival,
        } if player_id.value() == session
            && context.username() == "CommittedDeath"
            && (context.x(), context.y(), context.z()) == (pose.x, pose.y, pose.z)
            && dimension == "minecraft:overworld"
    ));
    assert!(matches!(
        deaths.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn respawn_commit_clears_player_hurt_resistance() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "RespawnResistance");
    let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    persisted.survival.apply_damage(SurvivalState::MAX_HEALTH);
    let expected_survival = persisted.survival;
    let expected_inventory = persisted.inventory.clone();
    let expected_carried_item = persisted.carried_item.clone();
    let expected_xp = persisted.xp.clone();
    registry.register_player_persistence(session, Arc::new(Mutex::new(persisted)));
    let (_, resistance) = PlayerHurtResistance::default().preview(10, 4.0);
    registry
        .lock_inner("install respawn resistance test state")
        .player_hurt_resistance
        .insert(session, resistance);

    let committed = registry.commit_player_survival(
        &SimulationAuthority::for_test(),
        session,
        &PlayerSurvivalPlan {
            expected_survival,
            updated_survival: SurvivalState::FULL,
            expected_inventory: expected_inventory.clone(),
            updated_inventory: expected_inventory,
            expected_carried_item,
            expected_xp: expected_xp.clone(),
            updated_xp: expected_xp,
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            position: Vec3::new(0.5, 64.0, 0.5),
        },
    );

    assert!(matches!(
        committed,
        Some(PlayerSurvivalCommitOutcome::Committed(_))
    ));
    let inner = registry.lock_inner("verify respawn projections reset");
    assert!(!inner.player_hurt_resistance.contains_key(&session));
    assert!(!inner.dead_sessions.contains(&session));
}

struct CountingEntityJournal(Arc<AtomicUsize>);

impl mc_entity::RegionalDecisionJournal for CountingEntityJournal {
    fn record_commit(
        &mut self,
        _decision: &mc_entity::RegionalCommitDecision,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn clear_commit(
        &mut self,
        _phase: mc_entity::RegionPhase,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        Ok(())
    }
}

struct BlockingEntityCommitJournal {
    blocked_uuid: uuid::Uuid,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
    commits: Arc<Mutex<Vec<mc_entity::RegionalCommitDecision>>>,
    failure: Option<mc_entity::RegionalDecisionJournalError>,
}

struct FailOnceEntityCommitJournal {
    failure: Option<mc_entity::RegionalDecisionJournalError>,
    commits: Arc<AtomicUsize>,
}

impl mc_entity::RegionalDecisionJournal for FailOnceEntityCommitJournal {
    fn record_commit(
        &mut self,
        _decision: &mc_entity::RegionalCommitDecision,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        match self.failure.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn clear_commit(
        &mut self,
        _phase: mc_entity::RegionPhase,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        Ok(())
    }
}

impl mc_entity::RegionalDecisionJournal for BlockingEntityCommitJournal {
    fn record_commit(
        &mut self,
        decision: &mc_entity::RegionalCommitDecision,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.commits
            .lock()
            .expect("entity journal commits")
            .push(decision.clone());
        if decision
            .upserts()
            .iter()
            .any(|snapshot| snapshot.uuid == self.blocked_uuid)
        {
            self.entered.send(()).expect("publish journal entry");
            self.release.recv().expect("release journal commit");
            if let Some(error) = self.failure {
                return Err(error);
            }
        }
        Ok(())
    }

    fn clear_commit(
        &mut self,
        _phase: mc_entity::RegionPhase,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        Ok(())
    }
}

#[test]
fn hostile_goal_diff_skips_unchanged_goal() {
    let current = hostile_wander_goal();
    assert_eq!(
        changed_hostile_goal(EntityId(7), &current, current.clone()),
        None
    );
    let next = GoalState::Idle;
    assert_eq!(
        changed_hostile_goal(EntityId(7), &current, next.clone()),
        Some((EntityId(7), next))
    );
}

fn dye_item_color(id: &mc_data::Identifier) -> Option<mc_entity::SheepColor> {
    use mc_entity::SheepColor;

    if id.namespace() != "minecraft" {
        return None;
    }
    match id.path() {
        "white_dye" => Some(SheepColor::White),
        "orange_dye" => Some(SheepColor::Orange),
        "magenta_dye" => Some(SheepColor::Magenta),
        "light_blue_dye" => Some(SheepColor::LightBlue),
        "yellow_dye" => Some(SheepColor::Yellow),
        "lime_dye" => Some(SheepColor::Lime),
        "pink_dye" => Some(SheepColor::Pink),
        "gray_dye" => Some(SheepColor::Gray),
        "light_gray_dye" => Some(SheepColor::LightGray),
        "cyan_dye" => Some(SheepColor::Cyan),
        "purple_dye" => Some(SheepColor::Purple),
        "blue_dye" => Some(SheepColor::Blue),
        "brown_dye" => Some(SheepColor::Brown),
        "green_dye" => Some(SheepColor::Green),
        "red_dye" => Some(SheepColor::Red),
        "black_dye" => Some(SheepColor::Black),
        _ => None,
    }
}

fn single_dye_ingredient_color(ingredient: &Ingredient) -> Option<mc_entity::SheepColor> {
    let [IngredientAlternative::Item(item)] = ingredient.alternatives.as_slice() else {
        return None;
    };
    dye_item_color(item)
}

#[test]
fn session_registry_stores_server_entities_in_physical_regions() {
    let registry = SessionRegistry::new();
    let mut entities = registry.lock_entities("test entity access");
    let west_id = entities.spawn(mc_entity::SpawnEntity::new(
        4,
        "minecraft:cow",
        Vec3::new(-0.5, 64.0, 0.5),
    ));
    let east_id = entities.spawn(mc_entity::SpawnEntity::new(
        4,
        "minecraft:cow",
        Vec3::new(128.5, 64.0, 0.5),
    ));

    assert_eq!(west_id, EntityId(SERVER_ENTITY_ID_START));
    assert_eq!(east_id, EntityId(SERVER_ENTITY_ID_START + 1));
    assert_eq!(entities.region_len(mc_entity::RegionKey::new(-1, 0)), 1);
    assert_eq!(entities.region_len(mc_entity::RegionKey::new(1, 0)), 1);

    for position in [
        Vec3::new(129.5, 64.0, 0.5),
        Vec3::new(-1.5, 64.0, 0.5),
        Vec3::new(130.5, 64.0, 0.5),
    ] {
        let expected = entities.snapshot(west_id).expect("west entity snapshot");
        let mut next = expected.clone();
        next.position = position;
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    assert_eq!(entities.region_len(mc_entity::RegionKey::new(-1, 0)), 0);
    assert_eq!(entities.region_len(mc_entity::RegionKey::new(1, 0)), 2);
}

#[test]
fn session_registry_uses_persistent_regional_entity_owners() {
    let registry = SessionRegistry::new();

    assert!(registry.entity_owner_lane_count() >= 1);
}

#[test]
fn sheep_recipe_mix_matches_all_local_vanilla_two_dye_recipes() {
    let recipe_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("data/vanilla/data/minecraft/recipe");
    if !recipe_dir.is_dir() {
        eprintln!("skipping: local vanilla recipes missing");
        return;
    }

    let mut expected = BTreeMap::new();
    for recipe in mc_data::recipes::load_recipes(recipe_dir).unwrap() {
        let RecipeKind::Shapeless(shapeless) = recipe.kind else {
            continue;
        };
        let [first, second] = shapeless.ingredients.as_slice() else {
            continue;
        };
        let Some(first) = single_dye_ingredient_color(first) else {
            continue;
        };
        let Some(second) = single_dye_ingredient_color(second) else {
            continue;
        };
        let Some(result) = dye_item_color(&recipe.result.item) else {
            continue;
        };
        let key = if first.id() <= second.id() {
            (first.id(), second.id())
        } else {
            (second.id(), first.id())
        };
        expected.insert(key, result.id());
    }

    let mut actual = BTreeMap::new();
    for first in mc_entity::SheepColor::ALL {
        for second in mc_entity::SheepColor::ALL {
            if first.id() > second.id() {
                continue;
            }
            if let Some(result) = sheep_recipe_mix(first, second) {
                actual.insert((first.id(), second.id()), result.id());
            }
        }
    }

    assert_eq!(expected.len(), 9, "local 26.1.2 two-dye recipe count");
    assert_eq!(actual, expected);
}

#[test]
fn unmixed_sheep_breeding_is_order_independent_and_uses_both_parents() {
    let mut saw_brown = false;
    let mut saw_black = false;
    for tick in 0..64 {
        let forward = sheep_breeding_color(
            EntityId(9),
            mc_entity::SheepColor::Brown,
            EntityId(3),
            mc_entity::SheepColor::Black,
            tick,
        );
        let reverse = sheep_breeding_color(
            EntityId(3),
            mc_entity::SheepColor::Black,
            EntityId(9),
            mc_entity::SheepColor::Brown,
            tick,
        );

        assert_eq!(forward, reverse);
        saw_brown |= forward == mc_entity::SheepColor::Brown;
        saw_black |= forward == mc_entity::SheepColor::Black;
    }

    assert!(saw_brown);
    assert!(saw_black);
}

#[test]
fn passive_mob_breeding_plan_pairs_each_parent_once() {
    let ready = mc_entity::AnimalBreedingState {
        age_ticks: 0,
        love_ticks: mc_entity::ANIMAL_LOVE_DURATION_TICKS
            - mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS
            + 1,
        sheep_wool: None,
    };
    let animals = [
        BreedingAnimal {
            id: EntityId(1),
            type_id: 4,
            type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(0.0, 64.0, 0.0),
            state: ready,
        },
        BreedingAnimal {
            id: EntityId(2),
            type_id: 4,
            type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(2.0, 64.0, 0.0),
            state: ready,
        },
        BreedingAnimal {
            id: EntityId(3),
            type_id: 4,
            type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(1.0, 64.0, 0.0),
            state: ready,
        },
    ];

    let plan = plan_breeding(17, &animals);

    assert_eq!(plan.births.len(), 1);
    assert_eq!(plan.births[0].position, Vec3::new(1.0, 64.0, 0.0));
    assert_eq!(
        plan.updates[0].state.age_ticks,
        mc_entity::PARENT_BREEDING_COOLDOWN_TICKS
    );
    assert_eq!(
        plan.updates[1].state.age_ticks,
        mc_entity::PARENT_BREEDING_COOLDOWN_TICKS
    );
    assert_eq!(plan.updates[2].state.age_ticks, 0);
    assert_eq!(plan.updates[2].state.love_ticks, ready.love_ticks - 1);
}

#[test]
fn passive_mob_grazing_plan_emits_dense_timer_update_without_mutating_input() {
    let sheep_id = EntityId(7);
    let mut store = mc_entity::EntityStore::new();
    let mut spawned = SpawnEntity::new(4, "minecraft:sheep", Vec3::new(4.75, 63.0, -1.25));
    spawned.animal = Some(mc_entity::AnimalBreedingState::adult_sheep(
        mc_entity::SheepColor::White,
    ));
    let actual_id = store.spawn(spawned);
    let mut expected = store.snapshot(actual_id).expect("sheep snapshot");
    expected.id = sheep_id;
    expected.retained.sheep_grazing_ticks = Some(5);
    let sheep = [GrazingSheep {
        expected: expected.clone(),
        is_baby: false,
    }];

    let advance = advance_sheep_grazing(99, &sheep);

    assert!(advance.plan.starts.is_empty());
    assert_eq!(advance.plan.actions.len(), 1);
    assert_eq!(advance.plan.actions[0].entity_id, sheep_id);
    assert_eq!(
        advance.plan.actions[0].block_position,
        mc_world::BlockPos { x: 4, y: 63, z: -2 }
    );
    assert_eq!(
        advance.timer_updates,
        [passive_mobs::SheepGrazingTimerUpdate {
            expected,
            remaining: Some(4),
        }]
    );
    assert_eq!(sheep[0].expected.retained.sheep_grazing_ticks, Some(5));
}

#[test]
fn campfire_world_commit_does_not_hold_the_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let expected = CampfireCookingState::default();
    let (commit_entered_tx, commit_entered_rx) = std::sync::mpsc::channel();
    let (release_commit_tx, release_commit_rx) = std::sync::mpsc::channel();

    let commit_registry = Arc::clone(&registry);
    let commit_thread = std::thread::spawn(move || {
        commit_registry
            .commit_campfire_cooking_legacy_for_test(
                position,
                &expected,
                CampfireCookingState::default(),
                || {
                    commit_entered_tx.send(()).expect("test receiver remains");
                    let _ = release_commit_rx.recv();
                    Ok::<_, ()>(())
                },
            )
            .expect("campfire commit succeeds");
    });
    commit_entered_rx
        .recv()
        .expect("campfire commit reaches world mutation");

    let time_registry = Arc::clone(&registry);
    let (time_changed_tx, time_changed_rx) = std::sync::mpsc::channel();
    let time_thread = std::thread::spawn(move || {
        time_registry.set_world_time(42);
        time_changed_tx.send(()).expect("test receiver remains");
    });

    time_changed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("campfire world mutation must not block unrelated session state");
    release_commit_tx.send(()).expect("commit thread remains");
    commit_thread.join().expect("commit thread succeeds");
    time_thread.join().expect("time thread succeeds");
    assert_eq!(registry.world_time(), 42);
}

#[test]
fn campfire_tick_changes_state_only_after_world_commit() {
    let registry = SessionRegistry::new();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let input = ItemStack::new(13, 1);
    let result = ItemStack::new(22, 1);
    registry
        .insert_campfire_cooking(position, input, result.clone(), 1)
        .expect("campfire input inserted");

    assert!(
        registry
            .tick_campfire_cooking_conditionally(position, 1, |_| false)
            .is_none()
    );
    assert_eq!(
        registry.campfire_cooking_state(position).slots[0]
            .as_ref()
            .expect("failed world commit preserves cooking slot")
            .ticks_remaining,
        1
    );

    let committed = registry
        .tick_campfire_cooking_conditionally(position, 1, |_| true)
        .expect("successful world commit advances cooking");
    assert_eq!(committed.completed, vec![result]);
    assert_eq!(committed.cooking.pending_outputs.len(), 1);
    assert_eq!(
        registry
            .campfire_cooking_state(position)
            .pending_outputs
            .len(),
        1
    );
}

#[test]
fn pending_campfire_output_materialization_is_uuid_idempotent() {
    let registry = SessionRegistry::new();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let output = PendingCampfireOutput {
        uuid: uuid::Uuid::from_u128(0xCAFE),
        stack: EntityItemStack::new(42, 2).with_damage(3),
    };

    let first = registry.materialize_pending_campfire_outputs_owned(
        &SimulationAuthority::for_test(),
        7,
        position,
        std::slice::from_ref(&output),
    );
    let second = registry.materialize_pending_campfire_outputs_owned(
        &SimulationAuthority::for_test(),
        7,
        position,
        std::slice::from_ref(&output),
    );

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].id, second[0].id);
    assert_eq!(first[0].uuid, output.uuid);
    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn world_clock_does_not_wait_for_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let (registry_locked_tx, registry_locked_rx) = std::sync::mpsc::channel();
    let (release_registry_tx, release_registry_rx) = std::sync::mpsc::channel();

    let lock_registry = Arc::clone(&registry);
    let lock_thread = std::thread::spawn(move || {
        let _inner = lock_registry.lock_inner("hold registry for clock isolation test");
        registry_locked_tx
            .send(())
            .expect("clock test receiver remains");
        release_registry_rx
            .recv()
            .expect("clock test release sender remains");
    });
    registry_locked_rx
        .recv()
        .expect("session registry lock is held");

    let clock_registry = Arc::clone(&registry);
    let (clock_advanced_tx, clock_advanced_rx) = std::sync::mpsc::channel();
    let clock_thread = std::thread::spawn(move || {
        let world_time = clock_registry.advance_world_time(1);
        let _ = clock_advanced_tx.send(world_time);
    });

    let clock_advanced = clock_advanced_rx.recv_timeout(Duration::from_secs(1));
    let _ = release_registry_tx.send(());
    lock_thread.join().expect("registry lock thread succeeds");
    clock_thread.join().expect("clock thread succeeds");

    assert_eq!(
        clock_advanced.expect("world clock must not wait for unrelated session state"),
        1
    );
    assert_eq!(registry.simulation_tick(), 1);
}

#[test]
fn idle_adult_animal_tick_does_not_rewrite_unchanged_state() {
    let registry = SessionRegistry::new();
    registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(1.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        }],
    );
    assert_eq!(registry.persisted_entity_records().len(), 1);

    let (births, dispatches) = registry.tick_animal_breeding(&SimulationAuthority::for_test());

    assert_eq!(births, 0);
    assert!(dispatches.is_empty());
    assert_eq!(
        registry.breeding_entity_scan_visits.load(Ordering::Relaxed),
        0,
        "idle adult animals should not enter the breeding tick"
    );
    assert_eq!(registry.breeding_state_update_count(), 0);
    assert_eq!(registry.breeding_commit_count(), 0);
}

#[test]
fn animal_age_countdown_waits_for_entity_save_barrier() {
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(CountingEntityJournal(Arc::clone(&commits))),
    );
    registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(1.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        }],
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(entity_id, mc_entity::AnimalBreedingState::baby(),));
    }
    commits.store(0, Ordering::Relaxed);

    let (births, dispatches) = registry.tick_animal_breeding(&SimulationAuthority::for_test());

    assert_eq!(births, 0);
    assert!(dispatches.is_empty());
    assert_eq!(commits.load(Ordering::Relaxed), 0);
    let saved = registry.persisted_entity_records();
    assert_eq!(
        saved[0].snapshot.animal.unwrap().age_ticks,
        mc_entity::BABY_START_AGE_TICKS + 1
    );
}

#[test]
fn entity_save_owner_barrier_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .entity_save_owner_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let save_registry = Arc::clone(&registry);
    let save = std::thread::spawn(move || save_registry.persisted_entity_save_snapshot());
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("save reaches regional owner barrier");
    let session_available = registry.inner.try_lock().is_ok();
    resume_tx.send(()).expect("release entity save barrier");
    let (records, _) = save.join().expect("entity save snapshot worker");

    assert!(
        session_available,
        "regional save barrier must not retain session state"
    );
    assert_eq!(records.records.len(), 1);
}

#[test]
fn breeding_plan_does_not_hold_session_or_entity_locks() {
    let registry = Arc::new(SessionRegistry::new());
    register_test_session(&registry, "BreedingPlanAlice");
    registry.ensure_chunk_herd_legacy_for_test(
        (0, 0),
        &[HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(1.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        }],
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(entity_id, mc_entity::AnimalBreedingState::baby()));
    }
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .breeding_plan_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(BreedingPlanProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let held_session = registry.lock_inner("hold session during breeding snapshot");
    let breeding_registry = Arc::clone(&registry);
    let breeding = std::thread::spawn(move || {
        breeding_registry.tick_animal_breeding(&SimulationAuthority::for_test())
    });
    let snapshot_result = reached_rx.recv_timeout(Duration::from_secs(1));
    drop(held_session);

    let (session_tx, session_rx) = std::sync::mpsc::channel();
    let session_registry = Arc::clone(&registry);
    let session_read = std::thread::spawn(move || {
        session_tx
            .send(session_registry.active_session_count())
            .expect("session read receiver");
    });
    let (entity_tx, entity_rx) = std::sync::mpsc::channel();
    let entity_registry = Arc::clone(&registry);
    let entity_read = std::thread::spawn(move || {
        entity_tx
            .send(entity_registry.server_entity_snapshot(entity_id))
            .expect("entity read receiver");
    });

    let session_result = session_rx.recv_timeout(Duration::from_secs(1));
    let entity_result = entity_rx.recv_timeout(Duration::from_secs(1));
    resume_tx.send(()).expect("release breeding plan");
    let (births, _) = breeding.join().expect("breeding worker");
    session_read.join().expect("session read worker");
    entity_read.join().expect("entity read worker");

    assert_eq!(births, 0);
    snapshot_result.expect("breeding snapshot must not wait for the session registry");
    assert_eq!(
        session_result.expect("pure breeding plan must release session registry"),
        1
    );
    assert!(
        entity_result
            .expect("pure breeding plan must release entity store")
            .is_some()
    );
}

#[test]
fn breeding_rejects_the_whole_plan_when_a_parent_changes_after_snapshot() {
    let registry = Arc::new(SessionRegistry::new());
    for x in [0.5, 1.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:cow".to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let parent_ids = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.id)
        .collect::<Vec<_>>();
    assert_eq!(parent_ids.len(), 2);
    let courtship_complete =
        mc_entity::ANIMAL_LOVE_DURATION_TICKS - mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS;
    let ready = mc_entity::AnimalBreedingState {
        age_ticks: 0,
        love_ticks: courtship_complete + 1,
        sheep_wool: None,
    };
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(parent_ids[0], ready));
        assert!(entities.set_animal_state(parent_ids[1], ready));
    }

    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .breeding_plan_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(BreedingPlanProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let breeding_registry = Arc::clone(&registry);
    let breeding = std::thread::spawn(move || {
        breeding_registry.tick_animal_breeding(&SimulationAuthority::for_test())
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("breeding reaches the post-snapshot boundary");

    let newer_parent_state = mc_entity::AnimalBreedingState::adult();
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(parent_ids[0], newer_parent_state));
    }
    resume_tx.send(()).expect("release breeding plan");
    let (births, dispatches) = breeding.join().expect("breeding worker");

    assert_eq!(births, 0);
    assert!(dispatches.is_empty());
    let entities = registry.lock_entities("test entity access");
    assert_eq!(entities.len(), 2, "stale plan must not spawn a child");
    assert_eq!(
        entities
            .snapshot(parent_ids[0])
            .and_then(|snapshot| snapshot.animal),
        Some(newer_parent_state),
        "stale plan must preserve the newer parent state"
    );
    assert_eq!(
        entities
            .snapshot(parent_ids[1])
            .and_then(|snapshot| snapshot.animal),
        Some(ready),
        "stale plan must not partially commit the other parent"
    );
}

#[test]
fn breeding_commits_and_publishes_once_across_a_region_boundary() {
    let registry = SessionRegistry::new();
    let observer = register_test_session(&registry, "BoundaryBreedingAlice");
    assert!(registry.mark_loaded(observer, (7, 0)).is_empty());
    assert!(registry.mark_loaded(observer, (8, 0)).is_empty());
    for x in [127.5, 128.5] {
        let dispatches = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:cow".to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
        assert_eq!(dispatches.len(), 1);
    }
    let parent_ids = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.id)
        .collect::<Vec<_>>();
    assert_eq!(parent_ids.len(), 2);
    let courtship_complete =
        mc_entity::ANIMAL_LOVE_DURATION_TICKS - mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS;
    let ready = mc_entity::AnimalBreedingState {
        age_ticks: 0,
        love_ticks: courtship_complete + 1,
        sheep_wool: None,
    };
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(parent_ids[0], ready));
        assert!(entities.set_animal_state(parent_ids[1], ready));
    }
    registry.entities.reset_owner_requests_for_test();

    let (births, dispatches) = registry.tick_animal_breeding(&SimulationAuthority::for_test());

    assert_eq!(births, 1);
    assert_eq!(registry.entities.owner_requests_for_test(), 6);
    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 3);
    let child = records
        .iter()
        .find(|record| {
            record
                .snapshot
                .animal
                .is_some_and(|animal| animal.age_ticks == mc_entity::BABY_START_AGE_TICKS)
        })
        .expect("one authoritative baby")
        .snapshot
        .clone();
    assert_eq!(
        records
            .iter()
            .filter_map(|record| record.snapshot.animal)
            .filter(|animal| animal.age_ticks == mc_entity::PARENT_BREEDING_COOLDOWN_TICKS)
            .count(),
        2
    );
    assert_eq!(
        dispatches
            .iter()
            .filter(|dispatch| {
                matches!(&dispatch.command, OutboundCommand::SpawnEntity(entity) if entity.id == child.id)
            })
            .count(),
        1
    );
    let inner = registry.inner.lock().expect("session registry poisoned");
    assert_eq!(
        inner
            .published_entity_snapshots
            .get(&child.id)
            .map(|entity| entity.id),
        Some(child.id)
    );
    assert_eq!(inner.entity_chunks.get(&child.id), Some(&(8, 0)));
}

#[test]
fn breeding_commit_releases_both_locks_before_session_publication() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = registry.lock_entities("test entity access");
        assert!(entities.set_animal_state(entity_id, mc_entity::AnimalBreedingState::baby()));
    }
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .breeding_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(BreedingCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let commit_registry = Arc::clone(&registry);
    let commit = std::thread::spawn(move || {
        commit_registry.tick_animal_breeding(&SimulationAuthority::for_test())
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("breeding reaches the publication boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entities_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release breeding commit");
    let (births, _) = commit.join().expect("breeding commit worker");

    assert!(
        session_available,
        "breeding publication must not retain session state"
    );
    assert!(
        entities_available,
        "breeding publication must release the entity store"
    );
    assert_eq!(births, 0);
}

#[test]
fn sheep_grazing_plan_does_not_hold_session_and_entity_locks_together() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "GrazingPlanLocksAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .sheep_grazing_plan_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SheepGrazingPlanProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let planning_registry = Arc::clone(&registry);
    let planning = std::thread::spawn(move || {
        planning_registry.plan_sheep_grazing(&SimulationAuthority::for_test(), 1)
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("sheep grazing plan reaches entity snapshot");

    let session_was_available = registry.inner.try_lock().is_ok();
    resume_tx.send(()).expect("release sheep grazing plan");
    planning.join().expect("sheep grazing planner joins");
    assert!(
        session_was_available,
        "entity snapshot must not retain the session registry"
    );
}

#[test]
fn sheep_grazing_start_releases_both_locks_before_session_publication() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let candidate = SheepGrazingCandidate {
        entity_id,
        block_position: mc_world::BlockPos { x: 0, y: 64, z: 0 },
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .sheep_grazing_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SheepGrazingCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let start_registry = Arc::clone(&registry);
    let start = std::thread::spawn(move || {
        start_registry.start_sheep_grazing(&SimulationAuthority::for_test(), &[candidate])
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("grazing start reaches the publication boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entities_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release grazing start");
    let (started, _) = start.join().expect("grazing start worker");

    assert!(
        session_available,
        "start publication must not retain session state"
    );
    assert!(
        entities_available,
        "start publication must release the entity store"
    );
    assert_eq!(started, 1);
}

#[test]
fn sheep_grazing_owner_validation_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let candidate = SheepGrazingCandidate {
        entity_id,
        block_position: mc_world::BlockPos { x: 0, y: 64, z: 0 },
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .sheep_grazing_owner_read_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let start_registry = Arc::clone(&registry);
    let start = std::thread::spawn(move || {
        start_registry.start_sheep_grazing(&SimulationAuthority::for_test(), &[candidate])
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("grazing reaches owner validation");
    let session_available = registry.inner.try_lock().is_ok();
    resume_tx.send(()).expect("release grazing owner read");
    let (started, _) = start.join().expect("grazing start worker");

    assert!(
        session_available,
        "grazing owner validation must not retain session state"
    );
    assert_eq!(started, 1);
}

#[test]
fn sheep_grazing_start_uses_constant_owner_requests() {
    let registry = SessionRegistry::new();
    for x in [0.5, 1.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:sheep".to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let candidates = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| SheepGrazingCandidate {
            entity_id: record.snapshot.id,
            block_position: mc_world::BlockPos {
                x: record.snapshot.position.x.floor() as i32,
                y: 64,
                z: 0,
            },
        })
        .collect::<Vec<_>>();
    registry.entities.reset_owner_requests_for_test();

    let (started, _) = registry.start_sheep_grazing(&SimulationAuthority::for_test(), &candidates);

    assert_eq!(started, 2);
    assert_eq!(registry.entities.owner_requests_for_test(), 4);
}

#[test]
fn sheep_grazing_finish_releases_both_locks_before_session_publication() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:sheep".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    assert!(registry.set_sheep_sheared_for_test(entity_id, true));
    assert!(registry.set_sheep_grazing_ticks_for_test(entity_id, Some(SHEEP_GRAZING_ACTION_TICK),));
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .sheep_grazing_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SheepGrazingCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let finish_registry = Arc::clone(&registry);
    let finish = std::thread::spawn(move || {
        finish_registry.finish_sheep_grazing(&SimulationAuthority::for_test(), &[entity_id])
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("grazing finish reaches the publication boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entities_available = registry.entities.owner_responsive_for_test();
    assert!(registry.set_sheep_sheared_for_test(entity_id, true));
    resume_tx.send(()).expect("release grazing finish");
    let (ate, _) = finish.join().expect("grazing finish worker");

    assert!(
        session_available,
        "finish publication must not retain session state"
    );
    assert!(
        entities_available,
        "finish publication must release the entity store"
    );
    assert_eq!(ate, 0, "stale grazing publication must be rejected");
    assert!(
        registry
            .server_entity_snapshot(entity_id)
            .and_then(|entity| entity.animal)
            .and_then(|animal| animal.sheep_wool)
            .is_some_and(|wool| wool.sheared)
    );
}

#[test]
fn sheep_grazing_finish_uses_constant_owner_requests() {
    let registry = SessionRegistry::new();
    for x in [0.5, 1.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:sheep".to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let entity_ids = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.id)
        .collect::<Vec<_>>();
    for entity_id in &entity_ids {
        assert!(registry.set_sheep_sheared_for_test(*entity_id, true));
    }
    for entity_id in &entity_ids {
        assert!(
            registry.set_sheep_grazing_ticks_for_test(*entity_id, Some(SHEEP_GRAZING_ACTION_TICK),)
        );
    }
    registry.entities.reset_owner_requests_for_test();

    let (ate, _) = registry.finish_sheep_grazing(&SimulationAuthority::for_test(), &entity_ids);

    assert_eq!(ate, 2);
    assert_eq!(registry.entities.owner_requests_for_test(), 4);
    assert!(entity_ids.into_iter().all(|entity_id| {
        registry
            .server_entity_snapshot(entity_id)
            .and_then(|entity| entity.animal)
            .and_then(|animal| animal.sheep_wool)
            .is_some_and(|wool| !wool.sheared)
    }));
}

#[test]
fn sheep_grazing_plan_only_visits_loaded_sheep() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "GrazingPlanIndexAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    for (entity_type, x) in [
        ("minecraft:sheep", 0.5),
        ("minecraft:sheep", 160.5),
        ("minecraft:cow", 1.5),
    ] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            entity_type.to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }

    let _ = registry.plan_sheep_grazing(&SimulationAuthority::for_test(), 1);

    assert_eq!(
        registry.sheep_grazing_entity_visits.load(Ordering::Relaxed),
        1,
        "grazing plan should visit only sheep indexed in loaded chunks"
    );
}

#[test]
fn off_phase_hostile_is_not_materialized_as_attack_candidate() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "OffPhaseHostileAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let phase = u64::from(entity_id.0.unsigned_abs()) % HOSTILE_MELEE_PERIOD_TICKS;
    let due_tick = if phase == 0 {
        HOSTILE_MELEE_PERIOD_TICKS
    } else {
        HOSTILE_MELEE_PERIOD_TICKS - phase
    };

    let (attacks, dispatches) = registry.tick_hostile_attacks(
        &SimulationAuthority::for_test(),
        due_tick.saturating_sub(1),
        mc_world::BlockStateId(0),
    );

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
    assert_eq!(registry.hostile_attack_candidate_count(), 0);
}

#[test]
fn hostile_scan_only_visits_entities_in_loaded_chunks() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "HostileScanIndexAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    for x in [0.5, 160.5, 320.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(x, 64.0, 1.5),
        );
    }

    let _ = registry.tick_hostile_attacks(
        &SimulationAuthority::for_test(),
        1,
        mc_world::BlockStateId(0),
    );

    assert_eq!(
        registry.hostile_entity_scan_visits.load(Ordering::Relaxed),
        1,
        "hostile scan should not visit entities outside loaded chunks"
    );
}

#[test]
fn hostile_candidate_scan_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "HostileScanAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let phase = u64::from(entity_id.0.unsigned_abs()) % HOSTILE_MELEE_PERIOD_TICKS;
    let due_tick = if phase == 0 {
        HOSTILE_MELEE_PERIOD_TICKS
    } else {
        HOSTILE_MELEE_PERIOD_TICKS - phase
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_scan_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HostileScanProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let hostile_registry = Arc::clone(&registry);
    let hostile_tick = std::thread::spawn(move || {
        hostile_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            mc_world::BlockStateId(0),
        )
    });
    reached_rx.recv().expect("hostile scan reaches probe");

    let (session_tx, session_rx) = std::sync::mpsc::channel();
    let session_registry = Arc::clone(&registry);
    let session_read = std::thread::spawn(move || {
        session_tx
            .send(session_registry.active_session_count())
            .expect("session read receiver");
    });
    let session_result = session_rx.recv_timeout(Duration::from_secs(1));
    resume_tx.send(()).expect("release hostile scan");
    hostile_tick.join().expect("hostile tick worker");
    session_read.join().expect("session read worker");

    assert_eq!(
        session_result.expect("hostile ECS scan must not hold session registry"),
        1
    );
}

#[test]
fn hostile_commit_releases_both_locks_before_arrow_publication() {
    let registry = Arc::new(SessionRegistry::new());
    registry.configure_arrow_kill_rewards(
        Some(2),
        Some(3),
        Some(77),
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let player = register_test_session(&registry, "HostileCommitAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:skeleton".to_owned(),
        Vec3::new(0.5, 64.0, 6.5),
    );
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let phase = u64::from(entity_id.0.unsigned_abs()) % SKELETON_SHOT_PERIOD_TICKS;
    let due_tick = if phase == 0 {
        SKELETON_SHOT_PERIOD_TICKS
    } else {
        SKELETON_SHOT_PERIOD_TICKS - phase
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HostileCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let commit_registry = Arc::clone(&registry);
    let commit = std::thread::spawn(move || {
        commit_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            mc_world::BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("hostile commit reaches arrow publication boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entities_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release hostile commit");
    let (attacks, _) = commit.join().expect("hostile commit worker");

    assert!(
        session_available,
        "arrow publication must not retain session state"
    );
    assert!(
        entities_available,
        "arrow publication must release the entity store"
    );
    assert_eq!(attacks, 1);
}

#[test]
fn skeleton_volley_uses_constant_owner_requests() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        Some(2),
        Some(3),
        Some(77),
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let player = register_test_session(&registry, "SkeletonVolleyAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:skeleton".to_owned(),
        Vec3::new(0.5, 64.0, 6.5),
    );
    let first_id = registry.persisted_entity_records()[0].snapshot.id;
    for filler in 1..SKELETON_SHOT_PERIOD_TICKS {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:cow".to_owned(),
            Vec3::new(160.5 + filler as f64, 64.0, 0.5),
        );
    }
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:skeleton".to_owned(),
        Vec3::new(2.5, 64.0, 6.5),
    );
    let phase = u64::from(first_id.0.unsigned_abs()) % SKELETON_SHOT_PERIOD_TICKS;
    let due_tick = if phase == 0 {
        SKELETON_SHOT_PERIOD_TICKS
    } else {
        SKELETON_SHOT_PERIOD_TICKS - phase
    };
    registry.entities.reset_owner_requests_for_test();

    let (attacks, _) = registry.tick_hostile_attacks(
        &SimulationAuthority::for_test(),
        due_tick,
        mc_world::BlockStateId(0),
    );

    assert_eq!(attacks, 2);
    assert_eq!(registry.entities.owner_requests_for_test(), 5);
}

#[test]
fn chest_world_commit_does_not_hold_unrelated_session_or_entity_state() {
    let registry = Arc::new(SessionRegistry::new());
    let session_id = register_test_session(&registry, "ChestCommitLocks");
    let position = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    assert_eq!(registry.register_chest_viewer(session_id, position), 1);
    let player_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session_id, Arc::clone(&player_state));
    let player = {
        let state = player_state.lock().expect("player state");
        ContainerPlayerPlan {
            expected_inventory: state.inventory.clone(),
            expected_carried_item: state.carried_item.clone(),
            updated_inventory: state.inventory.clone(),
            updated_carried_item: state.carried_item.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: Vec::new(),
            xp_orb: None,
        }
    };
    let (commit_entered_tx, commit_entered_rx) = std::sync::mpsc::channel();
    let (release_commit_tx, release_commit_rx) = std::sync::mpsc::channel();

    let commit_registry = Arc::clone(&registry);
    let commit_thread = std::thread::spawn(move || {
        commit_registry
            .commit_chest_slots(
                &SimulationAuthority::for_test(),
                ContainerCommitContext {
                    position,
                    expected_state_id: 1,
                    actor_session: session_id,
                    player: &player,
                },
                Vec::new(),
                || {
                    commit_entered_tx.send(()).expect("test receiver remains");
                    let _ = release_commit_rx.recv();
                    Ok::<_, ()>(())
                },
            )
            .expect("chest commit succeeds");
    });
    commit_entered_rx
        .recv()
        .expect("chest commit reaches world mutation");

    let time_registry = Arc::clone(&registry);
    let (time_changed_tx, time_changed_rx) = std::sync::mpsc::channel();
    let time_thread = std::thread::spawn(move || {
        time_registry.set_world_time(84);
        let _ = time_changed_tx.send(());
    });
    let entity_registry = Arc::clone(&registry);
    let (entity_read_tx, entity_read_rx) = std::sync::mpsc::channel();
    let entity_thread = std::thread::spawn(move || {
        let entities = entity_registry.lock_entities("test independent entity read");
        let count = entities.len();
        drop(entities);
        let _ = entity_read_tx.send(count);
    });

    let time_changed = time_changed_rx.recv_timeout(Duration::from_secs(1));
    let entity_read = entity_read_rx.recv_timeout(Duration::from_secs(1));
    let _ = release_commit_tx.send(());
    commit_thread.join().expect("commit thread succeeds");
    time_thread.join().expect("time thread succeeds");
    entity_thread.join().expect("entity thread succeeds");

    time_changed.expect("chest world mutation must not block unrelated session state");
    assert_eq!(
        entity_read.expect("chest world mutation must not block entity reads"),
        0
    );
    assert_eq!(registry.world_time(), 84);
}

#[test]
fn furnace_world_commit_does_not_hold_unrelated_session_or_entity_state() {
    let registry = Arc::new(SessionRegistry::new());
    let session_id = register_test_session(&registry, "FurnaceCommitLocks");
    let position = mc_world::BlockPos { x: 3, y: 64, z: 3 };
    assert_eq!(registry.register_furnace_viewer(session_id, position), 1);
    let player_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session_id, Arc::clone(&player_state));
    let player = {
        let state = player_state.lock().expect("player state");
        ContainerPlayerPlan {
            expected_inventory: state.inventory.clone(),
            expected_carried_item: state.carried_item.clone(),
            updated_inventory: state.inventory.clone(),
            updated_carried_item: state.carried_item.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: Vec::new(),
            xp_orb: None,
        }
    };
    let (commit_entered_tx, commit_entered_rx) = std::sync::mpsc::channel();
    let (release_commit_tx, release_commit_rx) = std::sync::mpsc::channel();

    let commit_registry = Arc::clone(&registry);
    let commit_thread = std::thread::spawn(move || {
        commit_registry
            .commit_furnace_slots(
                &SimulationAuthority::for_test(),
                ContainerCommitContext {
                    position,
                    expected_state_id: 1,
                    actor_session: session_id,
                    player: &player,
                },
                std::array::from_fn(|_| ItemStack::EMPTY),
                || {
                    commit_entered_tx.send(()).expect("test receiver remains");
                    let _ = release_commit_rx.recv();
                    Ok::<_, ()>(())
                },
            )
            .expect("furnace commit succeeds");
    });
    commit_entered_rx
        .recv()
        .expect("furnace commit reaches world mutation");

    let time_registry = Arc::clone(&registry);
    let (time_changed_tx, time_changed_rx) = std::sync::mpsc::channel();
    let time_thread = std::thread::spawn(move || {
        time_registry.set_world_time(126);
        let _ = time_changed_tx.send(());
    });
    let entity_registry = Arc::clone(&registry);
    let (entity_read_tx, entity_read_rx) = std::sync::mpsc::channel();
    let entity_thread = std::thread::spawn(move || {
        let entities = entity_registry.lock_entities("test independent entity read");
        let count = entities.len();
        drop(entities);
        let _ = entity_read_tx.send(count);
    });

    let time_changed = time_changed_rx.recv_timeout(Duration::from_secs(1));
    let entity_read = entity_read_rx.recv_timeout(Duration::from_secs(1));
    let _ = release_commit_tx.send(());
    commit_thread.join().expect("commit thread succeeds");
    time_thread.join().expect("time thread succeeds");
    entity_thread.join().expect("entity thread succeeds");

    time_changed.expect("furnace world mutation must not block unrelated session state");
    assert_eq!(
        entity_read.expect("furnace world mutation must not block entity reads"),
        0
    );
    assert_eq!(registry.world_time(), 126);
}

#[tokio::test]
async fn simulation_tick_subscription_wakes_on_owner_advance() {
    let registry = SessionRegistry::new();
    let mut ticks = registry.subscribe_simulation_ticks();

    registry.advance_world_time(1);
    ticks
        .changed()
        .await
        .expect("simulation tick sender remains");

    assert_eq!(*ticks.borrow_and_update(), 1);
}

#[test]
fn multiplayer_sleep_quorum_skips_once_and_disconnect_recomputes_waiters() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepAlice");
    let bob = register_test_session(&registry, "SleepBob");
    registry.set_world_time(12_542);

    assert_eq!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting {
            sleeping: 1,
            required: 2,
        }
    );
    assert_eq!(registry.world_time(), 12_542);
    assert_eq!(
        registry.begin_sleep(bob),
        SleepOutcome::Waiting {
            sleeping: 2,
            required: 2,
        }
    );
    registry.advance_world_time(99);
    assert!(
        registry
            .tick_sleep_owned(&SimulationAuthority::for_test())
            .is_empty()
    );
    registry.advance_world_time(1);
    let completed = registry.tick_sleep_owned(&SimulationAuthority::for_test());
    assert_eq!(registry.world_time(), 24_000);
    assert!(completed.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::WorldTime { world_time: 24_000 }
    )));
    assert_eq!(registry.begin_sleep(alice), SleepOutcome::Daytime);

    registry.set_world_time(36_542);
    assert!(matches!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting { .. }
    ));
    registry.advance_world_time(DEEP_SLEEP_TICKS);
    let disconnect_dispatches = registry.unregister(bob);
    assert_eq!(registry.world_time(), 48_000);
    assert_eq!(registry.sleeping_session_count_for_test(), 0);
    assert!(disconnect_dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::WorldTime { world_time: 48_000 }
        )
    }));
    let alice_bed = registry
        .sleeping_bed(alice)
        .expect("quorum completion keeps bed authority until release");
    assert!(disconnect_dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::WakeFromBed { bed } if *bed == alice_bed
        )
    }));
    let token = registry
        .claim_sleep_wake(alice, alice_bed)
        .expect("wake command stages an exact release token");
    let released = registry
        .complete_sleep_wake(token)
        .expect("confirmed bed release completes wake");
    assert!(released.dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::PlayerEntityData { entity_id, values }
            if *entity_id == i32::try_from(alice).unwrap()
                && values.iter().any(|value| matches!(
                    value,
                    EntityDataValue::Pose { pose: EntityPose::Standing, .. }
                ))
    )));

    let _charlie = register_test_session(&registry, "SleepCharlie");
    registry.set_world_time(60_542);
    assert!(matches!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting { .. }
    ));
    let _ = registry.unregister(alice);
    assert_eq!(registry.sleeping_session_count_for_test(), 0);
    assert_eq!(registry.world_time(), 60_542);
}

#[test]
fn multiplayer_sleep_uses_vanilla_percentage_spectators_and_deep_sleep_delay() {
    assert_eq!(sleepers_needed(0, 0), 1);
    assert_eq!(sleepers_needed(3, 34), 2);
    assert_eq!(sleepers_needed(1, 101), 2);

    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepPercentAlice");
    let _bob = register_test_session(&registry, "SleepPercentBob");
    let spectator = register_test_session(&registry, "SleepPercentSpectator");
    let mut spectator_state = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    spectator_state.game_mode = GameMode::Spectator;
    registry.register_player_persistence(spectator, Arc::new(Mutex::new(spectator_state)));
    registry.set_players_sleeping_percentage(50);
    registry.set_world_time(12_542);

    assert_eq!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting {
            sleeping: 1,
            required: 1,
        }
    );
    assert_eq!(registry.world_time(), 12_542);

    registry.advance_world_time(99);
    assert!(
        registry
            .tick_sleep_owned(&SimulationAuthority::for_test())
            .is_empty()
    );
    assert_eq!(registry.world_time(), 12_641);

    registry.advance_world_time(1);
    let dispatches = registry.tick_sleep_owned(&SimulationAuthority::for_test());
    assert_eq!(registry.world_time(), 24_000);
    assert!(dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::WorldTime { world_time: 24_000 }
    )));
    assert!(
        dispatches
            .iter()
            .any(|dispatch| matches!(dispatch.command, OutboundCommand::WakeFromBed { .. }))
    );
    let bed = registry
        .sleeping_bed(alice)
        .expect("sleep completion keeps bed authority until release");
    let token = registry
        .claim_sleep_wake(alice, bed)
        .expect("wake command stages an exact release token");
    let released = registry
        .complete_sleep_wake(token)
        .expect("confirmed bed release completes wake");
    assert!(released.dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::PlayerEntityData { entity_id, values }
            if *entity_id == i32::try_from(alice).unwrap()
                && values.iter().any(|value| matches!(
                    value,
                    EntityDataValue::Pose { pose: EntityPose::Standing, .. }
                ))
    )));
}

#[test]
fn natural_dawn_pushes_sleeping_player_back_to_standing() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepDawnAlice");
    registry.set_world_time(23_999);
    assert!(matches!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting { .. }
    ));

    registry.advance_world_time(1);
    let dispatches = registry.tick_sleep_owned(&SimulationAuthority::for_test());

    assert_eq!(registry.world_time(), 24_000);
    assert_eq!(registry.sleeping_session_count_for_test(), 0);
    let bed = registry
        .sleeping_bed(alice)
        .expect("natural dawn keeps bed authority until release");
    assert!(dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::WakeFromBed { bed: wake_bed } if wake_bed == bed
    )));
    let token = registry
        .claim_sleep_wake(alice, bed)
        .expect("dawn wake stages an exact release token");
    let released = registry
        .complete_sleep_wake(token)
        .expect("confirmed dawn bed release completes wake");
    assert!(released.dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::PlayerEntityData { entity_id, values }
            if *entity_id == i32::try_from(alice).unwrap()
                && values.iter().any(|value| matches!(
                    value,
                    EntityDataValue::Pose { pose: EntityPose::Standing, .. }
                ))
    )));
}

#[test]
fn completed_sleep_pushes_the_bed_to_the_sleeping_connection() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepWakeAlice");
    let bed = mc_world::BlockPos { x: 3, y: 64, z: -2 };
    registry.set_world_time(12_542);

    assert!(matches!(
        registry.begin_sleep_at(alice, bed),
        SleepOutcome::Waiting { .. }
    ));
    registry.advance_world_time(DEEP_SLEEP_TICKS);
    let dispatches = registry.tick_sleep_owned(&SimulationAuthority::for_test());

    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == alice
            && matches!(dispatch.command, OutboundCommand::WakeFromBed { bed: wake_bed } if wake_bed == bed)
    }));
}

#[test]
fn stopping_sleep_returns_the_bed_for_immediate_wake() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepStopAlice");
    let bed = mc_world::BlockPos { x: 3, y: 64, z: -2 };
    registry.set_world_time(12_542);
    assert!(matches!(
        registry.begin_sleep_at(alice, bed),
        SleepOutcome::Waiting { .. }
    ));

    assert_eq!(registry.stop_sleeping(alice), Some(bed));
    assert_eq!(registry.sleeping_session_count_for_test(), 0);
}

#[test]
fn second_player_cannot_reserve_an_occupied_bed() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepOccupiedAlice");
    let bob = register_test_session(&registry, "SleepOccupiedBob");
    let bed = mc_world::BlockPos { x: 3, y: 64, z: -2 };
    registry.set_world_time(12_542);
    assert!(matches!(
        registry.begin_sleep_at(alice, bed),
        SleepOutcome::Waiting { .. }
    ));

    assert_eq!(registry.begin_sleep_at(bob, bed), SleepOutcome::Occupied);
    assert_eq!(registry.sleeping_session_count_for_test(), 1);
}

#[test]
fn spectator_transition_pushes_deep_sleep_quorum_completion() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "SleepModeAlice");
    let bob = register_test_session(&registry, "SleepModeBob");
    for id in [alice, bob] {
        registry.register_player_persistence(
            id,
            Arc::new(Mutex::new(PlayerPersistedState::new_default(
                PlayerPose::new(0.5, 64.0, 0.5),
            ))),
        );
    }
    registry.set_world_time(12_542);
    assert_eq!(
        registry.begin_sleep(alice),
        SleepOutcome::Waiting {
            sleeping: 1,
            required: 2,
        }
    );
    registry.advance_world_time(DEEP_SLEEP_TICKS);

    let dispatches = registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            bob,
            PlayerStateEvent::GameMode(GameMode::Spectator),
        )
        .expect("spectator transition commits");

    assert_eq!(registry.world_time(), 24_000);
    assert_eq!(registry.sleeping_session_count_for_test(), 0);
    assert!(dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::WorldTime { world_time: 24_000 }
    )));
}

#[tokio::test]
async fn active_session_subscription_wakes_on_register_and_unregister() {
    let registry = SessionRegistry::new();
    let mut sessions = registry.subscribe_active_sessions();
    let (tx, _rx) = mpsc::channel(1);

    let (session_id, _) = registry.register(
        &profile("SessionEvents"),
        (0, 0),
        1,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    sessions.changed().await.expect("session sender remains");
    assert_eq!(*sessions.borrow_and_update(), 1);
    assert_eq!(registry.published_active_session_count(), 1);

    registry.unregister(session_id);
    sessions.changed().await.expect("session sender remains");
    assert_eq!(*sessions.borrow_and_update(), 0);
    assert_eq!(registry.published_active_session_count(), 0);
}

#[tokio::test]
async fn outbound_pressure_wait_wakes_on_counter_change() {
    let registry = SessionRegistry::new();
    let observed = registry.pressure_change_generation();
    let mut changed = Box::pin(registry.wait_for_pressure_change(observed));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(changed.as_mut(), cx).is_pending(),
            "pressure wait must stay pending before a producer changes a counter"
        );
        std::task::Poll::Ready(())
    })
    .await;

    registry.record_slow_client_pressure_shed();
    changed.await;
}

#[test]
fn outbound_pressure_producer_does_not_wait_for_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let observed = registry.pressure_change_generation();
    let (registry_locked_tx, registry_locked_rx) = std::sync::mpsc::channel();
    let (release_registry_tx, release_registry_rx) = std::sync::mpsc::channel();

    let locked_registry = Arc::clone(&registry);
    let lock_thread = std::thread::spawn(move || {
        let inner = locked_registry.lock_inner("test blocked session registry");
        registry_locked_tx.send(()).expect("test receiver remains");
        let _ = release_registry_rx.recv();
        drop(inner);
    });
    registry_locked_rx
        .recv()
        .expect("test holds session registry");

    let pressure_registry = Arc::clone(&registry);
    let (pressure_changed_tx, pressure_changed_rx) = std::sync::mpsc::channel();
    let pressure_thread = std::thread::spawn(move || {
        pressure_registry.record_slow_client_pressure_shed();
        let _ = pressure_changed_tx.send(());
    });

    let changed = pressure_changed_rx.recv_timeout(Duration::from_secs(1));
    let _ = release_registry_tx.send(());
    lock_thread.join().expect("registry lock thread succeeds");
    pressure_thread
        .join()
        .expect("pressure producer thread succeeds");

    changed.expect("pressure producer must not wait for unrelated session state");
    assert!(registry.pressure_change_generation() > observed);
}

#[tokio::test]
async fn prepared_chunk_wait_wakes_when_cache_is_published() {
    let registry = SessionRegistry::new();
    let chunk = (0, 0);
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedWait"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let observed = registry.prepared_change_generation();
    let mut changed = Box::pin(registry.wait_for_prepared_change(observed));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(changed.as_mut(), cx).is_pending(),
            "prepared wait must stay pending before a chunk is published"
        );
        std::task::Poll::Ready(())
    })
    .await;

    registry.cache_prepared_chunk(
        chunk,
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"prepared-chunk"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );
    tokio::time::timeout(Duration::from_secs(1), changed)
        .await
        .expect("prepared cache publication must wake its waiters");
}

#[test]
fn prepared_cache_publication_does_not_wait_for_session_state() {
    let registry = Arc::new(SessionRegistry::new());
    let chunk = (0, 0);
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedCachePublication"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"prepared-cache-publication"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    let inner = registry.lock_inner("hold session state during prepared cache publication");
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let publishing_registry = Arc::clone(&registry);
    let publisher = std::thread::spawn(move || {
        let published = publishing_registry.cache_prepared_chunk_if_current(chunk, 0, prepared);
        let _ = published_tx.send(published);
    });

    let published = published_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("prepared cache publication must not wait for session state");
    assert!(published);
    drop(inner);
    publisher.join().expect("prepared cache publisher succeeds");
}

#[test]
fn prewarmed_cache_publication_does_not_wait_for_session_state() {
    let registry = Arc::new(SessionRegistry::new());
    let chunk = (0, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"prewarmed-cache-publication"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    let inner = registry.lock_inner("hold session state during prewarmed cache publication");
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let publishing_registry = Arc::clone(&registry);
    let publisher = std::thread::spawn(move || {
        let published = publishing_registry.cache_prewarmed_chunk(chunk, 0, prepared, 1);
        let _ = published_tx.send(published);
    });

    let published = published_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("prewarmed cache publication must not wait for session state");
    assert!(published);
    drop(inner);
    publisher
        .join()
        .expect("prewarmed cache publisher succeeds");
}

#[test]
fn prepared_chunk_is_released_after_all_current_subscribers_load_it() {
    let registry = SessionRegistry::new();
    let chunk = (0, 0);
    let (alice_tx, _alice_rx) = mpsc::channel(1);
    let (bob_tx, _bob_rx) = mpsc::channel(1);
    let (alice, _) = registry.register(
        &profile("PreparedReleaseAlice"),
        chunk,
        0,
        HashSet::from([chunk]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, _) = registry.register(
        &profile("PreparedReleaseBob"),
        chunk,
        0,
        HashSet::from([chunk]),
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    registry.cache_prepared_chunk(chunk, Arc::clone(&prepared));

    let _ = registry.mark_loaded(alice, chunk);
    assert!(registry.prepared_chunk(chunk).is_some());

    let _ = registry.mark_loaded(bob, chunk);
    assert!(registry.prepared_chunk(chunk).is_none());
    assert!(
        !registry.cache_prepared_chunk_if_current(chunk, 0, prepared),
        "the sender must not reinsert a frame after every current subscriber loaded it"
    );
}

#[test]
fn prewarmed_chunk_remains_available_for_a_trailing_subscriber() {
    let registry = SessionRegistry::new();
    let old_chunk = (0, 0);
    let prewarmed_chunk = (0, 1);
    let (alice_tx, _alice_rx) = mpsc::channel(1);
    let (bob_tx, _bob_rx) = mpsc::channel(1);
    let (alice, _) = registry.register(
        &profile("PrewarmTrailingAlice"),
        old_chunk,
        0,
        HashSet::from([old_chunk]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, _) = registry.register(
        &profile("PrewarmTrailingBob"),
        old_chunk,
        0,
        HashSet::from([old_chunk]),
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    assert!(registry.cache_prewarmed_chunk(prewarmed_chunk, 0, prepared, 64));

    registry.replace_view(alice, prewarmed_chunk, 0, HashSet::from([prewarmed_chunk]));
    let _ = registry.mark_loaded(alice, prewarmed_chunk);
    assert!(
        registry.prepared_chunk(prewarmed_chunk).is_some(),
        "a prewarm-derived frame must survive the leading subscriber"
    );

    registry.replace_view(bob, prewarmed_chunk, 0, HashSet::from([prewarmed_chunk]));
    assert!(matches!(
        registry.prepared_chunk_or_wait_for_earlier_session(prewarmed_chunk, bob),
        SessionPreparedChunkClaimResult::Cached(_, 0)
    ));
    assert_eq!(registry.shed_prepared_chunks(), 1);
    assert!(registry.prepared_chunk(prewarmed_chunk).is_none());
}

#[test]
fn prewarmed_cache_hit_refreshes_lru_recency() {
    let registry = SessionRegistry::new();
    let first = (0, 1);
    let second = (1, 1);
    let third = (2, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });

    assert!(registry.cache_prewarmed_chunk(first, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(second, 0, Arc::clone(&prepared), 2));
    assert!(matches!(
        registry.prepared_chunk_or_claim(first),
        PreparedChunkClaimResult::Cached
    ));

    {
        let cache = registry.lock_prepared_cache("inspect prewarm LRU after cache hit");
        assert_eq!(
            cache.prewarmed_prepared,
            VecDeque::from([second, first]),
            "a cache hit must make the prepared frame most recent"
        );
    }

    assert!(registry.cache_prewarmed_chunk(third, 0, prepared, 2));
    let cache = registry.lock_prepared_cache("inspect prewarm LRU after eviction");
    assert!(cache.prepared.contains_key(&first));
    assert!(!cache.prepared.contains_key(&second));
    assert!(cache.prepared.contains_key(&third));
}

#[test]
fn prewarm_eviction_preserves_active_session_frontier() {
    let registry = SessionRegistry::new();
    let _session = register_test_session(&registry, "PrewarmFrontier");
    let current_side_edge = (3, 0);
    let stale = (100, 100);
    let another_current_edge = (3, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });

    assert!(registry.cache_prewarmed_chunk(current_side_edge, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(stale, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(another_current_edge, 0, prepared, 2));

    let cache = registry.lock_prepared_cache("inspect active prewarm frontier eviction");
    assert!(cache.prepared.contains_key(&current_side_edge));
    assert!(!cache.prepared.contains_key(&stale));
    assert!(cache.prepared.contains_key(&another_current_edge));
}

fn test_pressure(registry: &SessionRegistry) -> Arc<OutboundPressureMetrics> {
    Arc::clone(&registry.outbound_pressure)
}

fn test_recipient(
    registry: &SessionRegistry,
    id: SessionId,
    tx: mpsc::Sender<OutboundCommand>,
) -> SessionRecipient {
    let pressure = test_pressure(registry);
    SessionRecipient::unordered(id, tx, pressure)
}

fn publish_single_entity_spawn(
    dispatches: Vec<VisibilityDispatch>,
    outbound: &mut mpsc::Receiver<OutboundCommand>,
) -> EntityId {
    let entity_id = match dispatches.as_slice() {
        [
            VisibilityDispatch {
                command: OutboundCommand::SpawnEntity(entity),
                ..
            },
        ] => entity.id,
        other => panic!("expected one entity spawn dispatch, got {other:?}"),
    };
    dispatch_visibility_commands(dispatches);
    match outbound.try_recv() {
        Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == entity_id => entity_id,
        other => panic!("expected published entity spawn, got {other:?}"),
    }
}

fn server_entity_health(registry: &SessionRegistry, entity_id: EntityId) -> f32 {
    registry
        .lock_entities("read authoritative entity health")
        .snapshot(entity_id)
        .expect("entity remains authoritative")
        .health
}

fn profile(name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_string(),
    }
}

fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
    let (tx, _rx) = mpsc::channel(8);
    let (id, _) = registry.register(
        &profile(name),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    id
}

#[test]
fn player_entity_data_publication_distinguishes_observers_and_self() {
    let registry = SessionRegistry::new();
    let target = register_test_session(&registry, "StateTarget");
    let observer = register_test_session(&registry, "StateObserver");
    let hidden = register_test_session(&registry, "StateHidden");
    {
        let mut inner = registry.lock_inner("install player state publication visibility");
        inner
            .sessions
            .get_mut(&observer)
            .expect("observer session")
            .visible_players
            .insert(target);
    }
    let values = vec![EntityDataValue::Pose {
        index: ENTITY_DATA_POSE_INDEX,
        pose: EntityPose::Standing,
    }];

    let observer_only = registry.broadcast_player_entity_data(target, values.clone());
    assert_eq!(
        observer_only
            .iter()
            .map(|dispatch| dispatch.recipient.id)
            .collect::<HashSet<_>>(),
        HashSet::from([observer])
    );

    let including_self = registry.broadcast_player_entity_data_including_self(target, values);
    assert_eq!(including_self.len(), 2);
    assert_eq!(
        including_self
            .iter()
            .map(|dispatch| dispatch.recipient.id)
            .collect::<HashSet<_>>(),
        HashSet::from([target, observer])
    );
    assert!(
        including_self
            .iter()
            .all(|dispatch| dispatch.recipient.id != hidden)
    );
}

#[test]
fn outbound_publication_targets_exact_sessions_and_debug_count() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "OutboundAlice");
    let bob = register_test_session(&registry, "OutboundBob");
    let carol = register_test_session(&registry, "OutboundCarol");

    let chat = registry.broadcast_system_chat("hello".to_string());
    assert_eq!(chat.len(), 3);
    assert_eq!(
        chat.iter()
            .map(|dispatch| dispatch.recipient.id)
            .collect::<HashSet<_>>(),
        HashSet::from([alice, bob, carol])
    );
    assert!(chat.iter().all(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::SystemChat { message } if message == "hello"
        )
    }));

    let debug = registry.debug_outbound_pressure_dispatches(bob, 3);
    assert_eq!(debug.len(), 3);
    assert!(debug.iter().all(|dispatch| dispatch.recipient.id == bob));
    assert!(
        registry
            .debug_outbound_pressure_dispatches(u64::MAX, 3)
            .is_empty()
    );

    let (script_tx, mut script_rx) = mpsc::channel(8);
    let (script, _) = registry.register(
        &profile("OutboundScript"),
        (0, 0),
        2,
        HashSet::new(),
        script_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.send_script_system_chat(script, "direct".to_string()));
    assert!(matches!(
        script_rx.try_recv(),
        Ok(OutboundCommand::SystemChat { message }) if message == "direct"
    ));

    assert!(!registry.send_script_system_chat(u64::MAX, "missing".to_string()));
    assert!(!registry.disconnect_player(u64::MAX, "missing".to_string()));
    assert!(!registry.send_custom_payload(
        u64::MAX,
        Identifier::parse("solaris:missing").expect("static identifier"),
        Vec::new(),
    ));
}

#[test]
fn loaded_chunk_index_tracks_shared_load_unload_and_disconnect() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "LoadedIndexAlice");
    let bob = register_test_session(&registry, "LoadedIndexBob");
    let shared = (3, -2);
    let alice_only = (4, -2);

    registry.mark_loaded(alice, shared);
    registry.mark_loaded(alice, alice_only);
    registry.mark_loaded(bob, shared);
    registry.mark_loaded(bob, shared);
    {
        let inner = registry.lock_inner("inspect loaded chunk index");
        assert_eq!(inner.loaded_chunk_refcounts.get(&shared), Some(&2));
        assert_eq!(inner.loaded_chunk_refcounts.get(&alice_only), Some(&1));
    }

    registry.mark_unloaded(alice, &[shared]);
    registry.mark_unloaded(alice, &[shared]);
    {
        let inner = registry.lock_inner("inspect loaded chunk index after unload");
        assert_eq!(inner.loaded_chunk_refcounts.get(&shared), Some(&1));
    }

    registry.unregister(bob);
    assert_eq!(registry.loaded_chunks_sorted(), vec![alice_only]);
    registry.unregister(alice);
    assert!(registry.loaded_chunks_sorted().is_empty());
}

#[test]
fn prepared_chunk_claim_blocks_duplicate_until_released() {
    let registry = SessionRegistry::new();
    let chunk = (2, -3);

    let first_claim = match registry.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected first claim, got {other:?}"),
    };

    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));

    assert!(!registry.release_prepared_chunk_claim(
        chunk,
        PreparedChunkClaim {
            id: first_claim.id.wrapping_add(1),
            revision: first_claim.revision,
        }
    ));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));

    assert!(registry.release_prepared_chunk_claim(chunk, first_claim));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::Claimed(_)
    ));
}

#[test]
fn prepared_chunk_invalidation_revokes_inflight_publication() {
    let registry = SessionRegistry::new();
    let chunk = (2, -3);
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("PreparedInvalidation"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(32.5, 64.0, -47.5),
    );
    let claim = match registry.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected first claim, got {other:?}"),
    };
    let observed = registry.prepared_change_generation();

    registry.invalidate_prepared_chunks(&HashSet::from([chunk]));

    assert!(registry.prepared_change_generation() > observed);
    assert!(!registry.cache_prepared_chunk_if_current(
        chunk,
        claim.revision,
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"stale-prepared-chunk"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    ));
    assert!(registry.prepared_chunk(chunk).is_none());
    assert!(
        registry
            .mark_loaded_if_prepared_revision_current(session_id, chunk, claim.revision)
            .is_none()
    );
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));
    assert!(registry.release_prepared_chunk_claim(chunk, claim));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::Claimed(_)
    ));
}

#[test]
fn invalidating_unique_prewarmed_chunks_does_not_retain_revisions() {
    let registry = SessionRegistry::new();
    let chunks = (0..64).map(|index| (index, -index)).collect::<HashSet<_>>();
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"unique-prewarmed-chunk"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    for &chunk in &chunks {
        assert!(registry.cache_prewarmed_chunk(chunk, 0, Arc::clone(&prepared), chunks.len(),));
    }

    registry.invalidate_prepared_chunks(&chunks);

    let cache = registry.lock_prepared_cache("inspect invalidated prewarm revisions");
    assert!(cache.prewarmed_prepared.is_empty());
    assert!(cache.prepared.is_empty());
    assert_eq!(
        cache.prepared_revisions.len(),
        0,
        "unticketed, non-in-flight invalidated prewarm revisions must remain bounded"
    );
}

#[test]
fn memory_pressure_shed_releases_shared_prepared_frames() {
    let registry = SessionRegistry::new();
    let chunks = [(2, -3), (3, -3)];
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedMemoryShed"),
        chunks[0],
        1,
        HashSet::from(chunks),
        tx,
        PlayerPose::new(32.5, 64.0, -47.5),
    );
    let frames = chunks.map(|chunk| {
        let frame = Arc::new(PreparedChunkFrame {
            frame: Bytes::from(vec![u8::try_from(chunk.0).unwrap(); 4_096]),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 4_096,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        });
        registry.cache_prepared_chunk(chunk, Arc::clone(&frame));
        frame
    });
    assert_eq!(registry.pressure_snapshot().prepared_chunks, 2);
    assert!(frames.iter().all(|frame| Arc::strong_count(frame) == 2));
    let observed = registry.prepared_change_generation();

    assert_eq!(registry.shed_prepared_chunks(), 2);

    assert_eq!(registry.pressure_snapshot().prepared_chunks, 0);
    assert!(frames.iter().all(|frame| Arc::strong_count(frame) == 1));
    assert!(registry.prepared_change_generation() > observed);
}

#[test]
fn chest_slot_dispatches_claim_expected_state_exactly_once() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "ChestStateAlice");
    let bob = register_test_session(&registry, "ChestStateBob");
    let position = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let stack = ItemStack::new(10, 1);

    assert_eq!(registry.register_chest_viewer(alice, position), 1);
    assert_eq!(registry.register_chest_viewer(bob, position), 1);

    let (state_id, dispatches) = registry
        .try_chest_slot_dispatches(position, 1, alice, vec![stack.clone()])
        .expect("first mutation claims state 1");

    assert_eq!(state_id, 2);
    assert_eq!(registry.chest_state_id(position), 2);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].recipient.id, bob);
    match &dispatches[0].command {
        OutboundCommand::ChestSlots {
            position: command_position,
            state_id: command_state_id,
            slots,
        } => {
            assert_eq!(*command_position, position);
            assert_eq!(*command_state_id, 2);
            assert_eq!(slots.as_slice(), &[stack]);
        }
        other => panic!("expected chest slots dispatch, got {other:?}"),
    }

    let conflict = registry
        .try_chest_slot_dispatches(position, 1, bob, vec![ItemStack::new(11, 1)])
        .expect_err("state 1 cannot be claimed twice");
    assert_eq!(conflict, 2);
    assert_eq!(registry.chest_state_id(position), 2);

    let charlie = register_test_session(&registry, "ChestStateCharlie");
    assert_eq!(registry.register_chest_viewer(charlie, position), 2);
    registry.unregister_chest_viewer(alice, position);
    registry.unregister_chest_viewer(bob, position);
    registry.unregister_chest_viewer(charlie, position);
    assert_eq!(registry.chest_state_id(position), 1);
}

#[test]
fn server_chest_slot_dispatches_do_not_allocate_state_without_viewers() {
    let registry = SessionRegistry::new();
    let position = mc_world::BlockPos { x: 7, y: 64, z: 7 };

    let (state_id, dispatches) =
        registry.server_chest_slot_dispatches(position, vec![ItemStack::new(10, 1)]);

    assert_eq!(state_id, 1);
    assert!(dispatches.is_empty());
    assert_eq!(registry.chest_state_id(position), 1);
}

#[test]
fn furnace_slot_dispatches_claim_expected_state_exactly_once() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "FurnaceStateAlice");
    let bob = register_test_session(&registry, "FurnaceStateBob");
    let position = mc_world::BlockPos { x: 5, y: 64, z: 5 };
    let slots = [ItemStack::new(10, 1), ItemStack::EMPTY, ItemStack::EMPTY];

    assert_eq!(registry.register_furnace_viewer(alice, position), 1);
    assert_eq!(registry.register_furnace_viewer(bob, position), 1);

    let (state_id, dispatches) = registry
        .try_furnace_slot_dispatches(position, 1, alice, slots.clone())
        .expect("first mutation claims state 1");

    assert_eq!(state_id, 2);
    assert_eq!(registry.furnace_state_id(position), 2);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].recipient.id, bob);
    match &dispatches[0].command {
        OutboundCommand::FurnaceSlots {
            position: command_position,
            state_id: command_state_id,
            slots: command_slots,
        } => {
            assert_eq!(*command_position, position);
            assert_eq!(*command_state_id, 2);
            assert_eq!(command_slots, &slots);
        }
        other => panic!("expected furnace slots dispatch, got {other:?}"),
    }

    let conflict = registry
        .try_furnace_slot_dispatches(
            position,
            1,
            bob,
            [ItemStack::new(11, 1), ItemStack::EMPTY, ItemStack::EMPTY],
        )
        .expect_err("state 1 cannot be claimed twice");
    assert_eq!(conflict, 2);
    assert_eq!(registry.furnace_state_id(position), 2);

    let charlie = register_test_session(&registry, "FurnaceStateCharlie");
    assert_eq!(registry.register_furnace_viewer(charlie, position), 2);
    registry.unregister_furnace_viewer(alice, position);
    registry.unregister_furnace_viewer(bob, position);
    registry.unregister_furnace_viewer(charlie, position);
    assert_eq!(registry.furnace_state_id(position), 1);
}

#[test]
fn server_furnace_slot_dispatches_do_not_allocate_state_without_viewers() {
    let registry = SessionRegistry::new();
    let position = mc_world::BlockPos { x: 7, y: 64, z: 7 };

    let (state_id, dispatches) = registry.server_furnace_slot_dispatches(
        position,
        [ItemStack::new(10, 1), ItemStack::EMPTY, ItemStack::EMPTY],
    );

    assert_eq!(state_id, 1);
    assert!(dispatches.is_empty());
    assert_eq!(registry.furnace_state_id(position), 1);
}

#[test]
fn hostile_forgets_target_when_last_player_unregisters() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "TargetAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(4.5, 64.0, 0.5),
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    assert_eq!(queries.len(), 1);
    {
        let entities = registry.lock_entities("test entity access");
        let entity = entities.snapshots().next().expect("spawned hostile");
        assert!(matches!(entity.goal, GoalState::FollowPosition { .. }));
    }

    registry.unregister(alice);
    assert!(
        registry
            .tick_entities_and_collect_physics_queries(2)
            .is_empty()
    );

    let entities = registry.lock_entities("test entity access");
    let entity = entities.snapshots().next().expect("spawned hostile");
    assert!(matches!(entity.goal, GoalState::Wander { .. }));
}

#[test]
fn hostile_forgets_dead_player_target_during_goal_tick() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "DeadGoalTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(4.5, 64.0, 0.5),
    );

    assert_eq!(
        registry.tick_entities_and_collect_physics_queries(1).len(),
        1
    );
    registry
        .lock_inner("mark goal target dead")
        .dead_sessions
        .insert(player);
    assert!(
        registry
            .tick_entities_and_collect_physics_queries(2)
            .is_empty()
    );

    let entities = registry.lock_entities("test dead goal target");
    let entity = entities.snapshots().next().expect("spawned hostile");
    assert!(matches!(entity.goal, GoalState::Wander { .. }));
}

#[test]
fn fish_physics_queries_use_aquatic_water_rules() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "FishPhysicsAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cod".to_string(),
        Vec3::new(4.5, 62.0, 0.5),
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].kind, EntityPhysicsKind::AquaticLiving);
}

#[test]
fn all_living_aquatic_and_amphibious_classes_use_aquatic_water_rules() {
    for type_name in [
        "minecraft:guardian",
        "minecraft:elder_guardian",
        "minecraft:tadpole",
        "minecraft:nautilus",
        "minecraft:zombie_nautilus",
        "minecraft:axolotl",
        "minecraft:frog",
        "minecraft:turtle",
    ] {
        assert!(
            super::entity_physics_class::entity_type_uses_aquatic_physics(type_name),
            "{type_name}"
        );
    }
    assert!(!super::entity_physics_class::entity_type_uses_aquatic_physics("minecraft:zombie"));
}

#[test]
fn empty_entity_goal_tick_skips_regional_owner_request() {
    let registry = SessionRegistry::new();
    registry.entities.reset_owner_requests_for_test();

    assert!(
        registry
            .tick_entities_and_collect_physics_queries(1)
            .is_empty()
    );
    assert_eq!(registry.entities.owner_requests_for_test(), 0);
}

#[tokio::test]
async fn last_session_unregister_pushes_empty_event() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "EmptyEventAlice");
    let bob = register_test_session(&registry, "EmptyEventBob");
    let observed = registry.session_empty_generation();
    let mut became_empty = Box::pin(registry.wait_for_session_empty(observed));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(became_empty.as_mut(), cx).is_pending(),
            "empty event must wait while sessions remain"
        );
        std::task::Poll::Ready(())
    })
    .await;

    registry.unregister(alice);
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(became_empty.as_mut(), cx).is_pending(),
            "removing one of two sessions must not publish an empty event"
        );
        std::task::Poll::Ready(())
    })
    .await;

    registry.unregister(bob);
    became_empty.await;
    assert_eq!(registry.session_empty_generation(), observed + 1);
}

#[test]
fn melee_hostile_stops_without_rewriting_unchanged_idle_goal() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "CloseTargetAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    registry.entities.reset_owner_requests_for_test();

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(registry.entities.owner_requests_for_test(), 3);
    assert_eq!(queries[0].velocity.x, 0.0);
    assert_eq!(queries[0].velocity.z, 0.0);
    let entities = registry.lock_entities("test entity access");
    let entity = entities.snapshots().next().expect("spawned hostile");
    assert_eq!(entity.goal, GoalState::Idle);
}

#[test]
fn entity_tick_checkpoints_transient_motion_without_per_tick_journal() {
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(CountingEntityJournal(Arc::clone(&commits))),
    );
    let alice = register_test_session(&registry, "EntityTickJournalAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    commits.store(0, Ordering::Relaxed);

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(
        commits.load(Ordering::Relaxed),
        0,
        "goal preparation is not published and must wait for final physics durability"
    );
    let steps = queries
        .iter()
        .map(|query| EntityPhysicsStep {
            id: query.id,
            position: query.position,
            velocity: query.velocity,
            on_ground: query.on_ground,
            horizontal_collision: false,
        })
        .collect::<Vec<_>>();
    registry.apply_entity_physics_if_current_and_dispatch(1, &queries, &steps);

    assert_eq!(commits.load(Ordering::Relaxed), 0);
    let saved = registry.persisted_entity_records();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].snapshot.velocity, queries[0].velocity);
}

#[test]
fn living_physics_rotation_follows_collision_resolved_velocity() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("LivingRotationAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    dispatch_visibility_commands(spawn_dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    let cow_id = registry.persisted_entity_records()[0].id;
    assert!(registry.lock_entities("test entity access").set_goal(
        cow_id,
        GoalState::FollowPosition {
            target: Vec3::new(0.5, 64.0, 10.5),
            speed: 0.2,
        },
    ));
    let queries = registry.tick_entities_and_collect_physics_queries(1);
    assert_eq!(queries.len(), 1);
    assert!(queries[0].velocity.z > 0.0);
    let resolved_velocity = Vec3::new(0.125, 0.0, 0.0);
    let steps = [EntityPhysicsStep {
        id: cow_id,
        position: queries[0].position,
        velocity: resolved_velocity,
        on_ground: queries[0].on_ground,
        horizontal_collision: true,
    }];

    registry.apply_entity_physics_if_current_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &queries,
        &steps,
    );

    let saved = registry.persisted_entity_records();
    assert_eq!(saved[0].snapshot.velocity, resolved_velocity);
    assert_eq!(saved[0].snapshot.rotation.yaw, -90.0);
    assert_eq!(saved[0].snapshot.rotation.head_yaw, -90.0);
    let Ok(OutboundCommand::MoveEntityRelative(movement)) = rx.try_recv() else {
        panic!("expected living movement publication");
    };
    assert_eq!(movement.rotation.yaw, -90.0);
    assert_eq!(movement.rotation.head_yaw, -90.0);
}

#[test]
fn hostile_target_refresh_waits_for_entity_save_barrier() {
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(CountingEntityJournal(Arc::clone(&commits))),
    );
    let alice = register_test_session(&registry, "HostileTargetJournalAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(4.5, 64.0, 0.5),
    );
    commits.store(0, Ordering::Relaxed);

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    let entities = registry.lock_entities("test entity access");
    let zombie = entities.snapshots().next().expect("spawned zombie");
    assert!(matches!(zombie.goal, GoalState::FollowPosition { .. }));
    assert_eq!(
        commits.load(Ordering::Relaxed),
        0,
        "autonomous target refresh is recovered by the next tick and periodic save"
    );
}

#[test]
fn checkpoint_only_item_drop_waits_for_entity_save_barrier() {
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(CountingEntityJournal(Arc::clone(&commits))),
    );

    registry.spawn_item_drop_checkpoint_only_owned(
        &SimulationAuthority::for_test(),
        2,
        Vec3::new(0.5, 64.5, 0.5),
        EntityItemStack::new(10, 1),
    );

    assert_eq!(commits.load(Ordering::Relaxed), 0);
    let saved = registry.persisted_entity_records();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].snapshot.item_stack,
        Some(EntityItemStack::new(10, 1))
    );
}

#[test]
fn unloaded_entities_do_not_run_goal_ticks() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "ActiveChunkAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(8.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(160.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(320.5, 64.0, 0.5),
    );

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(
        registry
            .active_entity_selection_visits
            .load(Ordering::Relaxed),
        1,
        "initial active selection should visit only entities indexed in loaded chunks"
    );
    assert_ne!(queries[0].velocity, Vec3::ZERO);
    let entities = registry.lock_entities("test entity access");
    let far = entities
        .snapshots()
        .find(|entity| entity.position.x > 100.0)
        .expect("far zombie");
    assert_eq!(far.velocity, Vec3::ZERO);
}

#[test]
fn loaded_chunk_pathing_probe_accepts_loaded_world_height_position() {
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::new();
    let entity_aabbs = HashMap::new();
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        None,
    );

    assert_eq!(
        probe.can_entity_stand_at(
            EntityId(1),
            Vec3::new(0.5, f64::from(mc_world::chunk::MIN_Y), 0.5),
        ),
        PathingProbeResult::Walkable
    );
}

#[test]
fn loaded_chunk_pathing_probe_blocks_out_of_world_height_position() {
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::new();
    let entity_aabbs = HashMap::new();
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        None,
    );

    assert_eq!(
        probe.can_entity_stand_at(
            EntityId(1),
            Vec3::new(0.5, f64::from(mc_world::chunk::MIN_Y) - 0.1, 0.5,),
        ),
        PathingProbeResult::Blocked
    );
    assert_eq!(
        probe.can_entity_stand_at(
            EntityId(1),
            Vec3::new(0.5, f64::from(mc_world::chunk::MAX_Y), 0.5),
        ),
        PathingProbeResult::Blocked
    );
}

#[test]
fn loaded_chunk_pathing_probe_reports_unloaded_chunk_in_world_height() {
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::new();
    let entity_aabbs = HashMap::new();
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        None,
    );

    assert_eq!(
        probe.can_entity_stand_at(EntityId(1), Vec3::new(16.5, 64.0, 0.5)),
        PathingProbeResult::Unloaded
    );
}

#[test]
fn terrain_snapshot_chunks_cover_only_terrain_probe_footprints() {
    let terrain_entity = EntityId(1);
    let active_chunks = (-8..=8)
        .flat_map(|x| (-8..=8).map(move |z| (x, z)))
        .collect::<HashSet<_>>();
    let terrain_pathing_entities = HashSet::from([terrain_entity]);
    let entity_aabbs = HashMap::from([
        (terrain_entity, mc_physics::Aabb::COW),
        (EntityId(2), mc_physics::Aabb::COW),
    ]);

    let chunks = terrain_snapshot_chunks_for_probe_positions(
        [
            (terrain_entity, Vec3::new(16.1, 64.0, 0.5)),
            (EntityId(2), Vec3::new(96.5, 64.0, 96.5)),
        ],
        &terrain_pathing_entities,
        &entity_aabbs,
        &active_chunks,
    )
    .into_iter()
    .map(|chunk| (chunk.x, chunk.z))
    .collect::<HashSet<_>>();

    assert_eq!(chunks, HashSet::from([(0, 0), (1, 0)]));
}

fn two_block_wall_pathing_world() -> (mc_world::WorldReadView, mc_physics::BlockMaterialIds) {
    use std::collections::BTreeMap;

    let block = |name: &str, id: u32| mc_data::blocks::BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    let registry = Arc::new(
        BlockRegistry::from_report(&[block("minecraft:air", 0), block("minecraft:stone", 1)])
            .unwrap(),
    );
    let mut world = mc_world::WorldStorage::in_memory(registry);
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
    let _ = chunk.set_block(2, 64, 0, BlockStateId(1));
    let _ = chunk.set_block(2, 65, 0, BlockStateId(1));
    world.insert_generated_chunk(chunk_pos, chunk).unwrap();
    (
        world.read_view(),
        mc_physics::BlockMaterialIds::new(0, None, None),
    )
}

fn vanilla_block_state_id(block_name: &str, properties: &[(&str, &str)]) -> u32 {
    let blocks = mc_data::blocks::solaris_required_blocks_report();
    let block = blocks
        .iter()
        .find(|block| block.id.as_str() == block_name)
        .unwrap_or_else(|| panic!("missing vanilla block {block_name}"));
    block
        .states
        .iter()
        .find(|state| {
            state.properties.len() == properties.len()
                && properties.iter().all(|(name, value)| {
                    state
                        .properties
                        .get(*name)
                        .is_some_and(|actual| actual == value)
                })
        })
        .map(|state| state.id)
        .unwrap_or_else(|| panic!("missing vanilla state for {block_name}"))
}

fn vanilla_collision_pathing_world(
    blocks: &[(i32, i32, i32, u32)],
) -> (mc_world::WorldReadView, mc_physics::BlockMaterialIds) {
    let air = vanilla_block_state_id("minecraft:air", &[]);
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded vanilla blocks build a registry"),
    );
    let mut world = mc_world::WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        chunk_pos,
        BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for &(x, y, z, state) in blocks {
        let _ = chunk.set_block(x as u8, y, z as u8, BlockStateId(state));
    }
    world.insert_generated_chunk(chunk_pos, chunk).unwrap();
    (
        world.read_view(),
        mc_physics::BlockMaterialIds::new(air, None, None),
    )
}

fn with_terrain_pathing_probe(
    world_read: &mc_world::WorldReadView,
    materials: &mc_physics::BlockMaterialIds,
    test: impl FnOnce(&LoadedChunkPathingProbe<'_>, EntityId),
) {
    let entity_id = EntityId(1);
    let active_chunks = HashSet::from([(0, 0)]);
    let terrain_pathing_entities = HashSet::from([entity_id]);
    let entity_aabbs = HashMap::from([(entity_id, mc_physics::Aabb::COW)]);
    let snapshot = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        Some(LoadedTerrainPathingProbe::new(&snapshot, materials)),
    );
    test(&probe, entity_id);
}

#[test]
fn loaded_chunk_pathing_probe_allows_moving_beside_an_isolated_fence_post() {
    let fence = vanilla_block_state_id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    let mut blocks = Vec::new();
    for x in 0..=2 {
        for z in 0..=2 {
            blocks.push((x, 63, z, vanilla_block_state_id("minecraft:stone", &[])));
        }
    }
    blocks.push((1, 64, 1, fence));
    let (world_read, materials) = vanilla_collision_pathing_world(&blocks);
    with_terrain_pathing_probe(&world_read, &materials, |probe, entity_id| {
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(0.8, 64.0, 0.5)),
            PathingProbeResult::Walkable
        );
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(1.8, 64.0, 0.5)),
            PathingProbeResult::Walkable
        );
    });
}

#[test]
fn loaded_chunk_pathing_probe_blocks_an_isolated_fence_post_and_its_overheight_section() {
    let fence = vanilla_block_state_id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    let stone = vanilla_block_state_id("minecraft:stone", &[]);
    let mut blocks = Vec::new();
    for x in 0..=2 {
        for z in 0..=2 {
            blocks.push((x, 63, z, stone));
        }
    }
    blocks.push((1, 64, 1, fence));
    let (world_read, materials) = vanilla_collision_pathing_world(&blocks);
    with_terrain_pathing_probe(&world_read, &materials, |probe, entity_id| {
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(1.5, 64.0, 1.5)),
            PathingProbeResult::Blocked
        );
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(1.5, 65.0, 1.5)),
            PathingProbeResult::Blocked
        );
    });
}

#[test]
fn loaded_chunk_pathing_probe_supports_an_exact_fence_top_contact() {
    let fence = vanilla_block_state_id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    let (world_read, materials) = vanilla_collision_pathing_world(&[(1, 64, 1, fence)]);
    with_terrain_pathing_probe(&world_read, &materials, |probe, entity_id| {
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(1.5, 65.5, 1.5)),
            PathingProbeResult::Walkable
        );
    });
}

#[test]
fn loaded_chunk_pathing_probe_keeps_bottom_layer_fluid_walkable() {
    let water = vanilla_block_state_id("minecraft:water", &[("level", "0")]);
    let (world_read, materials) =
        vanilla_collision_pathing_world(&[(1, mc_world::chunk::MIN_Y, 1, water)]);
    let materials = materials.with_water_states(vec![water]);
    with_terrain_pathing_probe(&world_read, &materials, |probe, entity_id| {
        assert_eq!(
            probe.can_entity_stand_at(
                entity_id,
                Vec3::new(1.5, f64::from(mc_world::chunk::MIN_Y), 1.5),
            ),
            PathingProbeResult::Walkable
        );
    });
}

#[test]
fn loaded_chunk_pathing_probe_supports_bottom_slab_and_directional_stair_tops() {
    let slab = vanilla_block_state_id(
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let stair = vanilla_block_state_id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    let (world_read, materials) =
        vanilla_collision_pathing_world(&[(1, 63, 1, slab), (3, 63, 1, stair)]);
    with_terrain_pathing_probe(&world_read, &materials, |probe, entity_id| {
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(1.5, 63.5, 1.5)),
            PathingProbeResult::Walkable
        );
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(3.5, 63.5, 1.96)),
            PathingProbeResult::Walkable
        );
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(3.5, 64.0, 1.45)),
            PathingProbeResult::Walkable
        );
        assert_eq!(
            probe.can_entity_stand_at(entity_id, Vec3::new(3.5, 64.0, 1.97)),
            PathingProbeResult::Blocked
        );
    });
}

#[test]
fn loaded_chunk_pathing_probe_reads_published_terrain_collision_and_support() {
    let (world_read, materials) = two_block_wall_pathing_world();
    let snapshot = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let active_chunks = HashSet::from([(0, 0)]);
    let entity_id = EntityId(1);
    let terrain_pathing_entities = HashSet::from([entity_id]);
    let entity_aabbs = HashMap::from([(entity_id, mc_physics::Aabb::COW)]);
    let probe = LoadedChunkPathingProbe::new(
        &active_chunks,
        &terrain_pathing_entities,
        &entity_aabbs,
        Some(LoadedTerrainPathingProbe::new(&snapshot, &materials)),
    );

    assert_eq!(
        probe.can_entity_stand_at(entity_id, Vec3::new(2.5, 64.0, 0.5)),
        PathingProbeResult::Blocked
    );
    assert_eq!(
        probe.can_entity_stand_at(entity_id, Vec3::new(1.5, 64.0, 1.5)),
        PathingProbeResult::Walkable
    );
}

#[test]
fn regional_workers_share_autoscaler_cpu_admission() {
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 4);
    let permits = acquire_regional_worker_permits(&resources, 8);
    assert_eq!(permits.len(), 3);
    drop(permits);

    assert_eq!(
        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false),
        2
    );
    let busy = resources
        .try_acquire_cpu()
        .expect("reserve shared CPU slot");
    let permits = acquire_regional_worker_permits(&resources, 8);
    assert_eq!(permits.len(), 1);
    drop(permits);
    drop(busy);

    assert_eq!(
        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false),
        1
    );
    assert!(acquire_regional_worker_permits(&resources, 8).is_empty());
}

#[test]
fn entity_tick_detours_around_published_two_block_wall() {
    let (world_read, materials) = two_block_wall_pathing_world();
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("TerrainPathTarget"),
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
        "minecraft:zombie".to_string(),
        Vec3::new(1.75, 64.0, 0.5),
    );

    let direct_queries =
        registry.tick_entities_and_collect_physics_queries_with_terrain(1, &world_read, &materials);
    let direct = direct_queries
        .iter()
        .find(|query| query.position == Vec3::new(1.75, 64.0, 0.5))
        .expect("zombie physics query");
    let zombie_id = direct.id;
    assert!(direct.velocity.x > 0.0);
    assert_eq!(direct.velocity.z, 0.0);
    assert_eq!(registry.terrain_pathing_entity_count(), 0);

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: zombie_id,
            position: direct.position,
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: true,
        }],
    );
    assert_eq!(registry.terrain_pathing_entity_count(), 1);

    let detour_queries =
        registry.tick_entities_and_collect_physics_queries_with_terrain(2, &world_read, &materials);
    let zombie = detour_queries
        .iter()
        .find(|query| query.id == zombie_id)
        .expect("detouring zombie physics query");

    assert_ne!(zombie.velocity, Vec3::ZERO);
    assert!(zombie.velocity.x < 0.0);

    registry.apply_entity_physics_and_dispatch(
        2,
        &[EntityPhysicsStep {
            id: zombie_id,
            position: Vec3::new(1.75, 64.0, 2.0),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert_eq!(registry.terrain_pathing_entity_count(), 1);

    let clear_queries =
        registry.tick_entities_and_collect_physics_queries_with_terrain(3, &world_read, &materials);
    assert!(clear_queries.iter().any(|query| query.id == zombie_id));
    assert_eq!(registry.terrain_pathing_entity_count(), 0);
}

#[test]
fn climb_jump_collision_does_not_start_wall_detour() {
    let registry = SessionRegistry::new();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = registry.persisted_entity_records()[0].id;

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.42, 0.5),
            velocity: Vec3::new(2.0, mc_physics::LIVING_JUMP_SPEED_BLOCKS_PER_SECOND, 0.0),
            on_ground: false,
            horizontal_collision: true,
        }],
    );

    assert_eq!(registry.terrain_pathing_entity_count(), 0);
}

#[test]
fn entity_goal_pathing_compute_releases_store_and_rejects_stale_motion() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "GoalComputeTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(4.5, 64.0, 0.5),
    );
    let entity_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected entity spawn, got {command:?}"),
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_entity_goal_compute_probe(reached_tx, resume_rx);

    let tick_registry = Arc::clone(&registry);
    let tick =
        std::thread::spawn(move || tick_registry.tick_entities_and_collect_physics_queries(1));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entity goal compute boundary");

    let newer_velocity = Vec3::new(-0.125, 0.25, 0.375);
    let store_was_available = registry.entities.owner_responsive_for_test();
    assert!(
        registry
            .lock_entities("test entity access")
            .set_velocity(entity_id, newer_velocity)
    );
    resume_tx.send(()).expect("resume entity goal compute");
    let queries = tick.join().expect("entity goal tick");

    assert!(
        store_was_available,
        "bounded pathing must run without holding EntityStore"
    );
    let entity = queries
        .iter()
        .find(|query| query.id == entity_id)
        .expect("zombie physics query");
    assert_eq!(entity.velocity, newer_velocity);
}

#[test]
fn stale_entity_goal_apply_rechecks_current_simulation_membership() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "GoalMembershipTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(4.5, 64.0, 0.5),
    );
    let entity_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected entity spawn, got {command:?}"),
    };
    assert!(registry.lock_entities("test entity access").set_goal(
        entity_id,
        GoalState::FollowPosition {
            target: Vec3::new(10.5, 64.0, 0.5),
            speed: 0.2,
        },
    ));
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_entity_goal_compute_probe(reached_tx, resume_rx);

    let tick_registry = Arc::clone(&registry);
    let tick =
        std::thread::spawn(move || tick_registry.tick_entities_and_collect_physics_queries(23));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entity goal compute boundary");

    let mut entities = registry.lock_entities("move stale goal entity");
    let expected = entities.snapshot(entity_id).expect("goal entity snapshot");
    let mut moved = expected.clone();
    moved.position = Vec3::new(512.5, 64.0, 0.5);
    assert!(entities.replace_snapshot_if_current(expected, moved));
    drop(entities);
    assert!(
        registry
            .lock_entities("test entity access")
            .set_velocity(entity_id, Vec3::new(0.125, 0.0, 0.0))
    );
    assert!(
        registry
            .lock_entities("test entity access")
            .set_goal(entity_id, GoalState::Idle,)
    );
    resume_tx.send(()).expect("resume entity goal compute");
    let queries = tick.join().expect("entity goal tick");

    assert!(
        queries.iter().all(|query| query.id != entity_id),
        "stale active membership must not create a physics query: {queries:?}"
    );
}

#[test]
fn goal_projection_rechecks_grazing_entity_outside_goal_cas() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "GrazingProjectionTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let cow_spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(4.5, 64.0, 0.5),
    );
    let cow_id = match &cow_spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected cow spawn, got {command:?}"),
    };
    assert!(registry.lock_entities("test entity access").set_goal(
        cow_id,
        GoalState::FollowPosition {
            target: Vec3::new(10.5, 64.0, 0.5),
            speed: 0.2,
        },
    ));
    let sheep_spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:sheep".to_owned(),
        Vec3::new(6.5, 64.0, 0.5),
    );
    let sheep_id = match &sheep_spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected sheep spawn, got {command:?}"),
    };
    assert!(
        registry.set_sheep_grazing_ticks_for_test(sheep_id, Some(SHEEP_GRAZING_ANIMATION_TICKS),)
    );
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_entity_goal_compute_probe(reached_tx, resume_rx);

    let tick_registry = Arc::clone(&registry);
    let tick =
        std::thread::spawn(move || tick_registry.tick_entities_and_collect_physics_queries(23));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entity goal compute boundary");
    let mut entities = registry.lock_entities("move stale grazing entity");
    let expected = entities
        .snapshot(sheep_id)
        .expect("grazing entity snapshot");
    let mut moved = expected.clone();
    moved.position = Vec3::new(512.5, 64.0, 0.5);
    assert!(entities.replace_snapshot_if_current(expected, moved));
    drop(entities);
    resume_tx.send(()).expect("resume entity goal compute");
    let queries = tick.join().expect("entity goal tick");

    assert!(queries.iter().any(|query| query.id == cow_id));
    assert!(
        queries.iter().all(|query| query.id != sheep_id),
        "grazing entity outside the goal CAS must use current membership: {queries:?}"
    );
}

#[test]
fn chicken_pathing_uses_its_own_aabb_next_to_a_wall() {
    let (world_read, materials) = two_block_wall_pathing_world();
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("ChickenPathTarget"),
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
        "minecraft:chicken".to_string(),
        Vec3::new(1.75, 64.0, 0.5),
    );
    let chicken_id = registry.persisted_entity_records()[0].id;
    assert!(registry.lock_entities("test entity access").set_goal(
        chicken_id,
        GoalState::Wander {
            speed: 2.5,
            period_ticks: 80,
        },
    ));
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: chicken_id,
            position: Vec3::new(1.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: true,
        }],
    );

    let queries =
        registry.tick_entities_and_collect_physics_queries_with_terrain(2, &world_read, &materials);
    let chicken = queries
        .iter()
        .find(|query| query.id == chicken_id)
        .expect("chicken physics query");

    assert_ne!(chicken.velocity, Vec3::ZERO);
}

#[test]
fn registry_recovers_after_inner_mutex_poison() {
    let registry = SessionRegistry::new();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = registry.inner.lock().unwrap();
        panic!("poison registry");
    }));

    let id = register_test_session(&registry, "PoisonAlice");

    assert_eq!(registry.active_session_count(), 1);
    assert_eq!(id, 1);
}

#[test]
fn mark_loaded_does_not_wait_for_entity_store() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "LockOrderAlice");
    let entity_guard = registry.lock_entities("test entity access");
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let worker_registry = Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        let dispatches = worker_registry.mark_loaded(session, (0, 0));
        completed_tx
            .send(dispatches)
            .expect("mark-loaded completion receiver");
    });
    let completed = completed_rx.recv_timeout(std::time::Duration::from_secs(1));
    drop(entity_guard);
    worker.join().expect("mark-loaded worker");

    assert!(
        completed.is_ok(),
        "mark_loaded must use the published entity view instead of waiting for EntityStore"
    );
}

#[test]
fn committed_herd_batch_publication_preserves_visibility_order_and_indexes() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "BatchPublishAlice");
    let bob = register_test_session(&registry, "BatchPublishBob");
    let west = (0, 0);
    let east = (1, 0);
    registry.mark_loaded(alice, west);
    registry.mark_loaded(alice, east);
    registry.mark_loaded(bob, west);

    let mut store = mc_entity::EntityStore::new();
    let mut west_entity = SpawnEntity::new(4, "minecraft:cow", Vec3::new(0.5, 64.0, 0.5));
    west_entity.retained.spawn_tick = 77;
    let west_id = store.spawn(west_entity);
    let mut east_entity = SpawnEntity::new(4, "minecraft:cow", Vec3::new(16.5, 64.0, 0.5));
    east_entity.retained.spawn_tick = 77;
    let east_id = store.spawn(east_entity);
    let committed = [west_id, east_id]
        .into_iter()
        .map(|id| store.snapshot(id).expect("committed herd snapshot"))
        .collect::<Vec<_>>();
    assert!(
        registry
            .lock_entities("install committed herd owner state")
            .insert_snapshots_batch(committed.clone())
    );

    let dispatches = {
        let mut inner = registry.lock_inner("publish committed herd batch");
        install_committed_herd_spawns_locked(&mut inner, committed, 77)
    };

    assert_eq!(
        dispatches
            .iter()
            .filter_map(|dispatch| match &dispatch.command {
                OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![west_id, west_id, east_id],
        "batch publication keeps entity-major dispatch order"
    );
    let mut west_recipients = dispatches
        .iter()
        .filter_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) if snapshot.id == west_id => {
                Some(dispatch.recipient.id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    west_recipients.sort_unstable();
    assert_eq!(west_recipients, vec![alice, bob]);

    let entities = registry.lock_entities("inspect committed herd retained state");
    assert_eq!(entities.snapshot(west_id).unwrap().retained.spawn_tick, 77);
    assert_eq!(entities.snapshot(east_id).unwrap().retained.spawn_tick, 77);
    drop(entities);
    let inner = registry.lock_inner("inspect committed herd batch");
    assert_eq!(inner.entity_chunks.get(&west_id), Some(&west));
    assert_eq!(inner.entity_chunks.get(&east_id), Some(&east));
    assert!(inner.published_entity_snapshots.contains_key(&west_id));
    assert!(inner.published_entity_snapshots.contains_key(&east_id));
    assert!(inner.sessions[&alice].visible_entities.contains(&west_id));
    assert!(inner.sessions[&alice].visible_entities.contains(&east_id));
    assert!(inner.sessions[&bob].visible_entities.contains(&west_id));
    assert!(!inner.sessions[&bob].visible_entities.contains(&east_id));
    assert_eq!(inner.entity_dispatches.spawn, 3);
}

#[test]
fn ensure_chunk_herd_releases_session_lock_during_durable_unique_batch() {
    let chunk = (2, 0);
    let duplicate_uuid = herd_uuid(chunk, 0);
    let new_uuid = herd_uuid(chunk, 1);
    let commits = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let registry = Arc::new(SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(BlockingEntityCommitJournal {
            blocked_uuid: new_uuid,
            entered: entered_tx,
            release: release_rx,
            commits: Arc::clone(&commits),
            failure: None,
        }),
    ));
    let mut restored_store = mc_entity::EntityStore::new();
    let mut restored_spawn = SpawnEntity::new(4, "minecraft:cow", Vec3::new(32.5, 64.0, 0.5));
    restored_spawn.uuid = Some(duplicate_uuid);
    let restored_id = restored_store.spawn(restored_spawn);
    let restored = restored_store
        .snapshot(restored_id)
        .expect("restored duplicate snapshot");
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(
            0,
            vec![PersistedEntityRecord {
                snapshot: restored,
                age: 0,
                pickup_delay: 0,
            }],
        )),
        1
    );
    let alice = register_test_session(&registry, "HerdLockAlice");
    let spawns = vec![
        HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(32.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        },
        HerdSpawn {
            chunk,
            slot: 1,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(33.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        },
    ];

    let herd_registry = Arc::clone(&registry);
    let herd_spawns = spawns.clone();
    let herd = std::thread::spawn(move || {
        herd_registry.ensure_chunk_herd_legacy_for_test(chunk, &herd_spawns)
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("herd journal commit entered");

    let inner_available = registry.inner.try_lock().is_ok();
    let register_registry = Arc::clone(&registry);
    let (registered_tx, registered_rx) = std::sync::mpsc::channel();
    let registration = std::thread::spawn(move || {
        let bob = register_test_session(&register_registry, "HerdLockBob");
        registered_tx.send(()).expect("registration completion");
        bob
    });
    let registration_completed = registered_rx.recv_timeout(Duration::from_secs(1)).is_ok();
    let load_registry = Arc::clone(&registry);
    let (loaded_tx, loaded_rx) = std::sync::mpsc::channel();
    let load = std::thread::spawn(move || {
        let dispatches = load_registry.mark_loaded(alice, chunk);
        loaded_tx.send(()).expect("mark-loaded completion");
        dispatches
    });
    let load_completed = loaded_rx.recv_timeout(Duration::from_secs(1)).is_ok();

    release_tx.send(()).expect("release herd journal commit");
    let herd_dispatches = herd.join().expect("herd worker");
    let bob = registration.join().expect("registration worker");
    let alice_load_dispatches = load.join().expect("mark-loaded worker");

    assert!(
        inner_available,
        "durable entity commit must not retain SessionRegistry.inner"
    );
    assert!(
        registration_completed,
        "session registration must complete while the entity journal is blocked"
    );
    assert!(
        load_completed,
        "mark_loaded must complete before herd publication phase three"
    );
    assert_eq!(
        alice_load_dispatches
            .iter()
            .filter(|dispatch| matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntity(snapshot) if snapshot.uuid == duplicate_uuid
            ))
            .count(),
        1
    );
    assert_eq!(
        herd_dispatches
            .iter()
            .filter(|dispatch| matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntity(snapshot) if snapshot.uuid == new_uuid
            ))
            .map(|dispatch| dispatch.recipient.id)
            .collect::<Vec<_>>(),
        vec![alice]
    );

    let bob_load_dispatches = registry.mark_loaded(bob, chunk);
    assert_eq!(
        bob_load_dispatches
            .iter()
            .flat_map(|dispatch| match &dispatch.command {
                OutboundCommand::SpawnEntity(snapshot) => std::slice::from_ref(snapshot),
                OutboundCommand::SpawnEntities(snapshots) => snapshots.as_slice(),
                _ => &[],
            })
            .filter(|snapshot| snapshot.uuid == new_uuid)
            .count(),
        1,
        "mark_loaded after phase three must use published herd indexes"
    );
    let authoritative = registry
        .lock_entities("test entity access")
        .snapshots()
        .collect::<Vec<_>>();
    assert_eq!(authoritative.len(), 2);
    let new = authoritative
        .iter()
        .find(|snapshot| snapshot.uuid == new_uuid)
        .expect("new authoritative herd entity");
    {
        let inner = registry.lock_inner("inspect durable herd publication");
        for entity_id in [restored_id, new.id] {
            assert_eq!(inner.entity_chunks.get(&entity_id), Some(&chunk));
            assert!(inner.entities_by_chunk[&chunk].contains(&entity_id));
            let published = &inner.published_entity_snapshots[&entity_id];
            let wire = &inner.last_sent_entity_states[&entity_id];
            assert_eq!(wire.position, published.position);
            assert_eq!(wire.velocity, published.velocity);
            assert_eq!(wire.rotation, published.rotation);
            assert_eq!(wire.on_ground, published.on_ground);
            assert!(inner.sessions[&alice].visible_entities.contains(&entity_id));
            assert!(inner.sessions[&bob].visible_entities.contains(&entity_id));
        }
    }
    let commit_count = commits.lock().expect("entity journal commits").len();
    assert_eq!(commit_count, 2, "restore and one unique herd batch");
    assert_eq!(
        commits
            .lock()
            .expect("entity journal commits")
            .last()
            .expect("herd commit")
            .upserts()
            .iter()
            .map(|snapshot| snapshot.uuid)
            .collect::<Vec<_>>(),
        vec![new_uuid]
    );

    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &spawns)
            .is_empty()
    );
    assert_eq!(registry.persisted_entity_records().len(), 2);
    assert_eq!(
        commits.lock().expect("entity journal commits").len(),
        commit_count,
        "reissued chunk claim must be idempotent"
    );
}

#[test]
fn generated_hostile_herds_stop_at_the_vanilla_global_cap() {
    let registry = SessionRegistry::new();
    registry.set_world_time(13_000);

    for chunk_x in 0..30 {
        let chunk = (chunk_x, 0);
        let spawns = (0..MAX_HOSTILE_SPAWNS_PER_CHUNK)
            .map(|slot| HerdSpawn {
                chunk,
                slot: slot as u8,
                entity_type_id: 5,
                entity_type_name: "minecraft:zombie".to_owned(),
                position: Vec3::new(chunk_x as f64 * 16.0 + slot as f64, 64.0, 0.5),
                hostile: true,
                sheep_color: None,
            })
            .collect::<Vec<_>>();
        registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns);
    }

    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), VANILLA_HOSTILE_MOB_CAP);
    assert!(
        records
            .iter()
            .all(|record| record.snapshot.type_name == "minecraft:zombie")
    );
    let inner = registry.lock_inner("inspect hostile cap index");
    assert_eq!(inner.hostile_entities.len(), VANILLA_HOSTILE_MOB_CAP);
}

#[test]
fn generated_passive_herds_stop_at_the_vanilla_global_cap() {
    let registry = SessionRegistry::new();

    for chunk_x in 0..30 {
        let chunk = (chunk_x, 0);
        let spawns = (0..MAX_PASSIVE_SPAWNS_PER_CHUNK)
            .map(|slot| HerdSpawn {
                chunk,
                slot: slot as u8,
                entity_type_id: 6,
                entity_type_name: "minecraft:chicken".to_owned(),
                position: Vec3::new(chunk_x as f64 * 16.0 + slot as f64, 64.0, 0.5),
                hostile: false,
                sheep_color: None,
            })
            .collect::<Vec<_>>();
        registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns);
    }

    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), VANILLA_CREATURE_MOB_CAP);
    assert!(
        records
            .iter()
            .all(|record| record.snapshot.type_name == "minecraft:chicken")
    );
    let inner = registry.lock_inner("inspect passive cap index");
    assert_eq!(inner.natural_ground_mobs.len(), VANILLA_CREATURE_MOB_CAP);
}

#[test]
fn safe_chunk_herd_failure_releases_claim_for_one_exact_retry() {
    let chunk = (3, 0);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
            commits: Arc::clone(&commits),
        }),
    );
    let spawns = vec![HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 4,
        entity_type_name: "minecraft:cow".to_owned(),
        position: Vec3::new(48.5, 64.0, 0.5),
        hostile: false,
        sheep_color: None,
    }];

    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &spawns)
            .is_empty()
    );

    assert!(registry.persisted_entity_records().is_empty());
    {
        let inner = registry.lock_inner("inspect failed herd publication");
        assert!(!inner.spawned_entity_chunks.contains(&chunk));
        assert!(
            !inner
                .entity_chunks
                .values()
                .any(|indexed| *indexed == chunk)
        );
        assert!(!inner.entities_by_chunk.contains_key(&chunk));
        assert!(inner.published_entity_snapshots.is_empty());
        assert!(inner.last_sent_entity_states.is_empty());
    }
    registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &spawns)
            .is_empty()
    );
    assert_eq!(commits.load(Ordering::Relaxed), 2);
}

#[test]
fn unknown_chunk_herd_failure_keeps_claim_and_cannot_retry() {
    let chunk = (3, 1);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::OUTCOME_UNKNOWN),
            commits: Arc::clone(&commits),
        }),
    );
    let spawns = vec![HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 4,
        entity_type_name: "minecraft:cow".to_owned(),
        position: Vec3::new(48.5, 64.0, 16.5),
        hostile: false,
        sheep_color: None,
    }];

    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns)
    }));
    assert!(first.is_err());
    {
        let inner = registry.lock_inner("inspect uncertain herd claim");
        assert!(inner.spawned_entity_chunks.contains(&chunk));
        assert!(!inner.entities_by_chunk.contains_key(&chunk));
        assert!(inner.published_entity_snapshots.is_empty());
    }
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &spawns)
            .is_empty()
    );
    assert_eq!(commits.load(Ordering::Relaxed), 1);

    let mut probe = SpawnEntity::new(4, "minecraft:cow", Vec3::new(49.5, 64.0, 16.5));
    probe.uuid = Some(uuid::Uuid::from_u128(0x301));
    assert_eq!(
        registry.entities.handle.spawn_unique_batch([probe]),
        Err(mc_entity::RegionOwnerLaneError::OutcomeUnknown)
    );
}

#[test]
fn safe_pending_hostile_failure_restores_claim_for_one_exact_retry() {
    let chunk = (4, 1);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
            commits: Arc::clone(&commits),
        }),
    );
    let observer = register_test_session(&registry, "SafePendingRetryObserver");
    registry.mark_loaded(observer, chunk);
    let pending = [HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 5,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(64.5, 64.0, 16.5),
        hostile: true,
        sheep_color: None,
    }];
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &pending)
            .is_empty()
    );

    let first = registry.activate_pending_hostiles_owned(&SimulationAuthority::for_test());
    assert!(first.is_empty());
    assert_eq!(first.retryable_chunks(), [chunk]);
    assert!(
        registry
            .lock_inner("inspect restored pending hostile claim")
            .pending_hostile_spawns
            .contains_key(&chunk)
    );
    assert!(registry.persisted_entity_records().is_empty());

    let second = registry.activate_pending_hostiles_owned(&SimulationAuthority::for_test());
    assert!(!second.is_empty());
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert!(
        registry
            .activate_pending_hostiles_owned(&SimulationAuthority::for_test())
            .is_empty()
    );
    assert_eq!(commits.load(Ordering::Relaxed), 2);
}

#[test]
fn pending_hostile_activation_releases_session_lock_during_journal_commit() {
    let chunk = (4, 0);
    let hostile_uuid = herd_uuid(chunk, 0);
    let commits = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let registry = Arc::new(SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(BlockingEntityCommitJournal {
            blocked_uuid: hostile_uuid,
            entered: entered_tx,
            release: release_rx,
            commits: Arc::clone(&commits),
            failure: None,
        }),
    ));
    let alice = register_test_session(&registry, "PendingHostileAlice");
    assert!(registry.mark_loaded(alice, chunk).is_empty());
    let pending = vec![HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 1,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(64.5, 64.0, 0.5),
        hostile: true,
        sheep_color: None,
    }];
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &pending)
            .is_empty()
    );

    let activation_registry = Arc::clone(&registry);
    let activation = std::thread::spawn(move || {
        activation_registry.activate_pending_hostiles_owned(&SimulationAuthority::for_test())
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pending hostile journal commit entered");

    let inner_available = registry.inner.try_lock().is_ok();
    let observer_registry = Arc::clone(&registry);
    let (observer_tx, observer_rx) = std::sync::mpsc::channel();
    let observer = std::thread::spawn(move || {
        let bob = register_test_session(&observer_registry, "PendingHostileBob");
        let dispatches = observer_registry.mark_loaded(bob, chunk);
        observer_tx
            .send(())
            .expect("pending hostile observer completion");
        (bob, dispatches)
    });
    let observer_completed = observer_rx.recv_timeout(Duration::from_secs(1)).is_ok();

    release_tx
        .send(())
        .expect("release pending hostile journal commit");
    let activation_dispatches = activation.join().expect("pending hostile activation");
    let (bob, bob_load_dispatches) = observer.join().expect("pending hostile observer");

    assert!(
        inner_available,
        "pending hostile journal commit must not retain SessionRegistry.inner"
    );
    assert!(
        observer_completed,
        "register and mark_loaded must complete before pending hostile publication"
    );
    assert!(bob_load_dispatches.iter().all(|dispatch| !matches!(
        &dispatch.command,
        OutboundCommand::SpawnEntity(snapshot) if snapshot.uuid == hostile_uuid
    )));
    let mut spawned_for = activation_dispatches
        .iter()
        .filter(|dispatch| {
            matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntity(snapshot) if snapshot.uuid == hostile_uuid
            )
        })
        .map(|dispatch| dispatch.recipient.id)
        .collect::<Vec<_>>();
    spawned_for.sort_unstable();
    assert_eq!(spawned_for, vec![alice, bob]);

    let authoritative = registry
        .lock_entities("test entity access")
        .snapshots()
        .collect::<Vec<_>>();
    assert_eq!(authoritative.len(), 1);
    let hostile = &authoritative[0];
    assert_eq!(hostile.uuid, hostile_uuid);
    {
        let inner = registry.lock_inner("inspect pending hostile publication");
        assert!(!inner.pending_hostile_spawns.contains_key(&chunk));
        assert_eq!(inner.entity_chunks.get(&hostile.id), Some(&chunk));
        assert!(inner.entities_by_chunk[&chunk].contains(&hostile.id));
        let published = &inner.published_entity_snapshots[&hostile.id];
        let wire = &inner.last_sent_entity_states[&hostile.id];
        assert_eq!(wire.position, published.position);
        assert_eq!(wire.velocity, published.velocity);
        assert_eq!(wire.rotation, published.rotation);
        assert_eq!(wire.on_ground, published.on_ground);
        assert!(
            inner.sessions[&alice]
                .visible_entities
                .contains(&hostile.id)
        );
        assert!(inner.sessions[&bob].visible_entities.contains(&hostile.id));
    }
    let commit_count = commits.lock().expect("entity journal commits").len();
    assert_eq!(commit_count, 1);
    assert!(
        registry
            .activate_pending_hostiles_owned(&SimulationAuthority::for_test())
            .is_empty()
    );
    assert_eq!(commits.lock().expect("entity journal commits").len(), 1);
}

#[test]
fn direct_night_time_change_activates_loaded_pending_hostiles() {
    let chunk = (6, 0);
    let registry = SessionRegistry::new();
    let observer = register_test_session(&registry, "DirectNightObserver");
    registry.mark_loaded(observer, chunk);
    let pending = [HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 5,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(96.5, 64.0, 0.5),
        hostile: true,
        sheep_color: None,
    }];
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &pending)
            .is_empty()
    );
    assert!(registry.persisted_entity_records().is_empty());
    registry.reset_entity_owner_requests_for_test();

    registry.set_world_time_and_update_sleep(super::super::NIGHT_START_TICK);

    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].snapshot.type_name, "minecraft:zombie");
    assert_eq!(registry.entity_owner_requests_for_test(), 1);
}

#[test]
fn direct_time_set_between_herd_admission_and_claim_cannot_strand_hostiles() {
    let chunk = (9, 0);
    let registry = Arc::new(SessionRegistry::new());
    let observer = register_test_session(&registry, "DirectTimeHerdObserver");
    registry.mark_loaded(observer, chunk);
    let (claim_ready_tx, claim_ready_rx) = std::sync::mpsc::sync_channel(0);
    let (claim_resume_tx, claim_resume_rx) = std::sync::mpsc::sync_channel(0);
    registry.install_chunk_herd_claim_probe_for_test(claim_ready_tx, claim_resume_rx);
    let spawns = vec![HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 5,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(144.5, 64.0, 0.5),
        hostile: true,
        sheep_color: None,
    }];

    let herd_registry = Arc::clone(&registry);
    let herd =
        std::thread::spawn(move || herd_registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns));
    claim_ready_rx
        .recv()
        .expect("herd reached the pre-claim admission gate");
    registry.set_world_time_and_update_sleep(super::super::NIGHT_START_TICK);
    claim_resume_tx
        .send(())
        .expect("release herd into its session claim");

    let dispatches = herd.join().expect("herd worker");
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert!(
        registry
            .lock_inner("inspect direct time/herd interleaving")
            .pending_hostile_spawns
            .is_empty()
    );
    assert!(dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::SpawnEntity(snapshot) if snapshot.type_name == "minecraft:zombie"
    )));
}

#[test]
fn pending_hostile_chunks_commit_and_publish_as_one_batch() {
    let chunks = [(7, 0), (8, 0)];
    let blocked_uuid = herd_uuid(chunks[1], 0);
    let commits = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let registry = Arc::new(SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(BlockingEntityCommitJournal {
            blocked_uuid,
            entered: entered_tx,
            release: release_rx,
            commits: Arc::clone(&commits),
            failure: None,
        }),
    ));
    let observer = register_test_session(&registry, "PendingHostileBatchObserver");
    for chunk in chunks {
        registry.mark_loaded(observer, chunk);
        let pending = [HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 5,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(f64::from(chunk.0) * 16.0 + 0.5, 64.0, 0.5),
            hostile: true,
            sheep_color: None,
        }];
        assert!(
            registry
                .ensure_chunk_herd_legacy_for_test(chunk, &pending)
                .is_empty()
        );
    }
    registry.reset_entity_owner_requests_for_test();

    let activation_registry = Arc::clone(&registry);
    let activation = std::thread::spawn(move || {
        activation_registry.activate_pending_hostiles_owned(&SimulationAuthority::for_test())
    });
    entered_rx
        .recv()
        .expect("combined pending-hostile owner commit entered");

    assert_eq!(registry.entity_owner_requests_for_test(), 1);
    {
        let inner = registry.lock_inner("inspect atomic pending hostile publication");
        assert!(inner.published_entity_snapshots.is_empty());
        assert!(inner.entities_by_chunk.is_empty());
    }

    release_tx
        .send(())
        .expect("release combined pending-hostile owner commit");
    let dispatches = activation.join().expect("pending hostile batch activation");

    assert_eq!(registry.entity_owner_requests_for_test(), 1);
    assert_eq!(commits.lock().expect("entity journal commits").len(), 1);
    assert_eq!(registry.persisted_entity_records().len(), 2);
    assert_eq!(
        dispatches
            .iter()
            .filter(|dispatch| matches!(dispatch.command, OutboundCommand::SpawnEntity(_)))
            .count(),
        2
    );
    let inner = registry.lock_inner("inspect committed pending hostile batch");
    assert_eq!(inner.published_entity_snapshots.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|chunk| inner.entities_by_chunk.contains_key(chunk))
    );
}

#[test]
fn unknown_pending_hostile_failure_does_not_publish_or_retry() {
    let chunk = (5, 0);
    let hostile_uuid = herd_uuid(chunk, 0);
    let commits = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let registry = Arc::new(SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(BlockingEntityCommitJournal {
            blocked_uuid: hostile_uuid,
            entered: entered_tx,
            release: release_rx,
            commits: Arc::clone(&commits),
            failure: Some(mc_entity::RegionalDecisionJournalError::OUTCOME_UNKNOWN),
        }),
    ));
    let observer = register_test_session(&registry, "PendingHostileFailureObserver");
    assert!(registry.mark_loaded(observer, chunk).is_empty());
    let pending = vec![HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 1,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(chunk.0 as f64 * 16.0 + 0.5, 64.0, 0.5),
        hostile: true,
        sheep_color: None,
    }];
    assert!(
        registry
            .ensure_chunk_herd_legacy_for_test(chunk, &pending)
            .is_empty()
    );

    let activation_registry = Arc::clone(&registry);
    let activation = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            activation_registry.activate_pending_hostiles_owned(&SimulationAuthority::for_test())
        }))
    });
    entered_rx
        .recv()
        .expect("uncertain pending hostile journal commit entered");
    let inner_available = registry.inner.try_lock().is_ok();
    release_tx
        .send(())
        .expect("release uncertain pending hostile journal commit");
    assert!(activation.join().expect("pending hostile worker").is_err());

    assert!(inner_available);
    {
        let inner = registry.lock_inner("inspect uncertain pending hostile publication");
        assert!(!inner.pending_hostile_spawns.contains_key(&chunk));
        assert!(!inner.entities_by_chunk.contains_key(&chunk));
        assert!(inner.published_entity_snapshots.is_empty());
        assert!(inner.last_sent_entity_states.is_empty());
        assert!(inner.sessions[&observer].visible_entities.is_empty());
    }
    assert!(
        registry
            .activate_pending_hostiles_owned(&SimulationAuthority::for_test())
            .is_empty()
    );
    assert_eq!(commits.lock().expect("entity journal commits").len(), 1);

    let mut probe = SpawnEntity::new(
        4,
        "minecraft:cow",
        Vec3::new(chunk.0 as f64 * 16.0 + 1.5, 64.0, 0.5),
    );
    probe.uuid = Some(uuid::Uuid::from_u128(0x100));
    assert_eq!(
        registry.entities.handle.spawn_unique_batch([probe]),
        Err(mc_entity::RegionOwnerLaneError::OutcomeUnknown)
    );
}

#[test]
fn pressure_snapshot_does_not_wait_for_runtime_state_locks() {
    let registry = Arc::new(SessionRegistry::new());
    let entity_guard = registry.lock_entities("test entity access");
    let session_guard = registry.inner.lock().expect("session registry lock");
    let prepared_cache_guard = registry.prepared_cache.lock().expect("prepared cache lock");
    let container_guard = registry.containers.shards[0]
        .lock()
        .expect("container registry shard lock");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let worker_registry = Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        started_tx.send(()).expect("snapshot start receiver");
        completed_tx
            .send(worker_registry.pressure_snapshot())
            .expect("snapshot completion receiver");
    });
    started_rx.recv().expect("snapshot worker start");
    let _snapshot = completed_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("pressure snapshot must not wait for runtime state locks");
    drop(prepared_cache_guard);
    drop(container_guard);
    drop(session_guard);
    drop(entity_guard);
    worker.join().expect("snapshot worker");
}

#[test]
fn pressure_snapshot_aggregates_container_viewers_across_shards() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "ShardedContainerPressure");
    let first = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let second = mc_world::BlockPos {
        x: mc_entity::REGION_SIZE_CHUNKS * 16 + 1,
        y: 64,
        z: 1,
    };

    registry.register_chest_viewer(session, first);
    registry.register_chest_viewer(session, second);
    assert_eq!(registry.pressure_snapshot().chest_viewer_sets, 2);

    registry.unregister_chest_viewer(session, first);
    assert_eq!(registry.pressure_snapshot().chest_viewer_sets, 1);
    registry.unregister_chest_viewer(session, second);
    assert_eq!(registry.pressure_snapshot().chest_viewer_sets, 0);
}

#[test]
fn unregister_does_not_wait_for_entity_store() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "DisconnectAlice");
    let entity_guard = registry.lock_entities("test entity access");
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let worker_registry = Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        let dispatches = worker_registry.unregister(session);
        completed_tx
            .send(dispatches)
            .expect("unregister completion receiver");
    });
    let completed = completed_rx.recv_timeout(std::time::Duration::from_secs(1));
    drop(entity_guard);
    worker.join().expect("unregister worker");

    assert!(
        completed.is_ok(),
        "unregister must leave hostile target reconciliation to the simulation owner"
    );
}

#[test]
fn mark_loaded_spawns_latest_published_entity_snapshot() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "PublishedEntityAlice");
    assert!(
        registry
            .spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_owned(),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .is_empty()
    );
    let entity_id = registry.persisted_entity_records()[0].id;
    let position = Vec3::new(1.25, 64.5, 0.75);
    let velocity = Vec3::new(0.1, 0.0, -0.2);
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position,
            velocity,
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    let dispatches = registry.mark_loaded(session, (0, 0));

    assert!(matches!(
        dispatches.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::SpawnEntity(snapshot),
            ..
        }] if snapshot.id == entity_id
            && snapshot.position == position
            && snapshot.velocity == velocity
            && !snapshot.on_ground
    ));
}

#[test]
fn mark_loaded_does_not_wait_for_move_fanout() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "MoveFanoutAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected entity spawn, got {command:?}"),
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .move_fanout_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(MoveFanoutProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let physics_registry = Arc::clone(&registry);
    let physics = std::thread::spawn(move || {
        physics_registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    });
    let reached = reached_rx.recv_timeout(Duration::from_secs(1));

    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let load_registry = Arc::clone(&registry);
    let load = std::thread::spawn(move || {
        let dispatches = load_registry.mark_loaded(session, (1, 0));
        completed_tx
            .send(dispatches)
            .expect("mark-loaded completion receiver");
    });
    let completed = completed_rx.recv_timeout(Duration::from_secs(1));
    resume_tx.send(()).expect("release move fanout probe");
    physics.join().expect("physics worker");
    load.join().expect("mark-loaded worker");

    assert!(reached.is_ok(), "physics must reach the move fanout probe");
    assert!(
        completed.is_ok(),
        "mark_loaded must remain available while move packets are fanned out"
    );
}

#[test]
fn entity_physics_owner_apply_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "PhysicsOwnerApplyAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
            _ => None,
        })
        .expect("spawned zombie");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .physics_owner_apply_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let physics_registry = Arc::clone(&registry);
    let physics = std::thread::spawn(move || {
        physics_registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("physics reaches owner apply boundary");
    let session_available = registry.inner.try_lock().is_ok();
    resume_tx.send(()).expect("release physics owner apply");
    physics.join().expect("physics worker");

    assert!(
        session_available,
        "regional kinematics apply must not retain session state"
    );
}

#[test]
fn entity_physics_uses_prepare_and_one_current_read_after_commit() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "PhysicsPostStateAlice");
    registry.mark_loaded(session, (0, 0));
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
            _ => None,
        })
        .expect("spawned zombie");
    registry.reset_entity_owner_requests_for_test();

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        registry.entity_owner_requests_for_test(),
        3,
        "physics reads ECS for preparation, item expiry, and current-state publication"
    );
}

#[test]
fn delayed_entity_physics_apply_cannot_overwrite_newer_commit() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "PhysicsApplyRaceAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(snapshot) => Some(snapshot.id),
            _ => None,
        })
        .expect("spawned zombie");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .physics_owner_apply_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let delayed_registry = Arc::clone(&registry);
    let delayed = std::thread::spawn(move || {
        delayed_registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::new(0.1, 0.0, 0.0),
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("delayed physics reaches owner apply boundary");

    let newer_position = Vec3::new(1.5, 64.0, 0.5);
    let newer_velocity = Vec3::new(0.2, 0.0, 0.0);
    let newer_registry = Arc::clone(&registry);
    let (newer_done, newer_done_rx) = std::sync::mpsc::channel();
    let newer = std::thread::spawn(move || {
        newer_registry.apply_entity_physics_and_dispatch(
            2,
            &[EntityPhysicsStep {
                id: entity_id,
                position: newer_position,
                velocity: newer_velocity,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
        newer_done
            .send(())
            .expect("publish newer physics completion");
    });
    let newer_completed = newer_done_rx.recv_timeout(Duration::from_secs(1));
    resume_tx.send(()).expect("release delayed physics apply");
    delayed.join().expect("delayed physics worker");
    newer.join().expect("newer physics worker");

    assert!(
        newer_completed.is_ok(),
        "newer physics must not wait for the delayed apply's session state"
    );
    let current = registry
        .server_entity_snapshot(entity_id)
        .expect("entity remains published");
    assert_eq!(current.position, newer_position);
    assert_eq!(current.velocity, newer_velocity);
}

#[test]
fn entity_store_is_released_before_session_movement_plan() {
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "EntityReleaseAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(snapshot) => snapshot.id,
        command => panic!("expected entity spawn, got {command:?}"),
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .entity_apply_release_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let physics_registry = Arc::clone(&registry);
    let physics = std::thread::spawn(move || {
        physics_registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("physics must reach the session movement-plan probe");

    let entity_store_available = registry.entities.owner_responsive_for_test();
    let session_registry_busy = matches!(
        registry.inner.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    );
    resume_tx.send(()).expect("release entity apply probe");
    physics.join().expect("physics worker");

    assert!(
        entity_store_available,
        "movement planning must not retain EntityStore after entity apply"
    );
    assert!(
        session_registry_busy,
        "probe must run while the session-only movement plan still owns SessionRegistry"
    );
}

#[test]
fn save_all_recovers_after_player_persistence_mutex_poison() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "PoisonPersist");
    let state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session_id, Arc::clone(&state));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = state.lock().unwrap();
        panic!("poison player persistence");
    }));

    let snapshots = registry.persisted_player_states();

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, profile("PoisonPersist").uuid);
}

#[test]
fn airborne_arrow_does_not_expire_from_total_spawn_age() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let profile = profile("ArrowAlice");
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());

    let entity_id = publish_single_entity_spawn(
        registry.spawn_arrow_for_test(
            Some(session_id),
            1,
            Vec3::new(0.5, 64.0, 0.5),
            Vec3::new(0.0, 0.0, 1.0),
            Rotation::ZERO,
        ),
        &mut rx,
    );

    registry.advance_world_time(mc_entity::projectile_26_1_2::ARROW_DESPAWN_TICKS as u64);
    registry.apply_entity_physics_and_dispatch(1, &[]);

    assert!(registry.server_entity_snapshot(entity_id).is_some());
    assert!(rx.try_recv().is_err());
}

#[test]
fn grounded_arrow_despawns_only_from_projectile_kernel_age() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let profile = profile("ArrowBob");
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let entity_id = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.0, 0.6),
            velocity: Vec3::new(0.0, 0.0, 0.1),
            on_ground: false,
            horizontal_collision: false,
        }],
    );
    {
        let mut inner = registry.lock_session_entities("seed grounded arrow kernel age");
        let expected = inner
            .entities
            .snapshot(entity_id)
            .expect("spawned arrow is owned by the regional ECS");
        let mut next = expected.clone();
        let state = next
            .retained
            .arrow_state
            .as_mut()
            .expect("spawned arrow initializes ECS kernel state");
        state.in_ground = true;
        state.despawn_age = mc_entity::projectile_26_1_2::ARROW_DESPAWN_TICKS - 1;
        state.last_block_state = Some(mc_entity::projectile_26_1_2::BlockStateId::new(1));
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.0, 0.6),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: true,
        }],
        &[ArrowPhysicsFact {
            arrow_id: entity_id,
            block_hit: None,
            embedded_in_block: true,
            current_block_state: mc_world::BlockStateId(1),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        }],
    );

    assert!(registry.server_entity_snapshot(entity_id).is_none());
    while let Ok(command) = rx.try_recv() {
        assert!(!matches!(command, OutboundCommand::DamagePlayer { .. }));
    }
}

#[test]
fn spawned_arrow_uses_projectile_physics_kind() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowPhysicsAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(
        queries[0].kind,
        EntityPhysicsKind::ArrowProjectile {
            revision: Some(0),
            embedded_block: None
        }
    );
    let embedded_block = mc_entity::projectile_26_1_2::BlockPosition::new(1, 64, 2);
    {
        let mut inner = registry.lock_session_entities("seed retained arrow block position");
        let expected = inner.entities.snapshot(arrow_id).expect("arrow exists");
        let mut next = expected.clone();
        let state = next
            .retained
            .arrow_state
            .as_mut()
            .expect("arrow state is ECS-owned");
        state.in_ground = true;
        state.last_block_position = Some(embedded_block);
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    let queries = registry.tick_entities_and_collect_physics_queries(2);

    assert_eq!(
        queries[0].kind,
        EntityPhysicsKind::ArrowProjectile {
            revision: Some(0),
            embedded_block: Some(embedded_block)
        }
    );
}

#[test]
fn arrow_tick_uses_authoritative_water_and_retained_gravity_facts() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowWaterFacts");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    {
        let mut inner = registry.lock_session_entities("set retained arrow gravity fact");
        let expected = inner.entities.snapshot(arrow_id).expect("arrow exists");
        let mut next = expected.clone();
        next.retained
            .arrow_state
            .as_mut()
            .expect("arrow state is ECS-owned")
            .no_gravity = true;
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.0, 1.5),
            velocity: Vec3::new(0.0, 0.0, 1.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[ArrowPhysicsFact {
            arrow_id,
            block_hit: None,
            embedded_in_block: false,
            current_block_state: mc_world::BlockStateId(9),
            should_fall: true,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: true,
            in_water_or_rain: true,
        }],
    );

    let arrow = registry
        .lock_entities("inspect water arrow")
        .snapshot(arrow_id)
        .expect("water arrow remains authoritative");
    assert_eq!(arrow.position, Vec3::new(0.5, 64.0, 1.5));
    assert_eq!(arrow.velocity, Vec3::new(0.0, 0.0, f64::from(0.6_f32)));
}

#[test]
fn grounded_zero_velocity_arrow_is_pickup_candidate() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowPickupAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let entity_id = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    let candidates = registry.nearby_grounded_arrows(Vec3::new(0.5, 64.0, 0.5), 2.25);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, entity_id);
}

#[test]
fn moving_or_airborne_arrow_is_not_pickup_candidate() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowPickupBob");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );

    let candidates = registry.nearby_grounded_arrows(Vec3::new(0.5, 64.0, 0.5), 2.25);

    assert!(candidates.is_empty());
}

#[test]
fn arrow_pickup_claim_removes_once_and_dispatches_take() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowPickupCarol");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let entity_id = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    let dispatches = registry
        .claim_arrow_pickup_for_test(entity_id, session_id)
        .unwrap();

    assert!(
        registry
            .claim_arrow_pickup_for_test(entity_id, session_id)
            .is_none()
    );
    assert!(registry.server_entity_snapshot(entity_id).is_none());
    assert!(dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::TakeItemEntity { item_entity_id, amount: 1, .. } if item_entity_id == entity_id.0
    )));
    assert!(dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::DespawnEntity(entity) if entity.id == entity_id
    )));
}

#[test]
fn moving_arrow_hit_damages_entity_and_despawns_arrow() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let profile = profile("ArrowHitAlice");
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let cow_id = publish_single_entity_spawn(
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            2,
            "minecraft:cow".to_string(),
            Vec3::new(0.5, 64.0, 1.5),
        ),
        &mut rx,
    );
    let arrow_id = publish_single_entity_spawn(
        registry.spawn_arrow_for_test(
            Some(session_id),
            1,
            Vec3::new(0.5, 64.75, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Rotation::ZERO,
        ),
        &mut rx,
    );

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    let cow_health = registry
        .lock_entities("test read cow health")
        .snapshot(cow_id)
        .expect("cow remains after non-lethal arrow hit")
        .health;
    assert_eq!(cow_health, 6.0);
    let cow_velocity = registry
        .lock_entities("test read cow velocity")
        .snapshot(cow_id)
        .expect("cow remains after non-lethal arrow hit")
        .velocity;
    assert!(cow_velocity.z > 0.5);
    assert!(cow_velocity.y > 0.0);
    assert!(registry.server_entity_snapshot(arrow_id).is_none());

    let mut saw_hurt = false;
    let mut saw_knockback = false;
    let mut saw_arrow_despawn = false;
    while let Ok(command) = rx.try_recv() {
        match command {
            OutboundCommand::EntityEvent {
                entity_id,
                event_id,
            } => {
                saw_hurt |= entity_id == cow_id.0 && event_id == 2;
            }
            OutboundCommand::MoveEntityRelative(movement) => {
                saw_knockback |=
                    movement.id == cow_id && movement.send_velocity && movement.velocity.z > 0.5;
            }
            OutboundCommand::DespawnEntity(entity) => {
                saw_arrow_despawn |= entity.id == arrow_id;
            }
            _ => {}
        }
    }
    assert!(saw_hurt);
    assert!(saw_knockback);
    assert!(saw_arrow_despawn);
}

#[test]
fn arrow_segment_intersection_rejects_non_finite_coordinates() {
    let min = Vec3::new(-1.0, -1.0, -1.0);
    let max = Vec3::new(1.0, 1.0, 1.0);

    assert_eq!(
        segment_aabb_intersection_t(
            Vec3::new(f64::NAN, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            min,
            max,
        ),
        None
    );
    assert_eq!(
        segment_aabb_intersection_t(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(f64::INFINITY, 0.0, 0.0),
            min,
            max,
        ),
        None
    );
    assert_eq!(
        segment_aabb_intersection_t(
            Vec3::new(-f64::MAX, 0.0, 0.0),
            Vec3::new(f64::MAX, 0.0, 0.0),
            min,
            max,
        ),
        None
    );
}

#[test]
fn lethal_arrow_hit_spawns_rewards_then_finishes_mob_death_after_twenty_ticks() {
    let registry = SessionRegistry::new();
    let chicken = Identifier::parse("minecraft:chicken".to_string()).unwrap();
    let feather = Identifier::parse("minecraft:feather".to_string()).unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[mc_data::items::ItemReport {
        id: feather.clone(),
        protocol_id: 42,
    }]));
    let loot = Arc::new(mc_data::loot::LootTables::from_maps(
        BTreeMap::from([(chicken, feather)]),
        BTreeMap::new(),
    ));
    registry.configure_arrow_kill_rewards(
        Some(98),
        Some(99),
        None,
        items,
        Arc::new(ItemFactsTable::default()),
        loot,
    );
    let (tx, mut rx) = mpsc::channel(16);
    let profile = profile("LethalArrowAlice");
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let chicken_id = publish_single_entity_spawn(
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            2,
            "minecraft:chicken".to_string(),
            Vec3::new(0.5, 64.0, 1.5),
        ),
        &mut rx,
    );
    let arrow_id = publish_single_entity_spawn(
        registry.spawn_arrow_for_test(
            Some(session_id),
            1,
            Vec3::new(0.5, 64.75, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Rotation::ZERO,
        ),
        &mut rx,
    );

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        registry
            .lock_entities("inspect arrow-killed chicken")
            .snapshot(chicken_id)
            .expect("dying chicken remains authoritative")
            .lifecycle,
        EntityLifecycle::Despawning
    );
    assert!(registry.server_entity_snapshot(arrow_id).is_none());
    let mut saw_chicken_despawn = false;
    let mut saw_arrow_despawn = false;
    let mut saw_drop = false;
    let mut saw_xp = false;
    let mut saw_xp_pickup = false;
    let mut saw_death_event = false;
    while let Ok(command) = rx.try_recv() {
        match command {
            OutboundCommand::DespawnEntity(entity) if entity.id == chicken_id => {
                saw_chicken_despawn = true;
            }
            OutboundCommand::DespawnEntity(entity) if entity.id == arrow_id => {
                saw_arrow_despawn = true;
            }
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                saw_drop = entity.type_id == 98
                    && entity.position == Vec3::new(0.5, 64.0, 1.5)
                    && entity.item_stack == Some(EntityItemStack::new(42, 1));
            }
            OutboundCommand::SpawnEntity(entity)
                if entity.type_name == "minecraft:experience_orb" =>
            {
                saw_xp = entity.type_id == 99
                    && entity.position == Vec3::new(0.5, 64.0, 1.5)
                    && entity.experience_value == Some(1);
            }
            OutboundCommand::PickupCandidates(candidates) => {
                saw_xp_pickup = candidates
                    .iter()
                    .any(|entity| entity.experience_value == Some(1));
            }
            OutboundCommand::EntityEvent {
                entity_id,
                event_id: ENTITY_EVENT_DEATH,
            } if entity_id == chicken_id.0 => saw_death_event = true,
            _ => {}
        }
    }
    assert!(!saw_chicken_despawn);
    assert!(saw_arrow_despawn);
    assert!(saw_drop);
    assert!(saw_xp);
    assert!(saw_xp_pickup);
    assert!(saw_death_event);

    registry.advance_world_time(ENTITY_DEATH_TICKS);
    let death_dispatches =
        registry.tick_dying_entities(&SimulationAuthority::for_test(), registry.simulation_tick());
    assert!(death_dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::EntityEvent {
            entity_id,
            event_id: ENTITY_EVENT_DEATH_COMPLETE,
        } if entity_id == chicken_id.0
    )));
    assert!(death_dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::DespawnEntity(ref entity) if entity.id == chicken_id
    )));
    assert!(registry.server_entity_snapshot(chicken_id).is_none());
}

#[test]
fn lethal_arrow_hit_on_sheared_sheep_keeps_meat_and_removes_wool() {
    let registry = SessionRegistry::new();
    let sheep = Identifier::parse("minecraft:sheep").unwrap();
    let white_wool = Identifier::parse("minecraft:white_wool").unwrap();
    let mutton = Identifier::parse("minecraft:mutton").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: white_wool.clone(),
            protocol_id: 41,
        },
        mc_data::items::ItemReport {
            id: mutton.clone(),
            protocol_id: 42,
        },
    ]));
    let loot = Arc::new(mc_data::loot::LootTables::from_drop_lists(
        BTreeMap::from([(
            sheep,
            vec![
                mc_data::loot::LootDrop::single(white_wool),
                mc_data::loot::LootDrop::single(mutton),
            ],
        )]),
        BTreeMap::new(),
    ));
    registry.configure_arrow_kill_rewards(
        Some(98),
        Some(99),
        None,
        items,
        Arc::new(ItemFactsTable::default()),
        loot,
    );
    let (tx, mut rx) = mpsc::channel(16);
    let profile = profile("ShearedSheepArrowAlice");
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let sheep_id = publish_single_entity_spawn(
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            2,
            "minecraft:sheep".to_string(),
            Vec3::new(0.5, 64.0, 1.5),
        ),
        &mut rx,
    );
    {
        let mut inner = registry.lock_session_entities("mark arrow target sheep sheared");
        let mut animal = inner
            .entities
            .snapshot(sheep_id)
            .and_then(|entity| entity.animal)
            .expect("spawned sheep has wool state");
        let mut wool = animal.sheep_wool.expect("spawned sheep has wool data");
        wool.sheared = true;
        animal.sheep_wool = Some(wool);
        assert!(inner.entities.set_animal_state(sheep_id, animal));
    }
    let first_hit = registry
        .damage_server_entity_for_test(sheep_id, ARROW_ENTITY_HIT_DAMAGE)
        .expect("first hit damages sheep");
    assert!(!first_hit.killed);
    let arrow_id = publish_single_entity_spawn(
        registry.spawn_arrow_for_test(
            Some(session_id),
            1,
            Vec3::new(0.5, 64.75, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Rotation::ZERO,
        ),
        &mut rx,
    );

    registry.apply_entity_physics_and_dispatch(
        ENTITY_HURT_INVULNERABLE_TICKS,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        registry
            .lock_entities("inspect arrow-killed sheep")
            .snapshot(sheep_id)
            .expect("dying sheep remains authoritative")
            .lifecycle,
        EntityLifecycle::Despawning
    );
    let drops = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|command| match command {
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                entity.item_stack
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(drops, vec![EntityItemStack::new(42, 1)]);

    registry.advance_world_time(ENTITY_DEATH_TICKS);
    let death_dispatches =
        registry.tick_dying_entities(&SimulationAuthority::for_test(), registry.simulation_tick());
    assert!(death_dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::DespawnEntity(ref entity) if entity.id == sheep_id
    )));
    assert!(registry.server_entity_snapshot(sheep_id).is_none());
}

#[test]
fn player_attack_contract_distinguishes_rejection_immunity_and_damage() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "AttackContractAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let reachable_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected reachable cow spawn, got {other:?}"),
    };
    let buffered_reachable_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 6.8),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected buffered-reachable cow spawn, got {other:?}"),
    };
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 20.5),
    );
    let far_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.position.z == 20.5)
        .expect("far cow remains authoritative")
        .snapshot
        .id;
    let authority = SimulationAuthority::for_test();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: reachable_id,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: None,
            },
        ),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::Damaged { .. })
    ));
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: reachable_id,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: buffered_reachable_id,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: None,
            },
        ),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::Damaged { .. })
    ));
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: far_id,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: None,
            },
        ),
        PlayerAttackResult::ValidationRejected
    ));
}

#[test]
fn player_attack_uses_authoritative_held_spear_range() {
    let spear_name = mc_data::Identifier::parse("minecraft:wooden_spear").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[mc_data::items::ItemReport {
        id: spear_name.clone(),
        protocol_id: 1,
    }]));
    let item_facts = Arc::new(ItemFactsTable::from_entries([(
        spear_name,
        mc_data::item_components::ItemFacts {
            attack_range: Some(mc_data::item_components::AttackRangeFacts {
                min_reach: 2.0,
                max_reach: 4.5,
                min_creative_reach: 2.0,
                max_creative_reach: 6.5,
                hitbox_margin: 0.125,
                mob_factor: 0.5,
            }),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    let registry = SessionRegistry::new();
    registry.configure_player_combat(None, None, items, item_facts);
    let session_id = register_test_session(&registry, "SpearReachAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let mut state = PlayerPersistedState::new_default(pose);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(1, 1);
    let expected_inventory = state.inventory.clone();
    let expected_survival = state.survival;
    let expected_xp = state.xp.clone();
    registry.register_player_persistence(session_id, Arc::new(Mutex::new(state)));
    let costs = PlayerSurvivalPlan {
        expected_survival,
        updated_survival: expected_survival,
        expected_inventory: expected_inventory.clone(),
        updated_inventory: expected_inventory,
        expected_carried_item: ItemStack::EMPTY,
        expected_xp: expected_xp.clone(),
        updated_xp: expected_xp,
        active_shield: None,
        enchanting_table_input: None,
        item_entity_type_id: None,
        xp_orb_entity_type_id: None,
        position: Vec3::new(pose.x, pose.y, pose.z),
    };
    let spawn = |z| match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, z),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected cow spawn, got {other:?}"),
    };
    let spear_only = spawn(7.5);
    let too_far = spawn(9.0);
    let authority = SimulationAuthority::for_test();

    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: spear_only,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: too_far,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::ValidationRejected
    ));
}

#[test]
fn direct_player_melee_kill_pushes_one_authoritative_script_event() {
    fn attack_costs(state: &PlayerPersistedState, position: Vec3) -> PlayerSurvivalPlan {
        let mut updated_survival = state.survival;
        updated_survival.add_exhaustion(SurvivalState::ENTITY_ATTACK_EXHAUSTION);
        PlayerSurvivalPlan {
            expected_survival: state.survival,
            updated_survival,
            expected_inventory: state.inventory.clone(),
            updated_inventory: state.inventory.clone(),
            expected_carried_item: state.carried_item.clone(),
            expected_xp: state.xp.clone(),
            updated_xp: state.xp.clone(),
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            position,
        }
    }

    let registry = SessionRegistry::new();
    let mut events = registry.install_script_commit_event_outbox();
    let session_id = register_test_session(&registry, "EntityKillAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let persisted = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session_id, Arc::clone(&persisted));
    let spawn = |position| {
        let before = registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot.id)
            .collect::<HashSet<_>>();
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            5,
            "minecraft:cow".to_owned(),
            position,
        );
        registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot.id)
            .find(|id| !before.contains(id))
            .expect("spawned cow remains authoritative")
    };
    let nonlethal = spawn(Vec3::new(0.5, 64.0, 1.5));
    let lethal = spawn(Vec3::new(1.5, 64.0, 1.5));
    let unreachable = spawn(Vec3::new(0.5, 64.0, 20.5));
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let authority = SimulationAuthority::for_test();
    let next_costs = || {
        let state = persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        attack_costs(&state, Vec3::new(pose.x, pose.y, pose.z))
    };

    let costs = next_costs();
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: nonlethal,
                amount: 1.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::Damaged { .. })
    ));
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    let stale_costs = next_costs();
    persisted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .survival
        .add_exhaustion(0.25);
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: lethal,
                amount: 100.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &stale_costs)),
            },
        ),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    let _ = registry.update_pose(session_id, PlayerPose::new(4.5, 64.0, 0.5));
    let costs = next_costs();
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: unreachable,
                amount: 100.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    let costs = next_costs();
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: lethal,
                amount: 100.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::Killed { .. })
    ));
    let event = events.try_recv().expect("lethal melee commit event");
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerEntityKilled {
            player_id,
            context,
            dimension,
            entity_id,
            entity_type,
            source,
            game_mode: ScriptGameMode::Survival,
        } if player_id.value() == session_id
            && context.username() == "EntityKillAlice"
            && (context.x(), context.y(), context.z()) == (pose.x, pose.y, pose.z)
            && dimension == "minecraft:overworld"
            && entity_id.value() == u64::try_from(lethal.0).unwrap()
            && entity_type == "minecraft:cow"
            && source.as_str() == "melee"
    ));

    let costs = next_costs();
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: lethal,
                amount: 100.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    drop(events);
    let closed_outbox_target = spawn(Vec3::new(1.5, 64.0, 1.5));
    let costs = next_costs();
    assert!(matches!(
        registry.player_attack_server_entity(
            &authority,
            ServerEntityPlayerAttack {
                entity_id: closed_outbox_target,
                amount: 100.0,
                game_mode: GameMode::Survival,
                player_pose: pose,
                attacker: Some((session_id, &costs)),
            },
        ),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::Killed { .. })
    ));
}

#[test]
fn lethal_player_attack_spawns_every_simple_entity_loot_pool() {
    let registry = SessionRegistry::new();
    let cow = Identifier::parse("minecraft:cow").unwrap();
    let leather = Identifier::parse("minecraft:leather").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: leather.clone(),
            protocol_id: 41,
        },
        mc_data::items::ItemReport {
            id: beef.clone(),
            protocol_id: 42,
        },
    ]));
    let loot = Arc::new(mc_data::loot::LootTables::from_drop_lists(
        BTreeMap::from([(
            cow,
            vec![
                mc_data::loot::LootDrop::uniform(leather, 1, 2),
                mc_data::loot::LootDrop::uniform(beef, 1, 3),
            ],
        )]),
        BTreeMap::new(),
    ));
    registry.configure_arrow_kill_rewards(
        Some(98),
        Some(99),
        None,
        items,
        Arc::new(ItemFactsTable::default()),
        loot,
    );
    let session_id = register_test_session(&registry, "CowLootAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let cow_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected cow spawn dispatch, got {other:?}"),
    };

    let PlayerAttackResult::Damaged(outcome) = registry.player_attack_server_entity(
        &SimulationAuthority::for_test(),
        ServerEntityPlayerAttack {
            entity_id: cow_id,
            amount: 100.0,
            game_mode: GameMode::Survival,
            player_pose: PlayerPose::new(0.5, 64.0, 0.5),
            attacker: None,
        },
    ) else {
        panic!("reachable cow attack must damage");
    };
    let EntityAttackOutcome::Killed { dispatches, .. } = *outcome else {
        panic!("lethal cow attack must kill");
    };
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::PickupCandidates(candidates)
                if candidates
                    .iter()
                    .any(|entity| entity.experience_value.is_some_and(|value| value > 0))
        )
    }));
    let drops = dispatches
        .into_iter()
        .filter_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                entity.item_stack
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(drops.len(), 2);
    assert!(
        drops
            .iter()
            .any(|drop| { drop.item_id == 41 && (1..=2).contains(&drop.count) })
    );
    assert!(
        drops
            .iter()
            .any(|drop| { drop.item_id == 42 && (1..=3).contains(&drop.count) })
    );
}

#[test]
fn explosion_entity_targets_capture_living_entity_geometry() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ExplosionTargetObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let chicken_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_string(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    let baby_chicken_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_string(),
        Vec3::new(2.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected baby chicken spawn dispatch, got {other:?}"),
    };
    {
        let mut inner = registry.lock_session_entities("mark explosion target as baby");
        assert!(
            inner
                .entities
                .set_animal_state(baby_chicken_id, mc_entity::AnimalBreedingState::baby())
        );
    }

    let targets = registry.explosion_entity_targets(
        &SimulationAuthority::for_test(),
        Vec3::new(0.5, 64.06125, 0.5),
        8.0,
    );

    let target = targets
        .iter()
        .find(|target| target.entity_id == chicken_id)
        .expect("chicken inside explosion radius");
    assert_eq!(target.position, Vec3::new(1.5, 64.0, 0.5));
    let adult_half_width = f64::from(0.2_f32);
    let adult_height = f64::from(0.7_f32);
    let adult_eye_height = f64::from(0.644_f32);
    assert_eq!(
        target.eye_position,
        Vec3::new(1.5, 64.0 + adult_eye_height, 0.5)
    );
    assert_eq!(
        target.aabb_min,
        Vec3::new(1.5 - adult_half_width, 64.0, 0.5 - adult_half_width)
    );
    assert_eq!(
        target.aabb_max,
        Vec3::new(
            1.5 + adult_half_width,
            64.0 + adult_height,
            0.5 + adult_half_width
        )
    );

    let baby_target = targets
        .iter()
        .find(|target| target.entity_id == baby_chicken_id)
        .expect("baby chicken inside explosion radius");
    assert_eq!(baby_target.eye_position, Vec3::new(2.5, 64.28, 0.5));
    assert_eq!(baby_target.aabb_min, Vec3::new(2.35, 64.0, 0.35));
    assert_eq!(baby_target.aabb_max, Vec3::new(2.65, 64.4, 0.65));
}

#[test]
fn physics_queries_keep_adult_and_baby_geometry_separate() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "BabyPhysicsObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn_chicken = |position| match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_string(),
        position,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    let adult_id = spawn_chicken(Vec3::new(1.5, 64.0, 0.5));
    let baby_id = spawn_chicken(Vec3::new(2.5, 64.0, 0.5));
    {
        let mut inner = registry.lock_session_entities("mark physics target as baby");
        assert!(
            inner
                .entities
                .set_animal_state(baby_id, mc_entity::AnimalBreedingState::baby())
        );
    }

    let queries = registry.tick_entities_and_collect_physics_queries(1);
    let aabb = |id| {
        queries
            .iter()
            .find(|query| query.id == id)
            .map(|query| query.aabb)
            .expect("chicken physics query")
    };
    assert_eq!(
        aabb(adult_id),
        mc_physics::Aabb {
            half_width: f64::from(0.2_f32),
            height: f64::from(0.7_f32),
        }
    );
    assert_eq!(
        aabb(baby_id),
        mc_physics::Aabb {
            half_width: 0.15,
            height: 0.4,
        }
    );
}

#[test]
fn explosion_entity_impacts_publish_hurt_before_exact_velocity_delta() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ExplosionObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let chicken_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_string(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };

    let dispatches = registry.apply_explosion_entity_impacts(
        &SimulationAuthority::for_test(),
        &[ServerEntityExplosionImpact {
            entity_id: chicken_id,
            damage: 2.0,
            knockback: Vec3::new(0.4, 0.2, -0.1),
        }],
    );

    let inner = registry.lock_session_entities("inspect explosion damage");
    let chicken = inner
        .entities
        .snapshot(chicken_id)
        .expect("non-lethal explosion keeps chicken alive");
    assert_eq!(chicken.health, 2.0);
    assert_eq!(chicken.velocity, Vec3::new(0.4, 0.2, -0.1));
    drop(inner);
    let hurt_index = dispatches
        .iter()
        .position(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::EntityEvent {
                    entity_id,
                    event_id: 2,
                } if entity_id == chicken_id.0
            )
        })
        .expect("surviving chicken hurt event");
    let velocity_index = dispatches
        .iter()
        .position(|dispatch| {
            matches!(
                &dispatch.command,
                OutboundCommand::MoveEntityRelative(movement)
                    if movement.id == chicken_id
                        && movement.velocity == Vec3::new(0.4, 0.2, -0.1)
                        && movement.send_velocity
            )
        })
        .expect("surviving chicken velocity update");
    assert!(hurt_index < velocity_index);
}

#[test]
fn lethal_attack_keeps_dying_entity_until_twentieth_death_tick() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "DeathLifecycleObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let chicken_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_string(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };

    let outcome = registry
        .attack_server_entity(
            &SimulationAuthority::for_test(),
            chicken_id,
            100.0,
            None,
            &EntityKillRewards::default(),
        )
        .expect("lethal chicken attack");
    let EntityAttackOutcome::Killed { dispatches, .. } = outcome else {
        panic!("lethal attack must enter killed outcome");
    };
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::EntityEvent {
                entity_id,
                event_id: 3,
            } if entity_id == chicken_id.0
        )
    }));
    assert!(!dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::DespawnEntity(ref entity) if entity.id == chicken_id
        )
    }));
    let dying = registry
        .lock_entities("inspect dying chicken")
        .snapshot(chicken_id)
        .expect("dying chicken remains authoritative");
    assert_eq!(dying.lifecycle, EntityLifecycle::Despawning);
    assert_eq!(dying.retained.last_damage_tick, Some(0));
    assert_eq!(dying.retained.death_remove_tick, Some(20));

    registry.advance_world_time(19);
    assert!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 19)
            .is_empty()
    );
    assert!(registry.server_entity_snapshot(chicken_id).is_some());
    assert_eq!(
        registry
            .lock_entities("inspect pending death deadline")
            .snapshot(chicken_id)
            .expect("dying chicken remains before its deadline")
            .retained
            .death_remove_tick,
        Some(20)
    );

    registry.advance_world_time(1);
    let dispatches = registry.tick_dying_entities(&SimulationAuthority::for_test(), 20);
    let final_event = dispatches
        .iter()
        .position(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::EntityEvent {
                    entity_id,
                    event_id: 60,
                } if entity_id == chicken_id.0
            )
        })
        .expect("final death event");
    let removal = dispatches
        .iter()
        .position(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::DespawnEntity(ref entity) if entity.id == chicken_id
            )
        })
        .expect("dying chicken removal");
    assert!(final_event < removal);
    assert!(registry.server_entity_snapshot(chicken_id).is_none());
}

#[test]
fn lethal_player_attack_on_sheared_sheep_keeps_meat_and_removes_wool() {
    let registry = SessionRegistry::new();
    let sheep = Identifier::parse("minecraft:sheep").unwrap();
    let white_wool = Identifier::parse("minecraft:white_wool").unwrap();
    let mutton = Identifier::parse("minecraft:mutton").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: white_wool.clone(),
            protocol_id: 41,
        },
        mc_data::items::ItemReport {
            id: mutton.clone(),
            protocol_id: 42,
        },
    ]));
    let loot = Arc::new(mc_data::loot::LootTables::from_drop_lists(
        BTreeMap::from([(
            sheep,
            vec![
                mc_data::loot::LootDrop::single(white_wool),
                mc_data::loot::LootDrop::single(mutton),
            ],
        )]),
        BTreeMap::new(),
    ));
    registry.configure_arrow_kill_rewards(
        Some(98),
        None,
        None,
        items,
        Arc::new(ItemFactsTable::default()),
        loot,
    );
    let session_id = register_test_session(&registry, "ShearedSheepLootAlice");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let sheep_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:sheep".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected sheep spawn dispatch, got {other:?}"),
    };
    {
        let mut inner = registry.lock_session_entities("mark test sheep sheared");
        let mut animal = inner
            .entities
            .snapshot(sheep_id)
            .and_then(|entity| entity.animal)
            .expect("spawned sheep has wool state");
        let mut wool = animal.sheep_wool.expect("spawned sheep has wool data");
        wool.sheared = true;
        animal.sheep_wool = Some(wool);
        assert!(inner.entities.set_animal_state(sheep_id, animal));
    }

    let PlayerAttackResult::Damaged(outcome) = registry.player_attack_server_entity(
        &SimulationAuthority::for_test(),
        ServerEntityPlayerAttack {
            entity_id: sheep_id,
            amount: 100.0,
            game_mode: GameMode::Survival,
            player_pose: PlayerPose::new(0.5, 64.0, 0.5),
            attacker: None,
        },
    ) else {
        panic!("reachable sheep attack must damage");
    };
    let EntityAttackOutcome::Killed { dispatches, .. } = *outcome else {
        panic!("lethal sheep attack must kill");
    };
    let drops = dispatches
        .into_iter()
        .filter_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                entity.item_stack
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(drops, vec![EntityItemStack::new(42, 1)]);
}

#[test]
fn entity_kill_drops_use_authoritative_sheep_color() {
    let sheep = Identifier::parse("minecraft:sheep").unwrap();
    let white_wool = Identifier::parse("minecraft:white_wool").unwrap();
    let brown_wool = Identifier::parse("minecraft:brown_wool").unwrap();
    let mutton = Identifier::parse("minecraft:mutton").unwrap();
    let config = ArrowKillRewards {
        items: Some(Arc::new(ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: white_wool.clone(),
                protocol_id: 41,
            },
            mc_data::items::ItemReport {
                id: mutton.clone(),
                protocol_id: 42,
            },
            mc_data::items::ItemReport {
                id: brown_wool,
                protocol_id: 43,
            },
        ]))),
        item_facts: Some(Arc::new(ItemFactsTable::default())),
        loot: Some(Arc::new(mc_data::loot::LootTables::from_drop_lists(
            BTreeMap::from([(
                sheep,
                vec![
                    mc_data::loot::LootDrop::single(white_wool),
                    mc_data::loot::LootDrop::single(mutton),
                ],
            )]),
            BTreeMap::new(),
        ))),
        ..ArrowKillRewards::default()
    };

    let drops = entity_kill_drop_stacks(
        &config,
        "minecraft:sheep",
        Some(mc_entity::AnimalBreedingState::adult_sheep(
            mc_entity::SheepColor::Brown,
        )),
        0,
    );

    assert_eq!(drops, vec![ItemStack::new(42, 1), ItemStack::new(43, 1)]);
}

#[test]
fn moving_arrow_ignores_item_entities() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowHitBob");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    registry.spawn_item_drop(2, Vec3::new(0.5, 64.0, 1.5), EntityItemStack::new(42, 1));
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_some());
}

#[test]
fn moving_arrow_does_not_hit_entity_behind_authoritative_block_endpoint() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowBlockEndpointObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let target_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected target spawn dispatch, got {other:?}"),
    };
    let target_health = server_entity_health(&registry, target_id);
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
        &[ArrowPhysicsFact {
            arrow_id,
            block_hit: Some(ArrowBlockHitFact {
                arrow_id,
                block_state: mc_world::BlockStateId(7),
                block_position: mc_entity::projectile_26_1_2::BlockPosition::new(0, 64, 0),
                location: Vec3::new(0.5, 64.75, 0.75),
            }),
            embedded_in_block: true,
            current_block_state: mc_world::BlockStateId(7),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_some());
    assert_eq!(server_entity_health(&registry, target_id), target_health);
}

#[test]
fn moving_arrow_tied_with_authoritative_block_endpoint_hits_block() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowBlockTieObserver");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let target_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected target spawn dispatch, got {other:?}"),
    };
    let target_health = server_entity_health(&registry, target_id);
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
        &[ArrowPhysicsFact {
            arrow_id,
            block_hit: Some(ArrowBlockHitFact {
                arrow_id,
                block_state: mc_world::BlockStateId(7),
                block_position: mc_entity::projectile_26_1_2::BlockPosition::new(0, 64, 0),
                // A cow at z=1.5 has an expanded ray intersection at z=0.8.
                location: Vec3::new(0.5, 64.75, 0.8),
            }),
            embedded_in_block: true,
            current_block_state: mc_world::BlockStateId(7),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        }],
    );

    assert_eq!(server_entity_health(&registry, target_id), target_health);
    assert_eq!(
        registry
            .server_entity_snapshot(arrow_id)
            .expect("block-hit arrow remains authoritative")
            .position,
        Vec3::new(0.5, 64.75, 0.8 - f64::from(0.05_f32))
    );
    assert_eq!(
        registry
            .lock_entities("inspect tied block arrow state")
            .snapshot(arrow_id)
            .and_then(|snapshot| snapshot.retained.arrow_state)
            .and_then(|state| state.last_block_position),
        Some(mc_entity::projectile_26_1_2::BlockPosition::new(0, 64, 0))
    );
}

#[test]
fn moving_arrow_deduplicates_complete_chunk_candidates_before_kernel_prepare() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowDuplicateCandidates");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    assert!(registry.mark_loaded(session_id, (0, 1)).is_empty());
    let target_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 16.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected target spawn dispatch, got {other:?}"),
    };
    for z in [64.5, 80.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            2,
            "minecraft:cow".to_string(),
            Vec3::new(0.5, 64.0, z),
        );
    }
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 18.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    let target_health = server_entity_health(&registry, target_id);
    registry
        .lock_inner("duplicate arrow candidate across chunks")
        .entities_by_chunk
        .entry((0, 0))
        .or_default()
        .insert(target_id);

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 18.0),
            velocity: Vec3::new(0.0, 0.0, 18.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        server_entity_health(&registry, target_id),
        target_health - ARROW_ENTITY_HIT_DAMAGE
    );
    assert!(registry.server_entity_snapshot(arrow_id).is_none());
}

#[test]
fn piercing_arrow_damages_only_accepted_targets_at_limit_boundary() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "ArrowPiercingOrder");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 0.1),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 0.1),
            velocity: Vec3::new(0.0, 0.0, 0.1),
            on_ground: false,
            horizontal_collision: false,
        }],
    );
    {
        let mut inner = registry.lock_session_entities("set arrow piercing regression state");
        let expected = inner
            .entities
            .snapshot(arrow_id)
            .expect("spawned arrow is owned by the regional ECS");
        let mut next = expected.clone();
        next.retained
            .arrow_state
            .as_mut()
            .expect("spawned arrow initializes ECS kernel state")
            .pierce_level = 1;
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }
    let first_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected first target spawn dispatch, got {other:?}"),
    };
    let second_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 3.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected second target spawn dispatch, got {other:?}"),
    };
    let third_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 4.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected third target spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        2,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 5.0),
            velocity: Vec3::new(0.0, 0.0, 4.9),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(server_entity_health(&registry, first_id), 6.0);
    assert_eq!(server_entity_health(&registry, second_id), 6.0);
    assert_eq!(server_entity_health(&registry, third_id), 10.0);
    assert!(registry.server_entity_snapshot(arrow_id).is_none());
}

#[test]
fn stale_arrow_physics_does_not_commit_or_publish_projectile_work() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (session_id, _) = registry.register(
        &profile("StaleArrowPhysics"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let initial_position = Vec3::new(0.5, 64.75, 0.0);
    let initial_velocity = Vec3::new(0.0, 0.0, 2.0);
    let spawn_dispatches = registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        initial_position,
        initial_velocity,
        Rotation::ZERO,
    );
    let arrow_id = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    dispatch_visibility_commands(spawn_dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    let expected = [EntityPhysicsQuery {
        id: arrow_id,
        position: initial_position,
        velocity: initial_velocity,
        aabb: entity_aabb("minecraft:arrow"),
        on_ground: false,
        kind: EntityPhysicsKind::ArrowProjectile {
            revision: Some(99),
            embedded_block: None,
        },
    }];

    registry.apply_entity_physics_if_current_and_dispatch(
        1,
        &expected,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: initial_velocity,
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        registry
            .server_entity_snapshot(arrow_id)
            .expect("stale arrow remains authoritative")
            .position,
        initial_position
    );
    let received = rx.try_recv();
    assert!(
        matches!(received, Err(mpsc::error::TryRecvError::Empty)),
        "stale arrow physics published {received:?}"
    );
}

#[test]
fn removed_arrow_target_is_not_resolved_from_stale_tracking() {
    let registry = SessionRegistry::new();
    let session_id = register_test_session(&registry, "RemovedArrowTarget");
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let target_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected target spawn dispatch, got {other:?}"),
    };
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    {
        let mut inner = registry.lock_session_entities("remove stale arrow target");
        assert!(remove_server_entity_locked(&mut inner, target_id).is_some());
    }

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(target_id).is_none());
    assert!(registry.server_entity_snapshot(arrow_id).is_some());
}

#[test]
fn arrow_candidate_capacity_rejection_leaves_state_unchanged_and_emits_nothing() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(512);
    let (session_id, _) = registry.register(
        &profile("ArrowPublicationCapacity"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let mut target_ids = Vec::new();
    for index in 0..200 {
        let z = 1.0 + f64::from(index) * 0.05;
        let spawn_dispatches = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            41,
            "minecraft:enderman".to_string(),
            Vec3::new(0.5, 64.0, z),
        );
        let target_id = match &spawn_dispatches[0].command {
            OutboundCommand::SpawnEntity(entity) => entity.id,
            other => panic!("expected enderman spawn dispatch, got {other:?}"),
        };
        dispatch_visibility_commands(spawn_dispatches);
        target_ids.push(target_id);
    }
    let initial_position = Vec3::new(0.5, 64.75, 0.0);
    let initial_velocity = Vec3::new(0.0, 0.0, 12.0);
    let spawn_dispatches = registry.spawn_arrow_for_test(
        Some(session_id),
        1,
        initial_position,
        initial_velocity,
        Rotation::ZERO,
    );
    let arrow_id = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    dispatch_visibility_commands(spawn_dispatches);
    let first_health = server_entity_health(&registry, target_ids[0]);
    let last_health = server_entity_health(&registry, *target_ids.last().expect("targets exist"));
    for _ in 0..=target_ids.len() {
        assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    }
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 12.0),
            velocity: initial_velocity,
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    let arrow = registry
        .server_entity_snapshot(arrow_id)
        .expect("capacity rejection retains arrow");
    assert_eq!(arrow.position, initial_position);
    assert_eq!(arrow.velocity, initial_velocity);
    assert_eq!(server_entity_health(&registry, target_ids[0]), first_health);
    assert_eq!(
        server_entity_health(&registry, *target_ids.last().expect("targets exist")),
        last_health
    );
    let arrow = registry
        .lock_entities("inspect rejected arrow state")
        .snapshot(arrow_id)
        .expect("capacity rejection retains arrow");
    assert_eq!(
        arrow
            .retained
            .arrow_state
            .expect("arrow state remains owned by ECS")
            .projectile
            .revision,
        0
    );
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

fn spawn_separated_arrows(registry: &SessionRegistry, count: usize) -> Vec<EntityId> {
    let mut inner = registry.lock_session_entities("spawn saturation arrows");
    (0..count)
        .map(|index| {
            spawn_arrow_locked(
                &mut inner,
                None,
                1,
                Vec3::new(index as f64 * 4.0 + 0.5, 64.0, 0.5),
                Vec3::new(0.0, 0.0, 0.1),
                Rotation::ZERO,
            )
            .0
        })
        .collect()
}

fn current_arrow_steps(
    registry: &SessionRegistry,
    arrow_ids: &[EntityId],
) -> Vec<EntityPhysicsStep> {
    let entities = registry.lock_entities("prepare saturation arrow steps");
    arrow_ids
        .iter()
        .map(|&id| {
            let snapshot = entities.snapshot(id).expect("saturation arrow exists");
            EntityPhysicsStep {
                id,
                position: Vec3::new(
                    snapshot.position.x + snapshot.velocity.x,
                    snapshot.position.y + snapshot.velocity.y,
                    snapshot.position.z + snapshot.velocity.z,
                ),
                velocity: snapshot.velocity,
                on_ground: false,
                horizontal_collision: false,
            }
        })
        .collect()
}

fn arrow_revisions(registry: &SessionRegistry, arrow_ids: &[EntityId]) -> Vec<u64> {
    let entities = registry.lock_entities("inspect saturation arrow revisions");
    arrow_ids
        .iter()
        .map(|&id| {
            entities
                .snapshot(id)
                .and_then(|snapshot| snapshot.retained.arrow_state)
                .expect("saturation arrow state remains ECS-owned")
                .projectile
                .revision
        })
        .collect()
}

#[test]
fn arrow_batch_at_129_advances_every_arrow() {
    let registry = SessionRegistry::new();
    let arrow_ids = spawn_separated_arrows(&registry, 129);

    registry.apply_entity_physics_and_dispatch(1, &current_arrow_steps(&registry, &arrow_ids));

    assert!(
        arrow_revisions(&registry, &arrow_ids)
            .iter()
            .all(|&revision| revision == 1)
    );
}

#[test]
fn arrow_batch_at_130_defers_one_then_services_it() {
    let registry = SessionRegistry::new();
    let arrow_ids = spawn_separated_arrows(&registry, 130);

    registry.apply_entity_physics_and_dispatch(1, &current_arrow_steps(&registry, &arrow_ids));
    let first_revisions = arrow_revisions(&registry, &arrow_ids);
    assert_eq!(
        first_revisions
            .iter()
            .filter(|&&revision| revision == 1)
            .count(),
        129
    );
    assert_eq!(
        first_revisions
            .iter()
            .filter(|&&revision| revision == 0)
            .count(),
        1
    );

    registry.apply_entity_physics_and_dispatch(2, &current_arrow_steps(&registry, &arrow_ids));

    assert!(
        arrow_revisions(&registry, &arrow_ids)
            .iter()
            .all(|&revision| revision >= 1)
    );
}

#[test]
fn saturated_arrow_batches_make_bounded_fair_progress() {
    const ARROW_COUNT: usize = 400;
    let registry = SessionRegistry::new();
    let arrow_ids = spawn_separated_arrows(&registry, ARROW_COUNT);

    for tick in 1..=ARROW_COUNT.div_ceil(129) as u64 {
        let before = arrow_revisions(&registry, &arrow_ids);
        registry
            .apply_entity_physics_and_dispatch(tick, &current_arrow_steps(&registry, &arrow_ids));
        let after = arrow_revisions(&registry, &arrow_ids);
        let advanced = before
            .iter()
            .zip(&after)
            .filter(|(before, after)| after > before)
            .count();
        assert!(
            (1..=129).contains(&advanced),
            "tick {tick} advanced {advanced} arrows"
        );
    }

    assert!(
        arrow_revisions(&registry, &arrow_ids)
            .iter()
            .all(|&revision| revision >= 1)
    );
}

#[test]
fn moving_arrow_damages_player_target_but_not_owner() {
    let registry = SessionRegistry::new();
    let (owner_tx, mut owner_rx) = mpsc::channel(8);
    let (target_tx, mut target_rx) = mpsc::channel(8);
    let (owner_id, _) = registry.register(
        &profile("ArrowPlayerOwner"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.0),
    );
    let (target_id, _) = registry.register(
        &profile("ArrowPlayerTarget"),
        (0, 0),
        2,
        HashSet::new(),
        target_tx,
        PlayerPose::new(0.5, 64.0, 1.5),
    );
    registry.mark_loaded(owner_id, (0, 0));
    registry.mark_loaded(target_id, (0, 0));
    registry.register_player_persistence(
        owner_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.0),
        ))),
    );
    let target_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 1.5),
    )));
    registry.register_player_persistence(target_id, Arc::clone(&target_state));
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_none());
    let mut target_damaged = false;
    while let Ok(command) = target_rx.try_recv() {
        target_damaged |= matches!(command, OutboundCommand::PlayerDamageCommitted { .. });
    }
    let mut owner_damaged = false;
    while let Ok(command) = owner_rx.try_recv() {
        owner_damaged |= matches!(command, OutboundCommand::PlayerDamageCommitted { .. });
    }
    assert!(target_damaged);
    assert!(!owner_damaged);
    assert_eq!(
        target_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .survival
            .health,
        SurvivalState::MAX_HEALTH - ARROW_ENTITY_HIT_DAMAGE
    );
}

#[test]
fn shielded_player_hit_commits_shield_and_arrow_state_together() {
    let registry = SessionRegistry::new();
    let shield_name = mc_data::Identifier::parse("minecraft:shield").unwrap();
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: shield_name.clone(),
            protocol_id: 1,
        },
    ]));
    let item_facts = Arc::new(mc_data::item_components::ItemFactsTable::from_entries([(
        shield_name,
        mc_data::item_components::ItemFacts {
            max_damage: Some(336),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    registry.configure_player_combat(None, None, items, item_facts);

    let (owner_tx, _owner_rx) = mpsc::channel(8);
    let (target_tx, _target_rx) = mpsc::channel(8);
    let (owner, _) = registry.register(
        &profile("ArrowShieldOwner"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.0),
    );
    let mut target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    target_pose.yaw = 180.0;
    let (target, _) = registry.register(
        &profile("ArrowShieldTarget"),
        (0, 0),
        2,
        HashSet::new(),
        target_tx,
        target_pose,
    );
    registry.mark_loaded(owner, (0, 0));
    registry.mark_loaded(target, (0, 0));
    let shield_slot = 45;
    let shield = ItemStack::new(1, 1);
    let mut persisted = PlayerPersistedState::new_default(target_pose);
    persisted.inventory.slots[shield_slot] = shield.clone();
    let target_state = Arc::new(Mutex::new(persisted));
    registry.register_player_persistence(target, Arc::clone(&target_state));
    registry.set_active_shield(
        target,
        Some(ActiveShield {
            started_tick: 0,
            slot: shield_slot,
            expected_stack: shield,
        }),
    );
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    let (target_tx, mut target_rx) = mpsc::channel(8);
    registry
        .lock_inner("isolate shielded arrow publications")
        .sessions
        .get_mut(&target)
        .expect("shield target remains registered")
        .tx = target_tx;

    registry.apply_entity_physics_and_dispatch(
        10,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_some());
    let state = target_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.survival.health, SurvivalState::MAX_HEALTH);
    assert_eq!(
        state.inventory.slots[shield_slot],
        ItemStack::new(1, 1).with_damage(5)
    );
    drop(state);
    let command = target_rx
        .try_recv()
        .expect("shield commit publishes one authoritative player decision");
    assert!(
        matches!(
            &command,
            OutboundCommand::PlayerDamageCommitted { publication, .. }
                if publication.shield_blocked && publication.health == SurvivalState::MAX_HEALTH
        ),
        "unexpected shield publication: {command:?}"
    );
    assert!(matches!(
        target_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn stale_player_plan_rejects_arrow_and_prior_entity_targets_atomically() {
    let registry = Arc::new(SessionRegistry::new());
    let (owner_tx, _owner_rx) = mpsc::channel(16);
    let (target_tx, _target_rx) = mpsc::channel(16);
    let (owner, _) = registry.register(
        &profile("ArrowAtomicOwner"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.0),
    );
    let target_pose = PlayerPose::new(0.5, 64.0, 3.5);
    let (target, _) = registry.register(
        &profile("ArrowAtomicTarget"),
        (0, 0),
        2,
        HashSet::new(),
        target_tx,
        target_pose,
    );
    registry.mark_loaded(owner, (0, 0));
    registry.mark_loaded(target, (0, 0));
    let target_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(target_pose)));
    registry.register_player_persistence(target, Arc::clone(&target_state));
    let cow_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected cow spawn dispatch, got {other:?}"),
    };
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 5.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    let (arrow_before, cow_health) = {
        let mut inner = registry.lock_session_entities("seed atomic arrow transaction");
        let expected = inner.entities.snapshot(arrow_id).expect("spawned arrow");
        let mut next = expected.clone();
        next.retained
            .arrow_state
            .as_mut()
            .expect("spawned arrow has ECS kernel state")
            .pierce_level = 1;
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(expected, next.clone())
        );
        (
            next,
            inner.entities.snapshot(cow_id).expect("spawned cow").health,
        )
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_arrow_transaction_probe(reached_tx, resume_rx);
    let (owner_tx, mut owner_rx) = mpsc::channel(16);
    let (target_tx, mut target_rx) = mpsc::channel(16);
    {
        let mut inner = registry.lock_inner("isolate rejected arrow publications");
        inner
            .sessions
            .get_mut(&owner)
            .expect("arrow owner remains registered")
            .tx = owner_tx;
        inner
            .sessions
            .get_mut(&target)
            .expect("arrow target remains registered")
            .tx = target_tx;
    }
    let tick_registry = Arc::clone(&registry);
    let tick = std::thread::spawn(move || {
        tick_registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: arrow_id,
                position: Vec3::new(0.5, 64.75, 5.0),
                velocity: Vec3::new(0.0, 0.0, 5.0),
                on_ground: false,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv()
        .expect("arrow preparation reaches transaction boundary");
    target_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .survival
        .apply_damage(1.0);
    resume_tx.send(()).expect("release arrow transaction");
    tick.join().expect("arrow transaction worker joins");

    assert_eq!(
        registry
            .lock_entities("inspect atomically rejected arrow")
            .snapshot(arrow_id),
        Some(arrow_before)
    );
    assert_eq!(server_entity_health(&registry, cow_id), cow_health);
    assert_eq!(
        target_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .survival
            .health,
        SurvivalState::MAX_HEALTH - 1.0
    );
    assert!(matches!(
        owner_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        target_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn stale_entity_target_cas_rejects_arrow_and_all_target_damage() {
    let registry = Arc::new(SessionRegistry::new());
    let (owner_tx, _owner_rx) = mpsc::channel(16);
    let (owner, _) = registry.register(
        &profile("ArrowEntityCasOwner"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.0),
    );
    assert!(registry.mark_loaded(owner, (0, 0)).is_empty());
    let first_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected first cow spawn dispatch, got {other:?}"),
    };
    let stale_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 3.0),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected stale cow spawn dispatch, got {other:?}"),
    };
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 5.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    let (arrow_before, first_before, stale_before) = {
        let mut inner = registry.lock_session_entities("seed stale entity CAS transaction");
        let expected = inner.entities.snapshot(arrow_id).expect("spawned arrow");
        let mut next = expected.clone();
        next.retained
            .arrow_state
            .as_mut()
            .expect("spawned arrow has ECS kernel state")
            .pierce_level = 1;
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(expected, next.clone())
        );
        (
            next,
            inner.entities.snapshot(first_id).expect("first cow"),
            inner.entities.snapshot(stale_id).expect("stale cow"),
        )
    };
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_arrow_transaction_probe(reached_tx, resume_rx);
    let (owner_tx, mut owner_rx) = mpsc::channel(16);
    registry
        .lock_inner("isolate stale entity CAS publications")
        .sessions
        .get_mut(&owner)
        .expect("arrow owner remains registered")
        .tx = owner_tx;
    let tick_registry = Arc::clone(&registry);
    let tick = std::thread::spawn(move || {
        tick_registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: arrow_id,
                position: Vec3::new(0.5, 64.75, 5.0),
                velocity: Vec3::new(0.0, 0.0, 5.0),
                on_ground: false,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv()
        .expect("arrow preparation reaches entity CAS boundary");
    {
        let mut entities = registry.lock_entities("change later arrow target before CAS");
        let expected = entities.snapshot(stale_id).expect("stale target remains");
        let mut next = expected.clone();
        next.health -= 1.0;
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    resume_tx.send(()).expect("release entity CAS transaction");
    tick.join().expect("entity CAS transaction worker joins");

    let entities = registry.lock_entities("inspect rejected entity CAS transaction");
    assert_eq!(entities.snapshot(arrow_id), Some(arrow_before));
    assert_eq!(entities.snapshot(first_id), Some(first_before));
    let mut expected_stale = stale_before;
    expected_stale.health -= 1.0;
    assert_eq!(entities.snapshot(stale_id), Some(expected_stale));
    drop(entities);
    assert!(matches!(
        owner_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn rejected_player_damage_preserves_projectile_for_invulnerable_targets() {
    for (name, mode, dead) in [
        ("ArrowCreativeTarget", GameMode::Creative, false),
        ("ArrowSpectatorTarget", GameMode::Spectator, false),
        ("ArrowDeadTarget", GameMode::Survival, true),
    ] {
        let registry = SessionRegistry::new();
        let (owner_tx, _owner_rx) = mpsc::channel(8);
        let (target_tx, mut target_rx) = mpsc::channel(8);
        let (owner, _) = registry.register(
            &profile(&format!("{name}Owner")),
            (0, 0),
            2,
            HashSet::new(),
            owner_tx,
            PlayerPose::new(0.5, 64.0, 0.0),
        );
        let (target, _) = registry.register(
            &profile(name),
            (0, 0),
            2,
            HashSet::new(),
            target_tx,
            PlayerPose::new(0.5, 64.0, 1.5),
        );
        registry.mark_loaded(owner, (0, 0));
        registry.mark_loaded(target, (0, 0));
        let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 1.5));
        persisted.game_mode = mode;
        if dead {
            persisted.survival.apply_damage(SurvivalState::MAX_HEALTH);
        }
        let target_state = Arc::new(Mutex::new(persisted));
        registry.register_player_persistence(target, Arc::clone(&target_state));
        let arrow_id = match &registry.spawn_arrow_for_test(
            Some(owner),
            1,
            Vec3::new(0.5, 64.75, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Rotation::ZERO,
        )[0]
        .command
        {
            OutboundCommand::SpawnEntity(entity) => entity.id,
            other => panic!("expected arrow spawn dispatch, got {other:?}"),
        };

        registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: arrow_id,
                position: Vec3::new(0.5, 64.75, 2.0),
                velocity: Vec3::new(0.0, 0.0, 2.0),
                on_ground: false,
                horizontal_collision: false,
            }],
        );

        assert!(
            registry.server_entity_snapshot(arrow_id).is_some(),
            "{name}"
        );
        assert!(
            !target_rx.try_recv().is_ok_and(|command| matches!(
                command,
                OutboundCommand::PlayerDamageCommitted { .. }
            )),
            "{name}"
        );
        let state = target_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.game_mode, mode, "{name}");
        assert_eq!(state.survival.is_dead(), dead, "{name}");
    }
}

#[test]
fn closed_player_publication_queue_does_not_create_false_projectile_rejection() {
    let registry = SessionRegistry::new();
    let (owner_tx, _owner_rx) = mpsc::channel(8);
    let (target_tx, target_rx) = mpsc::channel(1);
    let (owner, _) = registry.register(
        &profile("ArrowClosedQueueOwner"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.0),
    );
    let (target, _) = registry.register(
        &profile("ArrowClosedQueueTarget"),
        (0, 0),
        2,
        HashSet::new(),
        target_tx,
        PlayerPose::new(0.5, 64.0, 1.5),
    );
    drop(target_rx);
    registry.mark_loaded(owner, (0, 0));
    registry.mark_loaded(target, (0, 0));
    let target_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 1.5),
    )));
    registry.register_player_persistence(target, Arc::clone(&target_state));
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_none());
    assert_eq!(
        target_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .survival
            .health,
        SurvivalState::MAX_HEALTH - ARROW_ENTITY_HIT_DAMAGE
    );
}

#[test]
fn moving_arrow_does_not_hit_owner_without_other_targets() {
    let registry = SessionRegistry::new();
    let (owner_tx, mut owner_rx) = mpsc::channel(8);
    let (owner_id, _) = registry.register(
        &profile("ArrowOwnerSafe"),
        (0, 0),
        2,
        HashSet::new(),
        owner_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(owner_id, (0, 0)).is_empty());
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(owner_id),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 2.0),
            velocity: Vec3::new(0.0, 0.0, 2.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_some());
    while let Ok(command) = owner_rx.try_recv() {
        assert!(!matches!(command, OutboundCommand::DamagePlayer { .. }));
    }
}

#[test]
fn moving_arrow_skips_owner_root_vehicle_before_external_target() {
    let registry = SessionRegistry::new();
    let observer = register_test_session(&registry, "ArrowOwnerRootVehicle");
    assert!(registry.mark_loaded(observer, (0, 0)).is_empty());
    let owner_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 1.0),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected owner spawn dispatch, got {other:?}"),
    };
    let vehicle_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        8,
        "minecraft:oak_boat".to_string(),
        Vec3::new(0.5, 64.0, 1.0),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected vehicle spawn dispatch, got {other:?}"),
    };
    let target_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_string(),
        Vec3::new(0.5, 64.0, 2.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected target spawn dispatch, got {other:?}"),
    };
    let arrow_id = match &registry.spawn_arrow_for_test(
        Some(observer),
        1,
        Vec3::new(0.5, 64.75, 0.0),
        Vec3::new(0.0, 0.0, 3.0),
        Rotation::ZERO,
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected arrow spawn dispatch, got {other:?}"),
    };
    {
        let mut inner = registry.lock_session_entities("install ECS owner vehicle relation");
        let vehicle = inner.entities.snapshot(vehicle_id).expect("vehicle exists");
        let mut next_vehicle = vehicle.clone();
        next_vehicle.vehicle = Some(mc_entity::VehicleState {
            kind: mc_entity::VehicleKind::Boat,
            passenger: Some(owner_id),
        });
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(vehicle, next_vehicle)
        );
        let arrow = inner.entities.snapshot(arrow_id).expect("arrow exists");
        let mut next_arrow = arrow.clone();
        next_arrow
            .retained
            .arrow_state
            .as_mut()
            .expect("arrow state is ECS-owned")
            .projectile
            .owner = Some(projectiles::projectile_identity(owner_id));
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(arrow, next_arrow)
        );
    }
    let vehicle_health = server_entity_health(&registry, vehicle_id);

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.75, 3.0),
            velocity: Vec3::new(0.0, 0.0, 3.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert_eq!(server_entity_health(&registry, vehicle_id), vehicle_health);
    assert_eq!(server_entity_health(&registry, target_id), 6.0);
    assert!(registry.server_entity_snapshot(arrow_id).is_none());
}

fn registration<'a>(
    profile: &'a LoggedInProfile,
    tx: mpsc::Sender<OutboundCommand>,
    max_sessions: usize,
) -> SessionRegistration<'a> {
    SessionRegistration {
        profile,
        properties: &[],
        center: (0, 0),
        view_distance: 2,
        desired: HashSet::new(),
        tx,
        pose: PlayerPose::new(0.5, 64.0, 0.5),
        max_sessions,
        script_operator: false,
        dimension: "minecraft:overworld",
    }
}

#[test]
fn try_register_enforces_max_sessions() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let full_alice = profile("FullAlice");
    let first = registry.try_register(registration(&full_alice, tx, 1));
    assert!(first.is_ok());

    let (tx, _rx) = mpsc::channel(8);
    let full_bob = profile("FullBob");
    let second = registry.try_register(registration(&full_bob, tx, 1));
    assert!(matches!(
        second,
        Err(SessionAdmissionError::ServerFull { active: 1, max: 1 })
    ));
}

#[tokio::test]
async fn profile_properties_reach_observer_player_info_wire_packet() {
    let registry = SessionRegistry::new();
    let chunk = (0, 0);
    let (observer_tx, _observer_rx) = mpsc::channel(8);
    let (observer, _) = registry.register(
        &profile("ProfileObserver"),
        chunk,
        2,
        HashSet::from([chunk]),
        observer_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let property = GameProfileProperty {
        name: "textures".to_owned(),
        value: "signed-texture-value".to_owned(),
        signature: Some("signed-texture-signature".to_owned()),
    };
    let profiled_player = profile("ProfileOwner");
    let (profiled_tx, _profiled_rx) = mpsc::channel(8);
    let (profiled, _) = registry
        .try_register(SessionRegistration {
            profile: &profiled_player,
            properties: std::slice::from_ref(&property),
            center: chunk,
            view_distance: 2,
            desired: HashSet::from([chunk]),
            tx: profiled_tx,
            pose: PlayerPose::new(1.5, 64.0, 0.5),
            max_sessions: usize::MAX,
            script_operator: false,
            dimension: "minecraft:overworld",
        })
        .unwrap();

    let dispatches = registry.mark_loaded(observer, chunk);
    let snapshot = dispatches
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnPlayer(snapshot)
                if dispatch.recipient.id == observer && snapshot.session_id == profiled =>
            {
                Some(snapshot)
            }
            _ => None,
        })
        .expect("observer receives profiled player's spawn");

    let (mut server_io, client_io) = duplex(4_096);
    super::super::wire_entities::send_player_spawn(
        &mut server_io,
        Compression::Disabled,
        &snapshot,
    )
    .await
    .unwrap();
    let mut reader = ConnectionReader::new(client_io);
    let mut buf = BytesMut::new();
    let player_info = read_packet_with_timeout::<PlayerInfoUpdate, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Play,
        PRE_PLAY_READ_TIMEOUT,
    )
    .await
    .unwrap();

    assert_eq!(player_info.entries.len(), 1);
    assert_eq!(player_info.entries[0].properties, vec![property]);
}

#[test]
fn replace_view_updates_view_distance_and_tickets() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (id, _) = registry.register(
        &profile("ViewDistanceAlice"),
        (0, 0),
        3,
        HashSet::from([(0, 0), (1, 0), (2, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );

    registry.replace_view(id, (0, 0), 1, HashSet::from([(0, 0)]));

    let inner = registry.lock_inner("test replace view");
    let session = inner.sessions.get(&id).unwrap();
    assert_eq!(session.view_distance, 1);
    assert_eq!(session.desired, HashSet::from([(0, 0)]));
    assert!(inner.tickets.contains_key(&(0, 0)));
    assert!(!inner.tickets.contains_key(&(1, 0)));
    assert!(!inner.tickets.contains_key(&(2, 0)));
}

#[test]
fn try_register_rejects_duplicate_profile_until_unregister() {
    let registry = SessionRegistry::new();
    let first_id = register_test_session(&registry, "DupAlice");
    let (tx, _rx) = mpsc::channel(8);

    let dup_alice = profile("DupAlice");
    let duplicate = registry.try_register(registration(&dup_alice, tx, 8));

    assert!(matches!(
        duplicate,
        Err(SessionAdmissionError::DuplicateProfile { existing_session })
            if existing_session == first_id
    ));
    let _ = registry.unregister(first_id);

    let (tx, _rx) = mpsc::channel(8);
    let dup_alice = profile("DupAlice");
    assert!(
        registry
            .try_register(registration(&dup_alice, tx, 8))
            .is_ok()
    );
}

#[test]
fn same_chunk_pose_update_only_moves_existing_observers() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("MoveAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob_tx, _bob_rx) = mpsc::channel(8);
    let (bob, _) = registry.register(
        &profile("MoveBob"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        bob_tx,
        PlayerPose::new(1.5, 64.0, 0.5),
    );
    let _ = registry.mark_loaded(alice, (0, 0));
    let _ = registry.mark_loaded(bob, (0, 0));

    let dispatches = registry.update_pose(bob, PlayerPose::new(2.5, 64.0, 0.5));

    assert_eq!(dispatches.len(), 1);
    assert!(matches!(
        &dispatches[0].command,
        OutboundCommand::MovePlayer(PlayerEntitySnapshot { session_id, .. }) if *session_id == bob
    ));
}

#[test]
fn chunk_crossing_pose_update_diffs_target_observers() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("CrossAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob_tx, _bob_rx) = mpsc::channel(8);
    let (bob, _) = registry.register(
        &profile("CrossBob"),
        (0, 0),
        2,
        HashSet::from([(0, 0), (1, 0)]),
        bob_tx,
        PlayerPose::new(1.5, 64.0, 0.5),
    );
    let (charlie_tx, _charlie_rx) = mpsc::channel(8);
    let (charlie, _) = registry.register(
        &profile("CrossCharlie"),
        (1, 0),
        2,
        HashSet::from([(1, 0)]),
        charlie_tx,
        PlayerPose::new(16.5, 64.0, 0.5),
    );
    let _ = registry.mark_loaded(alice, (0, 0));
    let _ = registry.mark_loaded(bob, (0, 0));
    let _ = registry.mark_loaded(charlie, (1, 0));

    let dispatches = registry.update_pose(bob, PlayerPose::new(16.5, 64.0, 0.5));

    assert_eq!(dispatches.len(), 2);
    assert!(dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::DespawnPlayer(PlayerEntitySnapshot { session_id, .. })
            if *session_id == bob && dispatch.recipient.id == alice
    )));
    assert!(dispatches.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::SpawnPlayer(PlayerEntitySnapshot { session_id, .. })
            if *session_id == bob && dispatch.recipient.id == charlie
    )));
}

#[test]
fn spawned_item_drop_has_vanilla_style_horizontal_scatter() {
    let registry = SessionRegistry::new();
    registry.spawn_item_drop(1, Vec3::new(10.5, 81.5, -2.5), EntityItemStack::new(42, 1));

    let dropped = registry.persisted_entity_records();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].velocity.y, 0.2);
    assert!(dropped[0].velocity.x != 0.0);
    assert!(dropped[0].velocity.z != 0.0);
    assert!(dropped[0].velocity.x.abs() <= 0.1);
    assert!(dropped[0].velocity.z.abs() <= 0.1);
}

#[test]
fn item_pickup_claims_entity_once() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "PickupAlice");
    let bob = register_test_session(&registry, "PickupBob");
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );

    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;

    let claimed = registry
        .claim_item_pickup_for_test(entity_id, alice, 3)
        .unwrap();
    assert_eq!(claimed.stack, EntityItemStack::new(42, 3));
    assert!(
        registry
            .claim_item_pickup_for_test(entity_id, bob, 3)
            .is_none()
    );
    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
}

#[test]
fn concurrent_item_pickup_has_one_claimant_and_exact_stack() {
    let registry = Arc::new(SessionRegistry::new());
    let alice = register_test_session(&registry, "ConcurrentPickupAlice");
    let bob = register_test_session(&registry, "ConcurrentPickupBob");
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;
    let gate = Arc::new(Barrier::new(3));
    let handles = [alice, bob].map(|collector| {
        let registry = Arc::clone(&registry);
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            gate.wait();
            registry.claim_item_pickup_for_test(entity_id, collector, 3)
        })
    });

    gate.wait();
    let claims = handles
        .into_iter()
        .map(|handle| handle.join().expect("item claimant joins"))
        .collect::<Vec<_>>();
    let claimed = claims.iter().filter_map(Option::as_ref).collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].stack, EntityItemStack::new(42, 3));
    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
}

#[test]
fn item_pickup_preserves_damage_on_partial_claim() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "DamagePickupAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_item_drop(
        1,
        Vec3::new(0.5, 64.0, 0.5),
        EntityItemStack::new(42, 3).with_damage(17),
    );
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;
    let before_pickup = registry.pressure_snapshot().entity_dispatches;

    let claimed = registry
        .claim_item_pickup_for_test(entity_id, alice, 1)
        .unwrap();
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0]
        .item_stack
        .as_ref()
        .unwrap()
        .clone();
    let after_pickup = registry.pressure_snapshot().entity_dispatches;

    assert_eq!(claimed.stack, EntityItemStack::new(42, 1).with_damage(17));
    assert!(matches!(
        claimed.dispatches.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::UpdateEntityData(_),
            ..
        }]
    ));
    assert_eq!(remaining, EntityItemStack::new(42, 2).with_damage(17));
    assert_eq!(after_pickup.data, before_pickup.data + 1);
    assert_eq!(after_pickup.take, before_pickup.take);
    assert_eq!(after_pickup.remove, before_pickup.remove);
}

#[test]
fn item_pickup_counts_take_and_remove_per_observer() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "PickupCounterAlice");
    let bob = register_test_session(&registry, "PickupCounterBob");
    let _ = registry.mark_loaded(alice, (0, 0));
    let _ = registry.mark_loaded(bob, (0, 0));
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;
    let before_pickup = registry.pressure_snapshot().entity_dispatches;

    let claimed = registry
        .claim_item_pickup_for_test(entity_id, alice, 3)
        .unwrap();
    let after_pickup = registry.pressure_snapshot().entity_dispatches;

    assert_eq!(claimed.dispatches.len(), 4);
    assert_eq!(after_pickup.take, before_pickup.take + 2);
    assert_eq!(after_pickup.remove, before_pickup.remove + 2);
}

#[test]
fn item_drop_records_age_and_remaining_pickup_delay() {
    let registry = SessionRegistry::new();
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    registry.advance_world_time(2);

    let records = registry.persisted_entity_records();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].age, 2);
    assert_eq!(records[0].pickup_delay, 2);
}

#[test]
fn item_drop_spawn_installs_required_retained_state_in_one_ecs_transaction() {
    let registry = SessionRegistry::new();
    registry.reset_entity_owner_requests_for_test();

    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));

    assert_eq!(registry.entity_owner_requests_for_test(), 2);
    let snapshot = registry
        .lock_entities("read atomic item spawn")
        .snapshots_vec()
        .into_iter()
        .find(|snapshot| snapshot.item_stack.is_some())
        .expect("spawned item remains authoritative");
    assert_eq!(snapshot.retained.spawn_tick, 0);
    assert_eq!(
        snapshot.retained.item_pickup_ready_tick,
        Some(ITEM_PICKUP_DELAY_TICKS)
    );
}

#[test]
fn setting_world_time_does_not_age_item_lifecycle() {
    let registry = SessionRegistry::new();
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    registry.advance_world_time(2);
    registry.set_world_time(100_000);

    let records = registry.persisted_entity_records();

    assert_eq!(records[0].age, 2);
    assert_eq!(records[0].pickup_delay, 2);
}

#[test]
fn restored_item_respects_remaining_pickup_delay() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "RestorePickupAlice");
    let mut retained = mc_entity::EntityRetainedState::default();
    retained.spawn_tick = 0;
    retained.item_pickup_ready_tick = Some(15);
    let record = PersistedEntityRecord {
        snapshot: mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(77),
            uuid: uuid::Uuid::nil(),
            type_id: 1,
            type_name: "minecraft:item".into(),
            position: Vec3::new(0.5, 64.0, 0.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: Some(EntityItemStack::new(42, 3)),
            experience_value: None,
            block_state: None,
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::new(),
            goal: mc_entity::GoalState::Idle,
            vehicle: None,
            animal: None,
            retained,
        },
        age: 12,
        pickup_delay: 3,
    };
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(12, vec![record])),
        1
    );
    assert_eq!(
        registry
            .lock_entities("inspect restored item ECS timing")
            .snapshot(EntityId(77))
            .unwrap()
            .retained
            .spawn_tick,
        0
    );

    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
    registry.advance_world_time(3);
    let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;
    assert!(
        registry
            .claim_item_pickup_for_test(entity_id, alice, 3)
            .is_some()
    );
}

#[test]
fn persisted_restore_preserves_forward_passenger_links_atomically() {
    let mut passenger_store = mc_entity::EntityStore::with_next_id(1);
    let passenger_id = passenger_store.spawn(mc_entity::SpawnEntity::new(
        4,
        "minecraft:cow",
        Vec3::new(0.5, 64.0, 0.5),
    ));
    let passenger = passenger_store
        .snapshot(passenger_id)
        .expect("passenger snapshot");
    assert_eq!(passenger_id, EntityId(2));

    let mut vehicle_store = mc_entity::EntityStore::new();
    let vehicle_id = vehicle_store.spawn(mc_entity::SpawnEntity::vehicle(
        mc_entity::VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(1.5, 64.0, 0.5),
    ));
    let mut vehicle = vehicle_store
        .snapshot(vehicle_id)
        .expect("vehicle snapshot");
    vehicle.vehicle.as_mut().expect("vehicle state").passenger = Some(passenger_id);
    assert_eq!(vehicle_id, EntityId(1));

    let registry = SessionRegistry::new();
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(
            0,
            vec![
                PersistedEntityRecord {
                    snapshot: vehicle,
                    age: 0,
                    pickup_delay: 0,
                },
                PersistedEntityRecord {
                    snapshot: passenger,
                    age: 0,
                    pickup_delay: 0,
                },
            ],
        )),
        2
    );
    let restored = registry.persisted_entity_records();
    let restored_vehicle = restored
        .iter()
        .find(|record| record.snapshot.id == vehicle_id)
        .expect("restored vehicle");
    assert_eq!(
        restored_vehicle
            .snapshot
            .vehicle
            .and_then(|vehicle| vehicle.passenger),
        Some(passenger_id)
    );
}

#[test]
fn vehicle_crossing_publishes_authoritative_passenger_motion() {
    let mut passenger_store = mc_entity::EntityStore::with_next_id(1);
    let passenger_id = passenger_store.spawn(mc_entity::SpawnEntity::new(
        4,
        "minecraft:cow",
        Vec3::new(127.75, 64.0, 0.5),
    ));
    let passenger = passenger_store
        .snapshot(passenger_id)
        .expect("passenger snapshot");
    let mut vehicle_store = mc_entity::EntityStore::new();
    let vehicle_id = vehicle_store.spawn(mc_entity::SpawnEntity::vehicle(
        mc_entity::VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    ));
    let mut vehicle = vehicle_store
        .snapshot(vehicle_id)
        .expect("vehicle snapshot");
    vehicle.vehicle.as_mut().expect("vehicle state").passenger = Some(passenger_id);
    let registry = SessionRegistry::new();
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(
            0,
            vec![
                PersistedEntityRecord {
                    snapshot: vehicle,
                    age: 0,
                    pickup_delay: 0,
                },
                PersistedEntityRecord {
                    snapshot: passenger,
                    age: 0,
                    pickup_delay: 0,
                },
            ],
        )),
        2
    );

    let accepted = registry.apply_entity_physics_and_dispatch_core(
        None,
        1,
        None,
        &[
            EntityPhysicsStep {
                id: vehicle_id,
                position: Vec3::new(128.25, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            },
            EntityPhysicsStep {
                id: passenger_id,
                position: Vec3::new(127.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            },
        ],
        &[],
    );

    assert_eq!(accepted.len(), 2);
    assert_eq!(
        accepted
            .iter()
            .find(|step| step.id == passenger_id)
            .expect("accepted passenger motion")
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );
    assert_eq!(
        registry
            .lock_inner("published passenger motion")
            .published_entity_snapshots[&passenger_id]
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn item_drop_despawns_after_lifetime() {
    let registry = SessionRegistry::new();
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    registry.advance_world_time(ITEM_DESPAWN_AGE_TICKS);

    registry.apply_entity_physics_and_dispatch(ITEM_DESPAWN_AGE_TICKS, &[]);

    assert!(registry.persisted_entity_records().is_empty());
}

#[test]
fn item_lifecycle_index_tracks_only_item_entities_and_clears_on_remove() {
    let registry = SessionRegistry::new();
    let collector = register_test_session(&registry, "ItemIndexCollector");
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    registry.spawn_xp_orb(2, Vec3::new(0.5, 64.0, 0.5), 3);
    registry.spawn_item_drop(3, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    let item_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.item_stack.is_some())
        .expect("spawned item")
        .snapshot
        .id;

    assert_eq!(
        registry
            .lock_entities("inspect item ECS timing")
            .snapshot(item_id)
            .unwrap()
            .retained
            .spawn_tick,
        0
    );
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    assert!(
        registry
            .claim_item_pickup_for_test(item_id, collector, 1)
            .is_some()
    );
    assert!(
        registry
            .lock_entities("inspect removed item ECS row")
            .snapshot(item_id)
            .is_none()
    );
}

#[test]
fn item_despawn_sweep_is_budgeted() {
    let registry = SessionRegistry::new();
    for index in 0..(ITEM_DESPAWN_SWEEP_BUDGET + 10) {
        registry.spawn_item_drop(
            1,
            Vec3::new(index as f64, 64.0, 0.5),
            EntityItemStack::new(42, 1),
        );
    }
    registry.advance_world_time(ITEM_DESPAWN_AGE_TICKS);

    registry.apply_entity_physics_and_dispatch(ITEM_DESPAWN_AGE_TICKS, &[]);

    let remaining = registry.persisted_entity_records().len();
    assert_eq!(remaining, 10);
}

#[test]
fn restored_arrow_total_age_does_not_bypass_kernel_grounded_age() {
    let registry = SessionRegistry::new();
    let record = PersistedEntityRecord {
        snapshot: mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(88),
            uuid: uuid::Uuid::nil(),
            type_id: 2,
            type_name: "minecraft:arrow".into(),
            position: Vec3::new(0.5, 64.0, 0.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::new(),
            goal: mc_entity::GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        },
        age: mc_entity::projectile_26_1_2::ARROW_DESPAWN_TICKS - 1,
        pickup_delay: 0,
    };
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(
            mc_entity::projectile_26_1_2::ARROW_DESPAWN_TICKS as u64 - 1,
            vec![record],
        )),
        1
    );
    registry.advance_world_time(1);

    registry.apply_entity_physics_and_dispatch(1, &[]);

    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn pressure_snapshot_counts_entity_spawn_move_and_pickup_dispatches() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "DispatchAlice");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());

    let start = registry.pressure_snapshot().entity_dispatches;
    let spawn_dispatches =
        registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
    assert_eq!(spawn_dispatches.len(), 1);

    let after_spawn = registry.pressure_snapshot().entity_dispatches;
    assert_eq!(after_spawn.spawn, start.spawn + 1);
    assert_eq!(after_spawn.move_relative, start.move_relative);
    assert_eq!(after_spawn.take, start.take);
    assert_eq!(after_spawn.remove, start.remove);

    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned entity").id
    };
    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    let after_move = registry.pressure_snapshot().entity_dispatches;
    assert_eq!(after_move.move_relative, after_spawn.move_relative + 1);

    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let claimed = registry
        .claim_item_pickup_for_test(entity_id, alice, 3)
        .unwrap();
    assert_eq!(claimed.dispatches.len(), 2);

    let after_pickup = registry.pressure_snapshot().entity_dispatches;
    assert_eq!(after_pickup.take, after_move.take + 1);
    assert_eq!(after_pickup.remove, after_move.remove + 1);
}

#[test]
fn entity_snapshot_read_does_not_wait_for_session_registry_lock() {
    let registry = Arc::new(SessionRegistry::new());
    let alice = register_test_session(&registry, "DetachedEntityRead");
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let dispatches =
        registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    let entity_id = dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned item is visible");
    let mut session_guard = Some(registry.inner.lock().expect("session registry poisoned"));
    let (sent, received) = std::sync::mpsc::sync_channel(1);

    std::thread::scope(|scope| {
        let registry = Arc::clone(&registry);
        scope.spawn(move || {
            sent.send(registry.server_entity_snapshot(entity_id))
                .expect("snapshot receiver stays connected");
        });
        let result = received.recv_timeout(Duration::from_millis(250));
        drop(session_guard.take());
        let snapshot =
            result.expect("entity snapshot read must not wait for the session registry lock");
        assert_eq!(snapshot.expect("spawned item exists").id, entity_id);
    });
}

#[test]
fn boundary_spawn_does_not_send_same_tick_relative_move_to_new_observer() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("BoundaryAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (1, 0)).is_empty());
    assert!(
        registry
            .spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_string(),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .is_empty()
    );
    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned entity").id
    };

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(16.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    assert!(rx.try_recv().is_err());
}

#[test]
fn planned_spawn_orders_concurrent_entity_movement_before_channel_delivery() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("OrderedSpawnAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    assert!(spawn.is_empty());
    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned entity").id
    };

    let spawn = registry.mark_loaded(alice, (0, 0));
    assert!(matches!(
        spawn.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::SpawnEntity(_),
            ..
        }]
    ));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert!(
        rx.try_recv().is_err(),
        "movement planned after spawn must wait for the spawn publication"
    );

    dispatch_visibility_commands(spawn);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(_))
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn dropped_planned_spawn_disconnects_without_releasing_entity_movement() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("DroppedSpawnAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(
        registry
            .spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_string(),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .is_empty()
    );
    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned entity").id
    };
    let spawn = registry.mark_loaded(alice, (0, 0));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert!(rx.try_recv().is_err());

    drop(spawn);
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::DisconnectPlayer { .. })
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn older_physics_movement_keeps_its_order_across_unlocked_fanout() {
    let registry = Arc::new(SessionRegistry::new());
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("OrderedMovementAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected zombie spawn, got {other:?}"),
    };
    dispatch_visibility_commands(spawn);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .move_fanout_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(MoveFanoutProbe {
        reached: reached_tx,
        resume: resume_rx,
    });
    let physics_registry = Arc::clone(&registry);
    let physics = std::thread::spawn(move || {
        physics_registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("older movement reaches unlocked fanout before failure timeout");

    dispatch_visibility_commands(
        registry
            .apply_player_melee_knockback_legacy_for_test(entity_id, Vec3::new(-0.5, 64.0, 0.5)),
    );
    assert!(
        rx.try_recv().is_err(),
        "newer movement must wait for the older reserved publication"
    );
    resume_tx.send(()).expect("release move fanout probe");
    physics.join().expect("physics worker");

    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement))
            if movement.id == entity_id && movement.wire_move.is_some()
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement))
            if movement.id == entity_id && movement.send_velocity
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn one_chunk_crossing_does_not_suppress_other_entity_movement() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("MixedBoundaryAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    assert!(registry.mark_loaded(alice, (1, 0)).is_empty());

    let crossing_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(15.5, 64.0, 0.5),
    );
    let crossing_id = crossing_dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("crossing zombie is visible");
    dispatch_visibility_commands(crossing_dispatches);

    let local_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let local_id = local_dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("local zombie is visible");
    dispatch_visibility_commands(local_dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[
            EntityPhysicsStep {
                id: crossing_id,
                position: Vec3::new(16.5, 64.0, 0.5),
                velocity: Vec3::new(0.1, 0.0, 0.0),
                on_ground: true,
                horizontal_collision: false,
            },
            EntityPhysicsStep {
                id: local_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::new(0.1, 0.0, 0.0),
                on_ground: true,
                horizontal_collision: false,
            },
        ],
    );

    let Ok(OutboundCommand::MoveEntitiesRelative(movements)) = rx.try_recv() else {
        panic!("both visible zombies should share one movement batch");
    };
    assert_eq!(
        movements
            .iter()
            .map(|movement| movement.id)
            .collect::<HashSet<_>>(),
        HashSet::from([crossing_id, local_id])
    );
    assert_eq!(
        registry
            .physics_boundary_observer_scans
            .load(Ordering::Relaxed),
        1,
        "observer history is needed only for the entity that crossed a chunk boundary"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn moving_mobs_send_velocity_with_relative_move() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("VelocityAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    dispatch_visibility_commands(spawn_dispatches);
    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned entity").id
    };

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.1, 0.5),
            velocity: Vec3::new(0.0, 0.05, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    let Ok(OutboundCommand::MoveEntityRelative(movement)) = rx.try_recv() else {
        panic!("expected relative mob movement");
    };
    assert!(movement.send_velocity);
}

#[test]
fn unchanged_mob_velocity_is_not_sent_again() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("VelocityRepeatAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    dispatch_visibility_commands(spawn_dispatches);
    let entity_id = registry
        .lock_entities("test entity access")
        .snapshots()
        .next()
        .expect("spawned entity")
        .id;
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    for position_x in [0.55, 0.60] {
        registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(position_x, 64.0, 0.5),
                velocity: Vec3::new(1.0, 0.0, 0.0),
                on_ground: true,
                horizontal_collision: false,
            }],
        );
    }

    let Ok(OutboundCommand::MoveEntityRelative(first)) = rx.try_recv() else {
        panic!("expected first relative movement");
    };
    let Ok(OutboundCommand::MoveEntityRelative(second)) = rx.try_recv() else {
        panic!("expected second relative movement");
    };
    assert!(first.send_velocity);
    assert!(!second.send_velocity);
}

#[test]
fn stopped_mob_sends_zero_velocity_without_position_delta() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("VelocityStopAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    dispatch_visibility_commands(spawn_dispatches);
    let entity_id = registry
        .lock_entities("test entity access")
        .snapshots()
        .next()
        .expect("spawned entity")
        .id;
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.55, 64.0, 0.5),
            velocity: Vec3::new(1.0, 0.0, 0.0),
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(_))
    ));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.55, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    let Ok(OutboundCommand::MoveEntityRelative(stopped)) = rx.try_recv() else {
        panic!("expected zero-velocity transition");
    };
    assert_eq!(stopped.wire_move, None);
    assert_eq!(stopped.velocity, Vec3::ZERO);
    assert!(stopped.send_velocity);
}

#[test]
fn stationary_goal_rotation_survives_physics_commit_and_is_dispatched() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("RotationAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    dispatch_visibility_commands(spawn_dispatches);
    let entity = registry
        .lock_entities("test entity access")
        .snapshots()
        .next()
        .expect("spawned entity");
    let entity_id = entity.id;
    let rotation = Rotation {
        yaw: 90.0,
        pitch: 0.0,
        head_yaw: 90.0,
    };
    registry
        .lock_entities("test entity access")
        .apply_kinematics([EntityKinematics {
            id: entity_id,
            position: entity.position,
            rotation,
            velocity: Vec3::ZERO,
            on_ground: true,
        }]);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: entity.position,
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        registry
            .server_entity_snapshot(entity_id)
            .expect("entity remains published")
            .rotation,
        rotation
    );
    let Ok(OutboundCommand::MoveEntityRelative(rotated)) = rx.try_recv() else {
        panic!("expected stationary rotation update");
    };
    assert_eq!(
        rotated.wire_move,
        Some(
            crate::play::wire_entities::ServerEntityWireMove::PositionRotation {
                delta: Vec3::ZERO,
            }
        )
    );
    assert_eq!(rotated.rotation, rotation);
}

#[test]
fn non_finite_entity_physics_is_rejected_before_visibility_mutation() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("FinitePhysicsAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = spawn_dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned zombie is visible");
    dispatch_visibility_commands(spawn_dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    let (published_before, chunk_before) = {
        let mut inner = registry.inner.lock().expect("session registry poisoned");
        inner
            .last_sent_entity_states
            .get_mut(&entity_id)
            .expect("spawned entity has tracker state")
            .tracking_update_count = 1;
        (
            inner
                .published_entity_snapshots
                .get(&entity_id)
                .cloned()
                .expect("published zombie snapshot"),
            inner.entity_chunks.get(&entity_id).copied(),
        )
    };

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(f64::NAN, 64.0, 0.5),
            velocity: Vec3::new(f64::INFINITY, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    let (published_after, chunk_after, tracker_count, teleport_delay) = {
        let inner = registry.inner.lock().expect("session registry poisoned");
        let tracker = inner
            .last_sent_entity_states
            .get(&entity_id)
            .expect("spawned entity keeps tracker state");
        (
            inner
                .published_entity_snapshots
                .get(&entity_id)
                .cloned()
                .expect("published zombie snapshot"),
            inner.entity_chunks.get(&entity_id).copied(),
            tracker.tracking_update_count,
            tracker.teleport_delay,
        )
    };
    assert_eq!(published_after.position, published_before.position);
    assert_eq!(published_after.rotation, published_before.rotation);
    assert_eq!(published_after.velocity, published_before.velocity);
    assert_eq!(published_after.on_ground, published_before.on_ground);
    assert_eq!(chunk_after, chunk_before);
    assert_eq!(tracker_count, 1);
    assert_eq!(teleport_delay, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn stale_entity_physics_result_does_not_overwrite_newer_kinematics() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("StalePhysicsAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let initial = registry
        .lock_entities("test entity access")
        .snapshots()
        .next()
        .expect("spawned entity");
    let expected = EntityPhysicsQuery {
        id: initial.id,
        position: initial.position,
        velocity: initial.velocity,
        aabb: mc_physics::Aabb::COW,
        on_ground: initial.on_ground,
        kind: EntityPhysicsKind::Living,
    };
    let newer_position = Vec3::new(1.5, 64.0, 0.5);
    let newer_velocity = Vec3::new(0.2, 0.0, 0.0);
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: initial.id,
            position: newer_position,
            velocity: newer_velocity,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    {
        let mut inner = registry.lock_inner("prepare stale physics tracker cadence");
        let tracker = inner
            .last_sent_entity_states
            .get_mut(&initial.id)
            .expect("spawned entity has tracker state");
        tracker.tracking_update_count = 59;
        tracker.teleport_delay = 399;
    }

    registry.apply_entity_physics_if_current_and_dispatch(
        2,
        &[expected],
        &[EntityPhysicsStep {
            id: initial.id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    let current = registry
        .server_entity_snapshot(initial.id)
        .expect("entity remains published");
    assert_eq!(current.position, newer_position);
    assert_eq!(current.velocity, newer_velocity);
    let inner = registry.lock_inner("verify stale physics tracker cadence");
    let tracker = inner
        .last_sent_entity_states
        .get(&initial.id)
        .expect("spawned entity keeps tracker state");
    assert_eq!(tracker.tracking_update_count, 59);
    assert_eq!(tracker.teleport_delay, 399);
}

#[test]
fn player_body_push_keeps_entity_chunk_index_with_authoritative_position() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("BoundaryPushPlayer"),
        (0, 0),
        1,
        HashSet::from([(0, 0), (1, 0)]),
        tx,
        PlayerPose::new(14.0, 64.0, 0.5),
    );
    registry.mark_loaded(player, (0, 0));
    registry.mark_loaded(player, (1, 0));
    let (old_tx, _old_rx) = mpsc::channel(8);
    let (old_observer, _) = registry.register(
        &profile("BoundaryPushOldObserver"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        old_tx,
        PlayerPose::new(8.0, 64.0, 0.5),
    );
    registry.mark_loaded(old_observer, (0, 0));
    let (new_tx, _new_rx) = mpsc::channel(8);
    let (new_observer, _) = registry.register(
        &profile("BoundaryPushNewObserver"),
        (1, 0),
        0,
        HashSet::from([(1, 0)]),
        new_tx,
        PlayerPose::new(20.0, 64.0, 0.5),
    );
    registry.mark_loaded(new_observer, (1, 0));
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(15.95, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned zombie is visible");

    let dispatches = registry.update_pose(player, PlayerPose::new(15.6, 64.0, 0.5));

    let inner = registry.inner.lock().expect("session registry poisoned");
    let position = inner
        .published_entity_snapshots
        .get(&entity_id)
        .expect("published zombie snapshot")
        .position;
    let expected_chunk = chunk_pos_from_coords(position.x, position.z);
    assert_eq!(expected_chunk, (0, 0));
    assert_eq!(inner.entity_chunks.get(&entity_id), Some(&expected_chunk));
    assert!(
        inner
            .entities_by_chunk
            .get(&expected_chunk)
            .is_some_and(|entities| entities.contains(&entity_id))
    );
    assert!(
        inner
            .entities_by_chunk
            .get(&(1, 0))
            .is_none_or(|entities| !entities.contains(&entity_id))
    );
    assert!(!dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == old_observer
            && matches!(
                &dispatch.command,
                OutboundCommand::DespawnEntity(entity) if entity.id == entity_id
            )
    }));
    assert!(!dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == new_observer
            && matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntity(entity) if entity.id == entity_id
            )
    }));
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == player
            && matches!(
                &dispatch.command,
                OutboundCommand::MoveEntityRelative(movement)
                    if movement.id == entity_id
                        && movement.wire_move.is_none()
                        && movement.velocity != Vec3::ZERO
            )
    }));
    assert!(!dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == new_observer
            && matches!(
                &dispatch.command,
                OutboundCommand::MoveEntityRelative(movement) if movement.id == entity_id
            )
    }));
    assert_eq!(
        inner
            .last_sent_entity_states
            .get(&entity_id)
            .expect("zombie wire state")
            .position,
        position
    );
}

#[test]
fn player_body_push_ignores_projectile_entities() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("ProjectilePushPlayer"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.0, 64.0, 0.5),
    );
    registry.mark_loaded(player, (0, 0));
    let initial_position = Vec3::new(5.0, 64.0, 0.5);
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:arrow".to_string(),
            initial_position,
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned arrow is visible");

    let _ = registry.update_pose(player, PlayerPose::new(4.8, 64.0, 0.5));

    assert_eq!(
        registry
            .server_entity_snapshot(entity_id)
            .expect("arrow remains published")
            .position,
        initial_position
    );
}

#[test]
fn player_body_push_only_visits_nearby_chunk_candidates() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("IndexedPushPlayer"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.0, 64.0, 0.5),
    );
    registry.mark_loaded(player, (0, 0));
    let near_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(0.6, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("near zombie is visible");
    for x in [160.5, 320.5] {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let initial = registry
        .server_entity_snapshot(near_id)
        .expect("near zombie snapshot");

    let _ = registry.update_pose(player, PlayerPose::new(0.3, 64.0, 0.5));

    let pushed = registry
        .server_entity_snapshot(near_id)
        .expect("near zombie remains");
    assert_eq!(pushed.position, initial.position);
    assert_ne!(pushed.velocity, initial.velocity);
    assert_eq!(
        registry.player_push_entity_visits.load(Ordering::Relaxed),
        1,
        "player body push should not visit entities in distant chunks"
    );
}

#[test]
fn accepted_pose_orders_body_push_before_player_movement_and_pickup() {
    let registry = SessionRegistry::new();
    let (player_tx, _player_rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("OrderedPosePlayer"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        player_tx,
        PlayerPose::new(0.0, 64.0, 0.5),
    );
    registry.mark_loaded(player, (0, 0));
    let (observer_tx, _observer_rx) = mpsc::channel(8);
    let (observer, _) = registry.register(
        &profile("OrderedPoseObserver"),
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        observer_tx,
        PlayerPose::new(3.0, 64.0, 0.5),
    );
    registry.mark_loaded(observer, (0, 0));
    let pushed_entity = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.6, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("near zombie is visible");
    registry.spawn_xp_orb(99, Vec3::new(0.5, 64.0, 0.5), 5);

    let dispatches = registry.update_pose(player, PlayerPose::new(0.3, 64.0, 0.5));

    let body_push = dispatches
        .iter()
        .position(|dispatch| {
            matches!(
                &dispatch.command,
                OutboundCommand::MoveEntityRelative(movement)
                    if movement.id == pushed_entity
            )
        })
        .expect("accepted pose publishes the body push");
    let player_movement = dispatches
        .iter()
        .position(|dispatch| {
            dispatch.recipient.id == observer
                && matches!(
                    &dispatch.command,
                    OutboundCommand::MovePlayer(snapshot) if snapshot.session_id == player
                )
        })
        .expect("accepted pose publishes player movement");
    let pickup = dispatches
        .iter()
        .position(|dispatch| {
            dispatch.recipient.id == player
                && matches!(
                    &dispatch.command,
                    OutboundCommand::PickupCandidates(candidates)
                        if candidates.iter().any(|candidate| candidate.experience_value == Some(5))
                )
        })
        .expect("accepted pose publishes pickup candidates");

    assert!(body_push < player_movement);
    assert!(player_movement < pickup);
}

#[test]
fn player_body_push_releases_both_locks_before_session_publication() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "PushCommitLocksAlice");
    let player_state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(player, Arc::clone(&player_state));
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let observer = register_test_session(&registry, "PushCommitLocksObserver");
    let _ = registry.mark_loaded(observer, (0, 0));
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.6, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("near zombie is visible");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .player_push_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PlayerPushCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let updated_pose = PlayerPose::new(0.3, 64.0, 0.5);
    let update_registry = Arc::clone(&registry);
    let update = std::thread::spawn(move || {
        update_registry.commit_player_pose(
            &SimulationAuthority::for_test(),
            player,
            updated_pose,
            0.0,
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("player push reaches the publication boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entities_available = registry.entities.owner_responsive_for_test();
    let newer_position = Vec3::new(2.5, 64.0, 0.5);
    registry.apply_entity_physics_and_dispatch(
        2,
        &[EntityPhysicsStep {
            id: entity_id,
            position: newer_position,
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    let persisted_pose = player_state.lock().expect("player persisted state").pose;
    let unload_dispatches = registry.mark_unloaded(observer, &[(0, 0)]);
    resume_tx.send(()).expect("release player push commit");
    let pose_dispatches = update
        .join()
        .expect("player pose update worker")
        .expect("accepted player pose commit")
        .0;

    assert!(
        session_available,
        "player push publication must not retain session state"
    );
    assert!(
        entities_available,
        "player push publication must release the entity store"
    );
    assert_eq!(persisted_pose.x, updated_pose.x);
    assert_eq!(persisted_pose.y, updated_pose.y);
    assert_eq!(persisted_pose.z, updated_pose.z);
    assert!(unload_dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == observer
            && matches!(&dispatch.command, OutboundCommand::DespawnPlayer(_))
    }));
    assert!(!pose_dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == observer
            && matches!(&dispatch.command, OutboundCommand::MovePlayer(_))
    }));
    assert_eq!(
        registry
            .server_entity_snapshot(entity_id)
            .expect("near zombie remains")
            .position,
        newer_position,
        "stale body-push publication must not overwrite newer physics"
    );
}

#[test]
fn player_body_push_rejects_removed_same_id_replacement_before_publication() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "PushReplacementPlayer");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let observer = register_test_session(&registry, "PushReplacementObserver");
    let _ = registry.mark_loaded(observer, (0, 0));
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.6, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("near zombie is visible");
    let published_before = registry
        .lock_inner("capture entity publication before replacement")
        .published_entity_snapshots
        .get(&entity_id)
        .cloned()
        .expect("zombie is published before pose update");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .player_push_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PlayerPushCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let update_registry = Arc::clone(&registry);
    let update = std::thread::spawn(move || {
        update_registry.update_pose(player, PlayerPose::new(0.3, 64.0, 0.5))
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("body push mutation reaches the publication boundary");
    let replacement = {
        let mut entities = registry.lock_entities("replace pushed entity before publication");
        let pushed = entities
            .snapshot(entity_id)
            .expect("body push mutation remains in ECS");
        assert_ne!(pushed.velocity, published_before.velocity);
        let removed = entities
            .remove_if_current(pushed)
            .expect("remove body-pushed entity");
        let mut replacement = removed;
        replacement.uuid = uuid::Uuid::from_u128(0xfeed_cafe);
        replacement.position = Vec3::new(4.5, 64.0, 0.5);
        assert!(entities.insert_snapshots_batch([replacement.clone()]));
        replacement
    };
    resume_tx.send(()).expect("release body push publication");
    let pose_dispatches = update.join().expect("pose update worker");

    assert!(!pose_dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::MoveEntityRelative(movement) if movement.id == entity_id
        )
    }));
    let authoritative = registry
        .server_entity_snapshot(entity_id)
        .expect("same-ID replacement remains authoritative");
    assert_eq!(authoritative.id, replacement.id);
    assert_eq!(authoritative.uuid, replacement.uuid);
    assert_eq!(authoritative.position, replacement.position);
    let inner = registry.lock_inner("verify stale body push publication was rejected");
    let published_after = inner
        .published_entity_snapshots
        .get(&entity_id)
        .expect("old entity publication remains until its owner publishes replacement");
    assert_eq!(published_after.id, published_before.id);
    assert_eq!(published_after.uuid, published_before.uuid);
    assert_eq!(published_after.position, published_before.position);
}

#[test]
fn moving_mobs_share_one_relative_move_batch_per_observer() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("MovementBatchAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());

    let mut entity_ids = Vec::new();
    for x in [0.5, 1.5] {
        let dispatches = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(x, 64.0, 0.5),
        );
        entity_ids.push(
            dispatches
                .iter()
                .find_map(|dispatch| match &dispatch.command {
                    OutboundCommand::SpawnEntity(entity) => Some(entity.id),
                    _ => None,
                })
                .expect("spawned zombie is visible"),
        );
        dispatch_visibility_commands(dispatches);
    }
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[
            EntityPhysicsStep {
                id: entity_ids[0],
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            },
            EntityPhysicsStep {
                id: entity_ids[1],
                position: Vec3::new(1.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            },
        ],
    );

    let Ok(OutboundCommand::MoveEntitiesRelative(movements)) = rx.try_recv() else {
        panic!("expected one relative movement batch");
    };
    assert_eq!(movements.len(), 2);
    assert_eq!(movements[0].id, entity_ids[0]);
    assert_eq!(movements[1].id, entity_ids[1]);
    assert!(rx.try_recv().is_err());
}

#[test]
fn bounded_natural_mobs_publish_changed_movement_every_tick() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (observer, _) = registry.register(
        &profile("NaturalMovementObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(observer, (0, 0)).is_empty());
    let dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        93,
        "minecraft:chicken".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned chicken is visible");
    dispatch_visibility_commands(dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    registry
        .lock_inner("mark bounded natural movement entity")
        .natural_ground_mobs
        .insert(entity_id);

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement)) if movement.id == entity_id
    ));
}

#[test]
fn bounded_natural_hostiles_publish_changed_movement_every_tick() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (observer, _) = registry.register(
        &profile("NaturalHostileMovementObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(observer, (0, 0)).is_empty());
    let dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let entity_id = dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("spawned zombie is visible");
    dispatch_visibility_commands(dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    registry
        .lock_inner("mark bounded natural hostile movement entity")
        .natural_hostile_mobs
        .insert(entity_id);

    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement)) if movement.id == entity_id
    ));
}

#[test]
fn dense_movement_shard_publishes_latest_state_when_entity_becomes_due() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(2048);
    let (alice, _) = registry.register(
        &profile("DenseMovementAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());

    let entity_count = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN + 1;
    let mut entity_ids = Vec::with_capacity(entity_count);
    for _ in 0..entity_count {
        let dispatches = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(0.5, 64.0, 0.5),
        );
        entity_ids.push(
            dispatches
                .iter()
                .find_map(|dispatch| match &dispatch.command {
                    OutboundCommand::SpawnEntity(entity) => Some(entity.id),
                    _ => None,
                })
                .expect("spawned zombie is visible"),
        );
        dispatch_visibility_commands(dispatches);
        assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    }

    let target_index = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN - 1;
    let target_id = entity_ids[target_index];
    let steps = |target_x| {
        entity_ids
            .iter()
            .enumerate()
            .map(|(index, &id)| EntityPhysicsStep {
                id,
                position: Vec3::new(
                    if index == target_index { target_x } else { 0.5 },
                    64.0,
                    0.5,
                ),
                velocity: Vec3::ZERO,
                on_ground: false,
                horizontal_collision: false,
            })
            .collect::<Vec<_>>()
    };

    registry.apply_entity_physics_and_dispatch(ENTITY_MOVE_SEND_INTERVAL_TICKS, &steps(0.75));
    if let Ok(command) = rx.try_recv() {
        let target_was_published = match command {
            OutboundCommand::MoveEntityRelative(movement) => movement.id == target_id,
            OutboundCommand::MoveEntitiesRelative(movements) => {
                movements.iter().any(|movement| movement.id == target_id)
            }
            command => panic!("unexpected movement command: {command:?}"),
        };
        assert!(
            !target_was_published,
            "target is outside the first rotating window"
        );
    }
    assert!(rx.try_recv().is_err());

    registry.apply_entity_physics_and_dispatch(ENTITY_MOVE_SEND_INTERVAL_TICKS * 2, &steps(1.0));
    let movement = match rx.try_recv().expect("target becomes due on the next turn") {
        OutboundCommand::MoveEntityRelative(movement) => movement,
        OutboundCommand::MoveEntitiesRelative(mut movements) => movements
            .drain(..)
            .find(|movement| movement.id == target_id)
            .expect("movement batch contains target"),
        command => panic!("unexpected movement command: {command:?}"),
    };
    assert_eq!(movement.id, target_id);
    assert_eq!(movement.position, Vec3::new(1.0, 64.0, 0.5));
}

#[test]
fn movement_fanout_skips_visibility_work_when_no_movement_is_published() {
    const SESSION_COUNT: usize = 64;

    let registry = SessionRegistry::new();
    assert!(
        registry
            .spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_owned(),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .is_empty()
    );
    let entity_id = registry.persisted_entity_records()[0].id;
    let unchanged = registry
        .server_entity_snapshot(entity_id)
        .expect("published no-op movement entity");
    for index in 0..SESSION_COUNT {
        let session_id = register_test_session(&registry, &format!("NoMovementObserver{index}"));
        registry
            .lock_inner("install no-op movement visibility")
            .sessions
            .get_mut(&session_id)
            .expect("registered no-op movement observer")
            .visible_entities = Arc::new(HashSet::from([entity_id]));
    }
    registry
        .lock_inner("place no-op movement outside tracker refresh cadence")
        .last_sent_entity_states
        .get_mut(&entity_id)
        .expect("spawned entity has tracker state")
        .tracking_update_count = 1;

    entity_simulation::reset_movement_fanout_work();
    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: unchanged.position,
            velocity: unchanged.velocity,
            on_ground: unchanged.on_ground,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        entity_simulation::take_movement_fanout_work(),
        entity_simulation::MovementFanoutWork::default()
    );
}

#[test]
fn movement_fanout_uses_exhaustive_scan_for_one_dense_movement() {
    const SESSION_COUNT: usize = 64;

    let registry = SessionRegistry::new();
    assert!(
        registry
            .spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_owned(),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .is_empty()
    );
    let entity_id = registry.persisted_entity_records()[0].id;

    let mut sessions = Vec::with_capacity(SESSION_COUNT);
    for index in 0..SESSION_COUNT {
        let (tx, rx) = mpsc::channel(2);
        let (session_id, _) = registry.register(
            &profile(&format!("DenseMovementObserver{index}")),
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        sessions.push((session_id, rx));
    }
    {
        let mut inner = registry.lock_inner("install dense movement visibility");
        for (session_id, _) in &sessions {
            inner
                .sessions
                .get_mut(session_id)
                .expect("registered movement observer")
                .visible_entities = Arc::new(HashSet::from([entity_id]));
        }
    }

    entity_simulation::reset_movement_fanout_work();
    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert_eq!(
        entity_simulation::take_movement_fanout_work(),
        entity_simulation::MovementFanoutWork {
            index_builds: 0,
            index_edge_visits: 0,
            exhaustive_membership_checks: SESSION_COUNT,
        }
    );
    for (_, receiver) in &mut sessions {
        assert!(matches!(
            receiver.try_recv(),
            Ok(OutboundCommand::MoveEntityRelative(movement)) if movement.id == entity_id
        ));
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn movement_fanout_indexes_sparse_disjoint_current_visibility() {
    const SESSION_COUNT: usize = 64;

    let registry = SessionRegistry::new();
    for _ in 0..SESSION_COUNT {
        assert!(
            registry
                .spawn_command_entity(
                    &SimulationAuthority::for_test(),
                    1,
                    "minecraft:zombie".to_owned(),
                    Vec3::new(0.5, 64.0, 0.5),
                )
                .is_empty()
        );
    }
    let mut entity_ids = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    entity_ids.sort_unstable();
    assert_eq!(entity_ids.len(), SESSION_COUNT);

    let mut sessions = Vec::with_capacity(SESSION_COUNT);
    for index in 0..SESSION_COUNT {
        let (tx, rx) = mpsc::channel(2);
        let (session_id, _) = registry.register(
            &profile(&format!("SparseMovementObserver{index}")),
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        sessions.push((session_id, rx));
    }
    {
        let mut inner = registry.lock_inner("install sparse movement visibility");
        for ((session_id, _), entity_id) in sessions.iter().zip(&entity_ids) {
            inner
                .sessions
                .get_mut(session_id)
                .expect("registered movement observer")
                .visible_entities = Arc::new(HashSet::from([*entity_id]));
        }
    }

    let steps = entity_ids
        .iter()
        .map(|&id| EntityPhysicsStep {
            id,
            position: Vec3::new(0.75, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        })
        .collect::<Vec<_>>();
    entity_simulation::reset_movement_fanout_work();
    registry.apply_entity_physics_and_dispatch(ENTITY_MOVE_SEND_INTERVAL_TICKS, &steps);

    assert_eq!(
        entity_simulation::take_movement_fanout_work(),
        entity_simulation::MovementFanoutWork {
            index_builds: 1,
            index_edge_visits: SESSION_COUNT,
            exhaustive_membership_checks: 0,
        }
    );
    for ((_, receiver), entity_id) in sessions.iter_mut().zip(entity_ids) {
        assert!(matches!(
            receiver.try_recv(),
            Ok(OutboundCommand::MoveEntityRelative(movement)) if movement.id == entity_id
        ));
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn pickup_entity_physics_event_pushes_candidate_to_nearby_player() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("PhysicsPickupAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);
    let entity_id = spawn_dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("XP orb is visible");
    dispatch_visibility_commands(spawn_dispatches);
    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::PickupCandidates(candidates))
            if candidates.iter().any(|candidate| candidate.id == entity_id)
    ));
    assert!(rx.try_recv().is_err());

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(1.0, 64.05, 1.0),
            velocity: Vec3::new(0.0, 0.05, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::PickupCandidates(candidates))
            if candidates.iter().any(|candidate| candidate.id == entity_id)
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement)) if movement.id == entity_id
    ));
}

#[test]
fn physics_pickup_snapshot_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "PhysicsPickupLocksAlice");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);
    let entity_id = spawn_dispatches
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("XP orb is visible");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .pickup_snapshot_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PickupSnapshotProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let physics_registry = Arc::clone(&registry);
    let physics = std::thread::spawn(move || {
        physics_registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(1.0, 64.05, 1.0),
                velocity: Vec3::new(0.0, 0.05, 0.0),
                on_ground: false,
                horizontal_collision: false,
            }],
        );
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("physics pickup reaches the ECS snapshot boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entity_owner_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release physics pickup snapshot");
    physics.join().expect("physics pickup worker");

    assert!(
        session_available,
        "physics pickup ECS snapshot must not retain session state"
    );
    assert!(
        entity_owner_available,
        "owner reads must not lend a store lock"
    );
}

#[test]
fn item_drop_relative_move_does_not_emit_extra_velocity_packet() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (alice, _) = registry.register(
        &profile("ItemVelocityAlice"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
    let spawn_dispatches =
        registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    dispatch_visibility_commands(spawn_dispatches);
    let entity_id = {
        let entities = registry.lock_entities("test entity access");
        entities.snapshots().next().expect("spawned item").id
    };

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.1, 0.5),
            velocity: Vec3::new(0.0, 0.05, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
    let Ok(OutboundCommand::MoveEntityRelative(movement)) = rx.try_recv() else {
        panic!("expected relative item movement");
    };
    assert!(!movement.send_velocity);
}

#[test]
fn pressure_snapshot_separates_best_effort_animation_drops() {
    let registry = SessionRegistry::new();
    let other_registry = SessionRegistry::new();
    let start = registry.pressure_snapshot();
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");

    dispatch_visibility_commands(vec![VisibilityDispatch {
        recipient: test_recipient(&registry, 1, tx),
        command: OutboundCommand::AnimatePlayer { entity_id: 2 },
    }]);

    let pressure = registry.pressure_snapshot();
    assert_eq!(
        pressure.best_effort_animation_drops,
        start.best_effort_animation_drops + 1
    );
    assert_eq!(
        pressure.reliable_command_drops, start.reliable_command_drops,
        "dropping cosmetic animation must not report reliable state loss"
    );
    assert_eq!(
        other_registry
            .pressure_snapshot()
            .best_effort_animation_drops,
        0,
        "outbound pressure must be scoped to one server instance"
    );
}

#[test]
fn stale_reliable_worker_cleanup_does_not_remove_replacement_queue() {
    let registry = SessionRegistry::new();
    let pressure = test_pressure(&registry);
    let recipient_id = 77;
    let stale_worker_id = pressure.record_reliable_retry_started();
    let stale_guard = ReliableRetryWorkerGuard {
        recipient_id,
        worker_id: stale_worker_id,
        pressure: Arc::clone(&pressure),
    };
    {
        let mut queues = pressure.lock_reliable_retry_queues();
        queues.insert(
            recipient_id,
            ReliableRetryQueue::new(
                OutboundCommand::SystemChat {
                    message: "old".to_string(),
                },
                MIN_RELIABLE_RETRY_QUEUE_CAPACITY,
                stale_worker_id,
            ),
        );
        queues.remove(&recipient_id);
        queues.insert(
            recipient_id,
            ReliableRetryQueue::new(
                OutboundCommand::SystemChat {
                    message: "replacement".to_string(),
                },
                MIN_RELIABLE_RETRY_QUEUE_CAPACITY,
                stale_worker_id.wrapping_add(1),
            ),
        );
    }

    drop(stale_guard);

    let queues = pressure.lock_reliable_retry_queues();
    assert!(
        queues.contains_key(&recipient_id),
        "cleanup from an older worker must not remove its replacement queue"
    );
}

#[tokio::test]
async fn reliable_visibility_commands_retry_when_channel_is_full() {
    let registry = SessionRegistry::new();
    let start = registry.pressure_snapshot().reliable_command_retries;
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let pressure_metrics = test_pressure(&registry);
    let retry_completed = pressure_metrics.reliable_retry_completed.notified();

    dispatch_visibility_commands(vec![VisibilityDispatch {
        recipient: test_recipient(&registry, 7, tx),
        command: OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
            session_id: 7,
            entity_id: 7,
            uuid: uuid::Uuid::nil(),
            name: "RetryPlayer".to_string(),
            properties: Vec::new(),
            pose: PlayerPose::new(0.5, 64.0, 0.5),
        }),
    }]);

    let pressure = registry.pressure_snapshot();
    assert_eq!(pressure.reliable_command_retries, start + 1);
    assert!(pressure.reliable_command_retries_in_flight > 0);
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
            session_id: 7,
            ..
        }))
    ));
    retry_completed.await;
    assert_eq!(
        registry
            .pressure_snapshot()
            .reliable_command_retries_in_flight,
        0
    );
}

#[tokio::test]
async fn reliable_visibility_backlog_preserves_distinct_commands_in_order() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let recipient = test_recipient(&registry, 98, tx);
    let dispatches = (0..3)
        .map(|index| VisibilityDispatch {
            recipient: recipient.clone(),
            command: OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id: 9_800 + index,
                entity_id: 9_800 + i32::try_from(index).unwrap(),
                uuid: uuid::Uuid::nil(),
                name: format!("OrderedRetry{index}"),
                properties: Vec::new(),
                pose: PlayerPose::new(0.5, 64.0, 0.5),
            }),
        })
        .collect();

    dispatch_visibility_commands(dispatches);

    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    let received = tokio::time::timeout(Duration::from_secs(1), async {
        let mut session_ids = Vec::new();
        for _ in 0..3 {
            match rx.recv().await {
                Some(OutboundCommand::SpawnPlayer(player)) => {
                    session_ids.push(player.session_id);
                }
                other => panic!("expected ordered player spawn, got {other:?}"),
            }
        }
        session_ids
    })
    .await
    .expect("reliable backlog must make exact progress before the failure timeout");

    assert_eq!(received, vec![9_800, 9_801, 9_802]);
    assert_eq!(registry.pressure_snapshot().reliable_command_drops, 0);
}

#[tokio::test]
async fn entity_movement_backlog_coalesces_to_latest_absolute_position() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let recipient = test_recipient(&registry, 96, tx);
    let movement = |delta_x| ServerEntityMove {
        id: EntityId(42),
        position: Vec3::new(delta_x, 64.0, 0.0),
        wire_move: Some(crate::play::wire_entities::ServerEntityWireMove::Position {
            delta: Vec3::new(delta_x, 0.0, 0.0),
        }),
        velocity: Vec3::ZERO,
        rotation: Rotation::ZERO,
        on_ground: true,
        send_velocity: false,
        send_head_rotation: false,
    };

    dispatch_visibility_commands(vec![
        VisibilityDispatch {
            recipient: recipient.clone(),
            command: OutboundCommand::MoveEntityRelative(movement(0.25)),
        },
        VisibilityDispatch {
            recipient,
            command: OutboundCommand::MoveEntityRelative(movement(0.5)),
        },
    ]);

    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    let movement = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("movement backlog must progress before the failure timeout");
    assert!(matches!(
        movement,
        Some(OutboundCommand::MoveEntitiesRelative(movements))
            if matches!(
                movements.as_slice(),
                [ServerEntityMove {
                    wire_move: Some(crate::play::wire_entities::ServerEntityWireMove::Absolute {
                        position
                    }),
                    ..
                }] if *position == Vec3::new(0.5, 64.0, 0.0)
            )
    ));
    assert!(rx.try_recv().is_err());
    assert_eq!(registry.pressure_snapshot().reliable_command_drops, 0);
}

#[tokio::test]
async fn reliable_visibility_retries_are_bounded_per_slow_recipient() {
    let registry = SessionRegistry::new();
    let start = registry.pressure_snapshot();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let pressure_metrics = test_pressure(&registry);
    let retry_completed = pressure_metrics.reliable_retry_completed.notified();

    let recipient = test_recipient(&registry, 97, tx);
    let dispatches = (0..16)
        .map(|idx| VisibilityDispatch {
            recipient: recipient.clone(),
            command: OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id: 9_700 + idx,
                entity_id: 9_700 + i32::try_from(idx).unwrap(),
                uuid: uuid::Uuid::nil(),
                name: format!("SlowRetry{idx}"),
                properties: Vec::new(),
                pose: PlayerPose::new(0.5, 64.0, 0.5),
            }),
        })
        .collect();

    dispatch_visibility_commands(dispatches);

    let pressure = registry.pressure_snapshot();
    assert_eq!(
        pressure.reliable_command_retries,
        start.reliable_command_retries + 1,
        "one slow recipient should have at most one reliable retry task in flight"
    );
    assert_eq!(
        pressure.reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight + 1,
        "one slow recipient should not accumulate unbounded pending retry tasks"
    );
    assert!(
        pressure.max_reliable_command_retries_in_flight
            <= start.max_reliable_command_retries_in_flight + 1,
        "one slow recipient should not raise max pending reliable retry tasks by more than one"
    );
    assert_eq!(
        pressure.reliable_command_drops, start.reliable_command_drops,
        "bounded reliable backlog must preserve accepted commands"
    );

    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    for expected in 9_700..9_716 {
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id,
                ..
            })) if session_id == expected
        ));
    }
    retry_completed.await;
    assert_eq!(
        registry
            .pressure_snapshot()
            .reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight
    );
}

#[tokio::test]
async fn reliable_visibility_backlog_overflow_closes_session() {
    let registry = SessionRegistry::new();
    let start = registry.pressure_snapshot();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let pressure_metrics = test_pressure(&registry);
    let retry_completed = pressure_metrics.reliable_retry_completed.notified();
    let recipient = test_recipient(&registry, 99, tx);
    let dispatches = (0..17)
        .map(|index| VisibilityDispatch {
            recipient: recipient.clone(),
            command: OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id: 9_900 + index,
                entity_id: 9_900 + i32::try_from(index).unwrap(),
                uuid: uuid::Uuid::nil(),
                name: format!("OverflowRetry{index}"),
                properties: Vec::new(),
                pose: PlayerPose::new(0.5, 64.0, 0.5),
            }),
        })
        .collect();

    dispatch_visibility_commands(dispatches);

    let pressure = registry.pressure_snapshot();
    assert_eq!(
        pressure.reliable_command_drops,
        start.reliable_command_drops + 17
    );
    assert_eq!(
        pressure.best_effort_animation_drops, start.best_effort_animation_drops,
        "reliable backlog loss must not be reported as cosmetic loss"
    );
    assert_eq!(
        pressure.slow_client_pressure_sheds,
        start.slow_client_pressure_sheds + 1
    );
    assert_eq!(
        pressure.reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight + 1
    );
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::DisconnectPlayer { reason })
            if reason == RELIABLE_RETRY_OVERFLOW_REASON
    ));
    retry_completed.await;
    assert_eq!(
        registry
            .pressure_snapshot()
            .reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight
    );
}

#[tokio::test]
async fn disconnect_player_retries_are_bounded_per_slow_recipient() {
    let registry = SessionRegistry::new();
    let start = registry.pressure_snapshot();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let (session_id, _) = registry.register(
        &profile("SlowDisconnectAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let pressure_metrics = test_pressure(&registry);
    let retry_completed = pressure_metrics.reliable_retry_completed.notified();

    for idx in 0..16 {
        assert!(registry.disconnect_player(session_id, format!("duplicate disconnect {idx}")));
    }

    let pressure = registry.pressure_snapshot();
    assert_eq!(
        pressure.reliable_command_retries,
        start.reliable_command_retries + 1,
        "duplicate disconnects for one slow recipient should schedule one reliable retry"
    );
    assert_eq!(
        pressure.reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight + 1,
        "duplicate disconnects for one slow recipient should not accumulate retry tasks"
    );
    assert!(
        pressure.max_reliable_command_retries_in_flight
            <= start.max_reliable_command_retries_in_flight + 1,
        "duplicate disconnects for one slow recipient should not raise max pending retries by more than one"
    );
    assert_eq!(
        pressure.reliable_command_drops, start.reliable_command_drops,
        "duplicate commands after a queued disconnect are irrelevant to the closing session"
    );

    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::DisconnectPlayer { reason })
            if reason == "duplicate disconnect 0"
    ));
    retry_completed.await;
    assert_eq!(
        registry
            .pressure_snapshot()
            .reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight
    );
}

#[tokio::test]
async fn custom_payload_retries_are_bounded_per_slow_recipient() {
    let registry = SessionRegistry::new();
    let start = registry.pressure_snapshot();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let (session_id, _) = registry.register(
        &profile("SlowPayloadAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let channel = Identifier::parse("solaris:test").unwrap();
    let pressure_metrics = test_pressure(&registry);
    let retry_completed = pressure_metrics.reliable_retry_completed.notified();

    for idx in 0..16 {
        assert!(registry.send_custom_payload(session_id, channel.clone(), vec![idx]));
    }

    let pressure = registry.pressure_snapshot();
    assert_eq!(
        pressure.reliable_command_retries,
        start.reliable_command_retries + 1,
        "duplicate custom payloads for one slow recipient should schedule one reliable retry"
    );
    assert_eq!(
        pressure.reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight + 1,
        "duplicate custom payloads for one slow recipient should not accumulate retry tasks"
    );
    assert!(
        pressure.max_reliable_command_retries_in_flight
            <= start.max_reliable_command_retries_in_flight + 1,
        "duplicate custom payloads for one slow recipient should not raise max pending retries by more than one"
    );
    assert_eq!(
        pressure.reliable_command_drops, start.reliable_command_drops,
        "bounded reliable backlog must preserve distinct custom payloads"
    );

    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    for expected in 0..16 {
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::CustomPayload {
                channel,
                payload,
            }) if channel.as_str() == "solaris:test" && payload == vec![expected]
        ));
    }
    retry_completed.await;
    assert_eq!(
        registry
            .pressure_snapshot()
            .reliable_command_retries_in_flight,
        start.reliable_command_retries_in_flight
    );
}

#[test]
fn concurrent_xp_pickup_returns_authoritative_value_once() {
    let registry = Arc::new(SessionRegistry::new());
    let alice = register_test_session(&registry, "XpAlice");
    let bob = register_test_session(&registry, "XpBob");
    registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);
    let entity_id = registry.nearby_experience_entities(Vec3::new(1.0, 64.0, 1.0), 2.25)[0].id;
    let gate = Arc::new(Barrier::new(3));
    let handles = [alice, bob].map(|collector| {
        let registry = Arc::clone(&registry);
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            gate.wait();
            registry.claim_experience_pickup_for_test(entity_id, collector)
        })
    });

    gate.wait();
    let claims = handles
        .into_iter()
        .map(|handle| handle.join().expect("XP claimant joins"))
        .collect::<Vec<_>>();
    let claimed = claims.iter().filter_map(Option::as_ref).collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].value, 5);
    assert!(
        registry
            .nearby_experience_entities(Vec3::new(1.0, 64.0, 1.0), 2.25)
            .is_empty()
    );
}

#[test]
fn pickup_claims_reject_entities_outside_player_radius() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "FarPickupAlice");
    let far_position = Vec3::new(10.5, 64.0, 0.5);

    registry.spawn_item_drop(1, far_position, EntityItemStack::new(42, 1));
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let item_id = registry.nearby_item_entities(far_position, 0.5)[0].id;

    registry.spawn_xp_orb(99, far_position, 5);
    let xp_id = registry.nearby_experience_entities(far_position, 0.5)[0].id;

    registry.spawn_arrow_for_test(
        None,
        2,
        far_position,
        Vec3::ZERO,
        Rotation {
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
        },
    );
    let arrow_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:arrow")
        .expect("spawned arrow")
        .snapshot
        .id;
    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: far_position,
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );

    assert!(
        registry
            .claim_item_pickup_for_test(item_id, player, 1)
            .is_none()
    );
    assert!(
        registry
            .claim_experience_pickup_for_test(xp_id, player)
            .is_none()
    );
    assert!(
        registry
            .claim_arrow_pickup_for_test(arrow_id, player)
            .is_none()
    );
}

#[test]
fn damage_server_entity_respects_hurt_invulnerability_ticks() {
    let registry = SessionRegistry::new();
    let entity_id = {
        let mut entities = registry.lock_entities("test entity access");
        entities.spawn(SpawnEntity::new(1, "minecraft:zombie", Vec3::ZERO))
    };

    let first = registry
        .damage_server_entity_for_test(entity_id, 5.0)
        .unwrap();
    assert_eq!(first.snapshot.health, 15.0);
    assert_eq!(first.snapshot.retained.last_damage_tick, Some(0));
    assert!(
        registry
            .damage_server_entity_for_test(entity_id, 5.0)
            .is_none()
    );

    registry.advance_world_time(ENTITY_HURT_INVULNERABLE_TICKS - 1);
    assert!(
        registry
            .damage_server_entity_for_test(entity_id, 5.0)
            .is_none()
    );

    registry.advance_world_time(1);
    let second = registry
        .damage_server_entity_for_test(entity_id, 5.0)
        .unwrap();
    assert_eq!(second.snapshot.health, 10.0);
    assert_eq!(
        second.snapshot.retained.last_damage_tick,
        Some(ENTITY_HURT_INVULNERABLE_TICKS)
    );
}

#[test]
fn late_spawn_restart_preserves_every_retained_tick_decision() {
    let source = SessionRegistry::new();
    source.advance_world_time(100);
    for (type_id, type_name, x) in [
        (1, "minecraft:zombie", 0.5),
        (2, "minecraft:chicken", 1.5),
        (3, "minecraft:sheep", 2.5),
    ] {
        source.spawn_command_entity(
            &SimulationAuthority::for_test(),
            type_id,
            type_name.to_owned(),
            Vec3::new(x, 64.0, 0.5),
        );
    }
    let records = source.persisted_entity_records();
    let id_for = |type_name: &str| {
        records
            .iter()
            .find(|record| record.snapshot.type_name == type_name)
            .expect("spawned entity record")
            .snapshot
            .id
    };
    let hurt_id = id_for("minecraft:zombie");
    let dying_id = id_for("minecraft:chicken");
    let sheep_id = id_for("minecraft:sheep");
    assert!(source.damage_server_entity_for_test(hurt_id, 5.0).is_some());
    assert!(
        source
            .damage_server_entity_for_test(dying_id, 100.0)
            .is_some_and(|damage| damage.killed)
    );
    assert!(source.set_sheep_grazing_ticks_for_test(sheep_id, Some(7)));
    source.advance_world_time(3);

    let checkpoint = source.persisted_entity_save_snapshot().0;
    assert_eq!(checkpoint.lifecycle_clock, 103);
    assert!(checkpoint.records.iter().all(|record| record.age == 3));
    let restored = SessionRegistry::new();
    assert_eq!(
        restored.restore_persisted_entities(checkpoint),
        3,
        "restart must restore the checkpoint atomically"
    );
    assert_eq!(restored.simulation_tick(), 103);
    let observer = register_test_session(&restored, "RestartClockObserver");
    let _ = restored.mark_loaded(observer, (0, 0));

    assert!(
        restored
            .damage_server_entity_for_test(hurt_id, 5.0)
            .is_none(),
        "three hurt-immunity ticks remain at restart"
    );
    for expected_remaining in [6, 5] {
        restored.advance_world_time(1);
        let grazing = restored
            .plan_sheep_grazing(&SimulationAuthority::for_test(), restored.simulation_tick());
        assert!(grazing.actions.is_empty());
        assert_eq!(
            restored
                .lock_entities("inspect restored grazing countdown")
                .snapshot(sheep_id)
                .expect("restored sheep")
                .retained
                .sheep_grazing_ticks,
            Some(expected_remaining)
        );
    }
    assert!(
        restored
            .damage_server_entity_for_test(hurt_id, 5.0)
            .is_none(),
        "damage remains rejected through tick 105"
    );
    restored.advance_world_time(1);
    let grazing =
        restored.plan_sheep_grazing(&SimulationAuthority::for_test(), restored.simulation_tick());
    assert_eq!(grazing.actions.len(), 1);
    assert_eq!(grazing.actions[0].entity_id, sheep_id);
    assert!(
        restored
            .damage_server_entity_for_test(hurt_id, 5.0)
            .is_some(),
        "damage is accepted at the exact restored immunity deadline"
    );

    restored.advance_world_time(13);
    assert_eq!(restored.simulation_tick(), 119);
    assert!(
        restored
            .tick_dying_entities(&SimulationAuthority::for_test(), 119)
            .is_empty()
    );
    assert!(restored.server_entity_snapshot(dying_id).is_some());
    restored.advance_world_time(1);
    let dispatches = restored.tick_dying_entities(&SimulationAuthority::for_test(), 120);
    assert!(
        dispatches.is_empty(),
        "a despawning entity is not republished to a post-restart observer"
    );
    assert!(restored.server_entity_snapshot(dying_id).is_none());
}

#[test]
fn empty_restart_restores_clock_and_invalid_clock_leaves_origin_unchanged() {
    let empty = SessionRegistry::new();
    assert_eq!(
        empty.restore_persisted_entities(PersistedEntityCheckpoint::new(
            777,
            Vec::<PersistedEntityRecord>::new(),
        )),
        0
    );
    assert_eq!(empty.simulation_tick(), 777);

    let overflow = SessionRegistry::new();
    assert_eq!(
        overflow.restore_persisted_entities(PersistedEntityCheckpoint::new(
            i64::MAX as u64 + 1,
            Vec::<PersistedEntityRecord>::new(),
        )),
        0
    );
    assert_eq!(overflow.simulation_tick(), 0);
}

#[test]
fn player_melee_knockback_pushes_living_target_away_from_player() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let (observer, _) = registry.register(
        &profile("MeleeKnockbackAlice"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(observer, (0, 0)).is_empty());
    let entity_id = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected zombie spawn dispatch, got {other:?}"),
    };

    let dispatches =
        registry.apply_player_melee_knockback_legacy_for_test(entity_id, Vec3::new(0.5, 64.0, 0.5));
    for dispatch in dispatches {
        dispatch.recipient.tx.try_send(dispatch.command).unwrap();
    }

    let velocity = registry
        .server_entity_snapshot(entity_id)
        .expect("zombie remains after non-lethal melee")
        .velocity;
    assert!(
        velocity.z > 0.0,
        "zombie should be pushed away from the player"
    );
    assert!(
        velocity.y > 0.0,
        "zombie should receive a small upward knockback"
    );
    let mut saw_knockback = false;
    while let Ok(command) = rx.try_recv() {
        if matches!(
            command,
            OutboundCommand::MoveEntityRelative(movement)
                if movement.id == entity_id && movement.send_velocity && movement.velocity.z > 0.0
        ) {
            saw_knockback = true;
        }
    }
    assert!(
        saw_knockback,
        "visible clients must receive the melee knockback velocity"
    );
}

#[test]
fn xp_orbs_are_spawned_and_found_by_pickup_radius() {
    let registry = SessionRegistry::new();

    registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);

    let nearby = registry.nearby_experience_entities(Vec3::new(1.5, 64.0, 1.0), 2.25);
    assert_eq!(nearby.len(), 1);
    assert_eq!(nearby[0].type_name, "minecraft:experience_orb");
    assert_eq!(nearby[0].experience_value, Some(5));
    assert!(
        registry
            .nearby_experience_entities(Vec3::new(10.0, 64.0, 10.0), 2.25)
            .is_empty()
    );
}

#[test]
fn xp_orb_spawn_pushes_nearby_pickup_candidate() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "SpawnPickupPushAlice");
    registry.mark_loaded(player, (0, 0));

    let dispatches = registry.spawn_xp_orb(99, Vec3::new(0.5, 64.0, 0.5), 5);

    let spawn_index = dispatches
        .iter()
        .position(|dispatch| {
            dispatch.recipient.id == player
                && matches!(dispatch.command, OutboundCommand::SpawnEntity(_))
        })
        .expect("nearby player receives XP orb spawn");
    let pickup_index = dispatches
        .iter()
        .position(|dispatch| {
            dispatch.recipient.id == player
                && matches!(
                    &dispatch.command,
                    OutboundCommand::PickupCandidates(candidates)
                        if candidates.len() == 1
                            && candidates[0].experience_value == Some(5)
                )
        })
        .expect("XP spawn pushes pickup work to the nearby player");
    assert!(spawn_index < pickup_index);
}

#[test]
fn xp_orb_spawn_pickup_snapshot_releases_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "SpawnPickupLocksAlice");
    registry.mark_loaded(player, (0, 0));
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .pickup_snapshot_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PickupSnapshotProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let spawn_registry = Arc::clone(&registry);
    let spawn =
        std::thread::spawn(move || spawn_registry.spawn_xp_orb(99, Vec3::new(0.5, 64.0, 0.5), 5));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("XP spawn reaches the ECS pickup snapshot boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entity_owner_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release XP pickup snapshot");
    let dispatches = spawn.join().expect("XP spawn worker");

    assert!(session_available, "session mutex must be released");
    assert!(
        entity_owner_available,
        "owner reads must not lend a store lock"
    );
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::PickupCandidates(candidates)
                if candidates.len() == 1 && candidates[0].experience_value == Some(5)
        )
    }));
}

#[test]
fn player_pose_event_pushes_nearby_pickup_candidates() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "PickupPushAlice");
    registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);

    let dispatches = registry.update_pose(player, PlayerPose::new(0.75, 64.0, 0.75));

    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::PickupCandidates(candidates)
                if candidates.len() == 1
                    && candidates[0].experience_value == Some(5)
        )
    }));
}

#[test]
fn pickup_candidate_snapshot_does_not_hold_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "PickupSnapshotLocksAlice");
    registry.spawn_xp_orb(99, Vec3::new(0.75, 64.0, 0.75), 5);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .pickup_snapshot_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PickupSnapshotProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let snapshot_registry = Arc::clone(&registry);
    let snapshot = std::thread::spawn(move || snapshot_registry.pickup_candidate_dispatch(player));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pickup collection reaches the ECS snapshot boundary");
    let session_available = registry.inner.try_lock().is_ok();
    let entity_owner_available = registry.entities.owner_responsive_for_test();
    resume_tx.send(()).expect("release pickup snapshot");
    let dispatch = snapshot
        .join()
        .expect("pickup snapshot worker")
        .expect("nearby XP candidate dispatch");

    assert!(
        session_available,
        "pickup ECS snapshot must not retain session state"
    );
    assert!(
        entity_owner_available,
        "owner reads must not lend a store lock"
    );
    assert!(matches!(
        dispatch.command,
        OutboundCommand::PickupCandidates(candidates)
            if candidates.len() == 1 && candidates[0].experience_value == Some(5)
    ));
}

#[test]
fn pickup_candidate_publication_rechecks_player_position() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "PickupSnapshotMoveAlice");
    registry.spawn_xp_orb(99, Vec3::new(0.75, 64.0, 0.75), 5);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .pickup_snapshot_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PickupSnapshotProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let snapshot_registry = Arc::clone(&registry);
    let snapshot = std::thread::spawn(move || snapshot_registry.pickup_candidate_dispatch(player));
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pickup collection reaches the ECS snapshot boundary");
    {
        let mut inner = registry.lock_inner("move player during pickup snapshot");
        inner
            .sessions
            .get_mut(&player)
            .expect("pickup session remains connected")
            .pose = PlayerPose::new(100.0, 64.0, 100.0);
    }
    resume_tx.send(()).expect("release pickup snapshot");

    assert!(
        snapshot.join().expect("pickup snapshot worker").is_none(),
        "pickup publication must reject candidates outside the player's current radius"
    );
}

#[test]
fn owner_blocked_item_does_not_push_pickup_candidate_to_owner() {
    let registry = SessionRegistry::new();
    let owner = register_test_session(&registry, "BlockedPickupOwner");
    let collector = register_test_session(&registry, "BlockedPickupCollector");
    registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let expires_tick = registry.simulation_tick().saturating_add(10);
    {
        let mut inner = registry.lock_session_entities("test owner pickup block");
        let expected = inner.entities.snapshot(entity_id).unwrap();
        let mut next = expected.clone();
        next.retained.item_pickup_owner_block = Some(mc_entity::EntityItemPickupOwnerBlock {
            owner_session: owner,
            expires_tick,
        });
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    let owner_dispatches = registry.update_pose(owner, PlayerPose::new(0.6, 64.0, 0.5));
    let collector_dispatches = registry.update_pose(collector, PlayerPose::new(0.6, 64.0, 0.5));

    assert!(!owner_dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::PickupCandidates(candidates)
                if candidates.iter().any(|candidate| candidate.id == entity_id)
        )
    }));
    assert!(collector_dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::PickupCandidates(candidates)
                if candidates.iter().any(|candidate| candidate.id == entity_id)
        )
    }));
}

#[test]
fn nearby_entity_candidates_only_visit_chunks_touched_by_the_radius() {
    let registry = SessionRegistry::new();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(15.0, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(16.5, 64.0, 0.5),
    );
    for index in 0..20 {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:cow".to_owned(),
            Vec3::new(160.5 + f64::from(index), 64.0, 160.5),
        );
    }

    let inner = registry.lock_session_entities("test nearby entity candidates");
    let candidates = nearby_entity_candidate_ids_locked(&inner, Vec3::new(15.5, 64.0, 0.5), 2.25);

    assert_eq!(candidates.len(), 2);
}

#[test]
fn arrow_entity_candidates_only_visit_chunks_touched_by_segment() {
    let registry = SessionRegistry::new();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(15.0, 64.0, 8.0),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:cow".to_owned(),
        Vec3::new(16.5, 64.0, 8.0),
    );
    for index in 0..20 {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:cow".to_owned(),
            Vec3::new(160.5 + f64::from(index) * 16.0, 64.0, 160.5),
        );
    }

    let inner = registry.lock_session_entities("test arrow entity candidates");
    let candidates = arrow_entity_candidate_snapshots_locked(
        &inner,
        Vec3::new(14.5, 64.5, 8.0),
        Vec3::new(17.0, 64.5, 8.0),
    );

    assert_eq!(candidates.len(), 2);
}

#[test]
fn hostile_entities_are_filtered_by_kind_and_radius() {
    let registry = SessionRegistry::new();
    let observer = register_test_session(&registry, "HostileQueryAlice");
    assert!(registry.mark_loaded(observer, (0, 0)).is_empty());
    let spawn_dispatches = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(1.0, 64.0, 1.0),
    );
    let zombie = match &spawn_dispatches[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected zombie spawn dispatch, got {other:?}"),
    };
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_owned(),
        Vec3::new(1.5, 64.0, 1.0),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(20.0, 64.0, 20.0),
    );
    assert_eq!(
        registry.restore_persisted_entities(PersistedEntityCheckpoint::new(
            0,
            vec![PersistedEntityRecord {
                snapshot: mc_entity::EntitySnapshot {
                    id: mc_entity::EntityId(777),
                    uuid: uuid::Uuid::nil(),
                    type_id: 1,
                    type_name: "minecraft:zombie".into(),
                    position: Vec3::new(1.25, 64.0, 1.0),
                    rotation: mc_entity::Rotation::ZERO,
                    velocity: Vec3::ZERO,
                    on_ground: true,
                    item_stack: Some(EntityItemStack::new(42, 1)),
                    experience_value: None,
                    block_state: None,
                    lifecycle: mc_entity::EntityLifecycle::Alive,
                    health: 20.0,
                    attributes: mc_entity::AttributeSet::new(),
                    goal: mc_entity::GoalState::Idle,
                    vehicle: None,
                    animal: None,
                    retained: mc_entity::EntityRetainedState::default(),
                },
                age: 0,
                pickup_delay: 0,
            }],
        )),
        1
    );

    let nearby = registry.nearby_hostile_entities(Vec3::new(1.0, 64.0, 1.0), 2.25);

    assert_eq!(nearby.len(), 1);
    assert_eq!(nearby[0].id, zombie);
    assert_eq!(nearby[0].type_name, "minecraft:zombie");
}

#[test]
fn bed_rest_prevention_uses_vanilla_cuboid() {
    let registry = SessionRegistry::new();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(8.75, 68.5, 8.75),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );

    assert!(
        registry.has_rest_preventing_hostile_near_bed(mc_world::BlockPos { x: 0, y: 64, z: 0 })
    );
    assert!(
        !registry.has_rest_preventing_hostile_near_bed(mc_world::BlockPos {
            x: 32,
            y: 64,
            z: 32,
        })
    );
}
