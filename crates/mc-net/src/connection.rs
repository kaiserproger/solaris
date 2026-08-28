//! Low-level read/write helpers for one connection.
//!
//! These wrap the pure-logic framing functions from `mc-protocol` with
//! tokio async I/O. They are deliberately state-agnostic — the state
//! machine in `server.rs` and the per-state modules decide *which*
//! [`mc_protocol::Packet`] type to read.

use bytes::{Buf, BytesMut};
use mc_protocol::frame::{Compression, FrameDecodePlan, encode_frame, try_decode_frame_plan};
use mc_protocol::packets::Packet;
use mc_protocol::{RawFrame, State};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::Semaphore;

use crate::encryption::MinecraftCipher;
use crate::error::ConnectionError;

pub(crate) const PRE_PLAY_READ_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const PRE_PLAY_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const MAX_PRE_PLAY_PACKETS: usize = 96;
pub(crate) const MAX_PRE_PLAY_BYTES: usize = 512 * 1024;
/// Maximum bytes retained while waiting for one incomplete serverbound frame.
///
/// The protocol encoder supports larger clientbound chunk frames, but every
/// accepted serverbound payload is independently bounded well below 1 MiB
/// (`CustomPayload` defaults to 32 KiB and container collections have explicit
/// count/hash budgets). Keeping a separate inbound ceiling prevents slow peers
/// from turning the generic frame maximum into per-connection memory pressure.
pub(crate) const MAX_INBOUND_BUFFER_BYTES: usize = 1024 * 1024;
#[cfg(not(test))]
pub(crate) const OUTBOUND_WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
pub(crate) const OUTBOUND_WRITE_STALL_TIMEOUT: Duration = Duration::from_millis(10);
const MAX_CONCURRENT_FRAME_DECODES: usize = 8;
static FRAME_DECODE_PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn frame_decode_permits() -> &'static Arc<Semaphore> {
    FRAME_DECODE_PERMITS.get_or_init(|| {
        let permits = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
            .clamp(1, MAX_CONCURRENT_FRAME_DECODES);
        Arc::new(Semaphore::new(permits))
    })
}

async fn run_bounded_frame_decode<T, F>(task: F) -> Result<T, ConnectionError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, mc_protocol::FramingError> + Send + 'static,
{
    let permit = Arc::clone(frame_decode_permits())
        .acquire_owned()
        .await
        .map_err(|error| ConnectionError::FrameDecodeWorker {
            reason: error.to_string(),
        })?;
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
    {
        Ok(result) => result.map_err(ConnectionError::from),
        Err(error) => Err(ConnectionError::FrameDecodeWorker {
            reason: error.to_string(),
        }),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PrePlayBudget {
    packets: usize,
    bytes: usize,
    max_packets: usize,
    max_bytes: usize,
}

impl PrePlayBudget {
    #[must_use]
    pub(crate) fn new(max_packets: usize, max_bytes: usize) -> Self {
        Self {
            packets: 0,
            bytes: 0,
            max_packets,
            max_bytes,
        }
    }

    fn record(&mut self, frame: &RawFrame) -> Result<(), ConnectionError> {
        self.packets = self.packets.saturating_add(1);
        if self.packets > self.max_packets {
            return Err(ConnectionError::PrePlayPacketBudgetExceeded {
                packets: self.packets,
                max: self.max_packets,
            });
        }
        self.bytes = self
            .bytes
            .saturating_add(frame.body.len().saturating_add(5));
        if self.bytes > self.max_bytes {
            return Err(ConnectionError::PrePlayByteBudgetExceeded {
                bytes: self.bytes,
                max: self.max_bytes,
            });
        }
        Ok(())
    }
}

pub(crate) struct ConnectionReader<R> {
    inner: R,
    cipher: Option<MinecraftCipher>,
}

impl<R> ConnectionReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            cipher: None,
        }
    }

    pub(crate) fn enable_encryption(
        &mut self,
        shared_secret: &[u8; 16],
        buffered: &mut BytesMut,
    ) -> Result<(), ConnectionError> {
        if self.cipher.is_some() {
            return Err(ConnectionError::EncryptionState {
                reason: "reader encryption enabled twice",
            });
        }
        let mut cipher = MinecraftCipher::new(shared_secret);
        cipher.decrypt_in_place(buffered);
        self.cipher = Some(cipher);
        Ok(())
    }
}

impl<R> AsyncRead for ConnectionReader<R>
where
    R: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if let Some(cipher) = self.cipher.as_mut() {
                    cipher.decrypt_in_place(&mut buf.filled_mut()[filled_before..]);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

pub(crate) struct ConnectionWriter<W> {
    inner: W,
    cipher: Option<MinecraftCipher>,
    pending: Vec<u8>,
    pending_plaintext: Vec<u8>,
    pending_offset: usize,
}

impl<W> ConnectionWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            cipher: None,
            pending: Vec::new(),
            pending_plaintext: Vec::new(),
            pending_offset: 0,
        }
    }

    pub(crate) fn enable_encryption(
        &mut self,
        shared_secret: &[u8; 16],
    ) -> Result<(), ConnectionError> {
        if self.cipher.is_some() {
            return Err(ConnectionError::EncryptionState {
                reason: "writer encryption enabled twice",
            });
        }
        if !self.pending.is_empty() {
            return Err(ConnectionError::EncryptionState {
                reason: "writer encryption changed during a pending write",
            });
        }
        self.cipher = Some(MinecraftCipher::new(shared_secret));
        Ok(())
    }
}

impl<W> ConnectionWriter<W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        while self.pending_offset < self.pending.len() {
            let result =
                Pin::new(&mut self.inner).poll_write(cx, &self.pending[self.pending_offset..]);
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

impl<W> AsyncWrite for ConnectionWriter<W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.cipher.is_none() {
            return Pin::new(&mut self.inner).poll_write(cx, buf);
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
            Poll::Ready(Ok(())) => Pin::new(&mut self.inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.poll_pending(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.inner).poll_shutdown(cx),
            other => other,
        }
    }
}

async fn write_with_stall_timeout<F>(write: F, timeout: Duration) -> Result<(), ConnectionError>
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
        return result.map_err(ConnectionError::from);
    }

    match tokio::time::timeout(timeout, write.as_mut()).await {
        Ok(result) => result.map_err(ConnectionError::from),
        Err(_) => Err(ConnectionError::WriteTimeout { timeout }),
    }
}

/// Read until exactly one frame can be parsed off `buf`, then return it.
///
/// The caller owns `buf` so successive packets share buffer allocations
/// instead of reallocating each time. On `Ok(_)` the frame has been
/// removed from the front of the buffer; any trailing already-read bytes
/// (the next packet, perhaps) remain.
pub async fn read_frame<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<RawFrame, ConnectionError>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        match try_decode_frame_plan(buf, compression)? {
            Some(FrameDecodePlan::Ready(frame)) => return Ok(frame),
            Some(FrameDecodePlan::Compressed(frame)) => {
                return run_bounded_frame_decode(move || frame.decode()).await;
            }
            None => {}
        }
        let remaining = MAX_INBOUND_BUFFER_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            return Err(ConnectionError::InboundBufferLimitExceeded {
                max: MAX_INBOUND_BUFFER_BYTES,
            });
        }
        let read = reader.take(remaining as u64).read_buf(buf).await?;
        if read == 0 {
            return Err(ConnectionError::Eof);
        }
    }
}

pub(crate) async fn read_frame_with_timeout<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    state: State,
    timeout: Duration,
) -> Result<RawFrame, ConnectionError>
where
    R: AsyncReadExt + Unpin,
{
    match tokio::time::timeout(timeout, read_frame(reader, buf, compression)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionError::ReadTimeout { state, timeout }),
    }
}

pub(crate) async fn read_frame_with_timeout_budgeted<R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    state: State,
    timeout: Duration,
    budget: &mut PrePlayBudget,
) -> Result<RawFrame, ConnectionError>
where
    R: AsyncReadExt + Unpin,
{
    let frame = read_frame_with_timeout(reader, buf, compression, state, timeout).await?;
    budget.record(&frame)?;
    Ok(frame)
}

#[cfg(test)]
pub(crate) async fn read_packet_with_timeout<P, R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    state: State,
    timeout: Duration,
) -> Result<P, ConnectionError>
where
    P: Packet,
    R: AsyncReadExt + Unpin,
{
    let mut frame = read_frame_with_timeout(reader, buf, compression, state, timeout).await?;
    if frame.id != P::ID {
        return Err(ConnectionError::UnexpectedPacketId {
            state,
            expected: P::ID,
            got: frame.id,
        });
    }
    let packet = P::decode(&mut frame.body)?;
    let trailing = frame.body.remaining();
    if trailing != 0 {
        return Err(ConnectionError::TrailingBytes {
            state,
            id: frame.id,
            trailing,
        });
    }
    Ok(packet)
}

pub(crate) async fn read_packet_with_timeout_budgeted<P, R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    state: State,
    timeout: Duration,
    budget: &mut PrePlayBudget,
) -> Result<P, ConnectionError>
where
    P: Packet,
    R: AsyncReadExt + Unpin,
{
    let mut frame =
        read_frame_with_timeout_budgeted(reader, buf, compression, state, timeout, budget).await?;
    if frame.id != P::ID {
        return Err(ConnectionError::UnexpectedPacketId {
            state,
            expected: P::ID,
            got: frame.id,
        });
    }
    let packet = P::decode(&mut frame.body)?;
    let trailing = frame.body.remaining();
    if trailing != 0 {
        return Err(ConnectionError::TrailingBytes {
            state,
            id: frame.id,
            trailing,
        });
    }
    Ok(packet)
}

/// Encode `packet`, frame it, and write the bytes to `writer`.
pub async fn write_packet<P, W>(
    writer: &mut W,
    packet: &P,
    compression: Compression,
) -> Result<(), ConnectionError>
where
    P: Packet,
    W: AsyncWrite + Unpin,
{
    let mut body = BytesMut::new();
    packet.encode(&mut body)?;
    let framed = encode_frame(P::ID, &body, compression)?;
    write_with_stall_timeout(writer.write_all(&framed), OUTBOUND_WRITE_STALL_TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::MinecraftCipher;
    use mc_protocol::PROTOCOL_VERSION;
    use mc_protocol::packets::handshake::{Handshake, NextState};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncWrite, AsyncWriteExt};

    struct StalledWriter;

    #[derive(Default)]
    struct PartialThenPendingWriter {
        polls: usize,
        written: Vec<u8>,
    }

    impl AsyncWrite for PartialThenPendingWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.polls += 1;
            match self.polls {
                1 => {
                    let written = buf.len().min(2);
                    self.written.extend_from_slice(&buf[..written]);
                    Poll::Ready(Ok(written))
                }
                2 => Poll::Pending,
                _ => {
                    self.written.extend_from_slice(buf);
                    Poll::Ready(Ok(buf.len()))
                }
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for StalledWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn read_packet_with_timeout_fails_when_peer_stalls() {
        let (_client, mut server) = tokio::io::duplex(64);
        let mut buf = BytesMut::new();

        let err = read_packet_with_timeout::<Handshake, _>(
            &mut server,
            &mut buf,
            Compression::Disabled,
            State::Handshake,
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            ConnectionError::ReadTimeout {
                state: State::Handshake,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn read_packet_with_timeout_reads_complete_frame() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let packet = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: 25565,
            next_state: NextState::Status,
        };
        let mut body = BytesMut::new();
        packet.encode(&mut body).unwrap();
        let framed = encode_frame(Handshake::ID, &body, Compression::Disabled).unwrap();
        client.write_all(&framed).await.unwrap();
        let mut buf = BytesMut::new();

        let decoded = read_packet_with_timeout::<Handshake, _>(
            &mut server,
            &mut buf,
            Compression::Disabled,
            State::Handshake,
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(decoded, packet);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compressed_frame_decode_runs_off_the_async_runtime_thread() {
        let runtime_thread = std::thread::current().id();
        let worker_thread = run_bounded_frame_decode(|| {
            Ok::<_, mc_protocol::FramingError>(std::thread::current().id())
        })
        .await
        .unwrap();

        assert_ne!(worker_thread, runtime_thread);
        assert!(frame_decode_permits().available_permits() <= MAX_CONCURRENT_FRAME_DECODES);
    }

    #[tokio::test]
    async fn read_frame_decodes_compressed_payload_through_production_path() {
        let (mut client, mut server) = tokio::io::duplex(8192);
        let body = vec![0xAB; 4096];
        let encoded = encode_frame(0x22, &body, Compression::Threshold(256)).unwrap();
        client.write_all(&encoded).await.unwrap();
        let mut buf = BytesMut::new();

        let frame = read_frame(&mut server, &mut buf, Compression::Threshold(256))
            .await
            .unwrap();

        assert_eq!(frame.id, 0x22);
        assert_eq!(&frame.body[..], &body);
    }

    #[tokio::test]
    async fn read_frame_rejects_slow_incomplete_payload_at_inbound_buffer_limit() {
        let (mut client, mut server) = tokio::io::duplex(MAX_INBOUND_BUFFER_BYTES * 2);
        let writer = tokio::spawn(async move {
            // VarInt21 for a frame longer than the Solaris serverbound budget,
            // followed by exactly enough bytes to fill that budget without
            // completing the declared frame.
            client.write_all(&[0xff, 0xff, 0x7f]).await.unwrap();
            client
                .write_all(&vec![0_u8; MAX_INBOUND_BUFFER_BYTES - 3])
                .await
                .unwrap();
        });
        let mut buf = BytesMut::new();

        let error = read_frame(&mut server, &mut buf, Compression::Disabled)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ConnectionError::InboundBufferLimitExceeded {
                max: MAX_INBOUND_BUFFER_BYTES
            }
        ));
        assert_eq!(buf.len(), MAX_INBOUND_BUFFER_BYTES);
        writer.await.unwrap();
    }

    #[test]
    fn pre_play_packet_budget_accepts_exact_limit_and_rejects_next_frame() {
        let mut budget = PrePlayBudget::new(2, usize::MAX);
        let frame = RawFrame {
            id: 0,
            body: bytes::Bytes::new(),
        };

        budget.record(&frame).unwrap();
        budget.record(&frame).unwrap();
        assert!(matches!(
            budget.record(&frame),
            Err(ConnectionError::PrePlayPacketBudgetExceeded { packets: 3, max: 2 })
        ));
    }

    #[test]
    fn pre_play_byte_budget_accepts_exact_limit_and_rejects_next_frame() {
        let frame = RawFrame {
            id: 0,
            body: bytes::Bytes::from_static(&[1, 2, 3]),
        };
        let frame_cost = frame.body.len() + 5;
        let mut budget = PrePlayBudget::new(usize::MAX, frame_cost);

        budget.record(&frame).unwrap();
        assert!(matches!(
            budget.record(&RawFrame {
                id: 1,
                body: bytes::Bytes::new(),
            }),
            Err(ConnectionError::PrePlayByteBudgetExceeded { bytes, max })
                if bytes == frame_cost + 5 && max == frame_cost
        ));
    }

    #[tokio::test]
    async fn write_packet_fails_when_peer_stops_reading() {
        let packet = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: 25565,
            next_state: NextState::Status,
        };
        let mut writer = StalledWriter;

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            write_packet(&mut writer, &packet, Compression::Disabled),
        )
        .await
        .expect("write_packet must bound an actual stalled socket write");

        assert!(matches!(result, Err(ConnectionError::WriteTimeout { .. })));
    }

    #[tokio::test]
    async fn encrypted_transport_decrypts_buffered_tail_before_socket_bytes() {
        const SECRET: [u8; 16] = *b"0123456789abcdef";
        let buffered_plaintext = b"buffered encrypted tail";
        let socket_plaintext = b"next encrypted read";
        let mut encrypted = [buffered_plaintext.as_slice(), socket_plaintext.as_slice()].concat();
        let mut peer_cipher = MinecraftCipher::new(&SECRET);
        peer_cipher.encrypt_in_place(&mut encrypted);

        let (mut peer, transport) = tokio::io::duplex(128);
        let mut reader = ConnectionReader::new(transport);
        let mut buffered = BytesMut::from(&encrypted[..buffered_plaintext.len()]);
        reader.enable_encryption(&SECRET, &mut buffered).unwrap();
        peer.write_all(&encrypted[buffered_plaintext.len()..])
            .await
            .unwrap();

        let mut socket_bytes = vec![0; socket_plaintext.len()];
        reader.read_exact(&mut socket_bytes).await.unwrap();

        assert_eq!(&buffered[..], buffered_plaintext);
        assert_eq!(&socket_bytes, socket_plaintext);
    }

    #[tokio::test]
    async fn encrypted_transport_writes_one_continuous_cipher_stream() {
        const SECRET: [u8; 16] = *b"0123456789abcdef";
        let first = b"first framed packet";
        let second = b"second framed packet";
        let (transport, mut peer) = tokio::io::duplex(128);
        let mut writer = ConnectionWriter::new(transport);
        writer.enable_encryption(&SECRET).unwrap();

        writer.write_all(first).await.unwrap();
        writer.write_all(second).await.unwrap();

        let mut encrypted = vec![0; first.len() + second.len()];
        peer.read_exact(&mut encrypted).await.unwrap();
        let mut peer_cipher = MinecraftCipher::new(&SECRET);
        peer_cipher.decrypt_in_place(&mut encrypted);

        assert_eq!(encrypted, [first.as_slice(), second.as_slice()].concat());
    }

    #[test]
    fn encrypted_transport_cancel_resume_requires_identical_plaintext() {
        const SECRET: [u8; 16] = *b"0123456789abcdef";
        let mut writer = ConnectionWriter::new(PartialThenPendingWriter::default());
        writer.enable_encryption(&SECRET).unwrap();
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        assert!(matches!(
            Pin::new(&mut writer).poll_write(&mut cx, b"abcd"),
            Poll::Pending
        ));
        let error = match Pin::new(&mut writer).poll_write(&mut cx, b"wxyz") {
            Poll::Ready(Err(error)) => error,
            other => panic!("different plaintext retry must fail, got {other:?}"),
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        assert!(matches!(
            Pin::new(&mut writer).poll_write(&mut cx, b"abcd"),
            Poll::Ready(Ok(4))
        ));
        let mut encrypted = writer.inner.written.clone();
        let mut peer_cipher = MinecraftCipher::new(&SECRET);
        peer_cipher.decrypt_in_place(&mut encrypted);
        assert_eq!(encrypted, b"abcd");
    }

    #[test]
    fn encryption_double_enable_fails_closed() {
        const SECRET: [u8; 16] = *b"0123456789abcdef";
        let mut writer = ConnectionWriter::new(PartialThenPendingWriter::default());
        writer.enable_encryption(&SECRET).unwrap();
        assert!(matches!(
            writer.enable_encryption(&SECRET),
            Err(ConnectionError::EncryptionState { .. })
        ));

        let (_peer, transport) = tokio::io::duplex(16);
        let mut reader = ConnectionReader::new(transport);
        let mut buffered = BytesMut::new();
        reader.enable_encryption(&SECRET, &mut buffered).unwrap();
        assert!(matches!(
            reader.enable_encryption(&SECRET, &mut buffered),
            Err(ConnectionError::EncryptionState { .. })
        ));
    }

    #[tokio::test]
    async fn encrypted_transport_preserves_write_stall_failure() {
        const SECRET: [u8; 16] = *b"0123456789abcdef";
        let packet = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: 25565,
            next_state: NextState::Status,
        };
        let mut writer = ConnectionWriter::new(StalledWriter);
        writer.enable_encryption(&SECRET).unwrap();

        let result = write_packet(&mut writer, &packet, Compression::Disabled).await;

        assert!(matches!(result, Err(ConnectionError::WriteTimeout { .. })));
    }
}
