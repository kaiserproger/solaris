//! In-process client driver for raw-TCP integration tests.
//!
//! Walks Handshake → Login → Configuration exactly as a vanilla client
//! does, then hands the caller a typed, frame-by-frame view of the Play
//! state. Used by `tests/chunk_stream.rs` to assert M3.e's view-
//! distance chunk burst against a live `mc_net::run` server.
//!
//! Layered intentionally above the binary `wire-probe`'s loose,
//! print-and-go style: every step is a fallible async fn and the
//! caller decides what to assert.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use mc_protocol::codec::ReadMc;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::{
    ClientboundChangeDifficulty, ClientboundPlayerAbilities, ClientboundSetHeldSlot, EntityEvent,
    LoginPlay,
};
use mc_protocol::{PROTOCOL_VERSION, RawFrame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

/// Minimal raw-TCP client. Speaks the wire protocol our server speaks
/// (no encryption, optional compression). The `compression` field flips
/// when we observe a Login-state `SetCompression` packet.
pub struct Client {
    stream: TcpStream,
    rbuf: BytesMut,
    pending: VecDeque<RawFrame>,
    compression: Compression,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameWaitLimits {
    pub max_skipped_frames: Option<usize>,
    pub max_skipped_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkippedFrameStats {
    pub frames: usize,
    pub bytes: usize,
}

impl SkippedFrameStats {
    fn record(&mut self, frame: &RawFrame, limits: FrameWaitLimits) -> Result<()> {
        self.frames += 1;
        self.bytes += frame.body.len();
        if let Some(max) = limits.max_skipped_frames
            && self.frames > max
        {
            bail!(
                "skipped {} frames while waiting for target packet; cap is {max}",
                self.frames
            );
        }
        if let Some(max) = limits.max_skipped_bytes
            && self.bytes > max
        {
            bail!(
                "skipped {} bytes while waiting for target packet; cap is {max}",
                self.bytes
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameWaitOutcome {
    pub frame: RawFrame,
    pub skipped: SkippedFrameStats,
}

impl Client {
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connect to {addr}"))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            rbuf: BytesMut::with_capacity(8192),
            pending: VecDeque::new(),
            compression: Compression::Disabled,
        })
    }

    pub fn compression(&self) -> Compression {
        self.compression
    }

    pub async fn write_packet<P: Packet>(&mut self, packet: &P) -> Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        let framed = encode_frame(P::ID, &body, self.compression)?;
        self.stream.write_all(&framed).await?;
        Ok(())
    }

    /// Read until exactly one frame can be parsed off the buffer.
    pub async fn read_frame(&mut self) -> Result<RawFrame> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        self.read_network_frame().await
    }

    async fn read_network_frame(&mut self) -> Result<RawFrame> {
        loop {
            if let Some(frame) = try_decode_frame(&mut self.rbuf, self.compression)? {
                return Ok(frame);
            }
            let n = self.stream.read_buf(&mut self.rbuf).await?;
            if n == 0 {
                bail!("peer closed");
            }
        }
    }

    pub async fn read_frame_with_timeout(&mut self, dur: Duration) -> Result<RawFrame> {
        match tokio::time::timeout(dur, self.read_frame()).await {
            Ok(r) => r,
            Err(_) => bail!("timed out after {:?} waiting for a frame", dur),
        }
    }

    pub async fn wait_for_frame_id_with_timeout(
        &mut self,
        packet_id: i32,
        dur: Duration,
    ) -> Result<RawFrame> {
        Ok(self
            .wait_for_frame_id_with_timeout_and_limits(packet_id, dur, FrameWaitLimits::default())
            .await?
            .frame)
    }

    pub async fn wait_for_frame_id_with_timeout_and_limits(
        &mut self,
        packet_id: i32,
        dur: Duration,
        limits: FrameWaitLimits,
    ) -> Result<FrameWaitOutcome> {
        if let Some(index) = self.pending.iter().position(|frame| frame.id == packet_id) {
            return Ok(FrameWaitOutcome {
                frame: self
                    .pending
                    .remove(index)
                    .expect("position came from pending frame buffer"),
                skipped: SkippedFrameStats::default(),
            });
        }

        let started = tokio::time::Instant::now();
        let mut skipped = SkippedFrameStats::default();
        loop {
            let Some(remaining) = dur.checked_sub(started.elapsed()) else {
                bail!(
                    "timed out after {:?} waiting for packet id 0x{packet_id:02X}",
                    dur
                );
            };
            if remaining.is_zero() {
                bail!(
                    "timed out after {:?} waiting for packet id 0x{packet_id:02X}",
                    dur
                );
            }
            let frame = match tokio::time::timeout(remaining, self.read_network_frame()).await {
                Ok(result) => result?,
                Err(_) => {
                    bail!(
                        "timed out after {:?} waiting for packet id 0x{packet_id:02X}",
                        dur
                    )
                }
            };
            if frame.id == packet_id {
                return Ok(FrameWaitOutcome { frame, skipped });
            }
            skipped.record(&frame, limits)?;
            self.pending.push_back(frame);
        }
    }

    /// Read the next frame and decode it as `P`. Errors if the frame
    /// id does not match `P::ID` or if there are trailing bytes after
    /// the decode.
    pub async fn read_typed<P: Packet>(&mut self) -> Result<P> {
        let mut frame = self.read_frame().await?;
        if frame.id != P::ID {
            bail!(
                "unexpected packet id: want 0x{:02X}, got 0x{:02X}",
                P::ID,
                frame.id
            );
        }
        let packet = P::decode(&mut frame.body)?;
        Ok(packet)
    }

    /// Send Handshake (NextState::Login) and LoginStart, walk login
    /// frames (auto-enabling compression on `SetCompression`), then
    /// send `LoginAcknowledged` and return the decoded `LoginSuccess`.
    pub async fn drive_login(&mut self, addr: SocketAddr, name: &str) -> Result<LoginSuccess> {
        self.write_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        })
        .await?;
        self.write_packet(&LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        })
        .await?;

        loop {
            let frame = self.read_frame().await?;
            // Heuristic: small body on the SetCompression id ⇒ flip
            // codec and keep reading. Same shape as `wire-probe`.
            if frame.id == SetCompression::ID && frame.body.len() <= 5 {
                let mut body = frame.body.clone();
                if let Ok(threshold) = body.read_varint() {
                    self.compression = if threshold < 0 {
                        Compression::Disabled
                    } else {
                        Compression::Threshold(threshold as usize)
                    };
                    continue;
                }
            }
            if frame.id == LoginSuccess::ID {
                let mut body = frame.body.clone();
                let success = LoginSuccess::decode(&mut body)?;
                self.write_packet(&LoginAcknowledged).await?;
                return Ok(success);
            }
            bail!("unexpected login frame id=0x{:02X}", frame.id);
        }
    }

    /// Walk Configuration: echo `ClientboundKnownPacks` back, drain
    /// `RegistryData` until `FinishConfiguration`, then send the ack.
    pub async fn drive_configuration(&mut self) -> Result<()> {
        let known = loop {
            let frame = self.read_frame().await?;
            if frame.id == ClientboundKnownPacks::ID {
                break ClientboundKnownPacks::decode(&mut frame.body.clone())?;
            }
        };
        self.write_packet(&ServerboundKnownPacks { packs: known.packs })
            .await?;
        loop {
            let frame = self.read_frame().await?;
            if frame.id == RegistryData::ID {
                continue;
            }
            if frame.id == UpdateTags::ID {
                continue;
            }
            if frame.id == FinishConfiguration::ID {
                self.write_packet(&AcknowledgeFinishConfiguration).await?;
                return Ok(());
            }
            bail!(
                "unexpected configuration frame id=0x{:02X} body_len={}",
                frame.id,
                frame.body.len()
            );
        }
    }

    /// Read the fixed Play-entry prelude Solaris emits before the command tree.
    pub async fn read_play_login(&mut self) -> Result<LoginPlay> {
        let login = self.read_typed::<LoginPlay>().await?;
        let _: ClientboundChangeDifficulty = self.read_typed().await?;
        let _: ClientboundPlayerAbilities = self.read_typed().await?;
        let _: ClientboundSetHeldSlot = self.read_typed().await?;
        let _: EntityEvent = self.read_typed().await?;
        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn skipped_frame_stats_count_frames_and_bytes() {
        let mut stats = SkippedFrameStats::default();
        stats
            .record(
                &RawFrame {
                    id: 0x01,
                    body: Bytes::from_static(b"abc"),
                },
                FrameWaitLimits::default(),
            )
            .expect("record skip");
        stats
            .record(
                &RawFrame {
                    id: 0x02,
                    body: Bytes::from_static(b"de"),
                },
                FrameWaitLimits::default(),
            )
            .expect("record skip");

        assert_eq!(
            stats,
            SkippedFrameStats {
                frames: 2,
                bytes: 5
            }
        );
    }

    #[test]
    fn skipped_frame_stats_enforce_caps() {
        let mut stats = SkippedFrameStats::default();
        let err = stats
            .record(
                &RawFrame {
                    id: 0x01,
                    body: Bytes::from_static(b"abc"),
                },
                FrameWaitLimits {
                    max_skipped_frames: Some(0),
                    max_skipped_bytes: Some(2),
                },
            )
            .expect_err("frame cap should reject first skip");

        assert!(err.to_string().contains("skipped 1 frames"));
    }
}
