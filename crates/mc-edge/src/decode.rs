//! Serverbound Play decode: pure packet-body -> [`PlayerInput`] mapping.
//!
//! Runs off-tick on I/O threads. [`DecodeState`] retains rotation for
//! position-only frames; rotation/status-only frames leave coordinates
//! absent so the core resolves them against its authoritative pose.
//!
//! A malformed body of a known packet is `Err` (the caller drops the
//! connection). Known packets without gameplay intent are validated and
//! ignored; unknown ids are ignored. `Swing` maps to nothing: the server
//! derives the animation from Dig/Attack/Use. `ChatCommand` becomes
//! [`PlayerInput::Command`] with the leading `/` stripped; a `/…` line sent
//! as plain chat stays `Chat`.

use mc_core::boundary::{
    BlockPos, ClickMode, ConnectionId, DigAction, EntityId, Hand, PlayerInput,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientCommandAction, ConfirmTeleportation, ContainerInput, InteractionHand, PlayerActionKind,
    PlayerCommandAction, ServerboundAttack, ServerboundChat, ServerboundChatAck,
    ServerboundChatCommand, ServerboundChatCommandSigned, ServerboundChatSessionUpdate,
    ServerboundChunkBatchReceived, ServerboundClientCommand, ServerboundClientInformation,
    ServerboundClientTickEnd, ServerboundCommandSuggestion, ServerboundContainerButtonClick,
    ServerboundContainerClick, ServerboundContainerClose, ServerboundContainerSlotStateChanged,
    ServerboundCustomClickAction, ServerboundCustomPayload, ServerboundInteract,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundMovePlayerPosRot,
    ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly, ServerboundMoveVehicle,
    ServerboundPickItemFromBlock, ServerboundPickItemFromEntity, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundPlayerCommand, ServerboundPlayerInput,
    ServerboundPlayerLoaded, ServerboundPunch, ServerboundRenameItem, ServerboundResourcePack,
    ServerboundSeenAdvancements, ServerboundSelectTrade, ServerboundSetBeacon,
    ServerboundSetCarriedItem, ServerboundSetCreativeModeSlot, ServerboundSignUpdate,
    ServerboundUseItem, ServerboundUseItemOn, unpack_block_pos,
};

/// Rotation of the last rotation-bearing movement frame, for position-only
/// frames.
#[derive(Debug, Clone, Default)]
pub struct DecodeState {
    pub last_yaw: f32,
    pub last_pitch: f32,
}

/// Decode one serverbound Play frame, appending its inputs to `out`.
pub fn decode_play_frame(
    conn: ConnectionId,
    id: i32,
    body: &[u8],
    state: &mut DecodeState,
    out: &mut Vec<PlayerInput>,
) -> Result<(), String> {
    match id {
        ConfirmTeleportation::ID => {
            let p = read::<ConfirmTeleportation>(body)?;
            out.push(PlayerInput::ConfirmTeleport {
                connection: conn,
                teleport_id: p.teleport_id,
            });
            // 26.3 folds the post-teleport position echo into the accept
            // packet (the 26.1.2 client sent a separate PosRot with
            // onGround=false); vanilla handles it as that same movement.
            state.last_yaw = p.yaw;
            state.last_pitch = p.pitch;
            out.push(PlayerInput::Move {
                connection: conn,
                position: p.position,
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: false,
            });
        }
        ServerboundSeenAdvancements::ID => {
            let tab = match read::<ServerboundSeenAdvancements>(body)? {
                ServerboundSeenAdvancements::OpenedTab(id) => Some(id.to_string()),
                ServerboundSeenAdvancements::ClosedScreen => None,
            };
            out.push(PlayerInput::SeenAdvancements {
                connection: conn,
                tab,
            });
        }
        ServerboundMovePlayerPos::ID => {
            let p = read::<ServerboundMovePlayerPos>(body)?;
            out.push(PlayerInput::Move {
                connection: conn,
                position: [p.x, p.y, p.z],
                yaw: state.last_yaw,
                pitch: state.last_pitch,
                on_ground: p.flags.on_ground,
            });
        }
        ServerboundMovePlayerPosRot::ID => {
            let p = read::<ServerboundMovePlayerPosRot>(body)?;
            state.last_yaw = p.yaw;
            state.last_pitch = p.pitch;
            out.push(PlayerInput::Move {
                connection: conn,
                position: [p.x, p.y, p.z],
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.flags.on_ground,
            });
        }
        ServerboundMovePlayerRot::ID => {
            let p = read::<ServerboundMovePlayerRot>(body)?;
            state.last_yaw = p.yaw;
            state.last_pitch = p.pitch;
            out.push(PlayerInput::MoveRotation {
                connection: conn,
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.flags.on_ground,
            });
        }
        ServerboundMovePlayerStatusOnly::ID => {
            let p = read::<ServerboundMovePlayerStatusOnly>(body)?;
            out.push(PlayerInput::MoveStatus {
                connection: conn,
                on_ground: p.flags.on_ground,
            });
        }
        ServerboundPlayerAction::ID => {
            let p = read::<ServerboundPlayerAction>(body)?;
            let position = block_pos(p.position);
            let dig = |action| PlayerInput::Dig {
                connection: conn,
                position,
                action,
                sequence: p.sequence,
            };
            out.push(match p.action {
                PlayerActionKind::StartDestroyBlock => dig(DigAction::Start),
                PlayerActionKind::AbortDestroyBlock => dig(DigAction::Cancel),
                PlayerActionKind::StopDestroyBlock => dig(DigAction::Finish),
                PlayerActionKind::DropAllItems => PlayerInput::DropSelected {
                    connection: conn,
                    whole_stack: true,
                },
                PlayerActionKind::DropItem => PlayerInput::DropSelected {
                    connection: conn,
                    whole_stack: false,
                },
                PlayerActionKind::ReleaseUseItem => {
                    PlayerInput::ReleaseUseItem { connection: conn }
                }
                PlayerActionKind::SwapItemWithOffhand => {
                    PlayerInput::SwapHands { connection: conn }
                }
                // Used-while-sneaking marker: no gameplay mapping.
                PlayerActionKind::Stab => return Ok(()),
                // 26.3 mining-face change while holding attack: no mapping.
                PlayerActionKind::ChangeDestroyDirection => return Ok(()),
            });
        }
        ServerboundUseItemOn::ID => {
            let p = read::<ServerboundUseItemOn>(body)?;
            out.push(PlayerInput::UseItemOn {
                connection: conn,
                hand: hand_of(p.hand),
                position: block_pos(p.position),
                face: p.direction as u8,
                cursor: [p.cursor_x, p.cursor_y, p.cursor_z],
                sequence: p.sequence,
            });
        }
        ServerboundUseItem::ID => {
            let p = read::<ServerboundUseItem>(body)?;
            out.push(PlayerInput::UseItem {
                connection: conn,
                hand: hand_of(p.hand),
                yaw: p.y_rot,
                pitch: p.x_rot,
                sequence: p.sequence,
            });
        }
        ServerboundAttack::ID => {
            let p = read::<ServerboundAttack>(body)?;
            out.push(PlayerInput::Attack {
                connection: conn,
                target: EntityId(p.entity_id),
            });
        }
        ServerboundInteract::ID => {
            let p = read::<ServerboundInteract>(body)?;
            let target = EntityId(p.entity_id);
            let hand = hand_of(p.hand);
            let location = [p.location.x, p.location.y, p.location.z];
            out.push(if p.using_secondary_action {
                PlayerInput::InteractEntitySecondary {
                    connection: conn,
                    target,
                    hand,
                    location,
                }
            } else {
                PlayerInput::InteractEntity {
                    connection: conn,
                    target,
                    hand,
                    location,
                }
            });
        }
        ServerboundSetCarriedItem::ID => {
            let p = read::<ServerboundSetCarriedItem>(body)?;
            let slot = u8::try_from(p.slot)
                .ok()
                .filter(|slot| *slot <= 8)
                .ok_or_else(|| format!("SetCarriedItem slot out of range: {}", p.slot))?;
            out.push(PlayerInput::SelectSlot {
                connection: conn,
                slot,
            });
        }
        // `includeData` only copies creative block-entity data or prints an
        // avatar profile; the native pick ignores it.
        ServerboundPickItemFromBlock::ID => {
            let p = read::<ServerboundPickItemFromBlock>(body)?;
            out.push(PlayerInput::PickItemFromBlock {
                connection: conn,
                position: block_pos(p.position),
            });
        }
        ServerboundPickItemFromEntity::ID => {
            let p = read::<ServerboundPickItemFromEntity>(body)?;
            out.push(PlayerInput::PickItemFromEntity {
                connection: conn,
                target: EntityId(p.entity_id),
            });
        }
        ServerboundChat::ID => {
            let p = read::<ServerboundChat>(body)?;
            validate_chat(&p.message)?;
            out.push(PlayerInput::Chat {
                connection: conn,
                message: p.message,
            });
        }
        ServerboundChatCommand::ID | ServerboundChatCommandSigned::ID => {
            let text = if id == ServerboundChatCommand::ID {
                read::<ServerboundChatCommand>(body)?.command
            } else {
                read::<ServerboundChatCommandSigned>(body)?.command
            };
            // Verbatim, slash stripped; the core parser owns the rest. Both
            // wire forms use the command bound, never chat's shorter limit.
            let command = text.strip_prefix('/').unwrap_or(&text).to_owned();
            validate_text_characters(&command)?;
            out.push(PlayerInput::Command {
                connection: conn,
                command,
                // The edge overwrites this with the authenticated session level.
                permission_level: 0,
            });
        }
        ServerboundCustomClickAction::ID => {
            let p = read::<ServerboundCustomClickAction>(body)?;
            out.push(PlayerInput::DialogClick {
                connection: conn,
                id: p.id.as_str().to_owned(),
                payload: p.payload,
            });
        }
        ServerboundSelectTrade::ID => {
            let p = read::<ServerboundSelectTrade>(body)?;
            out.push(PlayerInput::SelectTrade {
                connection: conn,
                offer: p.offer_index,
            });
        }
        ServerboundSetBeacon::ID => {
            let p = read::<ServerboundSetBeacon>(body)?;
            out.push(PlayerInput::SetBeacon {
                connection: conn,
                primary: p.primary,
                secondary: p.secondary,
            });
        }
        ServerboundContainerButtonClick::ID => {
            let p = read::<ServerboundContainerButtonClick>(body)?;
            out.push(PlayerInput::ContainerButton {
                connection: conn,
                window: window("ContainerButton", p.container_id)?,
                button: p.button_id,
            });
        }
        ServerboundContainerSlotStateChanged::ID => {
            let p = read::<ServerboundContainerSlotStateChanged>(body)?;
            out.push(PlayerInput::ContainerSlotState {
                connection: conn,
                window: window("ContainerSlotState", p.container_id)?,
                slot: p.slot_id,
                enabled: p.new_state,
            });
        }
        ServerboundContainerClick::ID => {
            let p = read::<ServerboundContainerClick>(body)?;
            out.push(PlayerInput::ContainerClick {
                connection: conn,
                window: window("ContainerClick", p.container_id)?,
                state_id: p.state_id,
                slot: p.slot_num,
                button: p.button_num,
                mode: click_mode_of(p.container_input),
            });
        }
        ServerboundContainerClose::ID => {
            let p = read::<ServerboundContainerClose>(body)?;
            out.push(PlayerInput::ContainerClose {
                connection: conn,
                window: window("ContainerClose", p.container_id)?,
            });
        }
        ServerboundPlaceRecipe::ID => {
            let p = read::<ServerboundPlaceRecipe>(body)?;
            let recipe = u32::try_from(p.recipe_display_id)
                .map_err(|_| format!("PlaceRecipe recipe out of range: {}", p.recipe_display_id))?;
            out.push(PlayerInput::PlaceRecipe {
                connection: conn,
                window: window("PlaceRecipe", p.container_id)?,
                recipe,
                make_all: p.use_max_items,
            });
        }
        ServerboundClientCommand::ID => match read::<ServerboundClientCommand>(body)?.action {
            ClientCommandAction::PerformRespawn => {
                out.push(PlayerInput::Respawn { connection: conn });
            }
            ClientCommandAction::RequestStats => {
                out.push(PlayerInput::RequestStats { connection: conn });
            }
            ClientCommandAction::RequestGameruleValues => {}
        },
        mc_protocol::packets::play::ServerboundEditBook::ID => {
            let p = read::<mc_protocol::packets::play::ServerboundEditBook>(body)?;
            out.push(PlayerInput::EditBook {
                connection: conn,
                slot: p.slot,
                pages: p.pages,
                title: p.title,
            });
        }
        ServerboundSignUpdate::ID => {
            let p = read::<ServerboundSignUpdate>(body)?;
            out.push(PlayerInput::SignUpdate {
                connection: conn,
                position: block_pos(p.position),
                front: p.is_front_text,
                lines: p.lines,
            });
        }
        ServerboundPlayerCommand::ID => match read::<ServerboundPlayerCommand>(body)?.action {
            PlayerCommandAction::StopSleeping => {
                out.push(PlayerInput::LeaveBed { connection: conn });
            }
            PlayerCommandAction::OpenInventory => {
                out.push(PlayerInput::OpenMountInventory { connection: conn });
            }
            action @ (PlayerCommandAction::StartSprinting | PlayerCommandAction::StopSprinting) => {
                out.push(PlayerInput::Sprint {
                    connection: conn,
                    sprinting: action == PlayerCommandAction::StartSprinting,
                });
            }
            PlayerCommandAction::StartFallFlying => {
                out.push(PlayerInput::StartFallFlying { connection: conn });
            }
            _ => {}
        },
        ServerboundPlayerInput::ID => {
            let input = read::<ServerboundPlayerInput>(body)?.input;
            out.extend([
                PlayerInput::VehicleInput {
                    connection: conn,
                    flags: input.flags(),
                },
                PlayerInput::Sneak {
                    connection: conn,
                    sneaking: input.shift,
                },
                PlayerInput::Sprint {
                    connection: conn,
                    sprinting: input.sprint,
                },
            ]);
        }
        ServerboundRenameItem::ID => {
            let p = read::<ServerboundRenameItem>(body)?;
            out.push(PlayerInput::RenameItem {
                connection: conn,
                name: p.name,
            });
        }
        ServerboundMoveVehicle::ID => {
            let p = read::<ServerboundMoveVehicle>(body)?;
            out.push(PlayerInput::MoveVehicle {
                connection: conn,
                position: [p.x, p.y, p.z],
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.on_ground,
            });
        }
        ServerboundSetCreativeModeSlot::ID => {
            let p = read::<ServerboundSetCreativeModeSlot>(body)?;
            out.push(PlayerInput::CreativeSlot {
                connection: conn,
                slot: p.slot,
                stack: p.item_stack,
            });
        }
        // Validated, no gameplay intent.
        // Unsigned system-chat mode: bound the session envelope without ever
        // advertising an unverified key to another client as a trusted session.
        ServerboundChatSessionUpdate::ID => validate::<ServerboundChatSessionUpdate>(body)?,
        ServerboundPunch::ID => validate::<ServerboundPunch>(body)?,
        ServerboundKeepAlive::ID => validate::<ServerboundKeepAlive>(body)?,
        ServerboundClientTickEnd::ID => validate::<ServerboundClientTickEnd>(body)?,
        ServerboundPlayerLoaded::ID => validate::<ServerboundPlayerLoaded>(body)?,
        ServerboundChunkBatchReceived::ID => validate::<ServerboundChunkBatchReceived>(body)?,
        ServerboundClientInformation::ID => validate::<ServerboundClientInformation>(body)?,
        ServerboundResourcePack::ID => validate::<ServerboundResourcePack>(body)?,
        ServerboundCustomPayload::ID => validate::<ServerboundCustomPayload>(body)?,
        ServerboundChatAck::ID => validate::<ServerboundChatAck>(body)?,
        ServerboundCommandSuggestion::ID => validate::<ServerboundCommandSuggestion>(body)?,
        _ => {}
    }
    Ok(())
}

/// Test convenience: one frame's inputs as a fresh `Vec`.
#[cfg(test)]
pub(crate) fn decode_play_frame_for(
    conn: ConnectionId,
    id: i32,
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<PlayerInput>, String> {
    let mut out = Vec::new();
    decode_play_frame(conn, id, body, state, &mut out)?;
    Ok(out)
}

fn validate<P: Packet>(body: &[u8]) -> Result<(), String> {
    read::<P>(body).map(|_| ())
}

/// Decode a whole packet body; trailing bytes are malformed.
fn read<P: Packet>(body: &[u8]) -> Result<P, String> {
    let mut buf = body;
    let packet = P::decode(&mut buf).map_err(|e| format!("decode failed: {e:?}"))?;
    if !buf.is_empty() {
        return Err(format!(
            "trailing {} bytes in packet {:#04x}",
            buf.len(),
            P::ID
        ));
    }
    Ok(packet)
}

fn window(packet: &str, container_id: i32) -> Result<u8, String> {
    u8::try_from(container_id).map_err(|_| format!("{packet} window out of range: {container_id}"))
}

fn block_pos(packed: i64) -> BlockPos {
    let (x, y, z) = unpack_block_pos(packed);
    BlockPos { x, y, z }
}

fn validate_chat(message: &str) -> Result<(), String> {
    let chars = message.chars().count();
    if chars > 256 {
        return Err(format!("chat too long: {chars} chars"));
    }
    validate_text_characters(message)
}

pub(crate) const ILLEGAL_CHAT_CHARACTERS: &str = "illegal characters in chat";

fn validate_text_characters(message: &str) -> Result<(), String> {
    if message
        .chars()
        .any(|c| c == '§' || c < ' ' || c == '\u{7f}')
    {
        return Err(ILLEGAL_CHAT_CHARACTERS.to_owned());
    }
    Ok(())
}

fn hand_of(hand: InteractionHand) -> Hand {
    match hand {
        InteractionHand::MainHand => Hand::Main,
        InteractionHand::OffHand => Hand::Off,
    }
}

fn click_mode_of(input: ContainerInput) -> ClickMode {
    match input {
        ContainerInput::Pickup => ClickMode::Pickup,
        ContainerInput::QuickMove => ClickMode::QuickMove,
        ContainerInput::Swap => ClickMode::Swap,
        ContainerInput::Clone => ClickMode::Clone,
        ContainerInput::Throw => ClickMode::Throw,
        ContainerInput::QuickCraft => ClickMode::QuickCraft,
        ContainerInput::PickupAll => ClickMode::PickupAll,
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod decode_tests;

#[cfg(test)]
#[path = "decode_item_use_tests.rs"]
mod decode_item_use_tests;

#[cfg(test)]
#[path = "decode_container_click_tests.rs"]
mod decode_container_click_tests;
