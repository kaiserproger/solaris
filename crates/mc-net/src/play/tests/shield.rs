use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_entity::{EntityId, Vec3};
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{
    EntityDataValue, GameMode, ItemStack, LIVING_ENTITY_DATA_FLAGS_INDEX,
    LIVING_ENTITY_FLAG_OFF_HAND, LIVING_ENTITY_FLAG_USING_ITEM,
};
use tokio::sync::mpsc;

use crate::error::ConnectionError;
use crate::login::LoggedInProfile;

use super::super::combat::{
    PlayerDamageKind, PlayerDamageRequest, SHIELD_FALLBACK_MAX_DAMAGE, ShieldUseState,
    shield_blocks_damage, shield_use_flags, shield_use_from_stack,
};
use super::super::commands::CommandPermissions;
use super::super::inventory::PlayerInventory;
use super::super::persistence::{PlayerPersistedState, XpState};
use super::super::player_damage_adapter::{PlayerDamageApplication, apply_player_damage};
use super::super::session::{self, EntityAttackOutcome, PlayerAttackResult};
use super::super::simulation::{self, SIMULATION_COMMAND_BATCH_LIMIT};
use super::super::survival::SurvivalState;
use super::super::{PlayerPose, damage_active_shield, shield_use_entity_data_value};
use super::{
    decode_container_set_slot_packets, register_survival_test_player, shield_item_state,
    simulation_channel, start_survival_test_owner,
};

#[test]
fn shield_use_starts_blocking_state_for_shield_stack() {
    let stack = ItemStack::new(77, 1);

    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        stack.clone(),
        12,
        true,
    )
    .expect("shield stack should start shield use");

    assert_eq!(shield_use.started_tick, 12);
    assert_eq!(shield_use.slot, PlayerInventory::HOTBAR_BASE);
    assert_eq!(shield_use.stack, stack);
}

#[test]
fn shield_use_metadata_uses_vanilla_living_entity_flags() {
    let main_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };
    let off_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert_eq!(shield_use_flags(None), 0);
    assert_eq!(
        shield_use_flags(Some(&main_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM
    );
    assert_eq!(
        shield_use_flags(Some(&off_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND
    );
    assert_eq!(
        shield_use_entity_data_value(Some(&off_hand)),
        EntityDataValue::Byte {
            index: LIVING_ENTITY_DATA_FLAGS_INDEX,
            value: LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND,
        }
    );
}

#[test]
fn shield_non_shield_use_does_not_block() {
    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        ItemStack::new(77, 1),
        12,
        false,
    );

    assert!(shield_use.is_none());
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        20,
        shield_use.as_ref(),
    ));
}

#[test]
fn shield_activation_delay_gates_damage() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 10,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        14,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        15,
        Some(&shield_use),
    ));
}

#[test]
fn shield_blocks_frontal_mob_and_arrow_sources() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 2.0)),
        10,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        90.0,
        Some(Vec3::new(-2.0, 0.0, 0.0)),
        10,
        Some(&shield_use),
    ));
}

#[test]
fn shield_side_boundary_blocks_but_back_and_unknown_sources_do_not() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(2.0, 0.0, 0.0)),
        10,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, -2.0)),
        10,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        None,
        10,
        Some(&shield_use),
    ));
}

#[test]
fn shield_block_damages_active_shield_stack() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    let changed = damage_active_shield(&mut state, 3.75).expect("shield should take durability");

    assert_eq!(changed, (slot, ItemStack::new(77, 1).with_damage(4)));
    assert_eq!(
        state.inventory.slots[slot],
        ItemStack::new(77, 1).with_damage(4)
    );
    assert_eq!(
        state.shield_use.as_ref().unwrap().stack,
        state.inventory.slots[slot]
    );
}

#[test]
fn shield_block_removes_broken_active_shield() {
    let mut state = shield_item_state();
    state.inventory.slots[45] = ItemStack::new(77, 1).with_damage(SHIELD_FALLBACK_MAX_DAMAGE - 4);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::OffHand,
        45,
        state.inventory.slots[45].clone(),
        1,
        true,
    );

    let changed = damage_active_shield(&mut state, 3.0).expect("shield break should update slot");

    assert_eq!(changed, (45, ItemStack::EMPTY));
    assert_eq!(state.inventory.slots[45], ItemStack::EMPTY);
    assert!(state.shield_use.is_none());
}

#[test]
fn permitted_game_mode_transition_clears_active_shield_immediately() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    super::super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Creative,
        CommandPermissions::from_op(true),
    );

    assert!(state.shield_use.is_none());
}

#[test]
fn denied_or_noop_game_mode_transition_keeps_active_shield() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    super::super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Creative,
        CommandPermissions::from_op(false),
    );
    assert!(state.shield_use.is_some());

    super::super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Survival,
        CommandPermissions::from_op(true),
    );
    assert!(state.shield_use.is_some());
}

#[tokio::test]
async fn projectile_shield_block_writes_scaled_slot_update() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "ProjectileShield", survival_state, &xp_state);
    let mut writer = Vec::new();

    let damage_applied = apply_player_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::Projectile,
                amount: 4.2,
                source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert!(
        !damage_applied,
        "a shielded hit must not authorize knockback"
    );
    assert_eq!(
        state.inventory.slots[slot],
        ItemStack::new(77, 1).with_damage(5)
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, slot as i16);
    assert_eq!(packets[0].item_stack, ItemStack::new(77, 1).with_damage(5));
}

#[tokio::test]
async fn authoritative_pvp_shield_block_refreshes_local_identity_before_retry() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let shield_slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[shield_slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        shield_slot,
        state.inventory.slots[shield_slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (session_id, _) = register_survival_test_player(
        &mut state,
        "ProjectileShieldCasRace",
        survival_state,
        &xp_state,
    );
    let attacker_profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ProjectileShieldCasAttacker"),
        name: "ProjectileShieldCasAttacker".to_owned(),
    };
    let attacker_pose = PlayerPose::new(0.5, 64.0, 2.5);
    let (attacker_tx, _attacker_rx) = mpsc::channel(8);
    let (attacker_session, _) = state.sessions.register(
        &attacker_profile,
        (0, 0),
        0,
        HashSet::new(),
        attacker_tx,
        attacker_pose,
    );
    state.sessions.register_player_persistence(
        attacker_session,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(attacker_pose))),
    );
    let (simulation, mut owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);

    let sessions = Arc::clone(&state.sessions);
    let owner_sessions = Arc::clone(&sessions);
    let (request_queued_tx, request_queued_rx) = tokio::sync::oneshot::channel();
    let (process_request_tx, process_request_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        assert!(owner.wait_for_command().await);
        request_queued_tx
            .send(())
            .expect("shield commit waiter remains active");
        process_request_rx
            .await
            .expect("test releases the queued shield commit");
        owner.process_tick(&owner_sessions, SIMULATION_COMMAND_BATCH_LIMIT);
        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    owner.shutdown();
                    break;
                }
                ready = owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    owner.process_tick(&owner_sessions, SIMULATION_COMMAND_BATCH_LIMIT);
                }
            }
        }
    });

    let mut writer = Vec::new();
    let damage_applied = {
        let damage = apply_player_damage(
            Some(&mut state),
            &mut writer,
            Compression::Disabled,
            &mut survival_state,
            &mut xp_state,
            GameMode::Survival,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.0, 64.0, 0.0),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::Projectile,
                    amount: 4.2,
                    source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
                },
            },
        );
        tokio::pin!(damage);
        tokio::select! {
            reached = request_queued_rx => {
                reached.expect("owner observes the queued shield commit");
            }
            result = &mut damage => {
                panic!("shield damage completed before its owner CAS race: {result:?}");
            }
        }

        let pvp_result = sessions.player_attack_entity(
            &simulation::SimulationAuthority::for_test(),
            session::PlayerEntityAttack {
                attacker_session,
                entity_id: EntityId(i32::try_from(session_id).unwrap()),
                amount: 4.0,
                attacker_costs: None,
                authority_tick: sessions.simulation_tick(),
            },
        );
        assert!(matches!(
            pvp_result,
            PlayerAttackResult::Damaged(outcome)
                if matches!(
                    *outcome,
                    EntityAttackOutcome::PlayerDamaged {
                        damage_applied: false,
                        ..
                    }
                )
        ));
        process_request_tx
            .send(())
            .expect("release the queued shield commit");

        damage.await.unwrap()
    };
    stop_tx.send(()).unwrap();
    owner_task.await.unwrap();

    assert!(
        !damage_applied,
        "the successfully retried shield blocks the hit"
    );
    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert_eq!(
        state.inventory.slots[shield_slot],
        ItemStack::new(77, 1).with_damage(10)
    );
    assert_eq!(
        state.shield_use.as_ref().map(|shield| &shield.stack),
        Some(&state.inventory.slots[shield_slot]),
        "the rejected stale attempt must not clear the still-authoritative shield"
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, shield_slot as i16);
    assert_eq!(packets[0].item_stack, state.inventory.slots[shield_slot]);
}

#[tokio::test]
async fn repeated_shield_cas_conflict_refreshes_owner_state_and_fails_closed() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let shield_slot = PlayerInventory::HOTBAR_BASE;
    let first_changed_slot = 10;
    let second_changed_slot = 11;
    state.inventory.slots[shield_slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        shield_slot,
        state.inventory.slots[shield_slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (session_id, persisted) = register_survival_test_player(
        &mut state,
        "ProjectileShieldRepeatedCasRace",
        survival_state,
        &xp_state,
    );
    let (simulation, mut owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);

    let sessions = Arc::clone(&state.sessions);
    let (first_queued_tx, first_queued_rx) = tokio::sync::oneshot::channel();
    let (process_first_tx, process_first_rx) = tokio::sync::oneshot::channel();
    let (second_queued_tx, second_queued_rx) = tokio::sync::oneshot::channel();
    let (process_second_tx, process_second_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        assert!(owner.wait_for_command().await);
        first_queued_tx
            .send(())
            .expect("first shield commit waiter remains active");
        process_first_rx
            .await
            .expect("test releases the first shield commit");
        owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);

        assert!(owner.wait_for_command().await);
        second_queued_tx
            .send(())
            .expect("retry shield commit waiter remains active");
        process_second_rx
            .await
            .expect("test releases the retry shield commit");
        owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);

        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    owner.shutdown();
                    break;
                }
                ready = owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);
                }
            }
        }
    });

    let mut writer = Vec::new();
    let error = {
        let damage = apply_player_damage(
            Some(&mut state),
            &mut writer,
            Compression::Disabled,
            &mut survival_state,
            &mut xp_state,
            GameMode::Survival,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.0, 64.0, 0.0),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::Projectile,
                    amount: 4.2,
                    source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
                },
            },
        );
        tokio::pin!(damage);

        tokio::select! {
            reached = first_queued_rx => {
                reached.expect("owner observes the first shield commit");
            }
            result = &mut damage => {
                panic!("shield damage completed before the first owner conflict: {result:?}");
            }
        }
        persisted.lock().unwrap().inventory.slots[first_changed_slot] = ItemStack::new(91, 1);
        process_first_tx
            .send(())
            .expect("release the first shield commit");

        tokio::select! {
            reached = second_queued_rx => {
                reached.expect("owner observes the bounded shield retry");
            }
            result = &mut damage => {
                panic!("shield damage completed before the retry owner conflict: {result:?}");
            }
        }
        persisted.lock().unwrap().inventory.slots[second_changed_slot] = ItemStack::new(92, 1);
        process_second_tx
            .send(())
            .expect("release the retry shield commit");

        damage
            .await
            .expect_err("a repeated exact-owner conflict must fail closed")
    };
    stop_tx.send(()).unwrap();
    owner_task.await.unwrap();

    assert!(matches!(
        error,
        ConnectionError::RuntimeUnavailable {
            operation: "committing shield durability after repeated owner state change"
        }
    ));
    assert_eq!(
        state.inventory.slots[first_changed_slot],
        ItemStack::new(91, 1)
    );
    assert_eq!(
        state.inventory.slots[second_changed_slot],
        ItemStack::new(92, 1)
    );
    assert_eq!(state.inventory.slots[shield_slot], ItemStack::new(77, 1));
    assert_eq!(
        state.shield_use.as_ref().map(|shield| &shield.stack),
        Some(&state.inventory.slots[shield_slot])
    );
    assert_eq!(survival_state, SurvivalState::FULL);
    assert!(writer.is_empty());
}
