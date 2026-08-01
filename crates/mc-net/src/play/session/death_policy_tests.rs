use super::*;
use mc_protocol::packets::play::{GameMode, ItemStack};
use mc_script::{ScriptEventKind, ScriptGameMode};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::play::persistence::{PlayerPersistedState, XpState};
use crate::play::simulation::{
    PlayerSurvivalCommitOutcome, PlayerSurvivalPlan, SimulationAuthority,
};
use crate::play::{PlayerInventory, PlayerPose, SurvivalState, recoverable_death_xp};

fn register_policy_session(registry: &SessionRegistry, name: &str) -> SessionId {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_string(),
    };
    let (tx, _rx) = mpsc::channel(8);
    registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0
}

#[test]
fn player_death_inventory_xp_policy_is_atomic_and_idempotent() {
    for (game_mode, keep_inventory, expect_drops) in [
        (GameMode::Survival, false, true),
        (GameMode::Adventure, false, true),
        (GameMode::Survival, true, false),
        (GameMode::Creative, false, false),
        (GameMode::Spectator, false, false),
    ] {
        let registry = SessionRegistry::new();
        let case_name = format!("DP{:?}{}", game_mode, u8::from(keep_inventory));
        let session = register_policy_session(&registry, &case_name);
        registry.set_keep_inventory(keep_inventory);
        let mut deaths = registry.install_script_commit_event_outbox();
        let pose = PlayerPose::new(7.5, 70.0, -2.5);
        let mut persisted = PlayerPersistedState::new_default(pose);
        persisted.game_mode = game_mode;
        persisted.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 3);
        persisted.carried_item = ItemStack::new(43, 2);
        persisted.xp = XpState {
            level: 3,
            progress: 0.5,
            total: 25,
            seed: 9,
        };
        let persisted = Arc::new(Mutex::new(persisted));
        registry.register_player_persistence(session, Arc::clone(&persisted));

        let before = persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut dead = before.survival;
        dead.apply_damage(SurvivalState::MAX_HEALTH);
        let plan = PlayerSurvivalPlan {
            expected_survival: before.survival,
            updated_survival: dead,
            expected_inventory: before.inventory.clone(),
            updated_inventory: before.inventory.clone(),
            expected_carried_item: before.carried_item.clone(),
            expected_xp: before.xp.clone(),
            updated_xp: before.xp.clone(),
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: Some(1),
            xp_orb_entity_type_id: Some(2),
            keep_inventory,
            position: Vec3::new(pose.x, pose.y, pose.z),
        };

        let first = registry
            .commit_player_survival(&SimulationAuthority::for_test(), session, &plan)
            .expect("registered player death commit");
        let PlayerSurvivalCommitOutcome::Committed(first) = first else {
            panic!("fresh death policy commit must apply")
        };
        assert!(first.died, "{game_mode:?} keep={keep_inventory}");
        let after = persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let records = registry.persisted_entity_records();

        if expect_drops {
            assert!(after.inventory.slots[1..].iter().all(ItemStack::is_empty));
            assert!(after.carried_item.is_empty());
            assert_eq!(
                after.xp,
                XpState {
                    seed: before.xp.seed,
                    ..XpState::default()
                }
            );
            let mut stacks = records
                .iter()
                .filter_map(|record| record.snapshot.item_stack.clone())
                .map(|stack| (stack.item_id, stack.count))
                .collect::<Vec<_>>();
            stacks.sort_unstable();
            assert_eq!(stacks, vec![(42, 3), (43, 2)]);
            assert!(records.iter().any(|record| {
                record.snapshot.experience_value == Some(recoverable_death_xp(&before.xp))
            }));
            assert_eq!(records.len(), 3);
        } else {
            assert_eq!(after.inventory.slots, before.inventory.slots);
            assert_eq!(after.carried_item, before.carried_item);
            assert_eq!(after.xp, before.xp);
            assert!(records.is_empty());
        }

        match game_mode {
            GameMode::Survival | GameMode::Adventure => {
                let expected_mode = if game_mode == GameMode::Survival {
                    ScriptGameMode::Survival
                } else {
                    ScriptGameMode::Adventure
                };
                let event = deaths.try_recv_required().unwrap_or_else(|error| {
                    panic!("missing {expected_mode:?} death event for {game_mode:?}: {error:?}")
                });
                match event.kind() {
                    ScriptEventKind::PlayerDied {
                        game_mode: event_mode,
                        ..
                    } => assert_eq!(*event_mode, expected_mode),
                    other => panic!("unexpected death event for {game_mode:?}: {other:?}"),
                }
            }
            GameMode::Creative | GameMode::Spectator => {
                assert!(matches!(
                    deaths.try_recv_required(),
                    Err(mpsc::error::TryRecvError::Empty)
                ));
            }
        }
        assert!(matches!(
            deaths.try_recv_required(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let mut snapshots_before_duplicate = records
            .into_iter()
            .map(|record| record.snapshot)
            .collect::<Vec<_>>();
        snapshots_before_duplicate.sort_unstable_by_key(|snapshot| snapshot.id);
        let duplicate_plan = PlayerSurvivalPlan {
            expected_survival: after.survival,
            updated_survival: after.survival,
            expected_inventory: after.inventory.clone(),
            updated_inventory: after.inventory.clone(),
            expected_carried_item: after.carried_item.clone(),
            expected_xp: after.xp.clone(),
            updated_xp: after.xp.clone(),
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: Some(1),
            xp_orb_entity_type_id: Some(2),
            keep_inventory,
            position: Vec3::new(pose.x, pose.y, pose.z),
        };
        let duplicate = registry
            .commit_player_survival(&SimulationAuthority::for_test(), session, &duplicate_plan)
            .expect("registered duplicate death commit");
        assert!(matches!(
            duplicate,
            PlayerSurvivalCommitOutcome::Committed(committed) if !committed.died
        ));
        let mut snapshots_after_duplicate = registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot)
            .collect::<Vec<_>>();
        snapshots_after_duplicate.sort_unstable_by_key(|snapshot| snapshot.id);
        assert_eq!(snapshots_after_duplicate, snapshots_before_duplicate);
        assert!(matches!(
            deaths.try_recv_required(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[test]
fn stale_keep_inventory_death_plan_rejects_without_side_effects() {
    let registry = SessionRegistry::new();
    let session = register_policy_session(&registry, "DPStale");
    registry.set_keep_inventory(true);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let mut state = PlayerPersistedState::new_default(pose);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 3);
    state.xp = XpState {
        level: 3,
        progress: 0.5,
        total: 25,
        seed: 9,
    };
    let persisted = Arc::new(Mutex::new(state));
    registry.register_player_persistence(session, Arc::clone(&persisted));
    let before = persisted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let mut dead = before.survival;
    dead.apply_damage(SurvivalState::MAX_HEALTH);

    let outcome = registry
        .commit_player_survival(
            &SimulationAuthority::for_test(),
            session,
            &PlayerSurvivalPlan {
                expected_survival: before.survival,
                updated_survival: dead,
                expected_inventory: before.inventory.clone(),
                updated_inventory: before.inventory.clone(),
                expected_carried_item: before.carried_item.clone(),
                expected_xp: before.xp.clone(),
                updated_xp: before.xp.clone(),
                active_shield: None,
                enchanting_table_input: None,
                item_entity_type_id: Some(1),
                xp_orb_entity_type_id: Some(2),
                keep_inventory: false,
                position: Vec3::new(pose.x, pose.y, pose.z),
            },
        )
        .expect("registered stale death plan");

    assert!(matches!(outcome, PlayerSurvivalCommitOutcome::Rejected(_)));
    let after = persisted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(after.survival, before.survival);
    assert_eq!(after.inventory.slots, before.inventory.slots);
    assert_eq!(after.carried_item, before.carried_item);
    assert_eq!(after.xp, before.xp);
    assert!(registry.persisted_entity_records().is_empty());
}
