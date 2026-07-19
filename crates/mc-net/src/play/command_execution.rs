use mc_entity::Vec3;
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{
    ClientCommandAction, ClientboundContainerSetSlot, ClientboundRespawn, ClientboundSetTime,
    ClientboundSystemChat, GameEvent, GameMode, ItemStack, ServerboundClientCommand,
    SetCenterChunk,
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::control_plane::{autoscale_action_label, autoscale_pressure_label};
use crate::error::ConnectionError;
use crate::server::ServerConfig;
use crate::{RuntimeControlHandle, connection::write_packet};

use super::chunk_stream::ChunkStreamState;
use super::combat::PlayerDamageKind;
use super::commands::{
    AdminCommand, CommandError, CommandPermissions, DebugCommand, SurvivalCommand,
    parse_admin_command, player_abilities_for_mode,
};
use super::inventory::{PlayerInventory, item_max_stack};
use super::movement::{PendingTeleport, clamp_player_coordinates, next_player_teleport_id};
use super::persistence::XpState;
use super::session::{SessionRegistry, dispatch_visibility_commands};
use super::simulation::SimulationHandle;
use super::survival::SurvivalState;
use super::{
    InteractionState, PlayerPose, clear_shield_use, commit_authoritative_player_pose,
    commit_player_inventory_candidate, commit_player_survival_update, replan_after_movement,
    send_player_position_sync, survival_damage_after_equipment, text_component_nbt,
    write_inventory_content_resync, write_inventory_slot_updates,
};

pub(super) fn prepare_game_mode_transition(
    interaction: Option<&mut InteractionState>,
    current: GameMode,
    requested: GameMode,
    permissions: CommandPermissions,
) {
    let Some(state) = interaction else {
        return;
    };
    state.pending_break = None;
    state.pending_use = None;
    if current != requested && permissions.can_change_game_mode() {
        clear_shield_use(state);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_player_command<W>(
    writer: &mut W,
    compression: Compression,
    raw: &str,
    permissions: CommandPermissions,
    game_mode: &mut GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    simulation: &SimulationHandle,
    mut interaction: Option<&mut InteractionState>,
    player_pose: &mut PlayerPose,
    runtime_control: Option<&RuntimeControlHandle>,
    chunk_stream: &mut Option<ChunkStreamState>,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let command = match parse_admin_command(raw, permissions) {
        Ok(command) => command,
        Err(err) => {
            send_command_feedback(writer, compression, command_error_message(err)).await?;
            debug!(command = %raw, error = ?err, "command rejected");
            return Ok(());
        }
    };

    match command {
        AdminCommand::GameMode(mode) => {
            prepare_game_mode_transition(interaction.as_deref_mut(), *game_mode, mode, permissions);
            apply_game_mode(
                writer,
                compression,
                simulation,
                game_mode,
                mode,
                permissions,
            )
            .await?;
            send_command_feedback(writer, compression, &format!("Set game mode to {mode:?}")).await
        }
        AdminCommand::PlayersSleepingPercentage(value) => {
            if let Some(value) = value {
                sessions.set_players_sleeping_percentage(value);
            }
            let value = sessions.players_sleeping_percentage();
            send_command_feedback(
                writer,
                compression,
                &format!("players_sleeping_percentage = {value}"),
            )
            .await
        }
        AdminCommand::Give { item, count } => {
            match apply_give_command(
                writer,
                interaction.as_deref_mut(),
                *player_pose,
                &item,
                count,
            )
            .await?
            {
                Ok(message) => send_command_feedback(writer, compression, &message).await,
                Err(message) => send_command_feedback(writer, compression, message).await,
            }
        }
        AdminCommand::SaveAll => {
            let report = crate::server::save_all_after_simulation_barrier(
                "player save-all",
                config,
                sessions,
                simulation,
            )
            .await;
            if report.is_ok() {
                send_command_feedback(
                    writer,
                    compression,
                    &format!(
                        "Saved {} players, {} entities, {} chunks",
                        report.players_saved, report.entities_saved, report.chunks_flushed
                    ),
                )
                .await
            } else {
                warn!(errors = report.errors.len(), "save-all command failed");
                send_command_feedback(writer, compression, "Save-all failed; see server log").await
            }
        }
        AdminCommand::Stop => {
            let report = crate::server::request_stop_after_save(
                &config.shutdown,
                runtime_control,
                crate::server::save_all_after_simulation_barrier(
                    "player stop",
                    config,
                    sessions,
                    simulation,
                ),
            )
            .await;
            if report.is_ok() {
                send_command_feedback(writer, compression, "Saved all state; stopping server")
                    .await?;
            } else {
                warn!(errors = report.errors.len(), "stop command save-all failed");
                send_command_feedback(writer, compression, "Stop aborted; save-all failed").await?;
            }
            Ok(())
        }
        AdminCommand::Status => {
            send_command_feedback(
                writer,
                compression,
                &runtime_control_status_message(runtime_control),
            )
            .await
        }
        AdminCommand::Teleport { x, y, z } => {
            if pending_teleport.is_some() {
                return send_command_feedback(
                    writer,
                    compression,
                    "Teleport pending confirmation; retry after client acknowledgement",
                )
                .await;
            }
            let (x, y, z) = clamp_player_coordinates(x, y, z);
            let old_center = player_pose.chunk_pos();
            player_pose.x = x;
            player_pose.y = y;
            player_pose.z = z;
            if let Some(state) = interaction.as_deref_mut() {
                state.pending_break = None;
                state.pending_use = None;
            }
            let new_center = player_pose.chunk_pos();
            commit_authoritative_player_pose(simulation, *player_pose).await?;
            let teleport_id = next_player_teleport_id(next_teleport_id);
            send_player_position_sync(writer, compression, teleport_id, *player_pose).await?;
            *pending_teleport = Some(PendingTeleport::new(
                teleport_id,
                sessions.simulation_tick(),
            ));
            replan_after_movement(
                writer,
                compression,
                chunk_stream,
                interaction.as_deref_mut(),
                old_center,
                new_center,
                player_pose.yaw,
            )
            .await?;
            send_command_feedback(writer, compression, &format!("Teleported to {x} {y} {z}")).await
        }
        AdminCommand::Kill => {
            let damage = survival_damage_after_equipment(
                interaction.as_deref(),
                10000.0,
                PlayerDamageKind::GenericKill,
            );
            let mut updated_survival = *survival_state;
            updated_survival.apply_damage(damage);
            if let Some(state) = interaction.as_deref_mut() {
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
                    *player_pose,
                )
                .await?;
            } else {
                *survival_state = updated_survival;
                write_packet(writer, &survival_state.as_packet(), compression).await?;
            }
            send_command_feedback(writer, compression, "Killed player").await
        }
        AdminCommand::Summon { entity, x, y, z } => {
            let Some(state) = interaction.as_deref_mut() else {
                send_command_feedback(
                    writer,
                    compression,
                    "Cannot summon before play state is ready",
                )
                .await?;
                return Ok(());
            };
            let Some(entity_type_id) = state.entity_types.id_of(&entity) else {
                send_command_feedback(writer, compression, "Unknown entity type").await?;
                return Ok(());
            };
            let position = Vec3::new(
                x.unwrap_or(player_pose.x),
                y.unwrap_or(player_pose.y),
                z.unwrap_or(player_pose.z),
            );
            match state
                .simulation
                .spawn_command_entity(
                    i32::try_from(entity_type_id).unwrap_or(i32::MAX),
                    entity.to_string(),
                    position,
                )
                .await
            {
                Ok(dispatches) => {
                    dispatch_visibility_commands(dispatches);
                    send_command_feedback(writer, compression, &format!("Summoned {entity}")).await
                }
                Err(error) => {
                    debug!(?error, %entity, "simulation summon request rejected");
                    send_command_feedback(writer, compression, "Summon rejected: simulation busy")
                        .await
                }
            }
        }
        AdminCommand::TimeSet(time) => match simulation.set_world_time(time).await {
            Ok(()) => {
                send_command_feedback(writer, compression, &format!("Set time to {time}")).await
            }
            Err(error) => {
                debug!(?error, time, "simulation time-set request rejected");
                send_command_feedback(writer, compression, "Time set rejected: simulation busy")
                    .await
            }
        },
        AdminCommand::Debug(command) => {
            apply_debug_command(
                writer,
                compression,
                survival_state,
                xp_state,
                interaction,
                *player_pose,
                command,
                permissions,
            )
            .await?;
            send_command_feedback(writer, compression, "Debug command executed").await
        }
    }
}

pub(super) fn runtime_control_status_message(
    runtime_control: Option<&RuntimeControlHandle>,
) -> String {
    let Some(runtime_control) = runtime_control else {
        return "Runtime control: disabled".to_string();
    };
    let snapshot = runtime_control.snapshot();
    let limits = snapshot.limits;
    format!(
        "Runtime control: draining={} action={} pressure={} limits=view_distance:{},send:{},load:{},generate:{} pressure_ticks={} healthy_ticks={} reason={}",
        snapshot.draining,
        autoscale_action_label(snapshot.last_decision.action),
        autoscale_pressure_label(snapshot.last_decision.pressure),
        limits.view_distance,
        limits.chunk_send_rate,
        limits.chunk_load_rate,
        limits.chunk_generate_rate,
        snapshot.pressure_ticks,
        snapshot.healthy_ticks,
        snapshot.last_decision.reason
    )
}

async fn apply_give_command<W>(
    writer: &mut W,
    interaction: Option<&mut InteractionState>,
    player_pose: PlayerPose,
    item: &Identifier,
    count: i32,
) -> Result<Result<String, &'static str>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(state) = interaction else {
        debug!(%item, count, "give command rejected: no interaction state");
        return Ok(Err("Cannot give items before play state is ready"));
    };
    let Some(item_id) = state.items.id_of(item) else {
        debug!(%item, "give command rejected: item not in registry");
        return Ok(Err("Unknown item"));
    };
    let stack = ItemStack::new(item_id, count);
    let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
    let mut candidate = state.inventory.clone();
    let (leftover, changed) = candidate.merge_stack(stack, max_stack);
    if !leftover.is_empty() {
        debug!(%item, count, "give command rejected: inventory full");
        return Ok(Err("Not enough inventory space"));
    }
    let expected_inventory = state.inventory.clone();
    state.inventory = candidate;
    if !commit_player_inventory_candidate(
        state,
        expected_inventory,
        state.carried_item.clone(),
        None,
        player_pose,
    )
    .await?
    {
        write_inventory_content_resync(state, writer).await?;
        debug!(%item, count, "give command rejected: player inventory changed");
        return Ok(Err("Player inventory changed; try again"));
    }
    write_inventory_slot_updates(state, writer, changed).await?;
    Ok(Ok(format!("Gave {count} of {item}")))
}

pub(super) async fn send_command_feedback<W>(
    writer: &mut W,
    compression: Compression,
    message: &str,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundSystemChat {
            content_nbt: text_component_nbt(message)?,
            overlay: false,
        },
        compression,
    )
    .await
}

pub(super) async fn send_world_time<W>(
    writer: &mut W,
    compression: Compression,
    sessions: &SessionRegistry,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &clientbound_session_world_time(sessions),
        compression,
    )
    .await
}

pub(super) fn clientbound_session_world_time(sessions: &SessionRegistry) -> ClientboundSetTime {
    clientbound_world_time(sessions.world_time())
}

pub(super) fn clientbound_world_time(time: u64) -> ClientboundSetTime {
    ClientboundSetTime {
        game_time: i64::try_from(time).unwrap_or(i64::MAX),
    }
}

pub(super) fn command_error_message(error: CommandError) -> &'static str {
    match error {
        CommandError::Unknown => "Unknown command",
        CommandError::PermissionDenied => "You do not have permission to use that command",
        CommandError::Usage(usage) => usage,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_debug_command<W>(
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    mut interaction: Option<&mut InteractionState>,
    player_pose: PlayerPose,
    command: DebugCommand,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.op {
        debug!(command = ?command, "debug command denied for non-op player");
        return Ok(());
    }

    match command {
        DebugCommand::Survival(command) => {
            let result = apply_survival_command(
                writer,
                compression,
                survival_state,
                xp_state,
                interaction.as_deref_mut(),
                player_pose,
                command,
            )
            .await;
            if survival_state.is_dead()
                && let Some(state) = interaction.as_mut()
            {
                state.pending_break = None;
            }
            result
        }
        DebugCommand::OutboundPressure { count } => {
            let Some(state) = interaction else {
                debug!(
                    count,
                    "outbound pressure probe ignored — no interaction state"
                );
                return Ok(());
            };
            dispatch_visibility_commands(
                state
                    .sessions
                    .debug_outbound_pressure_dispatches(state.session_id, count),
            );
            Ok(())
        }
        DebugCommand::Give {
            item,
            count,
            hotbar_slot,
        } => {
            let Some(state) = interaction else {
                debug!(%item, "debug give ignored — no interaction state");
                return Ok(());
            };
            let stack = if count <= 0 {
                ItemStack::EMPTY
            } else {
                let Some(item_id) = state.items.id_of(&item) else {
                    debug!(%item, "debug give ignored — item not in registry");
                    return Ok(());
                };
                ItemStack::new(item_id, count.min(i32::from(u8::MAX)))
            };
            let mut inventory = state.inventory.clone();
            inventory.set_hotbar(hotbar_slot, stack.clone());
            let expected_inventory = state.inventory.clone();
            state.inventory = inventory;
            if !commit_player_inventory_candidate(
                state,
                expected_inventory,
                state.carried_item.clone(),
                None,
                player_pose,
            )
            .await?
            {
                write_inventory_content_resync(state, writer).await?;
                debug!(%item, "debug give ignored - player inventory changed");
                return Ok(());
            }
            state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
            write_packet(
                writer,
                &ClientboundContainerSetSlot {
                    container_id: 0,
                    state_id: state.inventory_state_id,
                    slot: (PlayerInventory::HOTBAR_BASE + hotbar_slot as usize) as i16,
                    item_stack: stack,
                },
                compression,
            )
            .await
        }
    }
}

async fn apply_survival_command<W>(
    writer: &mut W,
    compression: Compression,
    state: &mut SurvivalState,
    xp_state: &mut XpState,
    interaction: Option<&mut InteractionState>,
    player_pose: PlayerPose,
    command: SurvivalCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let expected_inventory = interaction
        .as_deref()
        .map(|interaction| interaction.inventory.clone());
    let mut updated_state = *state;
    let mut updated_xp = xp_state.clone();
    match command {
        SurvivalCommand::Experience(points) => {
            updated_xp.add_points(points);
        }
        SurvivalCommand::Damage(amount) => {
            updated_state.apply_damage(survival_damage_after_equipment(
                interaction.as_deref(),
                amount,
                PlayerDamageKind::Generic,
            ));
        }
        SurvivalCommand::Heal(amount) => updated_state.heal(amount),
        SurvivalCommand::Feed { food, saturation } => updated_state.add_food(food, saturation),
        SurvivalCommand::Exhaust(amount) => {
            updated_state.add_exhaustion(amount);
        }
    }

    if let Some(interaction) = interaction {
        commit_player_survival_update(
            interaction,
            writer,
            state,
            xp_state,
            expected_inventory.expect("interaction inventory snapshot"),
            updated_state,
            updated_xp,
            None,
            true,
            player_pose,
        )
        .await?;
    } else {
        let survival_changed = *state != updated_state;
        let xp_changed = *xp_state != updated_xp;
        *state = updated_state;
        *xp_state = updated_xp;
        if survival_changed {
            write_packet(writer, &state.as_packet(), compression).await?;
        }
        if xp_changed {
            write_packet(writer, &xp_state.as_packet(), compression).await?;
        }
    }
    Ok(())
}

pub(super) async fn apply_game_mode<W>(
    writer: &mut W,
    compression: Compression,
    simulation: &SimulationHandle,
    current: &mut GameMode,
    requested: GameMode,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.can_change_game_mode() {
        debug!(mode = ?requested, "gamemode change denied for non-op player");
        return Ok(());
    }
    if *current == requested {
        return Ok(());
    }
    simulation
        .commit_game_mode(requested)
        .await
        .map_err(|error| {
            warn!(?error, ?requested, "game mode owner commit failed");
            ConnectionError::RuntimeUnavailable {
                operation: "committing game mode",
            }
        })?;
    *current = requested;
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_CHANGE_GAME_MODE,
            value: requested.id() as f32,
        },
        compression,
    )
    .await?;
    write_packet(writer, &player_abilities_for_mode(requested), compression).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_client_command<W>(
    writer: &mut W,
    compression: Compression,
    interaction: Option<&mut InteractionState>,
    chunk_stream: &mut Option<ChunkStreamState>,
    player_pose: &mut PlayerPose,
    respawn_pose: PlayerPose,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    respawn: &ClientboundRespawn,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
    current_tick: u64,
    command: ServerboundClientCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    match command.action {
        ClientCommandAction::PerformRespawn => {
            if !survival_state.is_dead() {
                return Ok(());
            }
            if let Some(state) = interaction {
                let expected_inventory = state.inventory.clone();
                if !commit_player_survival_update(
                    state,
                    writer,
                    survival_state,
                    xp_state,
                    expected_inventory,
                    SurvivalState::FULL,
                    xp_state.clone(),
                    None,
                    false,
                    respawn_pose,
                )
                .await?
                {
                    return Ok(());
                }
                state.pending_break = None;
            } else {
                *survival_state = SurvivalState::FULL;
            }
            *player_pose = respawn_pose;
            write_packet(writer, respawn, compression).await?;
            write_packet(
                writer,
                &GameEvent {
                    event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
                    value: 0.0,
                },
                compression,
            )
            .await?;
            write_packet(
                writer,
                &SetCenterChunk {
                    chunk_x: respawn_pose.chunk_pos().0,
                    chunk_z: respawn_pose.chunk_pos().1,
                },
                compression,
            )
            .await?;
            if let Some(stream) = chunk_stream.as_mut() {
                stream.replay_current_view(respawn_pose.yaw);
            }
            let teleport_id = next_player_teleport_id(next_teleport_id);
            send_player_position_sync(writer, compression, teleport_id, *player_pose).await?;
            *pending_teleport = Some(PendingTeleport::new(teleport_id, current_tick));
            write_packet(writer, &survival_state.as_packet(), compression).await?;
            Ok(())
        }
        ClientCommandAction::RequestStats | ClientCommandAction::RequestGameruleValues => {
            debug!(action = ?command.action, "client command ignored");
            Ok(())
        }
    }
}
