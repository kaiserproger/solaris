use bytes::{Buf, Bytes};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ConfirmTeleportation, ServerboundAttack, ServerboundChangeGameMode, ServerboundChat,
    ServerboundChatAck, ServerboundChatCommand, ServerboundChunkBatchReceived,
    ServerboundClientCommand, ServerboundClientInformation, ServerboundClientTickEnd,
    ServerboundCommandSuggestion, ServerboundContainerButtonClick, ServerboundContainerClick,
    ServerboundContainerClose, ServerboundCustomPayload, ServerboundInteract, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerPosRot, ServerboundMovePlayerRot,
    ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe, ServerboundPlayerAction,
    ServerboundPlayerCommand, ServerboundPlayerInput, ServerboundPlayerLoaded,
    ServerboundRecipeBookChangeSettings, ServerboundRecipeBookSeenRecipe, ServerboundResourcePack,
    ServerboundSelectTrade, ServerboundSetCarriedItem, ServerboundSignUpdate, ServerboundSwing,
    ServerboundUseItem, ServerboundUseItemOn,
};

use crate::error::ConnectionError;

fn decode_exact<P: Packet>(id: i32, body: &Bytes) -> Result<(), ConnectionError> {
    let mut body = body.clone();
    P::decode(&mut body)?;
    let trailing = body.remaining();
    if trailing != 0 {
        return Err(ConnectionError::TrailingBytes {
            state: mc_protocol::State::Play,
            id,
            trailing,
        });
    }
    Ok(())
}

/// Validate one recognized serverbound Play packet before it may count as
/// inbound liveness. Unknown IDs remain ignored and do not refresh activity.
pub(super) fn validate_serverbound_play_frame(
    id: i32,
    body: &Bytes,
) -> Result<bool, ConnectionError> {
    macro_rules! recognized {
        ($packet:ty) => {
            if id == <$packet>::ID {
                decode_exact::<$packet>(id, body)?;
                return Ok(true);
            }
        };
    }

    recognized!(ServerboundKeepAlive);
    recognized!(ConfirmTeleportation);
    recognized!(ServerboundMovePlayerPos);
    recognized!(ServerboundMovePlayerPosRot);
    recognized!(ServerboundMovePlayerRot);
    recognized!(ServerboundMovePlayerStatusOnly);
    recognized!(ServerboundPlayerAction);
    recognized!(ServerboundPlayerCommand);
    recognized!(ServerboundPlayerInput);
    recognized!(ServerboundUseItemOn);
    recognized!(ServerboundUseItem);
    recognized!(ServerboundSignUpdate);
    recognized!(ServerboundAttack);
    recognized!(ServerboundInteract);
    recognized!(ServerboundSwing);
    recognized!(ServerboundPlaceRecipe);
    recognized!(ServerboundSelectTrade);
    recognized!(ServerboundContainerButtonClick);
    recognized!(ServerboundContainerClick);
    recognized!(ServerboundContainerClose);
    recognized!(ServerboundRecipeBookChangeSettings);
    recognized!(ServerboundRecipeBookSeenRecipe);
    recognized!(ServerboundSetCarriedItem);
    recognized!(ServerboundClientCommand);
    recognized!(ServerboundClientInformation);
    recognized!(ServerboundCustomPayload);
    recognized!(ServerboundResourcePack);
    recognized!(ServerboundChatAck);
    recognized!(ServerboundChunkBatchReceived);
    recognized!(ServerboundClientTickEnd);
    recognized!(ServerboundPlayerLoaded);
    recognized!(ServerboundCommandSuggestion);
    recognized!(ServerboundChat);
    recognized!(ServerboundChatCommand);
    recognized!(ServerboundChangeGameMode);
    Ok(false)
}

#[cfg(test)]
mod tests {
    use bytes::{BufMut as _, BytesMut};

    use super::*;

    #[test]
    fn unknown_packet_does_not_count_as_valid_activity() {
        assert!(!validate_serverbound_play_frame(0x7fff, &Bytes::new()).unwrap());
    }

    #[test]
    fn valid_non_keepalive_packet_counts_after_exact_decode() {
        let mut body = BytesMut::new();
        ConfirmTeleportation { teleport_id: 0 }
            .encode(&mut body)
            .unwrap();
        assert!(validate_serverbound_play_frame(ConfirmTeleportation::ID, &body.freeze()).unwrap());
    }

    #[test]
    fn trailing_and_truncated_packets_fail_before_activity() {
        let mut trailing = BytesMut::new();
        ServerboundKeepAlive { id: 0 }
            .encode(&mut trailing)
            .unwrap();
        trailing.put_u8(0xff);
        assert!(matches!(
            validate_serverbound_play_frame(ServerboundKeepAlive::ID, &trailing.freeze()),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));

        assert!(
            validate_serverbound_play_frame(
                ServerboundMovePlayerPos::ID,
                &Bytes::from_static(&[0])
            )
            .is_err()
        );
    }

    #[test]
    fn exact_keepalive_and_mismatched_id_are_syntactically_valid() {
        let mut body = BytesMut::new();
        ServerboundKeepAlive { id: 77 }.encode(&mut body).unwrap();
        assert!(validate_serverbound_play_frame(ServerboundKeepAlive::ID, &body.freeze()).unwrap());
    }
}
