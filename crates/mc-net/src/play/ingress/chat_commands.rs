use super::super::*;

pub(crate) struct ChatCommandIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) compression: Compression,
    pub(in crate::play) scripts: Option<&'a ScriptEventSink>,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) dimension: &'a str,
    pub(in crate::play) player_uuid: &'a str,
    pub(in crate::play) player_name: &'a str,
    pub(in crate::play) permissions: CommandPermissions,
    pub(in crate::play) peer: std::net::SocketAddr,
    pub(in crate::play) player_pose: &'a mut PlayerPose,
    pub(in crate::play) game_mode: &'a mut GameMode,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) config: &'a ServerConfig,
    pub(in crate::play) sessions: &'a SessionRegistry,
    pub(in crate::play) simulation: &'a SimulationHandle,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) runtime_control: Option<&'a RuntimeControlHandle>,
    pub(in crate::play) chunk_pipeline_resources: &'a ChunkPipelineResources,
    pub(in crate::play) chunk_stream: &'a mut Option<ChunkStreamState>,
    pub(in crate::play) next_teleport_id: &'a mut i32,
    pub(in crate::play) pending_teleport: &'a mut Option<PendingTeleport>,
}

pub(crate) fn is_serverbound_chat_command_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundCommandSuggestion::ID | ServerboundChat::ID | ServerboundChatCommand::ID
    )
}

/// Re-resolve live operator authority for a connected player.
///
/// The console can grant or revoke operator status while a player is online;
/// the refreshed command tree keeps client suggestions in step.
#[expect(
    clippy::too_many_arguments,
    reason = "the writer, live connection and identity are distinct authorities"
)]
pub(crate) async fn refresh_live_permissions<W>(
    writer: &mut W,
    compression: Compression,
    scripts: Option<&ScriptEventSink>,
    config: &ServerConfig,
    player_uuid: &str,
    player_name: &str,
    login_resolved: CommandPermissions,
    peer: std::net::SocketAddr,
) -> Result<CommandPermissions, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let live = config
        .command_permissions
        .live_permissions_for(player_name, player_uuid, peer);
    if live != login_resolved {
        let plugin_roots = scripts.map_or_else(Vec::new, ScriptEventSink::player_command_roots);
        let operator_roots = scripts.map_or_else(Vec::new, ScriptEventSink::operator_command_roots);
        write_packet(
            writer,
            &command_tree_packet_with_plugin_roots(live, &plugin_roots, &operator_roots),
            compression,
        )
        .await?;
    }
    Ok(live)
}

pub(crate) async fn handle_serverbound_chat_command<W>(
    context: ChatCommandIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let ChatCommandIngressContext {
        writer,
        compression,
        scripts,
        session_id,
        dimension,
        player_uuid,
        player_name,
        permissions,
        peer,
        player_pose,
        game_mode,
        survival_state,
        xp_state,
        config,
        sessions,
        simulation,
        interaction,
        runtime_control,
        chunk_pipeline_resources,
        chunk_stream,
        next_teleport_id,
        pending_teleport,
    } = context;
    let permissions = refresh_live_permissions(
        writer,
        compression,
        scripts,
        config,
        player_uuid,
        player_name,
        permissions,
        peer,
    )
    .await?;
    let mut body = frame.body;
    match frame.id {
        ServerboundCommandSuggestion::ID => {
            let request = ServerboundCommandSuggestion::decode(&mut body)?;
            let plugin_command_roots =
                scripts.map_or_else(Vec::new, ScriptEventSink::player_command_roots);
            let operator_plugin_command_roots =
                scripts.map_or_else(Vec::new, ScriptEventSink::operator_command_roots);
            let suggestions = command_suggestions_with_plugin_roots(
                &request.command,
                permissions,
                &plugin_command_roots,
                &operator_plugin_command_roots,
            );
            debug!(
                request_id = request.id,
                command = %request.command,
                count = suggestions.suggestions.len(),
                "command suggestions requested"
            );
            write_packet(
                writer,
                &ClientboundCommandSuggestions {
                    id: request.id,
                    start: suggestions.start,
                    length: suggestions.length,
                    suggestions: suggestions
                        .suggestions
                        .into_iter()
                        .map(|text| mc_protocol::packets::play::CommandSuggestionEntry {
                            text,
                            tooltip_nbt: None,
                        })
                        .collect(),
                },
                compression,
            )
            .await?;
        }
        ServerboundChat::ID => {
            let chat = ServerboundChat::decode(&mut body)?;
            if chat.message.eq_ignore_ascii_case("blink") {
                debug!(
                    target: "solaris::chunk_visibility",
                    event = "chunk_blink_marker",
                    session_id,
                    player_name,
                    player_uuid,
                    tick = sessions.simulation_tick(),
                    pose = ?player_pose,
                    "player reported chunk blink"
                );
            }
            if let Some(scripts) = scripts {
                scripts.enqueue_event(ScriptEvent::player_chat_with_context(
                    ScriptPlayerId::new(session_id),
                    chat.message.clone(),
                    script_player_context_from_values(
                        player_uuid,
                        player_name,
                        permissions,
                        *player_pose,
                    ),
                ));
            }
            dispatch_visibility_commands(
                sessions.broadcast_system_chat(format!("<{}> {}", player_name, chat.message)),
            );
        }
        ServerboundChatCommand::ID => {
            let command = ServerboundChatCommand::decode(&mut body)?;
            if let Some(scripts) = scripts {
                match scripts.enqueue_player_command_with_context(
                    session_id,
                    script_player_context_from_values(
                        player_uuid,
                        player_name,
                        permissions,
                        *player_pose,
                    ),
                    &command.command,
                ) {
                    mc_script::PlayerCommandAdmission::Enqueued => {
                        debug!(command = %command.command, "player command routed to component plugin");
                        return Ok(());
                    }
                    mc_script::PlayerCommandAdmission::Dropped => {
                        debug!(
                            command = %command.command,
                            "player command dropped because the component event queue is full"
                        );
                        return Ok(());
                    }
                    mc_script::PlayerCommandAdmission::PermissionDenied => {
                        send_command_feedback(
                            writer,
                            compression,
                            command_error_message(CommandError::PermissionDenied),
                        )
                        .await?;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            execute_player_command(
                writer,
                compression,
                &command.command,
                permissions,
                game_mode,
                survival_state,
                xp_state,
                config,
                sessions,
                session_id,
                dimension,
                simulation,
                interaction,
                player_pose,
                runtime_control,
                chunk_pipeline_resources,
                chunk_stream,
                next_teleport_id,
                pending_teleport,
            )
            .await?;
        }
        _ => unreachable!("chat/command helper only accepts chat/command packet ids"),
    }
    Ok(())
}
