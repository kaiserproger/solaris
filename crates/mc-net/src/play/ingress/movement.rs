use super::super::*;

pub(crate) struct PlayerMovementIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) compression: Compression,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) sessions: &'a SessionRegistry,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) dimension: &'a str,
    pub(in crate::play) chunk_stream: &'a mut Option<ChunkStreamState>,
    pub(in crate::play) simulation: &'a SimulationHandle,
    pub(in crate::play) script_zone_observer: &'a mut Option<ScriptZoneObserver>,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) player_pose: &'a mut PlayerPose,
    pub(in crate::play) current_tick: u64,
    pub(in crate::play) next_teleport_id: &'a mut i32,
    pub(in crate::play) pending_teleport: &'a mut Option<PendingTeleport>,
}

pub(in crate::play) async fn handle_accepted_absolute_movement<W>(
    context: PlayerMovementIngressContext<'_, W>,
    movement: AcceptedAbsoluteMovement,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let PlayerMovementIngressContext {
        writer,
        compression,
        sessions,
        session_id,
        dimension,
        chunk_stream,
        mut interaction,
        script_zone_observer,
        simulation,
        survival_state,
        xp_state,
        game_mode,
        player_pose,
        current_tick,
        next_teleport_id,
        pending_teleport,
    } = context;
    let movement = normalize_absolute_player_movement(movement)?;
    let old_center = player_pose.chunk_pos();
    let old_pose = *player_pose;
    let mut new_pose = *player_pose;
    new_pose.x = movement.x;
    new_pose.y = movement.y;
    new_pose.z = movement.z;
    if let Some((yaw, pitch)) = movement.yaw_pitch {
        new_pose.yaw = yaw;
        new_pose.pitch = pitch;
    }
    new_pose.flags = movement.flags;
    refresh_player_water_state(interaction.as_deref(), &mut new_pose).await;
    refresh_player_fall_state(old_pose, &mut new_pose);
    if game_mode != GameMode::Spectator
        && correct_player_collision(
            interaction.as_deref(),
            writer,
            compression,
            old_pose,
            new_pose,
            current_tick,
            next_teleport_id,
            pending_teleport,
        )
        .await?
    {
        *player_pose = old_pose;
        return Ok(());
    }

    let exhaustion = if game_mode == GameMode::Survival {
        movement_exhaustion(old_pose, new_pose)
    } else {
        0.0
    };
    let committed_pose =
        match commit_authoritative_player_movement(simulation, new_pose, exhaustion).await {
            Ok(committed) => committed,
            Err(SimulationRequestError::PlayerMovementRejected(reason)) => {
                warn!(
                    ?reason,
                    "authoritative player movement rejected; correcting client pose"
                );
                let teleport_id = next_player_teleport_id(next_teleport_id);
                send_player_position_sync(writer, compression, teleport_id, old_pose).await?;
                *pending_teleport = Some(PendingTeleport::new(teleport_id, current_tick));
                return Ok(());
            }
            Err(error) => return Err(simulation_pose_commit_error(error)),
        };
    *player_pose = new_pose;
    if let Some(observer) = script_zone_observer.as_mut() {
        observer.observe(*player_pose).await;
    }
    if game_mode == GameMode::Survival {
        committed_pose.apply_resources_to(survival_state);
        if committed_pose.resources_changed {
            write_packet(writer, &survival_state.as_packet(), compression).await?;
        }
    }
    if game_mode == GameMode::Survival
        && let Some(state) = interaction.as_deref_mut()
    {
        maybe_trample_farmland(state, writer, old_pose, *player_pose).await?;
    }
    if game_mode == GameMode::Survival {
        apply_fall_damage(
            sessions,
            session_id,
            dimension,
            interaction.as_deref_mut(),
            writer,
            compression,
            survival_state,
            xp_state,
            old_pose,
            *player_pose,
        )
        .await?;
    }
    let new_center = player_pose.chunk_pos();
    replan_after_movement(
        writer,
        compression,
        chunk_stream,
        interaction,
        old_center,
        new_center,
        player_pose.yaw,
    )
    .await?;
    Ok(())
}

pub(crate) fn is_serverbound_movement_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundMovePlayerPos::ID
            | ServerboundMovePlayerPosRot::ID
            | ServerboundMovePlayerRot::ID
            | ServerboundMovePlayerStatusOnly::ID
    )
}

pub(crate) async fn handle_serverbound_movement<W>(
    context: PlayerMovementIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
    client_loaded: bool,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !client_loaded {
        return Ok(());
    }
    let packet_name = match frame.id {
        ServerboundMovePlayerPos::ID => "ServerboundMovePlayerPos",
        ServerboundMovePlayerPosRot::ID => "ServerboundMovePlayerPosRot",
        ServerboundMovePlayerRot::ID => "ServerboundMovePlayerRot",
        ServerboundMovePlayerStatusOnly::ID => "ServerboundMovePlayerStatusOnly",
        _ => unreachable!("movement helper only accepts movement packet ids"),
    };
    if guard_pending_teleport_movement(context.pending_teleport, packet_name) {
        return Ok(());
    }
    let mut body = frame.body;
    match frame.id {
        ServerboundMovePlayerPos::ID => {
            let movement = ServerboundMovePlayerPos::decode(&mut body)?;
            handle_accepted_absolute_movement(
                context,
                AcceptedAbsoluteMovement {
                    x: movement.x,
                    y: movement.y,
                    z: movement.z,
                    yaw_pitch: None,
                    flags: movement.flags,
                },
            )
            .await
        }
        ServerboundMovePlayerPosRot::ID => {
            let movement = ServerboundMovePlayerPosRot::decode(&mut body)?;
            handle_accepted_absolute_movement(
                context,
                AcceptedAbsoluteMovement {
                    x: movement.x,
                    y: movement.y,
                    z: movement.z,
                    yaw_pitch: Some((movement.yaw, movement.pitch)),
                    flags: movement.flags,
                },
            )
            .await
        }
        ServerboundMovePlayerRot::ID => {
            let movement = ServerboundMovePlayerRot::decode(&mut body)?;
            validate_player_rotation(movement.yaw, movement.pitch)?;
            let PlayerMovementIngressContext {
                writer,
                compression,
                interaction,
                chunk_stream,
                simulation,
                player_pose,
                ..
            } = context;
            let old_pose = *player_pose;
            player_pose.yaw = movement.yaw;
            player_pose.pitch = movement.pitch;
            player_pose.flags = movement.flags;
            commit_authoritative_player_pose(simulation, player_pose, old_pose).await?;
            let center = player_pose.chunk_pos();
            replan_after_movement(
                writer,
                compression,
                chunk_stream,
                interaction,
                center,
                center,
                player_pose.yaw,
            )
            .await
        }
        ServerboundMovePlayerStatusOnly::ID => {
            let movement = ServerboundMovePlayerStatusOnly::decode(&mut body)?;
            let PlayerMovementIngressContext {
                simulation,
                player_pose,
                ..
            } = context;
            let old_pose = *player_pose;
            player_pose.flags = movement.flags;
            commit_authoritative_player_pose(simulation, player_pose, old_pose).await
        }
        _ => unreachable!("movement helper only accepts movement packet ids"),
    }
}
