use super::super::*;

pub(crate) struct PlayerStateIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) compression: Compression,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) simulation: &'a SimulationHandle,
    pub(in crate::play) sessions: &'a SessionRegistry,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) script_gameplay_events: Option<&'a ScriptGameplayEventPublisher>,
    pub(in crate::play) game_mode: &'a mut GameMode,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) player_pose: &'a mut PlayerPose,
    pub(in crate::play) next_teleport_id: &'a mut i32,
    pub(in crate::play) pending_teleport: &'a mut Option<PendingTeleport>,
}

pub(crate) fn is_serverbound_player_state_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundPlayerAction::ID | ServerboundPlayerCommand::ID | ServerboundPlayerInput::ID
    )
}

pub(crate) async fn handle_serverbound_player_state<W>(
    context: PlayerStateIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let PlayerStateIngressContext {
        writer,
        compression,
        mut interaction,
        simulation,
        sessions,
        session_id,
        script_gameplay_events,
        game_mode,
        survival_state,
        xp_state,
        player_pose,
        next_teleport_id,
        pending_teleport,
    } = context;
    let mut body = frame.body;
    match frame.id {
        ServerboundPlayerAction::ID => {
            let action = ServerboundPlayerAction::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_player_action(
                    state,
                    writer,
                    script_gameplay_events,
                    *game_mode,
                    survival_state,
                    xp_state,
                    *player_pose,
                    action,
                )
                .await?;
            } else {
                debug!(
                    action = ?action.action,
                    sequence = action.sequence,
                    "PlayerAction ignored — no world configured"
                );
            }
        }
        ServerboundPlayerCommand::ID => {
            let command = ServerboundPlayerCommand::decode(&mut body)?;
            let old_pose = *player_pose;
            match command.action {
                PlayerCommandAction::StartSprinting => player_pose.sprinting = true,
                PlayerCommandAction::StopSprinting => player_pose.sprinting = false,
                PlayerCommandAction::PressShiftKey => player_pose.shifting = true,
                PlayerCommandAction::ReleaseShiftKey => player_pose.shifting = false,
                PlayerCommandAction::StopSleeping => {
                    if let Some(bed) = sessions.request_sleep_wake(session_id)
                        && let Some(state) = interaction.as_deref_mut()
                    {
                        wake_player_from_bed(
                            WakePlayerFromBedContext {
                                state,
                                writer,
                                compression,
                                simulation,
                                player_pose,
                                next_teleport_id,
                                pending_teleport,
                                game_mode,
                            },
                            bed,
                        )
                        .await?;
                    }
                    return Ok(());
                }
                _ => {}
            }
            refresh_player_water_state(interaction.as_deref(), player_pose).await;
            commit_authoritative_player_pose(simulation, player_pose, old_pose).await?;
        }
        ServerboundPlayerInput::ID => {
            let input = ServerboundPlayerInput::decode(&mut body)?.input;
            let old_pose = *player_pose;
            player_pose.input = input;
            player_pose.sprinting = input.sprint;
            player_pose.shifting = input.shift;
            refresh_player_water_state(interaction.as_deref(), player_pose).await;
            commit_authoritative_player_pose(simulation, player_pose, old_pose).await?;
        }
        _ => unreachable!("player state helper only accepts player-state packet ids"),
    }
    Ok(())
}
