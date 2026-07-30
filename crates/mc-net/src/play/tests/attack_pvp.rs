use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use mc_data::{
    Identifier,
    item_components::ItemFactsTable,
    items::{ItemRegistry, ItemReport},
};
use mc_entity::{EntityItemStack, Vec3};
use mc_protocol::{
    Compression,
    packets::play::{GameMode, ItemStack, ServerboundAttack},
};

use crate::play::{
    ActiveShield, ENTITY_HURT_INVULNERABLE_TICKS, ITEM_PICKUP_DELAY_TICKS, InteractionState,
    LoggedInProfile, OutboundCommand, PlayerDamageApplication, PlayerDamageKind,
    PlayerDamageRequest, PlayerHurtResistance, PlayerHurtResolution, PlayerPersistedState,
    PlayerPose, SHIELD_ACTIVATION_DELAY_TICKS, SHIELD_FALLBACK_MAX_DAMAGE, SurvivalState, XpState,
    apply_player_damage, arrow_entity_type_id, begin_player_attack_attempt, handle_attack,
    held_attack_damage_at_tick, held_attack_speed, item_entity_type_id, mpsc, simulation,
    xp_orb_entity_type_id,
};

use super::{interaction_state_for_items, spawn_test_simulation_owner, start_survival_test_owner};

fn assert_attack_damage_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.000_01,
        "expected attack damage {expected}, got {actual}"
    );
}

pub(super) fn attack_strength_test_state() -> (InteractionState, u32, u32) {
    let sword_name = Identifier::parse("minecraft:stone_sword").unwrap();
    let axe_name = Identifier::parse("minecraft:stone_axe").unwrap();
    let shield_name = Identifier::parse("minecraft:shield").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: sword_name.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: axe_name.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: shield_name.clone(),
            protocol_id: 12,
        },
    ]));
    let sword = items.id_of(&sword_name).unwrap();
    let axe = items.id_of(&axe_name).unwrap();
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(ItemFactsTable::from_entries([
        (
            sword_name,
            mc_data::item_components::ItemFacts {
                attack_speed_modifier: Some(-2.4),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            axe_name,
            mc_data::item_components::ItemFacts {
                attack_speed_modifier: Some(-3.2),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            shield_name,
            mc_data::item_components::ItemFacts {
                max_damage: Some(SHIELD_FALLBACK_MAX_DAMAGE as u32),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]));
    (state, sword, axe)
}

#[test]
fn empty_hand_attack_strength_scales_partial_and_full_damage() {
    let (state, _, _) = attack_strength_test_state();

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        4.0,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            102,
        ),
        0.4,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            105,
        ),
        1.0,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            None,
            0,
        ),
        1.0,
    );
}

#[test]
fn sword_attack_speed_modifier_scales_partial_and_full_damage() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        1.6,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            106,
        ),
        3.121_599_7,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            112,
        ),
        7.0,
    );
}

#[test]
fn axe_attack_speed_modifier_scales_partial_and_full_damage() {
    let (mut state, _, axe) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(axe, 1);

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        0.8,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            112,
        ),
        2.8,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            125,
        ),
        7.0,
    );
}

#[test]
fn attack_damage_scales_all_playable_modes_without_recording_before_validation() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);

    for game_mode in [GameMode::Survival, GameMode::Adventure, GameMode::Creative] {
        state.last_entity_attack_tick = Some(100);
        let damage = begin_player_attack_attempt(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            game_mode,
            state.last_entity_attack_tick,
            106,
        )
        .expect("non-spectator attack attempt");

        assert_attack_damage_close(damage, 2.081_599_7);
        assert_eq!(state.last_entity_attack_tick, Some(100));
    }

    state.last_entity_attack_tick = Some(100);
    assert_eq!(
        begin_player_attack_attempt(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            GameMode::Spectator,
            state.last_entity_attack_tick,
            106,
        ),
        None
    );
    assert_eq!(state.last_entity_attack_tick, Some(100));
}

#[test]
fn pvp_hurt_resistance_rejects_weaker_hits_and_applies_stronger_difference() {
    let mut resistance = PlayerHurtResistance::default();

    assert_eq!(
        resistance.resolve(100, 5.0),
        PlayerHurtResolution::Apply {
            amount: 5.0,
            fresh_hurt: true,
        }
    );
    assert_eq!(resistance.resolve(100, 1.0), PlayerHurtResolution::Rejected);
    assert_eq!(
        resistance.resolve(100, 7.0),
        PlayerHurtResolution::Apply {
            amount: 2.0,
            fresh_hurt: false,
        }
    );
    assert_eq!(resistance.resolve(109, 7.0), PlayerHurtResolution::Rejected);
    assert_eq!(
        resistance.resolve(110, 3.0),
        PlayerHurtResolution::Apply {
            amount: 3.0,
            fresh_hurt: true,
        }
    );
}

#[test]
fn queued_same_tick_pvp_hits_share_one_victim_hurt_resistance_state() {
    let mut resistance = PlayerHurtResistance::default();

    let first = resistance.resolve(42, 4.0);
    let second = resistance.resolve(42, 4.0);

    assert!(matches!(first, PlayerHurtResolution::Apply { .. }));
    assert_eq!(second, PlayerHurtResolution::Rejected);
}

#[test]
fn hurt_resistance_preview_changes_state_only_after_authority_commit() {
    let resistance = PlayerHurtResistance::default();
    let (first, committed) = resistance.preview(42, 4.0);
    let (retry, _) = resistance.preview(42, 4.0);

    assert!(matches!(first, PlayerHurtResolution::Apply { .. }));
    assert_eq!(
        retry, first,
        "a rejected commit must leave resistance unchanged"
    );
    assert_eq!(
        committed.preview(42, 4.0).0,
        PlayerHurtResolution::Rejected,
        "the next hit is rejected only after the transition is committed"
    );
}

#[tokio::test]
async fn adventure_player_accepts_pvp_damage() {
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let mut writer = Vec::new();

    let applied = apply_player_damage(
        None,
        &mut writer,
        Compression::Disabled,
        &mut survival,
        &mut xp,
        GameMode::Adventure,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.5, 64.0, 0.5),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::PlayerAttack,
                amount: 4.0,
                source_origin: Some(Vec3::new(0.5, 64.0, 1.5)),
            },
        },
    )
    .await
    .unwrap();

    assert!(applied);
    assert_eq!(survival.health, 16.0);
}

#[tokio::test]
async fn creative_and_spectator_players_reject_pvp_damage() {
    for game_mode in [GameMode::Creative, GameMode::Spectator] {
        let mut survival = SurvivalState::FULL;
        let mut xp = XpState::default();
        let mut writer = Vec::new();

        let applied = apply_player_damage(
            None,
            &mut writer,
            Compression::Disabled,
            &mut survival,
            &mut xp,
            game_mode,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.5, 64.0, 0.5),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::PlayerAttack,
                    amount: 4.0,
                    source_origin: Some(Vec3::new(0.5, 64.0, 1.5)),
                },
            },
        )
        .await
        .unwrap();

        assert!(!applied);
        assert_eq!(survival, SurvivalState::FULL);
        assert!(writer.is_empty());
    }
}

async fn run_pvp_commit_cost_case(
    damage_expected: bool,
    keep_target_queue: bool,
) -> (InteractionState, SurvivalState) {
    let (mut state, sword, _) = attack_strength_test_state();
    let sword_name = Identifier::parse("minecraft:stone_sword").unwrap();
    let shield_name = Identifier::parse("minecraft:shield").unwrap();
    state.item_facts = Arc::new(ItemFactsTable::from_entries([
        (
            sword_name,
            mc_data::item_components::ItemFacts {
                max_damage: Some(131),
                weapon: true,
                weapon_damage_per_attack: Some(1),
                attack_damage_modifier: Some(4.0),
                attack_speed_modifier: Some(-2.4),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            shield_name.clone(),
            mc_data::item_components::ItemFacts {
                max_damage: Some(SHIELD_FALLBACK_MAX_DAMAGE as u32),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]));
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);
    let mut survival = SurvivalState {
        exhaustion: 3.95,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "PvpCostAttacker", survival, &xp);

    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(if damage_expected { 902 } else { 901 }),
        name: if damage_expected {
            "PvpCostAccepted".to_owned()
        } else {
            "PvpCostRejected".to_owned()
        },
    };
    let mut target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    target_pose.yaw = 180.0;
    let (target_tx, target_rx) = mpsc::channel(8);
    let mut target_rx = keep_target_queue.then_some(target_rx);
    let (target_session, _) = state.sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::new(),
        target_tx,
        target_pose,
    );
    let mut target_state = PlayerPersistedState::new_default(target_pose);
    if !damage_expected {
        let shield = state.items.id_of(&shield_name).unwrap();
        target_state.inventory.slots[45] = ItemStack::new(shield, 1);
    }
    let target_state = Arc::new(Mutex::new(target_state));
    state
        .sessions
        .register_player_persistence(target_session, Arc::clone(&target_state));
    if !damage_expected {
        state.sessions.set_active_shield(
            target_session,
            Some(ActiveShield {
                started_tick: state.sessions.world_time(),
                slot: 45,
                expected_stack: target_state.lock().unwrap().inventory.slots[45].clone(),
            }),
        );
        state
            .sessions
            .advance_world_time(SHIELD_ACTIVATION_DELAY_TICKS);
    }
    let mut writer = Vec::new();

    tokio::time::timeout(
        Duration::from_secs(1),
        handle_attack(
            &mut state,
            &mut writer,
            GameMode::Survival,
            &mut survival,
            &mut xp,
            PlayerPose::new(0.5, 64.0, 0.5),
            ServerboundAttack {
                entity_id: i32::try_from(target_session).unwrap(),
            },
        ),
    )
    .await
    .expect("PvP authority must not wait for the target connection loop")
    .unwrap();

    assert!(
        state.last_entity_attack_tick.is_some(),
        "reachable target must pass PvP validation"
    );
    assert_eq!(
        target_state.lock().unwrap().survival.health < SurvivalState::FULL.health,
        damage_expected
    );
    if !damage_expected {
        assert!(
            target_state.lock().unwrap().inventory.slots[45]
                .damage
                .is_some(),
            "shield durability must commit before hurt resistance"
        );
    }
    if let Some(target_rx) = target_rx.as_mut() {
        let command = tokio::time::timeout(Duration::from_secs(1), target_rx.recv())
            .await
            .expect("PvP commit must publish to the target connection");
        let Some(OutboundCommand::PlayerDamageCommitted { publication, .. }) = command else {
            panic!("PvP commit must publish committed target state");
        };
        assert_eq!(publication.shield_blocked, !damage_expected);
        assert!(
            publication.knockback.is_some(),
            "fresh damage and a full shield block both publish target knockback"
        );
    }

    stop.send(()).unwrap();
    owner_task.await.unwrap();
    (state, survival)
}

#[tokio::test]
async fn authoritative_pvp_commit_gates_exhaustion_and_weapon_durability() {
    let (rejected, rejected_survival) = run_pvp_commit_cost_case(false, true).await;
    assert!(rejected.last_entity_attack_tick.is_some());
    assert_eq!(rejected_survival.saturation, 5.0);
    assert_eq!(rejected_survival.exhaustion, 3.95);
    assert_eq!(rejected.inventory.held(0).unwrap().damage, None);
    let rejected_persisted = rejected
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("PvpCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("rejected attacker authority state remains registered");
    assert_eq!(rejected_persisted.inventory.held(0).unwrap().damage, None);
    assert_eq!(rejected_persisted.survival, rejected_survival);

    let (accepted, accepted_survival) = run_pvp_commit_cost_case(true, true).await;
    assert!(accepted.last_entity_attack_tick.is_some());
    assert_eq!(accepted_survival.saturation, 4.0);
    assert!((accepted_survival.exhaustion - 0.05).abs() < 0.000_01);
    assert_eq!(accepted.inventory.held(0).unwrap().damage, Some(1));
    let persisted = accepted
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("PvpCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("attacker authority state remains registered");
    assert_eq!(persisted.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(persisted.survival, accepted_survival);
}

#[tokio::test]
async fn dropped_target_publication_does_not_undo_authoritative_pvp_costs() {
    let (attacker, survival) = run_pvp_commit_cost_case(true, false).await;

    assert!(attacker.last_entity_attack_tick.is_some());
    assert_eq!(survival.saturation, 4.0);
    assert!((survival.exhaustion - 0.05).abs() < 0.000_01);
    assert_eq!(attacker.inventory.held(0).unwrap().damage, Some(1));
}

#[tokio::test]
async fn server_entity_attack_commits_weapon_costs_in_authority() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "MobCostAttacker", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.0),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 1.0), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie has an entity id");
    let mut writer = Vec::new();

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        PlayerPose::new(0.5, 64.0, 0.5),
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();

    assert_eq!(state.inventory.held(0).unwrap().damage, Some(1));
    let persisted = state
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("MobCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("attacker authority state remains registered");
    assert_eq!(persisted.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(persisted.survival, survival);

    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test]
async fn out_of_reach_attack_does_not_reset_attacker_strength() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "OutOfReachAttack", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 20.5),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 20.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie has an entity id");
    let last_attack_tick = state.sessions.simulation_tick() + 1;
    state.last_entity_attack_tick = Some(last_attack_tick);
    let mut writer = Vec::new();

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        PlayerPose::new(0.5, 64.0, 0.5),
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();

    assert_eq!(state.last_entity_attack_tick, Some(last_attack_tick));
    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test]
async fn reachable_mob_hurt_immunity_still_resets_attacker_strength() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "ImmuneMobAttack", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 1.0),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie is attackable");
    let mut writer = Vec::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();
    let first_attempt_tick = state.last_entity_attack_tick.unwrap();
    let writer_len_after_first_hit = writer.len();
    state.sessions.advance_world_time(1);

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();
    let immune_attempt_tick = state.last_entity_attack_tick.unwrap();

    assert!(immune_attempt_tick > first_attempt_tick);
    assert_eq!(writer.len(), writer_len_after_first_hit);
    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_lethal_attacks_create_one_drop_and_one_xp_reward() {
    let rotten_flesh = Identifier::parse("minecraft:rotten_flesh").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: rotten_flesh,
        protocol_id: 10,
    }]));
    let rotten_flesh_id = items
        .id_of(&Identifier::parse("minecraft:rotten_flesh").unwrap())
        .unwrap();
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut alice = interaction_state_for_items(Arc::clone(&items));
    let mut bob = interaction_state_for_items(items);
    bob.sessions = Arc::clone(&alice.sessions);
    let (simulation, simulation_stop_tx, simulation_task) =
        spawn_test_simulation_owner(Arc::clone(&alice.sessions));
    alice.entity_types = Arc::clone(&entity_types);
    bob.entity_types = entity_types;
    alice.sessions.configure_arrow_kill_rewards(
        item_entity_type_id(&alice.entity_types),
        xp_orb_entity_type_id(&alice.entity_types),
        arrow_entity_type_id(&alice.entity_types),
        Arc::clone(&alice.items),
        Arc::clone(&alice.item_facts),
        Arc::clone(&alice.loot),
    );
    let alice_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(82),
        name: "LethalAlice".to_string(),
    };
    let bob_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(83),
        name: "LethalBob".to_string(),
    };
    let (alice_tx, _alice_rx) = mpsc::channel(16);
    let (bob_tx, _bob_rx) = mpsc::channel(16);
    let desired = HashSet::from([(0, 0)]);
    let (alice_id, _) = alice.sessions.register(
        &alice_profile,
        (0, 0),
        0,
        desired.clone(),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob_id, _) = alice.sessions.register(
        &bob_profile,
        (0, 0),
        0,
        desired,
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let spawn = PlayerPose::new(0.5, 64.0, 0.5);
    alice.sessions.register_player_persistence(
        alice_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(spawn))),
    );
    alice.sessions.register_player_persistence(
        bob_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(spawn))),
    );
    let _ = alice.sessions.mark_loaded(alice_id, (0, 0));
    let _ = alice.sessions.mark_loaded(bob_id, (0, 0));
    alice.session_id = alice_id;
    bob.session_id = bob_id;
    alice.simulation = simulation.for_session(alice_id);
    bob.simulation = simulation.for_session(bob_id);
    alice.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let target = alice
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie is attackable");
    let pre_damage = alice
        .sessions
        .damage_server_entity_for_test(target.id, 19.0)
        .expect("prime zombie to one health");
    assert_eq!(pre_damage.snapshot.health, 1.0);
    alice
        .sessions
        .advance_world_time(ENTITY_HURT_INVULNERABLE_TICKS);

    let gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_task = {
        let gate = Arc::clone(&gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            gate.wait().await;
            handle_attack(
                &mut alice,
                &mut writer,
                GameMode::Survival,
                &mut survival,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
                ServerboundAttack {
                    entity_id: target.id.0,
                },
            )
            .await
            .expect("Alice lethal attack task succeeds");
            (alice, xp)
        })
    };
    let bob_task = {
        let gate = Arc::clone(&gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            gate.wait().await;
            handle_attack(
                &mut bob,
                &mut writer,
                GameMode::Survival,
                &mut survival,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
                ServerboundAttack {
                    entity_id: target.id.0,
                },
            )
            .await
            .expect("Bob lethal attack task succeeds");
            (bob, xp)
        })
    };
    gate.wait().await;
    let (alice, _alice_xp) = alice_task.await.expect("Alice lethal task joins");
    let (_bob, _bob_xp) = bob_task.await.expect("Bob lethal task joins");
    let _ = simulation_stop_tx.send(());
    simulation_task.await.expect("simulation owner joins");

    assert!(alice.sessions.server_entity_snapshot(target.id).is_some());
    alice.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let drops = alice
        .sessions
        .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].item_stack,
        Some(EntityItemStack::new(rotten_flesh_id, 1))
    );
    let experience = alice
        .sessions
        .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(experience.len(), 1);
    assert_eq!(experience[0].experience_value, Some(5));
}
