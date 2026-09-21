use super::super::*;

pub(crate) struct PlayerControlIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) compression: Compression,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) chunk_stream: &'a mut Option<ChunkStreamState>,
    pub(in crate::play) simulation: &'a SimulationHandle,
    pub(in crate::play) sessions: &'a SessionRegistry,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) player_pose: &'a mut PlayerPose,
    pub(in crate::play) respawn_pose: PlayerPose,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) respawn: &'a ClientboundRespawn,
    pub(in crate::play) next_teleport_id: &'a mut i32,
    pub(in crate::play) pending_teleport: &'a mut Option<PendingTeleport>,
    pub(in crate::play) client_load: &'a mut ClientLoadGate,
    pub(in crate::play) breathing_state: &'a mut PlayerBreathingState,
    pub(in crate::play) game_mode: &'a mut GameMode,
    pub(in crate::play) permissions: CommandPermissions,
}

pub(crate) fn is_serverbound_player_control_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundSetCarriedItem::ID
            | ServerboundClientCommand::ID
            | ServerboundChangeGameMode::ID
    )
}

pub(crate) async fn handle_serverbound_player_control<W>(
    context: PlayerControlIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let PlayerControlIngressContext {
        writer,
        compression,
        mut interaction,
        chunk_stream,
        simulation,
        sessions,
        session_id,
        player_pose,
        respawn_pose,
        survival_state,
        xp_state,
        respawn,
        next_teleport_id,
        pending_teleport,
        client_load,
        breathing_state,
        game_mode,
        permissions,
    } = context;
    let mut body = frame.body;
    match frame.id {
        ServerboundSetCarriedItem::ID => {
            let pick = ServerboundSetCarriedItem::decode(&mut body)?;
            if (0..=8).contains(&pick.slot) {
                let slot = pick.slot as u8;
                let selection_changed = interaction
                    .as_deref()
                    .is_some_and(|state| state.selected_hotbar_slot() != slot);
                simulation
                    .commit_selected_hotbar_slot(slot)
                    .await
                    .map_err(|error| {
                        warn!(?error, slot, "hotbar selection owner commit failed");
                        ConnectionError::RuntimeUnavailable {
                            operation: "committing hotbar selection",
                        }
                    })?;
                if selection_changed && let Some(state) = interaction.as_deref_mut() {
                    state.pending_break = None;
                    state.pending_use = None;
                    clear_shield_use(state);
                    debug!(slot, "hotbar selection updated");
                }
            } else {
                debug!(slot = pick.slot, "invalid hotbar selection ignored");
            }
        }
        ServerboundClientCommand::ID => {
            let command = ServerboundClientCommand::decode(&mut body)?;
            let was_dead = survival_state.is_dead();
            handle_client_command(
                writer,
                compression,
                interaction.as_deref_mut(),
                chunk_stream,
                player_pose,
                respawn_pose,
                survival_state,
                xp_state,
                respawn,
                next_teleport_id,
                pending_teleport,
                sessions.simulation_tick(),
                command,
            )
            .await?;
            if was_dead && !survival_state.is_dead() {
                client_load.restart_after_respawn();
                if breathing_state.reset() {
                    publish_player_air_supply(sessions, session_id, *breathing_state);
                }
            }
            if was_dead && !survival_state.is_dead() {
                commit_authoritative_player_teleport(simulation, *player_pose).await?;
            }
        }
        ServerboundChangeGameMode::ID => {
            let command = ServerboundChangeGameMode::decode(&mut body)?;
            prepare_game_mode_transition(interaction, *game_mode, command.mode, permissions);
            apply_game_mode(
                writer,
                compression,
                simulation,
                game_mode,
                command.mode,
                permissions,
            )
            .await?;
            sessions.update_player_game_mode(session_id, *game_mode);
            if let Some(entry) = sessions.tab_list_entry(session_id) {
                dispatch_visibility_commands(sessions.broadcast_player_info_update(
                    PlayerInfoUpdate {
                        actions: PlayerInfoActions::UPDATE_GAME_MODE,
                        entries: vec![entry],
                    },
                ));
            }
        }
        _ => unreachable!("player control helper only accepts player-control packet ids"),
    }
    Ok(())
}
