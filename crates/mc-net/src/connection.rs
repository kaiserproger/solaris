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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ConnectionError;

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

/// Read and decode one packet, asserting it matches the expected type.
pub async fn read_packet<P, R>(
    reader: &mut R,
    buf: &mut BytesMut,
    compression: Compression,
    state: State,
) -> Result<P, ConnectionError>
where
    P: Packet,
    R: AsyncReadExt + Unpin,
{
    let mut frame = read_frame(reader, buf, compression).await?;
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
