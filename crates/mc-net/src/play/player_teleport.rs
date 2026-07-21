use mc_protocol::frame::Compression;
use mc_script::{ScriptPlayerTeleportFailure, ScriptPosition};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::error::ConnectionError;

use super::chunk_stream::ChunkStreamState;
use super::movement::{PendingTeleport, next_player_teleport_id};
use super::session::{ScriptPlayerTeleportCommand, SessionRegistry};
use super::simulation::SimulationHandle;
use super::{
    InteractionState, PlayerPose, ScriptZoneObserver, clear_shield_use, refresh_player_water_state,
    replan_after_movement, send_player_position_sync,
};

pub(super) fn prepare_script_player_teleport(
    current: PlayerPose,
    position: ScriptPosition,
    teleport_pending: bool,
) -> Result<PlayerPose, ScriptPlayerTeleportFailure> {
    if teleport_pending {
        return Err(ScriptPlayerTeleportFailure::TeleportPending);
    }
    let mut candidate = current;
    candidate.x = position.x();
    candidate.y = position.y();
    candidate.z = position.z();
    candidate.fall_start_y = position.y();
    Ok(candidate)
}

pub(super) fn clear_player_interactions_for_teleport(state: &mut InteractionState) {
    state.pending_break = None;
    state.delayed_break = None;
    state.pending_use = None;
    clear_shield_use(state);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_script_player_teleport<W>(
    command: ScriptPlayerTeleportCommand,
    writer: &mut W,
    compression: Compression,
    interaction: &mut Option<&mut InteractionState>,
    chunk_stream: &mut Option<ChunkStreamState>,
    simulation: &SimulationHandle,
    script_zone_observer: &mut Option<ScriptZoneObserver>,
    sessions: &SessionRegistry,
    player_pose: &mut PlayerPose,
    next_teleport_id: &mut i32,
    pending_teleport: &mut Option<PendingTeleport>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut candidate = match prepare_script_player_teleport(
        *player_pose,
        command.position,
        pending_teleport.is_some(),
    ) {
        Ok(candidate) => candidate,
        Err(failure) => {
            command.complete(Err(failure));
            return Ok(());
        }
    };
    refresh_player_water_state(interaction.as_deref(), &mut candidate).await;
    let (_, completion) = command.into_owner_completion();
    if let Err(error) = simulation
        .commit_script_player_teleport(candidate, completion)
        .await
    {
        warn!(?error, "script player teleport owner commit failed");
        return Ok(());
    }

    let old_center = player_pose.chunk_pos();
    let new_center = candidate.chunk_pos();
    *player_pose = candidate;
    if let Some(state) = interaction.as_deref_mut() {
        clear_player_interactions_for_teleport(state);
    }
    let teleport_id = next_player_teleport_id(next_teleport_id);
    *pending_teleport = Some(PendingTeleport::new(
        teleport_id,
        sessions.simulation_tick(),
    ));

    let wire_result =
        match send_player_position_sync(writer, compression, teleport_id, *player_pose).await {
            Ok(()) => {
                replan_after_movement(
                    writer,
                    compression,
                    chunk_stream,
                    interaction.as_deref_mut(),
                    old_center,
                    new_center,
                    player_pose.yaw,
                )
                .await
            }
            Err(error) => Err(error),
        };
    if let Some(observer) = script_zone_observer.as_mut() {
        observer.observe(*player_pose).await;
    }
    wire_result
}
