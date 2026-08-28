use bytes::{Buf, Bytes};
use mc_extension::DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES;
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

macro_rules! for_each_serverbound_play_packet {
    ($visit:ident) => {
        $visit!(ServerboundKeepAlive);
        $visit!(ConfirmTeleportation);
        $visit!(ServerboundMovePlayerPos);
        $visit!(ServerboundMovePlayerPosRot);
        $visit!(ServerboundMovePlayerRot);
        $visit!(ServerboundMovePlayerStatusOnly);
        $visit!(ServerboundPlayerAction);
        $visit!(ServerboundPlayerCommand);
        $visit!(ServerboundPlayerInput);
        $visit!(ServerboundUseItemOn);
        $visit!(ServerboundUseItem);
        $visit!(ServerboundSignUpdate);
        $visit!(ServerboundAttack);
        $visit!(ServerboundInteract);
        $visit!(ServerboundSwing);
        $visit!(ServerboundPlaceRecipe);
        $visit!(ServerboundSelectTrade);
        $visit!(ServerboundContainerButtonClick);
        $visit!(ServerboundContainerClick);
        $visit!(ServerboundContainerClose);
        $visit!(ServerboundRecipeBookChangeSettings);
        $visit!(ServerboundRecipeBookSeenRecipe);
        $visit!(ServerboundSetCarriedItem);
        $visit!(ServerboundClientCommand);
        $visit!(ServerboundClientInformation);
        $visit!(ServerboundCustomPayload);
        $visit!(ServerboundResourcePack);
        $visit!(ServerboundChatAck);
        $visit!(ServerboundChunkBatchReceived);
        $visit!(ServerboundClientTickEnd);
        $visit!(ServerboundPlayerLoaded);
        $visit!(ServerboundCommandSuggestion);
        $visit!(ServerboundChat);
        $visit!(ServerboundChatCommand);
        $visit!(ServerboundChangeGameMode);
    };
}

pub(super) fn decode_exact<P: Packet>(id: i32, body: &Bytes) -> Result<P, ConnectionError> {
    let mut body = body.clone();
    let packet = P::decode(&mut body)?;
    let trailing = body.remaining();
    if trailing != 0 {
        return Err(ConnectionError::TrailingBytes {
            state: mc_protocol::State::Play,
            id,
            trailing,
        });
    }
    Ok(packet)
}

/// Validate one recognized serverbound Play packet before it may count as
/// inbound liveness. Unknown IDs remain ignored and do not refresh activity.
pub(super) fn validate_serverbound_play_frame(
    id: i32,
    body: &Bytes,
) -> Result<bool, ConnectionError> {
    if id == ServerboundCustomPayload::ID && body.len() > DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES {
        return Ok(false);
    }

    macro_rules! recognized {
        ($packet:ty) => {
            if id == <$packet>::ID {
                decode_exact::<$packet>(id, body)?;
                return Ok(true);
            }
        };
    }

    for_each_serverbound_play_packet!(recognized);
    Ok(false)
}

#[cfg(test)]
mod tests {
    use bytes::{BufMut as _, BytesMut};

    use super::*;

    #[test]
    fn recognized_packet_inventory_has_unique_ids() {
        let mut ids = Vec::new();
        macro_rules! collect_id {
            ($packet:ty) => {
                ids.push(<$packet>::ID);
            };
        }
        for_each_serverbound_play_packet!(collect_id);
        assert_eq!(ids.len(), 35);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 35, "serverbound Play packet IDs must be unique");
    }

    #[test]
    fn every_recognized_packet_has_a_dispatch_classification() {
        fn family_routes(packet: &str, id: i32) -> bool {
            match packet {
                "ServerboundMovePlayerPos"
                | "ServerboundMovePlayerPosRot"
                | "ServerboundMovePlayerRot"
                | "ServerboundMovePlayerStatusOnly" => {
                    super::super::is_serverbound_movement_packet(id)
                }
                "ServerboundPlayerAction"
                | "ServerboundPlayerCommand"
                | "ServerboundPlayerInput" => super::super::is_serverbound_player_state_packet(id),
                "ServerboundUseItemOn"
                | "ServerboundUseItem"
                | "ServerboundSignUpdate"
                | "ServerboundAttack"
                | "ServerboundInteract"
                | "ServerboundSwing" => super::super::is_serverbound_use_interaction_packet(id),
                "ServerboundPlaceRecipe"
                | "ServerboundSelectTrade"
                | "ServerboundContainerButtonClick"
                | "ServerboundContainerClick"
                | "ServerboundContainerClose" => super::super::is_serverbound_container_packet(id),
                "ServerboundSetCarriedItem"
                | "ServerboundClientCommand"
                | "ServerboundChangeGameMode" => {
                    super::super::is_serverbound_player_control_packet(id)
                }
                "ServerboundRecipeBookChangeSettings"
                | "ServerboundRecipeBookSeenRecipe"
                | "ServerboundClientInformation"
                | "ServerboundCustomPayload"
                | "ServerboundResourcePack"
                | "ServerboundChatAck"
                | "ServerboundChunkBatchReceived"
                | "ServerboundClientTickEnd"
                | "ServerboundPlayerLoaded" => {
                    super::super::is_serverbound_client_metadata_packet(id)
                }
                "ServerboundCommandSuggestion" | "ServerboundChat" | "ServerboundChatCommand" => {
                    super::super::is_serverbound_chat_command_packet(id)
                }
                _ => false,
            }
        }

        macro_rules! assert_dispatch_branch {
            ($packet:ty) => {
                let packet = stringify!($packet);
                let id = <$packet>::ID;
                let direct = id == ServerboundKeepAlive::ID || id == ConfirmTeleportation::ID;
                assert!(
                    direct || family_routes(packet, id),
                    "recognized packet {packet} is missing from direct or family Play dispatch classification"
                );
            };
        }
        for_each_serverbound_play_packet!(assert_dispatch_branch);
    }

    #[test]
    fn unknown_packet_does_not_count_as_valid_activity() {
        assert!(!validate_serverbound_play_frame(0x7fff, &Bytes::new()).unwrap());
    }

    #[test]
    fn oversized_custom_payload_is_ignored_without_refreshing_activity() {
        let body = Bytes::from(vec![0_u8; DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1]);
        assert!(!validate_serverbound_play_frame(ServerboundCustomPayload::ID, &body).unwrap());
    }

    #[test]
    fn valid_non_keepalive_packet_counts_after_exact_decode() {
        let mut body = BytesMut::new();
        ConfirmTeleportation { teleport_id: 7 }
            .encode(&mut body)
            .unwrap();
        let body = body.freeze();
        assert!(validate_serverbound_play_frame(ConfirmTeleportation::ID, &body).unwrap());
        let decoded =
            decode_exact::<ConfirmTeleportation>(ConfirmTeleportation::ID, &body).unwrap();
        assert_eq!(decoded.teleport_id, 7);
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
