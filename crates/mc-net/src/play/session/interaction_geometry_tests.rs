use mc_entity::Vec3;
use mc_physics::Aabb;
use mc_protocol::packets::play::{GameMode, pack_block_pos};

use super::interaction_geometry::{
    entity_aabb, player_aabb_for_pose, within_block_reach, within_entity_attack_reach,
    within_entity_reach,
};
use crate::play::PlayerPose;

fn pose_with_eye(x: f64, y: f64, z: f64) -> PlayerPose {
    PlayerPose::new(x, y - 1.62, z)
}

#[test]
fn block_reach_uses_block_bounds_and_strict_vanilla_verification_boundary() {
    let block = pack_block_pos(0, 64, 0);
    let inside_survival = pose_with_eye(-5.5 + 1.0e-6, 64.5, 0.5);
    let survival_boundary = pose_with_eye(-5.5, 64.5, 0.5);
    let inside_creative = pose_with_eye(-6.0 + 1.0e-6, 64.5, 0.5);
    let creative_boundary = pose_with_eye(-6.0, 64.5, 0.5);

    assert!(within_block_reach(
        inside_survival,
        block,
        GameMode::Survival
    ));
    assert!(!within_block_reach(
        survival_boundary,
        block,
        GameMode::Survival
    ));
    assert!(within_block_reach(
        inside_creative,
        block,
        GameMode::Creative
    ));
    assert!(!within_block_reach(
        creative_boundary,
        block,
        GameMode::Creative
    ));
}

#[test]
fn entity_interaction_and_default_attack_keep_their_distinct_boundaries() {
    let position = Vec3::new(0.0, 64.0, 0.0);
    let aabb = Aabb {
        half_width: 0.3,
        height: 1.8,
    };
    let survival_boundary = pose_with_eye(-6.3, 65.0, 0.0);
    let creative_boundary = pose_with_eye(-8.3, 65.0, 0.0);

    assert!(!within_entity_reach(
        survival_boundary,
        position,
        aabb,
        GameMode::Survival
    ));
    assert!(within_entity_attack_reach(
        survival_boundary,
        position,
        aabb,
        GameMode::Survival,
        None,
    ));
    assert!(!within_entity_reach(
        creative_boundary,
        position,
        aabb,
        GameMode::Creative
    ));
    assert!(within_entity_attack_reach(
        creative_boundary,
        position,
        aabb,
        GameMode::Creative,
        None,
    ));
}

#[test]
fn survival_player_can_attack_a_sheep_two_blocks_away() {
    let player = PlayerPose::new(0.5, 64.0, 0.5);
    let sheep = Vec3::new(2.5, 64.0, 0.5);

    assert!(within_entity_attack_reach(
        player,
        sheep,
        entity_aabb("minecraft:sheep"),
        GameMode::Survival,
        None,
    ));
}

#[test]
fn non_finite_reach_inputs_are_rejected() {
    let block = pack_block_pos(0, 64, 0);
    let pose = pose_with_eye(f64::NAN, 64.5, 0.5);
    let aabb = Aabb {
        half_width: 0.3,
        height: 1.8,
    };

    assert!(!within_block_reach(pose, block, GameMode::Survival));
    assert!(!within_entity_reach(
        pose,
        Vec3::new(0.0, 64.0, 0.0),
        aabb,
        GameMode::Survival
    ));
    assert!(!within_entity_attack_reach(
        pose,
        Vec3::new(0.0, 64.0, 0.0),
        aabb,
        GameMode::Survival,
        None,
    ));
    assert!(!within_entity_reach(
        pose_with_eye(0.0, 64.5, 0.5),
        Vec3::new(f64::INFINITY, 64.0, 0.0),
        aabb,
        GameMode::Survival
    ));
}

#[test]
fn player_pose_changes_eye_height_and_target_bounds() {
    let block = pack_block_pos(0, 64, 0);
    let mut standing = PlayerPose::new(-5.48, 62.0, 0.5);
    let standing_box = player_aabb_for_pose(standing);
    assert!(within_block_reach(standing, block, GameMode::Survival));

    standing.shifting = true;
    let crouching_box = player_aabb_for_pose(standing);
    assert!(!within_block_reach(standing, block, GameMode::Survival));

    standing.swimming = true;
    let swimming_box = player_aabb_for_pose(standing);
    assert!(!within_block_reach(standing, block, GameMode::Survival));

    assert_eq!(standing_box.height, 1.8);
    assert_eq!(crouching_box.height, 1.5);
    assert_eq!(swimming_box.height, 0.6);
}

#[test]
fn spear_attack_range_uses_item_component_margin() {
    let range = mc_data::item_components::AttackRangeFacts {
        min_reach: 2.0,
        max_reach: 4.5,
        min_creative_reach: 2.0,
        max_creative_reach: 6.5,
        hitbox_margin: 0.125,
        mob_factor: 0.5,
    };
    let position = Vec3::new(0.0, 64.0, 0.0);
    let aabb = Aabb {
        half_width: 0.3,
        height: 1.8,
    };
    let spear_only = pose_with_eye(-7.8, 65.0, 0.0);
    let outside_spear = pose_with_eye(-7.926, 65.0, 0.0);

    assert!(!within_entity_attack_reach(
        spear_only,
        position,
        aabb,
        GameMode::Survival,
        None,
    ));
    assert!(within_entity_attack_reach(
        spear_only,
        position,
        aabb,
        GameMode::Survival,
        Some(range),
    ));
    assert!(!within_entity_attack_reach(
        outside_spear,
        position,
        aabb,
        GameMode::Survival,
        Some(range),
    ));
}
