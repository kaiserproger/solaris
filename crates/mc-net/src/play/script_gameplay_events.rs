use mc_data::items::ItemRegistry;
use mc_protocol::packets::play::GameMode;
use mc_script::{
    ScriptCraftingSource, ScriptEvent, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId,
    ScriptQueueError,
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

    pub(super) async fn publish_block_placed(
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
                "committed block placement has no registry identity"
            );
            return false;
        };
        let game_mode = match game_mode {
            GameMode::Survival => ScriptGameMode::Survival,
            GameMode::Creative => ScriptGameMode::Creative,
            _ => {
                warn!(
                    ?game_mode,
                    "committed block placement has unsupported script game mode"
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
        let event = match ScriptEvent::try_player_block_placed_with_context(
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
                warn!(?error, "committed block placement script event is invalid");
                return false;
            }
        };
        match self.sink.enqueue_required_event(event).await {
            Ok(()) => true,
            Err(ScriptQueueError::Closed) => {
                warn!("script event queue closed after committed block placement");
                false
            }
            Err(error) => {
                warn!(
                    ?error,
                    "committed block placement script event was rejected"
                );
                false
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn publish_item_crafted(
        &self,
        items: &ItemRegistry,
        item_id: u32,
        count: u64,
        craft_count: u32,
        source: ScriptCraftingSource,
        pose: PlayerPose,
        game_mode: GameMode,
    ) -> bool {
        let Some(item) = items.name_of(item_id) else {
            warn!(item_id, "committed craft has no registry identity");
            return false;
        };
        let game_mode = match game_mode {
            GameMode::Survival => ScriptGameMode::Survival,
            GameMode::Creative => ScriptGameMode::Creative,
            GameMode::Adventure => ScriptGameMode::Adventure,
            _ => {
                warn!(
                    ?game_mode,
                    "committed craft has unsupported script game mode"
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
        let event = match ScriptEvent::try_player_item_crafted_with_context(
            self.player_id,
            context,
            &self.dimension,
            item.as_str(),
            count,
            craft_count,
            source,
            game_mode,
        ) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "committed craft script event is invalid");
                return false;
            }
        };
        match self.sink.enqueue_required_event(event).await {
            Ok(()) => true,
            Err(ScriptQueueError::Closed) => {
                warn!("script event queue closed after committed craft");
                false
            }
            Err(error) => {
                warn!(?error, "committed craft script event was rejected");
                false
            }
        }
    }
}
