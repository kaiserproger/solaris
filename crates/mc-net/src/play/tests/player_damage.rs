use std::sync::Arc;

use mc_data::items::{ItemRegistry, ItemReport};
use mc_entity::Vec3;
use mc_protocol::{
    Compression,
    codec::Identifier,
    packets::play::{GameMode, ItemStack},
};
use tokio::sync::mpsc;

use super::super::combat::{
    PlayerDamageKind, PlayerDamageRequest, SHIELD_FALLBACK_MAX_DAMAGE, shield_use_from_stack,
};
use super::super::inventory::PlayerInventory;
use super::super::persistence::XpState;
use super::super::player_damage_adapter::{
    PlayerDamageApplication, apply_contact_block_damage, apply_player_damage,
    apply_player_damage_publication, player_melee_knockback,
};
use super::super::survival::SurvivalState;
use super::super::{
    PlayerDamagePublication, PlayerInventorySlotDelta, PlayerPose, melee_knockback,
    player_pose_collides_with_solid, shield_block_knockback,
};
use super::{
    attack_strength_test_state, campfire_test_interaction_state, decode_container_set_slot_packets,
    interaction_state_for_items, shield_item_state, start_survival_test_owner,
};

#[tokio::test]
async fn player_collision_allows_lit_campfire_overlap_for_contact_damage() {
    let state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;

    assert!(!player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.0, 0.5)).await);
}

#[tokio::test]
async fn lit_campfire_contact_damage_uses_survival_death_path() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut survival_state = SurvivalState {
        health: 1.0,
        ..SurvivalState::FULL
    };
    let mut xp_state = XpState {
        level: 5,
        progress: 0.0,
        total: 55,
        seed: 0,
    };
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireDeath", survival_state, &xp_state);
    let mut writer = Vec::new();

    apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(0.5, 65.0, 0.5),
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!(survival_state.is_dead());
    assert!(state.pending_break.is_none());
    assert_eq!(xp_state.total, 0);
    assert!(!writer.is_empty());
}

#[tokio::test]
async fn committed_campfire_death_survives_client_write_failure() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut deaths = state.sessions.install_script_commit_event_outbox();
    let mut survival_state = SurvivalState {
        health: 1.0,
        ..SurvivalState::FULL
    };
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireFail", survival_state, &xp_state);
    let (mut writer, reader) = tokio::io::duplex(64);
    drop(reader);

    let result = apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(0.5, 65.0, 0.5),
    )
    .await;
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!(
        result.is_err(),
        "closed client transport must reject publication"
    );
    assert!(survival_state.is_dead());
    let event = deaths
        .try_recv_required()
        .expect("owner commit must publish death before client transport");
    assert!(matches!(
        event.kind(),
        mc_script::ScriptEventKind::PlayerDied {
            context,
            game_mode: mc_script::ScriptGameMode::Survival,
            ..
        } if context.username() == "CampfireFail"
    ));
    assert!(matches!(
        deaths.try_recv_required(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn lit_campfire_contact_damage_uses_player_width_edge_overlap() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireEdge", survival_state, &xp_state);
    let mut writer = Vec::new();

    apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(1.3, 64.0, 0.5),
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, 19.0);
    assert!(!writer.is_empty());
}

#[tokio::test]
async fn pushed_hostile_damage_shield_block_writes_break_clear_slot_update() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let slot = 45;
    state.inventory.slots[slot] = ItemStack::new(77, 1).with_damage(SHIELD_FALLBACK_MAX_DAMAGE - 4);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::OffHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "HostileShield", survival_state, &xp_state);
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
                kind: PlayerDamageKind::MobAttack,
                amount: 3.0,
                source_origin: Some(Vec3::new(0.0, 64.0, 1.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert_eq!(state.inventory.slots[slot], ItemStack::EMPTY);
    assert!(state.shield_use.is_none());
    assert!(
        !damage_applied,
        "a shielded hit must not authorize knockback"
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, slot as i16);
    assert_eq!(packets[0].item_stack, ItemStack::EMPTY);
}

#[tokio::test]
async fn pushed_hostile_damage_uses_equipped_iron_chestplate_and_damages_armor() {
    let chestplate = Identifier::parse("minecraft:iron_chestplate").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chestplate,
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(items);
    state.sessions.set_world_time(10);
    state.inventory.slots[6] = ItemStack::new(11, 1);
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "HostileArmor", survival_state, &xp_state);
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
                kind: PlayerDamageKind::MobAttack,
                amount: 3.0,
                source_origin: Some(Vec3::new(0.0, 64.0, 1.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!((survival_state.health - 17.54).abs() < 0.001);
    assert!(
        damage_applied,
        "committed positive damage authorizes knockback"
    );
    assert_eq!(
        state.inventory.slots[6],
        ItemStack::new(11, 1).with_damage(1)
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, 6);
    assert_eq!(packets[0].item_stack, ItemStack::new(11, 1).with_damage(1));
}

#[test]
fn grounded_player_melee_knockback_matches_vanilla_base_impulse() {
    let knockback = melee_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce knockback");
    let motion = player_melee_knockback(knockback);

    assert!(motion.x.abs() < f64::EPSILON);
    assert!((motion.y - 0.4).abs() < f64::EPSILON);
    assert!((motion.z - 0.400_000_005_960_464_5).abs() < f64::EPSILON);
}

#[test]
fn player_melee_knockback_fails_closed_for_zero_horizontal_direction() {
    assert_eq!(
        melee_knockback(0.0, 0.0, true, Vec3::new(0.0, 64.0, 0.0)),
        None
    );
}

#[test]
fn shield_block_knockback_matches_vanilla_base_response() {
    let knockback = shield_block_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce shield response");
    let motion = player_melee_knockback(knockback);

    assert!(motion.x.abs() < f64::EPSILON);
    assert!((motion.y - 0.4).abs() < f64::EPSILON);
    assert!((motion.z - 0.5).abs() < f64::EPSILON);
}

#[test]
fn older_victim_publication_preserves_newer_attacker_costs() {
    let (mut state, sword, _) = attack_strength_test_state();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(sword, 1).with_damage(1);
    state.inventory.slots[5] = ItemStack::new(42, 1);
    let mut survival = SurvivalState {
        exhaustion: 0.1,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();

    let applied = apply_player_damage_publication(
        Some(&mut state),
        &mut survival,
        &mut xp,
        PlayerDamagePublication {
            expected_health: SurvivalState::MAX_HEALTH,
            health: 16.0,
            inventory: vec![PlayerInventorySlotDelta {
                slot: 5,
                expected: ItemStack::new(42, 1),
                updated: ItemStack::new(42, 1).with_damage(2),
            }],
            carried_item: None,
            xp: None,
            died: false,
            fresh_hurt: true,
            shield_blocked: false,
            shield_cooldown: None,
            knockback: None,
        },
    );

    assert!(applied.survival_changed);
    assert_eq!(survival.health, 16.0);
    assert_eq!(survival.exhaustion, 0.1);
    assert_eq!(state.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(state.inventory.slots[5].damage, Some(2));
}

#[test]
fn stale_damage_publication_does_not_apply_health_side_effects() {
    let mut survival = SurvivalState {
        health: 18.0,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();
    let knockback = melee_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce knockback");

    let applied = apply_player_damage_publication(
        None,
        &mut survival,
        &mut xp,
        PlayerDamagePublication {
            expected_health: SurvivalState::MAX_HEALTH,
            health: 0.0,
            inventory: Vec::new(),
            carried_item: None,
            xp: None,
            died: true,
            fresh_hurt: true,
            shield_blocked: false,
            shield_cooldown: None,
            knockback: Some(knockback),
        },
    );

    assert_eq!(survival.health, 18.0);
    assert!(!applied.survival_changed);
    assert!(!applied.died);
    assert!(!applied.fresh_hurt);
    assert_eq!(applied.knockback, None);
}
