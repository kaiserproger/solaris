use mc_data::block_facts::FluidKind;
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{EntityVec3, GameMode, ItemStack};
use mc_world::BlockPos;
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::connection::write_packet;
use crate::error::ConnectionError;

use super::combat::{MeleeKnockback, PlayerDamageKind, PlayerDamageRequest};
use super::inventory::damage_equipped_armor;
use super::movement::{fall_damage_amount, player_touches_lit_campfire_in_snapshot};
use super::persistence::XpState;
use super::session::PlayerDamagePublication;
use super::survival::SurvivalState;
use super::{
    InteractionState, PlayerPose, PlayerSurvivalUpdateOutcome, clear_shield_use,
    commit_player_survival_update, commit_player_survival_update_with_shield,
    finish_committed_shield_damage, plan_active_shield_damage, player_body_block_snapshot,
    player_pose_collides_with_solid, refresh_shield_use_state, shield_blocks_current_damage,
    survival_damage_after_equipment,
};

pub(super) async fn apply_fall_damage<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    old_pose: PlayerPose,
    new_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if old_pose.in_water || new_pose.in_water {
        return Ok(());
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground {
        return Ok(());
    }
    let damage = fall_damage_amount(old_pose, new_pose);
    if damage <= 0.0 || survival_state.is_dead() {
        return Ok(());
    }
    let applied_damage =
        survival_damage_after_equipment(state.as_deref(), damage, PlayerDamageKind::Fall);
    let mut updated_survival = *survival_state;
    updated_survival.apply_damage(applied_damage);
    if let Some(state) = state {
        let expected_inventory = state.inventory.clone();
        commit_player_survival_update(
            state,
            writer,
            survival_state,
            xp_state,
            expected_inventory,
            updated_survival,
            xp_state.clone(),
            None,
            true,
            new_pose,
        )
        .await?;
    } else {
        *survival_state = updated_survival;
        write_packet(writer, &survival_state.as_packet(), compression).await?;
    }
    Ok(())
}

pub(super) async fn apply_contact_block_damage<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    _compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    game_mode: GameMode,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival || survival_state.is_dead() {
        return Ok(());
    }
    let Some(state) = state else {
        return Ok(());
    };
    let Some((amount, kind)) = contact_block_damage(state, player_pose).await else {
        return Ok(());
    };

    let expected_inventory = state.inventory.clone();
    let applied_damage = survival_damage_after_equipment(Some(state), amount, kind);
    let mut updated_survival = *survival_state;
    updated_survival.apply_damage(applied_damage);
    if kind.damages_armor() {
        damage_equipped_armor(state, amount);
    }
    commit_player_survival_update(
        state,
        writer,
        survival_state,
        xp_state,
        expected_inventory,
        updated_survival,
        xp_state.clone(),
        None,
        true,
        player_pose,
    )
    .await?;
    Ok(())
}

pub(super) async fn contact_block_damage(
    state: &InteractionState,
    player_pose: PlayerPose,
) -> Option<(f32, PlayerDamageKind)> {
    if player_pose_collides_with_solid(Some(state), player_pose).await {
        return Some((1.0, PlayerDamageKind::Suffocation));
    }

    let half_width = 0.301;
    let snapshot = player_body_block_snapshot(state, player_pose, half_width);
    let min_x = (player_pose.x - half_width).floor() as i32;
    let max_x = (player_pose.x + half_width).floor() as i32;
    let min_y = player_pose.y.floor() as i32;
    let max_y = (player_pose.y + player_pose.body_height() - 1.0e-6).floor() as i32;
    let min_z = (player_pose.z - half_width).floor() as i32;
    let max_z = (player_pose.z + half_width).floor() as i32;
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let Some(state_id) = snapshot.get_cached_block(BlockPos { x, y, z }) else {
                    continue;
                };
                if state
                    .block_facts
                    .fluid(state_id.0)
                    .is_some_and(|fluid| fluid.kind == FluidKind::Lava)
                {
                    return Some((4.0, PlayerDamageKind::Lava));
                }
                let Some(block_state) = state.blocks.by_id(state_id) else {
                    continue;
                };
                if matches!(
                    block_state.block.id.as_str(),
                    "minecraft:fire" | "minecraft:soul_fire"
                ) {
                    return Some((1.0, PlayerDamageKind::Fire));
                }
            }
        }
    }
    player_touches_lit_campfire_in_snapshot(&state.blocks, &snapshot, player_pose)
        .then_some((1.0, PlayerDamageKind::Campfire))
}

pub(super) fn player_melee_knockback(knockback: MeleeKnockback) -> EntityVec3 {
    let MeleeKnockback { x, y, z } = knockback;
    EntityVec3 { x, y, z }
}

pub(super) struct AppliedPlayerDamagePublication {
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) survival_changed: bool,
    pub(super) xp_changed: bool,
    pub(super) died: bool,
    pub(super) fresh_hurt: bool,
    pub(super) shield_cooldown: Option<super::session::ShieldCooldownPublication>,
    pub(super) knockback: Option<MeleeKnockback>,
}

pub(super) fn apply_player_damage_publication(
    interaction: Option<&mut InteractionState>,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    publication: PlayerDamagePublication,
) -> AppliedPlayerDamagePublication {
    let old_survival = *survival_state;
    let old_xp = xp_state.clone();
    let shield_cooldown = publication.shield_cooldown.clone();
    let health_accepted = survival_state.health == publication.expected_health;
    if health_accepted {
        survival_state.health = publication.health;
    }
    if let Some(xp) = publication.xp
        && *xp_state == xp.expected
    {
        *xp_state = xp.updated;
    }

    let mut changed_slots = Vec::new();
    if let Some(state) = interaction {
        for delta in publication.inventory {
            let Some(slot) = state.inventory.slots.get_mut(delta.slot) else {
                continue;
            };
            if *slot == delta.expected {
                *slot = delta.updated.clone();
                changed_slots.push((delta.slot, delta.updated));
            }
        }
        if let Some(delta) = publication.carried_item
            && state.carried_item == delta.expected
        {
            state.carried_item = delta.updated;
        }
        if publication.shield_blocked
            && let Some(shield) = state.shield_use.as_mut()
        {
            shield.stack = state.inventory.slots[shield.slot].clone();
        }
        if shield_cooldown.is_some() {
            clear_shield_use(state);
        } else {
            refresh_shield_use_state(state);
        }
        if health_accepted && publication.died {
            state.pending_break = None;
            state.pending_use = None;
            clear_shield_use(state);
        }
    }

    AppliedPlayerDamagePublication {
        changed_slots,
        survival_changed: old_survival != *survival_state,
        xp_changed: old_xp != *xp_state,
        died: health_accepted && publication.died,
        fresh_hurt: health_accepted && publication.fresh_hurt,
        shield_cooldown,
        knockback: health_accepted.then_some(publication.knockback).flatten(),
    }
}

pub(super) struct PlayerDamageApplication {
    pub(super) player_pose: PlayerPose,
    pub(super) request: PlayerDamageRequest,
}

pub(super) async fn apply_player_damage<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    game_mode: GameMode,
    damage: PlayerDamageApplication,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let PlayerDamageApplication {
        player_pose,
        request,
    } = damage;
    if matches!(game_mode, GameMode::Creative | GameMode::Spectator)
        || !request.amount.is_finite()
        || request.amount <= 0.0
        || !request.kind.is_supported()
        || survival_state.is_dead()
    {
        return Ok(false);
    }
    let mut state = state;
    if request.kind.can_be_blocked_by_shield()
        && let Some(state) = state.as_deref_mut()
    {
        let mut shield_commit_attempts = 0;
        loop {
            if !shield_blocks_current_damage(state, player_pose, request.source_origin) {
                break;
            }
            let expected_inventory = state.inventory.clone();
            let Some(planned_shield_damage) = plan_active_shield_damage(state, request.amount)
            else {
                return Ok(false);
            };
            shield_commit_attempts += 1;
            let transition = planned_shield_damage.transition.clone();
            let committed = commit_player_survival_update_with_shield(
                state,
                writer,
                survival_state,
                xp_state,
                expected_inventory,
                *survival_state,
                xp_state.clone(),
                Some(transition),
                None,
                true,
                player_pose,
            )
            .await?;
            if matches!(committed, PlayerSurvivalUpdateOutcome::Committed) {
                finish_committed_shield_damage(state, planned_shield_damage);
                return Ok(false);
            }

            if shield_commit_attempts >= 2 {
                if shield_blocks_current_damage(state, player_pose, request.source_origin) {
                    debug!(
                        session_id = state.session_id,
                        "shield damage stayed stale after exact owner-state retry"
                    );
                    return Err(ConnectionError::RuntimeUnavailable {
                        operation: "committing shield durability after repeated owner state change",
                    });
                }
                break;
            }
        }
    }
    let applied_damage =
        survival_damage_after_equipment(state.as_deref(), request.amount, request.kind);
    let mut updated_survival = *survival_state;
    if applied_damage > 0.0 {
        updated_survival.apply_damage(applied_damage);
    }
    if let Some(state) = state {
        let expected_inventory = state.inventory.clone();
        if applied_damage > 0.0 && request.kind.damages_armor() {
            damage_equipped_armor(state, request.amount);
        }
        let committed = commit_player_survival_update(
            state,
            writer,
            survival_state,
            xp_state,
            expected_inventory,
            updated_survival,
            xp_state.clone(),
            None,
            true,
            player_pose,
        )
        .await?;
        return Ok(applied_damage > 0.0 && committed);
    } else {
        *survival_state = updated_survival;
        write_packet(writer, &survival_state.as_packet(), compression).await?;
    }
    Ok(applied_damage > 0.0)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use mc_protocol::frame::try_decode_frame;
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::play::ClientboundSetHealth;

    use super::{
        Compression, GameMode, PlayerDamageApplication, PlayerDamageKind, PlayerDamageRequest,
        PlayerPose, SurvivalState, XpState, apply_player_damage,
    };

    #[tokio::test]
    async fn common_environmental_damage_writes_health_and_invalid_sources_fail_closed() {
        for kind in [
            PlayerDamageKind::Fire,
            PlayerDamageKind::Lava,
            PlayerDamageKind::Drowning,
            PlayerDamageKind::Suffocation,
            PlayerDamageKind::Starvation,
        ] {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            assert!(
                apply_player_damage(
                    None,
                    &mut writer,
                    Compression::Disabled,
                    &mut survival,
                    &mut xp,
                    GameMode::Survival,
                    PlayerDamageApplication {
                        player_pose: PlayerPose::new(0.5, 64.0, 0.5),
                        request: PlayerDamageRequest {
                            kind,
                            amount: 2.0,
                            source_origin: None,
                        },
                    },
                )
                .await
                .expect("environmental damage adapter")
            );
            assert_eq!(survival.health, 18.0, "{kind:?}");
            let mut bytes = BytesMut::from(writer.as_slice());
            let mut frame = try_decode_frame(&mut bytes, Compression::Disabled)
                .expect("complete health frame")
                .expect("health frame");
            assert_eq!(frame.id, ClientboundSetHealth::ID, "{kind:?}");
            let packet = ClientboundSetHealth::decode(&mut frame.body).expect("decode health");
            assert_eq!(packet.health, 18.0, "{kind:?}");
            assert!(bytes.is_empty(), "{kind:?} emitted an extra packet");
        }

        for (kind, amount) in [
            (PlayerDamageKind::Generic, f32::NAN),
            (PlayerDamageKind::Generic, f32::INFINITY),
            (PlayerDamageKind::Unsupported, 2.0),
        ] {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            assert!(
                !apply_player_damage(
                    None,
                    &mut writer,
                    Compression::Disabled,
                    &mut survival,
                    &mut xp,
                    GameMode::Survival,
                    PlayerDamageApplication {
                        player_pose: PlayerPose::new(0.5, 64.0, 0.5),
                        request: PlayerDamageRequest {
                            kind,
                            amount,
                            source_origin: None,
                        },
                    },
                )
                .await
                .expect("invalid damage adapter rejection")
            );
            assert_eq!(survival, SurvivalState::FULL);
            assert!(writer.is_empty());
        }
    }
}
