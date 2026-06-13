//! Low-level read/write helpers for one connection.
//!
//! These wrap the pure-logic framing functions from `mc-protocol` with
//! tokio async I/O. They are deliberately state-agnostic — the state
//! machine in `server.rs` and the per-state modules decide *which*
//! [`mc_protocol::Packet`] type to read.

use bytes::{Buf, BytesMut};
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::{RawFrame, State};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ConnectionError;

pub(crate) const PRE_PLAY_READ_TIMEOUT: Duration = Duration::from_secs(10);

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
        if let Some(frame) = try_decode_frame(buf, compression)? {
            return Ok(frame);
        }
        let read = reader.read_buf(buf).await?;
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

/// Encode `packet`, frame it, and write the bytes to `writer`.
pub async fn write_packet<P, W>(
    writer: &mut W,
    packet: &P,
    compression: Compression,
) -> Result<(), ConnectionError>
where
    P: Packet,
    W: AsyncWriteExt + Unpin,
{
    let mut body = BytesMut::new();
    packet.encode(&mut body)?;
    let framed = encode_frame(P::ID, &body, compression)?;
    writer.write_all(&framed).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::PROTOCOL_VERSION;
    use mc_protocol::packets::handshake::{Handshake, NextState};
    use tokio::io::AsyncWriteExt;

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
}
