//! Wire-capture probe.
//!
//! Connects to a Minecraft Java Edition server, walks the protocol
//! through Handshake → Login → Configuration → Play (offline-mode),
//! and dumps every received clientbound frame. Used to validate
//! Solaris' guessed packet IDs against a real vanilla server.
//!
//! Usage:
//!   wire-probe --addr 127.0.0.1:25566 [--name probe] [--play-seconds 10]
//!   wire-probe --addr 127.0.0.1:25565 --server-kind vanilla \
//!     --scenario player-water-entry --out .analysis/physics-oracles/player-water-entry.vanilla.capture

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bytes::BytesMut;
use clap::Parser;
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::codec::ReadMc;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, ServerboundKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ClientboundSetEntityData, ClientboundSetHealth, ClientboundTakeItemEntity,
    ConfirmTeleportation, Direction, GameMode, InteractionHand, LoginPlay, MoveEntityPos,
    MoveEntityPosRot, MovePlayerFlags, PlayerActionKind, PlayerCommandAction, PlayerInput,
    RemoveEntities, ServerboundChangeGameMode, ServerboundChatCommand, ServerboundClientTickEnd,
    ServerboundKeepAlive, ServerboundMovePlayerPosRot, ServerboundPlayerAction,
    ServerboundPlayerCommand, ServerboundPlayerInput, ServerboundPlayerLoaded,
    ServerboundSetCarriedItem, ServerboundSwing, SetEntityMotion, SynchronizePlayerPosition,
    pack_block_pos, unpack_block_pos,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "wire-probe",
    about = "Dump clientbound frames from an MC server"
)]
struct Cli {
    /// Address of the target server, e.g. 127.0.0.1:25566.
    #[arg(long)]
    addr: String,
    /// Player name to send in LoginStart.
    #[arg(long, default_value = "probe")]
    name: String,
    /// How many seconds to keep reading clientbound frames after Play
    /// state is reached. Default 5s — long enough to see the spawn
    /// burst, short enough not to wait for the first keepalive.
    #[arg(long, default_value_t = 5)]
    play_seconds: u64,
    /// Stable scenario name written into captures.
    #[arg(long, default_value = "spawn-burst")]
    scenario: String,
    /// Server kind written into captures: vanilla, solaris, or local.
    #[arg(long, default_value = "local")]
    server_kind: String,
    /// Optional capture output path. Use a gitignored path such as
    /// .analysis/physics-oracles/<scenario>.<server-kind>.capture.
    #[arg(long)]
    out: Option<PathBuf>,
}

struct CaptureWriter {
    file: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeScenario {
    Passive,
    PlayerShallowWaterEntry,
    PlayerDeepWaterSwim,
    PlayerWaterSurfaceExit,
    ItemWaterDropWindow,
    EntityLandPassiveMotion,
    EntityHostileMotion,
    EntityAquaticMotion,
    CollisionWallStepFall,
}

impl ProbeScenario {
    fn from_name(name: &str) -> Self {
        match name {
            "player-shallow-water-entry" => Self::PlayerShallowWaterEntry,
            "player-deep-water-swim" => Self::PlayerDeepWaterSwim,
            "player-water-surface-exit" => Self::PlayerWaterSurfaceExit,
            "item-water-drop-window" => Self::ItemWaterDropWindow,
            "entity-land-passive-motion" => Self::EntityLandPassiveMotion,
            "entity-hostile-motion" => Self::EntityHostileMotion,
            "entity-aquatic-motion" => Self::EntityAquaticMotion,
            "collision-wall-step-fall" => Self::CollisionWallStepFall,
            _ => Self::Passive,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PlayStart {
    entity_id: i32,
    sync: SynchronizePlayerPosition,
}

fn log_frame(
    capture: &mut CaptureWriter,
    label: &str,
    frame: &mc_protocol::RawFrame,
) -> Result<()> {
    capture.line(format!(
        "[{label}] id=0x{:02X} body_len={} body={}",
        frame.id,
        frame.body.len(),
        hexdump_short(&frame.body, 48)
    ))?;
    if frame.id == AddEntity::ID {
        match AddEntity::decode(&mut frame.body.clone()) {
            Ok(add) => capture.line(format!(
                "    ↳ AddEntity: entity_id={} type_id={} pos=({:.6}, {:.6}, {:.6}) movement=({:.6}, {:.6}, {:.6}) data={}",
                add.entity_id,
                add.entity_type_id,
                add.x,
                add.y,
                add.z,
                add.movement.x,
                add.movement.y,
                add.movement.z,
                add.data
            ))?,
            Err(err) => capture.line(format!("    ↳ AddEntity decode_error={err}"))?,
        }
    } else if frame.id == BlockUpdate::ID {
        match BlockUpdate::decode(&mut frame.body.clone()) {
            Ok(update) => {
                let (x, y, z) = unpack_block_pos(update.position);
                capture.line(format!(
                    "    ↳ BlockUpdate: pos=({x}, {y}, {z}) state_id={}",
                    update.state_id
                ))?
            }
            Err(err) => capture.line(format!("    ↳ BlockUpdate decode_error={err}"))?,
        }
    } else if frame.id == BlockChangedAck::ID {
        match BlockChangedAck::decode(&mut frame.body.clone()) {
            Ok(ack) => capture.line(format!("    ↳ BlockChangedAck: sequence={}", ack.sequence))?,
            Err(err) => capture.line(format!("    ↳ BlockChangedAck decode_error={err}"))?,
        }
    } else if frame.id == SetEntityMotion::ID {
        match SetEntityMotion::decode(&mut frame.body.clone()) {
            Ok(motion) => capture.line(format!(
                "    ↳ SetEntityMotion: entity_id={} movement=({:.6}, {:.6}, {:.6})",
                motion.entity_id, motion.movement.x, motion.movement.y, motion.movement.z
            ))?,
            Err(err) => capture.line(format!("    ↳ SetEntityMotion decode_error={err}"))?,
        }
    } else if frame.id == MoveEntityPos::ID {
        match MoveEntityPos::decode(&mut frame.body.clone()) {
            Ok(movement) => capture.line(format!(
                "    ↳ MoveEntityPos: entity_id={} delta=({:.6}, {:.6}, {:.6}) on_ground={}",
                movement.entity_id,
                f64::from(movement.delta_x) / 4096.0,
                f64::from(movement.delta_y) / 4096.0,
                f64::from(movement.delta_z) / 4096.0,
                movement.on_ground
            ))?,
            Err(err) => capture.line(format!("    ↳ MoveEntityPos decode_error={err}"))?,
        }
    } else if frame.id == MoveEntityPosRot::ID {
        match MoveEntityPosRot::decode(&mut frame.body.clone()) {
            Ok(movement) => capture.line(format!(
                "    ↳ MoveEntityPosRot: entity_id={} delta=({:.6}, {:.6}, {:.6}) yaw={} pitch={} on_ground={}",
                movement.entity_id,
                f64::from(movement.delta_x) / 4096.0,
                f64::from(movement.delta_y) / 4096.0,
                f64::from(movement.delta_z) / 4096.0,
                movement.yaw,
                movement.pitch,
                movement.on_ground
            ))?,
            Err(err) => capture.line(format!("    ↳ MoveEntityPosRot decode_error={err}"))?,
        }
    } else if frame.id == ClientboundSetEntityData::ID {
        match ClientboundSetEntityData::decode(&mut frame.body.clone()) {
            Ok(data) => capture.line(format!(
                "    ↳ SetEntityData: entity_id={} values={:?}",
                data.entity_id, data.values
            ))?,
            Err(err) => capture.line(format!("    ↳ SetEntityData decode_error={err}"))?,
        }
    } else if frame.id == ClientboundContainerSetSlot::ID {
        match ClientboundContainerSetSlot::decode(&mut frame.body.clone()) {
            Ok(slot) => capture.line(format!(
                "    ↳ ContainerSetSlot: container_id={} state_id={} slot={} stack={:?}",
                slot.container_id, slot.state_id, slot.slot, slot.item_stack
            ))?,
            Err(err) => capture.line(format!("    ↳ ContainerSetSlot decode_error={err}"))?,
        }
    } else if frame.id == ClientboundSetHealth::ID {
        match ClientboundSetHealth::decode(&mut frame.body.clone()) {
            Ok(health) => capture.line(format!(
                "    ↳ SetHealth: health={:.3} food={} saturation={:.3}",
                health.health, health.food, health.saturation
            ))?,
            Err(err) => capture.line(format!("    ↳ SetHealth decode_error={err}"))?,
        }
    } else if frame.id == ClientboundTakeItemEntity::ID {
        match ClientboundTakeItemEntity::decode(&mut frame.body.clone()) {
            Ok(take) => capture.line(format!(
                "    ↳ TakeItemEntity: item_entity_id={} player_entity_id={} amount={}",
                take.item_entity_id, take.player_entity_id, take.amount
            ))?,
            Err(err) => capture.line(format!("    ↳ TakeItemEntity decode_error={err}"))?,
        }
    } else if frame.id == RemoveEntities::ID {
        match RemoveEntities::decode(&mut frame.body.clone()) {
            Ok(remove) => capture.line(format!(
                "    ↳ RemoveEntities: entity_ids={:?}",
                remove.entity_ids
            ))?,
            Err(err) => capture.line(format!("    ↳ RemoveEntities decode_error={err}"))?,
        }
    } else if frame.id == SynchronizePlayerPosition::ID {
        match SynchronizePlayerPosition::decode(&mut frame.body.clone()) {
            Ok(sync) => capture.line(format!(
                "    ↳ SynchronizePlayerPosition: teleport_id={} pos=({:.6}, {:.6}, {:.6}) delta=({:.6}, {:.6}, {:.6}) yaw={:.3} pitch={:.3} flags={}",
                sync.teleport_id,
                sync.x,
                sync.y,
                sync.z,
                sync.dx,
                sync.dy,
                sync.dz,
                sync.yaw,
                sync.pitch,
                sync.relative_flags
            ))?,
            Err(err) => capture.line(format!(
                "    ↳ SynchronizePlayerPosition decode_error={err}"
            ))?,
        }
    }
    Ok(())
}

impl CaptureWriter {
    fn new(path: Option<PathBuf>) -> Result<Self> {
        let file = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("create capture directory {}", parent.display())
                    })?;
                }
                Some(
                    File::create(&path)
                        .with_context(|| format!("create capture {}", path.display()))?,
                )
            }
            None => None,
        };
        Ok(Self { file })
    }

    fn line(&mut self, line: impl AsRef<str>) -> io::Result<()> {
        let line = line.as_ref();
        println!("{line}");
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
        }
        Ok(())
    }
}

fn hexdump_short(bytes: &[u8], max: usize) -> String {
    let display = &bytes[..bytes.len().min(max)];
    let mut s = String::new();
    for b in display {
        s.push_str(&format!("{b:02x} "));
    }
    if bytes.len() > max {
        s.push('…');
    }
    s.trim_end().to_string()
}

fn fall_step(x: f64, y: f64, z: f64, on_ground: bool) -> ServerboundMovePlayerPosRot {
    ServerboundMovePlayerPosRot {
        x,
        y,
        z,
        yaw: 90.0,
        pitch: 0.0,
        flags: MovePlayerFlags::new(on_ground, false),
    }
}

/// State for tracking the connection.
struct Probe {
    stream: TcpStream,
    rbuf: BytesMut,
    compression: Compression,
}

impl Probe {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            rbuf: BytesMut::with_capacity(8192),
            compression: Compression::Disabled,
        }
    }

    async fn write_packet_logged<P: Packet>(
        &mut self,
        packet: &P,
        state: &str,
        name: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        capture.line(format!(
            "[{state}->SERVER:{name}] id=0x{:02X} body_len={} body={}",
            P::ID,
            body.len(),
            hexdump_short(&body, 48)
        ))?;
        let framed = encode_frame(P::ID, &body, self.compression)?;
        self.stream.write_all(&framed).await?;
        Ok(())
    }

    /// Read a single frame, blocking on the socket as needed.
    async fn read_frame(&mut self) -> Result<mc_protocol::RawFrame> {
        loop {
            if let Some(frame) = try_decode_frame(&mut self.rbuf, self.compression)? {
                return Ok(frame);
            }
            let n = self.stream.read_buf(&mut self.rbuf).await?;
            if n == 0 {
                anyhow::bail!("peer closed");
            }
        }
    }

    /// Read whatever frames arrive in `duration`, dumping each.
    async fn dump_for(
        &mut self,
        label: &str,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<usize> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut count = 0;
        loop {
            let timeout = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                self.read_frame(),
            )
            .await;
            match timeout {
                Ok(Ok(frame)) => {
                    log_frame(capture, label, &frame)?;
                    if label == "PLAY" && frame.id == ClientboundKeepAlive::ID {
                        let mut body = frame.body.clone();
                        let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                        self.write_packet_logged(
                            &ServerboundKeepAlive { id: keepalive.id },
                            "PLAY",
                            "KeepAlive",
                            capture,
                        )
                        .await?;
                    }
                    // If a Set Compression packet is implied (very small
                    // body, id matching login.0x03), enable compression
                    // for subsequent frames. We don't try to be smart
                    // here — we just detect the marker pattern. The
                    // server-state machine is more reliable, but this
                    // probe is a minimum viable wire dumper.
                    count += 1;
                }
                Ok(Err(e)) => {
                    capture.line(format!("[{label}] read ended: {e}"))?;
                    return Ok(count);
                }
                Err(_) => {
                    return Ok(count);
                }
            }
        }
    }

    async fn dump_until_play_start(
        &mut self,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<PlayStart> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut entity_id = None;
        loop {
            let frame = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                self.read_frame(),
            )
            .await
            .context("timed out waiting for Play start frames")??;
            log_frame(capture, "PLAY", &frame)?;
            if frame.id == ClientboundKeepAlive::ID {
                let mut body = frame.body.clone();
                let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                self.write_packet_logged(
                    &ServerboundKeepAlive { id: keepalive.id },
                    "PLAY",
                    "KeepAlive",
                    capture,
                )
                .await?;
                continue;
            }
            if frame.id == LoginPlay::ID {
                let login = LoginPlay::decode(&mut frame.body.clone())?;
                capture.line(format!(
                    "    ↳ LoginPlay: entity_id={} dimension={} game_mode={}",
                    login.entity_id, login.dimension_name, login.game_mode
                ))?;
                entity_id = Some(login.entity_id);
                continue;
            }
            if frame.id == SynchronizePlayerPosition::ID {
                let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
                let entity_id = entity_id.unwrap_or(0);
                if entity_id == 0 {
                    capture.line("    ↳ LoginPlay entity id not observed before Sync; using 0")?;
                }
                return Ok(PlayStart { entity_id, sync });
            }
        }
    }

    async fn start_scripted_play(
        &mut self,
        script_name: &str,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<PlayStart> {
        capture.line(format!("=== SCRIPT {script_name} ==="))?;
        let start = self
            .dump_until_play_start(Duration::from_secs(play_seconds.max(5)), capture)
            .await?;
        self.write_packet_logged(
            &ConfirmTeleportation {
                teleport_id: start.sync.teleport_id,
            },
            "PLAY",
            "ConfirmTeleportation",
            capture,
        )
        .await?;
        self.write_packet_logged(&ServerboundPlayerLoaded, "PLAY", "PlayerLoaded", capture)
            .await?;
        if let Some(player_name) = vanilla_setup_player {
            self.setup_player_water_fixture(player_name, capture)
                .await?;
        }
        Ok(start)
    }

    async fn setup_player_water_fixture(
        &mut self,
        player_name: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        capture.line("=== VANILLA SETUP player-water fixture ===")?;
        let commands = [
            "gamerule doDaylightCycle false".to_string(),
            "gamerule randomTickSpeed 0".to_string(),
            "fill -8 63 -4 12 70 12 air".to_string(),
            "fill -8 63 -4 12 63 12 stone".to_string(),
            "fill -5 64 -2 -1 64 1 water".to_string(),
            "fill -5 64 3 -1 66 6 water".to_string(),
            format!("tp {player_name} -4.8 64 0.5 90 0"),
        ];
        for command in commands {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        Ok(())
    }

    async fn setup_item_drop_fixture(
        &mut self,
        player_name: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        capture.line("=== VANILLA SETUP item-water-drop-window fixture ===")?;
        let commands = [
            "gamerule doDaylightCycle false".to_string(),
            "gamerule randomTickSpeed 0".to_string(),
            "kill @e[type=!player]".to_string(),
            format!("clear {player_name}"),
            format!("give {player_name} minecraft:wooden_shovel"),
            "fill -2 63 -4 2 67 4 air".to_string(),
            "fill -2 63 -4 2 63 4 stone".to_string(),
            "setblock 0 64 1 dirt".to_string(),
            format!("tp {player_name} 0.5 64.0 3.5 180 37"),
            format!("gamemode survival {player_name}"),
        ];
        for command in commands {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        Ok(())
    }

    async fn setup_entity_water_fixture(
        &mut self,
        player_name: &str,
        entity: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        capture.line(format!(
            "=== VANILLA SETUP entity-water fixture entity={entity} ==="
        ))?;
        let commands = [
            "gamerule doDaylightCycle false".to_string(),
            "gamerule randomTickSpeed 0".to_string(),
            "difficulty normal".to_string(),
            "kill @e[type=!player]".to_string(),
            "fill -4 63 -4 4 70 4 air".to_string(),
            "fill -4 63 -4 4 63 4 stone".to_string(),
            "fill -2 64 -2 2 66 2 water".to_string(),
            format!("tp {player_name} 0.5 68.0 5.5 180 35"),
            "kill @e[type=!player]".to_string(),
            format!("summon {entity} 0.5 65.0 0.5"),
        ];
        for command in commands {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        Ok(())
    }

    async fn setup_collision_wall_step_fall_fixture(
        &mut self,
        player_name: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        capture.line("=== VANILLA SETUP collision-wall-step-fall fixture ===")?;
        let commands = [
            "gamerule doDaylightCycle false".to_string(),
            "gamerule randomTickSpeed 0".to_string(),
            "difficulty peaceful".to_string(),
            "fill -8 63 -4 12 75 12 air".to_string(),
            "fill -8 63 -4 12 63 12 stone".to_string(),
            "fill -5 64 -2 -1 64 1 water".to_string(),
            "setblock 2 64 10 stone".to_string(),
            "setblock 2 65 10 stone".to_string(),
            "setblock 4 64 -3 stone".to_string(),
            format!("gamemode survival {player_name}"),
            format!("tp {player_name} 0.5 64.0 0.5 90 0"),
        ];
        for command in commands {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        Ok(())
    }

    async fn send_chat_command_and_drain(
        &mut self,
        command: String,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        self.write_packet_logged(
            &ServerboundChatCommand { command },
            "PLAY",
            "ChatCommand",
            capture,
        )
        .await?;
        self.write_packet_logged(&ServerboundClientTickEnd, "PLAY", "ClientTickEnd", capture)
            .await?;
        self.drain_setup_frames(Duration::from_millis(150), capture)
            .await?;
        Ok(())
    }

    async fn send_client_ticks_for(
        &mut self,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut ticks = 0;
        while tokio::time::Instant::now() < deadline {
            if ticks % 4 == 0 {
                self.write_packet_logged(
                    &ServerboundSwing {
                        hand: InteractionHand::MainHand,
                    },
                    "PLAY",
                    "Swing(MainHand)",
                    capture,
                )
                .await?;
            }
            self.write_packet_logged(&ServerboundClientTickEnd, "PLAY", "ClientTickEnd", capture)
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            ticks += 1;
        }
        Ok(())
    }

    async fn drain_setup_frames(
        &mut self,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            let timeout = tokio::time::timeout(remaining, self.read_frame()).await;
            let frame = match timeout {
                Ok(Ok(frame)) => frame,
                Ok(Err(err)) => {
                    capture.line(format!("[PLAY] setup read ended: {err}"))?;
                    return Ok(());
                }
                Err(_) => return Ok(()),
            };
            log_frame(capture, "PLAY", &frame)?;
            if frame.id == ClientboundKeepAlive::ID {
                let mut body = frame.body.clone();
                let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                self.write_packet_logged(
                    &ServerboundKeepAlive { id: keepalive.id },
                    "PLAY",
                    "KeepAlive",
                    capture,
                )
                .await?;
            } else if frame.id == SynchronizePlayerPosition::ID {
                let mut body = frame.body.clone();
                let sync = SynchronizePlayerPosition::decode(&mut body)?;
                self.write_packet_logged(
                    &ConfirmTeleportation {
                        teleport_id: sync.teleport_id,
                    },
                    "PLAY",
                    "ConfirmTeleportation(setup)",
                    capture,
                )
                .await?;
            }
        }
    }

    async fn run_player_shallow_water_entry(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        let start = self
            .start_scripted_play(
                "player-shallow-water-entry",
                play_seconds,
                capture,
                vanilla_setup_player,
            )
            .await?;

        let movement_flags = MovePlayerFlags::new(true, false);
        let water_steps = [
            ServerboundMovePlayerPosRot {
                x: -4.8,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: movement_flags,
            },
            ServerboundMovePlayerPosRot {
                x: -4.2,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: -15.0,
                flags: MovePlayerFlags::new(false, false),
            },
        ];
        for step in water_steps {
            self.write_packet_logged(&step, "PLAY", "MovePlayerPosRot", capture)
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.write_packet_logged(
            &ServerboundPlayerCommand {
                entity_id: start.entity_id,
                action: PlayerCommandAction::StartSprinting,
                data: 0,
            },
            "PLAY",
            "PlayerCommand(StartSprinting)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundPlayerInput {
                input: PlayerInput {
                    forward: true,
                    jump: true,
                    sprint: true,
                    ..PlayerInput::default()
                },
            },
            "PLAY",
            "PlayerInput(forward+jump+sprint)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: -3.6,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: -15.0,
                flags: MovePlayerFlags::new(false, false),
            },
            "PLAY",
            "MovePlayerPosRot(swim-look)",
            capture,
        )
        .await?;

        let n = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(3)), capture)
            .await?;
        capture.line(format!("script captured {n} post-input Play-state frames"))?;
        Ok(())
    }

    async fn run_player_deep_water_swim(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        let start = self
            .start_scripted_play(
                "player-deep-water-swim",
                play_seconds,
                capture,
                vanilla_setup_player,
            )
            .await?;

        let movement_flags = MovePlayerFlags::new(false, false);
        let water_steps = [
            ServerboundMovePlayerPosRot {
                x: -3.5,
                y: 64.2,
                z: 4.5,
                yaw: 90.0,
                pitch: -30.0,
                flags: movement_flags,
            },
            ServerboundMovePlayerPosRot {
                x: -3.0,
                y: 65.0,
                z: 4.5,
                yaw: 90.0,
                pitch: -30.0,
                flags: movement_flags,
            },
        ];
        for step in water_steps {
            self.write_packet_logged(&step, "PLAY", "MovePlayerPosRot", capture)
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.write_packet_logged(
            &ServerboundPlayerCommand {
                entity_id: start.entity_id,
                action: PlayerCommandAction::StartSprinting,
                data: 0,
            },
            "PLAY",
            "PlayerCommand(StartSprinting)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundPlayerInput {
                input: PlayerInput {
                    forward: true,
                    jump: true,
                    sprint: true,
                    ..PlayerInput::default()
                },
            },
            "PLAY",
            "PlayerInput(forward+jump+sprint)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: -2.5,
                y: 65.6,
                z: 4.5,
                yaw: 90.0,
                pitch: -30.0,
                flags: movement_flags,
            },
            "PLAY",
            "MovePlayerPosRot(swim-up)",
            capture,
        )
        .await?;

        let n = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(3)), capture)
            .await?;
        capture.line(format!("script captured {n} post-input Play-state frames"))?;
        Ok(())
    }

    async fn run_player_water_surface_exit(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        let start = self
            .start_scripted_play(
                "player-water-surface-exit",
                play_seconds,
                capture,
                vanilla_setup_player,
            )
            .await?;

        let movement_flags = MovePlayerFlags::new(false, false);
        let water_steps = [
            ServerboundMovePlayerPosRot {
                x: -2.5,
                y: 65.4,
                z: 4.5,
                yaw: 90.0,
                pitch: -20.0,
                flags: movement_flags,
            },
            ServerboundMovePlayerPosRot {
                x: -1.5,
                y: 66.2,
                z: 4.5,
                yaw: 90.0,
                pitch: -20.0,
                flags: movement_flags,
            },
        ];
        for step in water_steps {
            self.write_packet_logged(&step, "PLAY", "MovePlayerPosRot", capture)
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.write_packet_logged(
            &ServerboundPlayerCommand {
                entity_id: start.entity_id,
                action: PlayerCommandAction::StartSprinting,
                data: 0,
            },
            "PLAY",
            "PlayerCommand(StartSprinting)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundPlayerInput {
                input: PlayerInput {
                    forward: true,
                    jump: true,
                    sprint: true,
                    ..PlayerInput::default()
                },
            },
            "PLAY",
            "PlayerInput(forward+jump+sprint)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: 0.5,
                y: 64.0,
                z: 4.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
            "PLAY",
            "MovePlayerPosRot(exit-to-land)",
            capture,
        )
        .await?;

        let n = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(3)), capture)
            .await?;
        capture.line(format!("script captured {n} post-input Play-state frames"))?;
        Ok(())
    }

    async fn run_item_water_drop_window(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        let start = self
            .start_scripted_play("item-water-drop-window", play_seconds, capture, None)
            .await?;
        let target = if let Some(player_name) = vanilla_setup_player {
            self.setup_item_drop_fixture(player_name, capture).await?;
            (0, 64, 1)
        } else {
            (0, start.sync.y.floor() as i32 - 2, 0)
        };
        let (target_x, target_y, target_z) = target;

        self.write_packet_logged(
            &ServerboundSetCarriedItem { slot: 0 },
            "PLAY",
            "SetCarriedItem(slot=0)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: f64::from(target_x) + 0.5,
                y: f64::from(target_y),
                z: f64::from(target_z) + 2.5,
                yaw: 180.0,
                pitch: 37.0,
                flags: MovePlayerFlags::new(true, false),
            },
            "PLAY",
            "MovePlayerPosRot(drop-target)",
            capture,
        )
        .await?;
        let target_pos = pack_block_pos(target_x, target_y, target_z);
        self.write_packet_logged(
            &ServerboundPlayerAction {
                action: PlayerActionKind::StartDestroyBlock,
                position: target_pos,
                direction: Direction::South,
                sequence: 201,
            },
            "PLAY",
            "PlayerAction(StartDestroyBlock)",
            capture,
        )
        .await?;
        self.write_packet_logged(&ServerboundClientTickEnd, "PLAY", "ClientTickEnd", capture)
            .await?;
        self.write_packet_logged(
            &ServerboundSwing {
                hand: InteractionHand::MainHand,
            },
            "PLAY",
            "Swing(MainHand)",
            capture,
        )
        .await?;
        self.drain_setup_frames(Duration::from_millis(150), capture)
            .await?;
        self.send_client_ticks_for(Duration::from_millis(1500), capture)
            .await?;
        self.write_packet_logged(
            &ServerboundPlayerAction {
                action: PlayerActionKind::StopDestroyBlock,
                position: target_pos,
                direction: Direction::South,
                sequence: 202,
            },
            "PLAY",
            "PlayerAction(StopDestroyBlock)",
            capture,
        )
        .await?;
        self.write_packet_logged(&ServerboundClientTickEnd, "PLAY", "ClientTickEnd", capture)
            .await?;

        let visible_frames = self
            .dump_for("PLAY", Duration::from_millis(400), capture)
            .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: f64::from(target_x) + 0.5,
                y: f64::from(target_y),
                z: f64::from(target_z) + 0.5,
                yaw: 180.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
            "PLAY",
            "MovePlayerPosRot(pickup-window)",
            capture,
        )
        .await?;
        let pickup_frames = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(5)), capture)
            .await?;
        capture.line(format!(
            "script captured {visible_frames} visible-window frames and {pickup_frames} pickup-window frames"
        ))?;
        Ok(())
    }

    async fn run_entity_water_motion(
        &mut self,
        script_name: &str,
        entity: &str,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        self.start_scripted_play(script_name, play_seconds, capture, None)
            .await?;
        if let Some(player_name) = vanilla_setup_player {
            self.setup_entity_water_fixture(player_name, entity, capture)
                .await?;
        }
        let frames = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(8)), capture)
            .await?;
        capture.line(format!(
            "script captured {frames} entity-water Play-state frames"
        ))?;
        Ok(())
    }

    async fn run_collision_wall_step_fall(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
        vanilla_setup_player: Option<&str>,
    ) -> Result<()> {
        let start = self
            .start_scripted_play("collision-wall-step-fall", play_seconds, capture, None)
            .await?;
        if let Some(player_name) = vanilla_setup_player {
            self.setup_collision_wall_step_fall_fixture(player_name, capture)
                .await?;
        }
        self.write_packet_logged(
            &ServerboundChangeGameMode {
                mode: GameMode::Survival,
            },
            "PLAY",
            "ChangeGameMode(Survival)",
            capture,
        )
        .await?;

        let teleport = |name: Option<&str>, x: f64, y: f64, z: f64| {
            name.map(|name| format!("tp {name} {x} {y} {z} 90 0"))
        };
        if let Some(command) = teleport(vanilla_setup_player, 0.5, 64.0, 0.5) {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        self.write_move_and_drain(
            "flat-ground",
            ServerboundMovePlayerPosRot {
                x: 0.6,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
            capture,
        )
        .await?;
        if let Some(command) = teleport(vanilla_setup_player, 1.5, 64.0, 10.5) {
            self.send_chat_command_and_drain(command, capture).await?;
        } else {
            self.write_move_and_drain(
                "wall-start",
                ServerboundMovePlayerPosRot {
                    x: 1.5,
                    y: 64.0,
                    z: 10.5,
                    yaw: 90.0,
                    pitch: 0.0,
                    flags: MovePlayerFlags::new(true, false),
                },
                capture,
            )
            .await?;
        }
        self.write_move_and_drain(
            "wall-collision",
            ServerboundMovePlayerPosRot {
                x: 2.5,
                y: 64.0,
                z: 10.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
            capture,
        )
        .await?;
        if let Some(command) = teleport(vanilla_setup_player, 3.5, 64.0, -2.5) {
            self.send_chat_command_and_drain(command, capture).await?;
        } else {
            self.write_move_and_drain(
                "non-step-start",
                ServerboundMovePlayerPosRot {
                    x: 3.5,
                    y: 64.0,
                    z: -2.5,
                    yaw: 90.0,
                    pitch: 0.0,
                    flags: MovePlayerFlags::new(true, false),
                },
                capture,
            )
            .await?;
        }
        self.write_move_and_drain(
            "full-block-non-step",
            ServerboundMovePlayerPosRot {
                x: 4.5,
                y: 64.0,
                z: -2.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
            capture,
        )
        .await?;
        if let Some(command) = teleport(vanilla_setup_player, 6.5, 70.0, 0.5) {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        for (label, step) in [
            ("fall-y69", fall_step(6.5, 69.0, 0.5, false)),
            ("fall-y68", fall_step(6.5, 68.0, 0.5, false)),
            ("fall-y67", fall_step(6.5, 67.0, 0.5, false)),
            ("fall-y66", fall_step(6.5, 66.0, 0.5, false)),
            ("fall-y65", fall_step(6.5, 65.0, 0.5, false)),
            ("fall-landing", fall_step(6.5, 64.0, 0.5, true)),
        ] {
            self.write_move_and_drain(label, step, capture).await?;
        }
        if let Some(command) = teleport(vanilla_setup_player, -4.5, 70.0, 0.5) {
            self.send_chat_command_and_drain(command, capture).await?;
        }
        for (label, step) in [
            ("water-fall-y69", fall_step(-4.5, 69.0, 0.5, false)),
            ("water-fall-y68", fall_step(-4.5, 68.0, 0.5, false)),
            ("water-fall-y67", fall_step(-4.5, 67.0, 0.5, false)),
            ("water-fall-y66", fall_step(-4.5, 66.0, 0.5, false)),
            ("water-fall-y65", fall_step(-4.5, 65.0, 0.5, false)),
            ("water-entry", fall_step(-4.5, 64.0, 0.5, false)),
        ] {
            self.write_move_and_drain(label, step, capture).await?;
        }

        let n = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(3)), capture)
            .await?;
        capture.line(format!(
            "script captured {n} final collision-wall-step-fall frames for entity_id={}",
            start.entity_id
        ))?;
        Ok(())
    }

    async fn write_move_and_drain(
        &mut self,
        label: &str,
        step: ServerboundMovePlayerPosRot,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        self.write_packet_logged(
            &step,
            "PLAY",
            &format!("MovePlayerPosRot({label})"),
            capture,
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(75)).await;
        let _ = self
            .dump_for("PLAY", Duration::from_millis(125), capture)
            .await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    let cli = Cli::parse();
    let mut capture = CaptureWriter::new(cli.out.clone())?;
    let captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before Unix epoch")?
        .as_millis();
    capture.line("# solaris-wire-capture-v1")?;
    capture.line(format!("scenario={}", cli.scenario))?;
    capture.line(format!("server_kind={}", cli.server_kind))?;
    capture.line(format!("addr={}", cli.addr))?;
    capture.line(format!("player={}", cli.name))?;
    capture.line(format!("protocol_version={PROTOCOL_VERSION}"))?;
    capture.line(format!("play_seconds={}", cli.play_seconds))?;
    capture.line(format!("captured_at_unix_ms={captured_at}"))?;
    capture.line(
        "packet_source=.analysis/protocol-dump.txt or javap net.minecraft.network.protocol.*",
    )?;
    capture.line("")?;
    let stream = TcpStream::connect(&cli.addr)
        .await
        .with_context(|| format!("connect to {}", cli.addr))?;
    stream.set_nodelay(true)?;
    let mut probe = Probe::new(stream);

    let port = cli
        .addr
        .rsplit_once(':')
        .map(|(_, p)| p.parse().unwrap_or(25565))
        .unwrap_or(25565);

    // Handshake → Login
    probe
        .write_packet_logged(
            &Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "127.0.0.1".into(),
                server_port: port,
                next_state: NextState::Login,
            },
            "HANDSHAKE",
            "Handshake(Login)",
            &mut capture,
        )
        .await?;

    probe
        .write_packet_logged(
            &LoginStart {
                name: cli.name.clone(),
                player_uuid: Uuid::nil(),
            },
            "LOGIN",
            "LoginStart",
            &mut capture,
        )
        .await?;

    capture.line("=== LOGIN STATE ===")?;
    capture.line("(reading frames until we see a known terminal packet)")?;

    // Read up to a small fixed budget of login frames, looking for
    // LoginSuccess. SetCompression may arrive first and enable
    // compression for all subsequent frames including LoginSuccess
    // itself.
    loop {
        let frame = probe.read_frame().await?;
        capture.line(format!(
            "[LOGIN] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        ))?;

        // Set Compression has id 0x03 in modern protocol and body is a
        // single VarInt. Detect this heuristically and flip our codec.
        if frame.id == 0x03
            && frame.body.len() <= 5
            && let Ok(threshold) = frame.body.clone().read_varint()
        {
            capture.line(format!(
                "    ↳ looks like SetCompression(threshold={threshold}); enabling compression"
            ))?;
            probe.compression = if threshold < 0 {
                Compression::Disabled
            } else {
                Compression::Threshold(threshold as usize)
            };
            continue;
        }

        // LoginSuccess detection: try to decode and if it has the right
        // shape, ack.
        if frame.id == LoginSuccess::ID {
            let mut body = frame.body.clone();
            if let Ok(success) = LoginSuccess::decode(&mut body) {
                capture.line(format!(
                    "    ↳ LoginSuccess: name={} uuid={} properties={}",
                    success.name,
                    success.uuid,
                    success.properties.len()
                ))?;
                probe
                    .write_packet_logged(
                        &LoginAcknowledged,
                        "LOGIN",
                        "LoginAcknowledged",
                        &mut capture,
                    )
                    .await?;
                break;
            }
        }

        // Disconnect/anything-unexpected: dump and bail.
        if frame.id == 0x00 {
            let preview = String::from_utf8_lossy(&frame.body);
            capture.line(format!("    ↳ possible LoginDisconnect: {preview}"))?;
            return Ok(());
        }
    }

    capture.line("")?;
    capture.line("=== CONFIGURATION STATE ===")?;

    // Wait for ClientboundKnownPacks, echo back, then drain RegistryData
    // / UpdateTags / FinishConfiguration. Send Ack when we see Finish.
    let mut sent_known_packs_response = false;
    let mut got_finish = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !got_finish {
        let frame = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            probe.read_frame(),
        )
        .await
        .context("timed out waiting for Configuration frames")??;

        capture.line(format!(
            "[CONFIG] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        ))?;

        // Heuristic: ClientboundKnownPacks has a non-trivial body
        // (starts with a VarInt count > 0 and then strings). We try
        // a typed decode first.
        if !sent_known_packs_response
            && let Ok(known) = ClientboundKnownPacks::decode(&mut frame.body.clone())
            && !known.packs.is_empty()
            && known.packs[0]
                .namespace
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_')
        {
            capture.line(format!(
                "    ↳ ClientboundKnownPacks (id 0x{:02X}): {:?}",
                frame.id, known.packs
            ))?;
            probe
                .write_packet_logged(
                    &ServerboundKnownPacks {
                        packs: known.packs.clone(),
                    },
                    "CONFIG",
                    "KnownPacks",
                    &mut capture,
                )
                .await?;
            sent_known_packs_response = true;
            continue;
        }

        // FinishConfiguration is empty body. The first empty-body
        // frame we see after KnownPacks is almost certainly it.
        if frame.body.is_empty() && sent_known_packs_response {
            capture.line(format!(
                "    ↳ assumed FinishConfiguration (id 0x{:02X}); sending Ack",
                frame.id
            ))?;
            probe
                .write_packet_logged(
                    &AcknowledgeFinishConfiguration,
                    "CONFIG",
                    "AcknowledgeFinishConfiguration",
                    &mut capture,
                )
                .await?;
            got_finish = true;
        }
    }

    capture.line("")?;
    capture.line(format!(
        "=== PLAY STATE (reading for {}s) ===",
        cli.play_seconds
    ))?;
    let vanilla_setup_player = (cli.server_kind == "vanilla").then_some(cli.name.as_str());
    match ProbeScenario::from_name(&cli.scenario) {
        ProbeScenario::Passive => {
            let n = probe
                .dump_for("PLAY", Duration::from_secs(cli.play_seconds), &mut capture)
                .await?;
            capture.line(format!("captured {n} Play-state frames"))?;
        }
        ProbeScenario::PlayerShallowWaterEntry => {
            probe
                .run_player_shallow_water_entry(
                    cli.play_seconds,
                    &mut capture,
                    vanilla_setup_player,
                )
                .await?;
        }
        ProbeScenario::PlayerDeepWaterSwim => {
            probe
                .run_player_deep_water_swim(cli.play_seconds, &mut capture, vanilla_setup_player)
                .await?;
        }
        ProbeScenario::PlayerWaterSurfaceExit => {
            probe
                .run_player_water_surface_exit(cli.play_seconds, &mut capture, vanilla_setup_player)
                .await?;
        }
        ProbeScenario::ItemWaterDropWindow => {
            probe
                .run_item_water_drop_window(cli.play_seconds, &mut capture, vanilla_setup_player)
                .await?;
        }
        ProbeScenario::EntityLandPassiveMotion => {
            probe
                .run_entity_water_motion(
                    "entity-land-passive-motion",
                    "minecraft:cow",
                    cli.play_seconds,
                    &mut capture,
                    vanilla_setup_player,
                )
                .await?;
        }
        ProbeScenario::EntityHostileMotion => {
            probe
                .run_entity_water_motion(
                    "entity-hostile-motion",
                    "minecraft:zombie",
                    cli.play_seconds,
                    &mut capture,
                    vanilla_setup_player,
                )
                .await?;
        }
        ProbeScenario::EntityAquaticMotion => {
            probe
                .run_entity_water_motion(
                    "entity-aquatic-motion",
                    "minecraft:cod",
                    cli.play_seconds,
                    &mut capture,
                    vanilla_setup_player,
                )
                .await?;
        }
        ProbeScenario::CollisionWallStepFall => {
            probe
                .run_collision_wall_step_fall(cli.play_seconds, &mut capture, vanilla_setup_player)
                .await?;
        }
    }

    Ok(())
}
