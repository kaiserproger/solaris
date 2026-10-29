//! Low-level transport for one connection.
//!
//! Wraps the pure-logic framing functions from `mc-protocol` with bounded
//! tokio async I/O, plus the AES-CFB8 stream cipher from [`crate::crypto`].
//!
//! The type is deliberately state-agnostic: the state machine and the
//! per-state modules decide *which* packet type to read.

use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::State;
use mc_protocol::frame::{Compression, RawFrame, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

use crate::crypto::MinecraftCipher;

/// Per-packet read timeout before play.
pub const PRE_PLAY_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Total deadline for handshake, status and login, before configuration.
pub const PRE_PLAY_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum packets accepted during pre-play.
pub const MAX_PRE_PLAY_PACKETS: usize = 96;
/// Maximum decoded body bytes accepted during pre-play.
pub const MAX_PRE_PLAY_BYTES: usize = 512 * 1024;
/// Maximum bytes retained while waiting for one incomplete serverbound frame.
pub const MAX_INBOUND_BUFFER_BYTES: usize = 1024 * 1024;
/// Maximum time a single outbound write may stall.
pub const OUTBOUND_WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Running packet/byte budget for the pre-play phase.
#[derive(Debug, Clone, Copy)]
pub struct PrePlayBudget {
    packets: usize,
    bytes: usize,
    max_packets: usize,
    max_bytes: usize,
}

impl PrePlayBudget {
    #[must_use]
    pub fn new(max_packets: usize, max_bytes: usize) -> Self {
        Self {
            packets: 0,
            bytes: 0,
            max_packets,
            max_bytes,
        }
    }

    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(MAX_PRE_PLAY_PACKETS, MAX_PRE_PLAY_BYTES)
    }

    fn record(&mut self, frame: &RawFrame) -> anyhow::Result<()> {
        self.packets = self.packets.saturating_add(1);
        if self.packets > self.max_packets {
            anyhow::bail!(
                "pre-play packet budget exceeded: {} > {}",
                self.packets,
                self.max_packets
            );
        }
        self.bytes = self
            .bytes
            .saturating_add(frame.body.len().saturating_add(5));
        if self.bytes > self.max_bytes {
            anyhow::bail!(
                "pre-play byte budget exceeded: {} > {}",
                self.bytes,
                self.max_bytes
            );
        }
        Ok(())
    }
}

struct EncryptedReadHalf {
    read: tokio::net::tcp::OwnedReadHalf,
    cipher: Option<MinecraftCipher>,
}

impl EncryptedReadHalf {
    fn decrypt_in_place(&mut self, bytes: &mut [u8]) {
        if let Some(cipher) = self.cipher.as_mut() {
            cipher.decrypt_in_place(bytes);
        }
    }
}

impl AsyncRead for EncryptedReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.read).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                self.decrypt_in_place(&mut buf.filled_mut()[filled_before..]);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

struct EncryptedWriteHalf {
    write: tokio::net::tcp::OwnedWriteHalf,
    cipher: Option<MinecraftCipher>,
    pending: Vec<u8>,
    pending_plaintext: Vec<u8>,
    pending_offset: usize,
}

impl EncryptedWriteHalf {
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        while self.pending_offset < self.pending.len() {
            let result =
                Pin::new(&mut self.write).poll_write(cx, &self.pending[self.pending_offset..]);
            match result {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "encrypted transport wrote zero bytes",
                    )));
                }
                Poll::Ready(Ok(written)) => self.pending_offset += written,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.pending.clear();
        self.pending_plaintext.clear();
        self.pending_offset = 0;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for EncryptedWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.cipher.is_none() {
            return Pin::new(&mut self.write).poll_write(cx, buf);
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if self.pending.is_empty() {
            self.pending_plaintext.extend_from_slice(buf);
            self.pending.extend_from_slice(buf);
            let Self {
                cipher, pending, ..
            } = &mut *self;
            cipher
                .as_mut()
                .expect("encrypted writer has a cipher")
                .encrypt_in_place(pending);
        } else if self.pending_plaintext.as_slice() != buf {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "encrypted write retried with different plaintext after cancellation",
            )));
        }

        let accepted = self.pending_plaintext.len();
        match self.poll_pending(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(accepted)),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.poll_pending(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.write).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.poll_pending(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.write).poll_shutdown(cx),
            other => other,
        }
    }
}

/// One framed, optionally compressed and encrypted connection.
///
/// Owns the split socket halves, the shared read buffer, the compression
/// state, and the optional cipher pair. Caller drives the state machine
/// via [`Connection::read_frame`] / [`Connection::send_packet`].
pub struct Connection {
    reader: EncryptedReadHalf,
    writer: EncryptedWriteHalf,
    buf: BytesMut,
    compression: Compression,
    budget: PrePlayBudget,
    /// Encode item stacks for a client that cannot resolve plugin item models.
    vanilla_item_models: bool,
    /// 26.1.2 translator when the handshake declared protocol 775.
    legacy: Option<crate::legacy::Session>,
}

impl Connection {
    #[must_use]
    pub fn new(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            reader: EncryptedReadHalf { read, cipher: None },
            writer: EncryptedWriteHalf {
                write,
                cipher: None,
                pending: Vec::new(),
                pending_plaintext: Vec::new(),
                pending_offset: 0,
            },
            buf: BytesMut::with_capacity(4096),
            compression: Compression::Disabled,
            budget: PrePlayBudget::with_defaults(),
            vanilla_item_models: false,
            legacy: None,
        }
    }

    /// Translate every later packet for a 26.1.2 (protocol 775) client.
    pub fn set_legacy(&mut self, session: crate::legacy::Session) {
        self.legacy = Some(session);
    }

    /// The 26.1.2 profile when this is a legacy session.
    #[must_use]
    pub fn legacy(&self) -> Option<&std::sync::Arc<crate::legacy::Profile>> {
        self.legacy.as_ref().map(crate::legacy::Session::profile)
    }

    /// Encode later item stacks for a client that cannot resolve plugin item
    /// models (see `vanilla_items`).
    pub fn show_vanilla_item_models(&mut self) {
        self.vanilla_item_models = true;
    }

    /// Reads exactly one decrypted, budget-checked frame, applying the
    /// per-packet read timeout.
    pub async fn read_frame(&mut self) -> anyhow::Result<RawFrame> {
        self.read_frame_in(State::Handshake).await
    }

    /// Reads one frame attributed to `state` for timeout diagnostics.
    /// Pre-play only: the 10 s deadline and the packet/byte budget guard
    /// handshake/login/configuration. Play connections must use
    /// [`Connection::read_frame_play`] (no read deadline; client silence is
    /// governed by keepalive, not by a read timeout).
    pub async fn read_frame_in(&mut self, state: State) -> anyhow::Result<RawFrame> {
        loop {
            let frame = tokio::time::timeout(
                PRE_PLAY_READ_TIMEOUT,
                read_frame_inner(&mut self.reader, &mut self.buf, self.compression),
            )
            .await
            .map_err(|_| anyhow::anyhow!("read timed out in {state:?}"))??;
            self.budget.record(&frame)?;
            match &mut self.legacy {
                None => return Ok(frame),
                Some(legacy) => {
                    if let Some(frame) = legacy.serverbound(frame)? {
                        return Ok(frame);
                    }
                }
            }
        }
    }

    /// Reads one Play frame with NO read deadline and NO pre-play budget.
    /// A quiet client is normal in Play (keepalive 15 s / timeout 30 s
    /// governs liveness); dropping for read silence breaks survival joins
    /// where the client thinks before its next packet (~13 s observed).
    pub async fn read_frame_play(&mut self) -> anyhow::Result<RawFrame> {
        read_translated(
            &mut self.reader,
            &mut self.buf,
            self.compression,
            self.legacy.as_mut(),
        )
        .await
    }

    /// Reads and decodes one typed packet, rejecting trailing bytes.
    pub async fn read_packet<P: Packet>(&mut self, state: State) -> anyhow::Result<P> {
        let mut frame = self.read_frame_in(state).await?;
        if frame.id != P::ID {
            anyhow::bail!(
                "unexpected packet id in {state:?}: expected {:#04x}, got {:#04x}",
                P::ID,
                frame.id
            );
        }
        let packet = P::decode(&mut frame.body)?;
        let trailing = frame.body.remaining();
        if trailing != 0 {
            anyhow::bail!(
                "trailing bytes in {state:?} packet {:#04x}: {trailing}",
                frame.id
            );
        }
        Ok(packet)
    }

    /// Encodes, frames and writes one packet with the write-stall fence.
    pub async fn send_packet<P: Packet>(&mut self, p: &P) -> anyhow::Result<()> {
        let mut body = BytesMut::new();
        mc_protocol::packets::play::with_vanilla_item_models(self.vanilla_item_models, || {
            p.encode(&mut body)
        })?;
        let framed = match &mut self.legacy {
            None => encode_frame(P::ID, &body, self.compression)?,
            Some(legacy) => match legacy.clientbound(P::ID, &body)? {
                Some((id, body)) => encode_frame(id, &body, self.compression)?,
                None => return Ok(()),
            },
        };
        write_with_stall_timeout(self.writer.write_all(&framed), OUTBOUND_WRITE_STALL_TIMEOUT)
            .await?;
        Ok(())
    }

    pub fn set_compression(&mut self, c: Compression) {
        self.compression = c;
    }

    /// Installs AES-128-CFB8 in both directions (key == IV == secret) and
    /// decrypts any already-buffered bytes through the inbound stream.
    ///
    /// Panics on a second call: enabling twice would desynchronise the
    /// stream ciphers, which is always a server logic bug (the login flow
    /// enables exactly once).
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        assert!(
            self.reader.cipher.is_none() && self.writer.cipher.is_none(),
            "connection encryption enabled twice"
        );
        assert!(
            self.writer.pending.is_empty(),
            "encryption changed during a pending write"
        );
        let mut cipher = MinecraftCipher::new(secret);
        cipher.decrypt_in_place(&mut self.buf);
        self.reader.cipher = Some(MinecraftCipher::new(secret));
        self.writer.cipher = Some(MinecraftCipher::new(secret));
    }

    #[must_use]
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Split for Play so reading and writing progress independently: a
    /// slow or stalled socket write never delays reading the client's
    /// inputs and acks, and reading never delays writes.
    #[must_use]
    pub fn into_play_halves(self) -> (PlayReader, PlayWriter) {
        let reader_legacy = self.legacy.as_ref().map(crate::legacy::Session::split);
        (
            PlayReader {
                reader: self.reader,
                buf: self.buf,
                compression: self.compression,
                legacy: reader_legacy,
            },
            PlayWriter {
                writer: self.writer,
                compression: self.compression,
                legacy: self.legacy,
            },
        )
    }
}

/// Read half of a Play connection (see [`Connection::into_play_halves`]).
pub struct PlayReader {
    reader: EncryptedReadHalf,
    buf: BytesMut,
    compression: Compression,
    legacy: Option<crate::legacy::Session>,
}

impl PlayReader {
    /// Reads one Play frame with no read deadline (as
    /// [`Connection::read_frame_play`]).
    pub async fn read_frame(&mut self) -> anyhow::Result<RawFrame> {
        read_translated(
            &mut self.reader,
            &mut self.buf,
            self.compression,
            self.legacy.as_mut(),
        )
        .await
    }
}

/// Write half of a Play connection (see [`Connection::into_play_halves`]).
pub struct PlayWriter {
    writer: EncryptedWriteHalf,
    compression: Compression,
    legacy: Option<crate::legacy::Session>,
}

impl PlayWriter {
    /// As [`Connection::send_packet`].
    pub async fn send_packet<P: Packet>(&mut self, p: &P) -> anyhow::Result<()> {
        let mut body = BytesMut::new();
        p.encode(&mut body)?;
        let framed = encode_frame(P::ID, &body, self.compression)?;
        self.write_bytes(&framed).await
    }

    /// Writes pre-framed bytes (whole frames, one or more) with the same stall
    /// fence as [`send_packet`](Self::send_packet): the routed packets are
    /// already encoded. A legacy session re-frames them in its own protocol.
    pub async fn write_bytes(&mut self, framed: &[u8]) -> anyhow::Result<()> {
        let translated;
        let framed = match &mut self.legacy {
            None => framed,
            Some(legacy) => {
                translated = legacy.clientbound_frames(framed, self.compression)?;
                &translated[..]
            }
        };
        write_with_stall_timeout(self.writer.write_all(framed), OUTBOUND_WRITE_STALL_TIMEOUT)
            .await?;
        Ok(())
    }
}

/// Reads Play frames until one survives legacy translation (all do for a
/// canonical session).
async fn read_translated<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    mut legacy: Option<&mut crate::legacy::Session>,
) -> anyhow::Result<RawFrame>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        let frame = read_frame_inner(reader, buf, compression).await?;
        match legacy.as_deref_mut() {
            None => return Ok(frame),
            Some(legacy) => {
                if let Some(frame) = legacy.serverbound(frame)? {
                    return Ok(frame);
                }
            }
        }
    }
}

async fn read_frame_inner<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
) -> anyhow::Result<RawFrame>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        if let Some(frame) = try_decode_frame(buf, compression)? {
            return Ok(frame);
        }
        let remaining = MAX_INBOUND_BUFFER_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            anyhow::bail!("inbound buffer limit exceeded ({MAX_INBOUND_BUFFER_BYTES} bytes)");
        }
        let read = reader.take(remaining as u64).read_buf(buf).await?;
        if read == 0 {
            anyhow::bail!("connection closed (EOF)");
        }
    }
}

async fn write_with_stall_timeout<F>(write: F, timeout: Duration) -> anyhow::Result<()>
where
    F: Future<Output = std::io::Result<()>>,
{
    tokio::pin!(write);

    let initial_result = poll_fn(|cx| {
        Poll::Ready(match write.as_mut().poll(cx) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        })
    })
    .await;
    if let Some(result) = initial_result {
        return Ok(result?);
    }

    match tokio::time::timeout(timeout, write.as_mut()).await {
        Ok(result) => result.map_err(anyhow::Error::from),
        Err(_) => anyhow::bail!("write stalled for {timeout:?}"),
    }
}

/// Encode a plain text component in the network-NBT form used by
/// clientbound disconnect packets.
pub fn text_component_nbt(text: &str) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &mc_nbt::Tag::Compound(vec![(
            "text".to_owned(),
            mc_nbt::Tag::String(text.to_owned()),
        )]),
    )?;
    Ok(out)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
