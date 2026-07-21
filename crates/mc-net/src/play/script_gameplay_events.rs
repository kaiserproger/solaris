use mc_protocol::packets::play::GameMode;
use mc_script::{
    ScriptEvent, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId, ScriptQueueError,
};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};
use tracing::warn;

use super::{CommandPermissions, PlayerPose};
use crate::server::ScriptEventSink;

#[derive(Clone)]
pub(super) struct ScriptGameplayEventPublisher {
    sink: ScriptEventSink,
    player_id: ScriptPlayerId,
    uuid: String,
    username: String,
    permissions: CommandPermissions,
    dimension: String,
}

impl ScriptGameplayEventPublisher {
    pub(super) fn new(
        sink: ScriptEventSink,
        player_id: ScriptPlayerId,
        uuid: impl Into<String>,
        username: impl Into<String>,
        permissions: CommandPermissions,
        dimension: impl Into<String>,
    ) -> Self {
        Self {
            sink,
            player_id,
            uuid: uuid.into(),
            username: username.into(),
            permissions,
            dimension: dimension.into(),
        }
    }

    pub(super) async fn publish_block_broken(
        &self,
        blocks: &BlockRegistry,
        state: BlockStateId,
        position: BlockPos,
        pose: PlayerPose,
        game_mode: GameMode,
    ) -> bool {
        let Some(block) = blocks.by_id(state) else {
            warn!(
                state = state.0,
                "committed block break has no registry identity"
            );
            return false;
        };
        let game_mode = match game_mode {
            GameMode::Survival => ScriptGameMode::Survival,
            GameMode::Creative => ScriptGameMode::Creative,
            _ => {
                warn!(
                    ?game_mode,
                    "committed block break has unsupported script game mode"
                );
                return false;
            }
        };
        let context = ScriptPlayerContext::new(
            &self.uuid,
            &self.username,
            self.permissions.op,
            pose.x,
            pose.y,
            pose.z,
        );
        let event = match ScriptEvent::try_player_block_broken_with_context(
            self.player_id,
            context,
            &self.dimension,
            block.block.id.as_str(),
            position.x,
            position.y,
            position.z,
            game_mode,
        ) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "committed block break script event is invalid");
                return false;
            }
        };
        match self.sink.enqueue_required_event(event).await {
            Ok(()) => true,
            Err(ScriptQueueError::Closed) => {
                warn!("script event queue closed after committed block break");
                false
            }
            Err(error) => {
                warn!(?error, "committed block break script event was rejected");
                false
            }
        }
    }
}
