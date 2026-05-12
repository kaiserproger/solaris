//! Status state — vanilla "server list ping" handshake.
//!
//! Wire choreography (after the initial Handshake with `next_state =
//! Status` has put the connection into this state):
//!
//! ```text
//! C → S  StatusRequest   (id 0x00, empty)
//! S → C  StatusResponse  (id 0x00, JSON string)
//! C → S  PingRequest     (id 0x01, i64 payload)
//! S → C  PongResponse    (id 0x01, same i64 payload)
//! ```
//!
//! After PongResponse the server closes the connection.

use bytes::{Buf, BufMut};

use super::Packet;
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;

// -----------------------------------------------------------------------
// Serverbound
// -----------------------------------------------------------------------

/// Empty serverbound packet asking the server for its status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

/// Serverbound ping carrying an opaque `i64` payload. The server must
/// echo it back unchanged in [`PongResponse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingRequest {
    pub payload: i64,
}

impl Packet for PingRequest {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.payload);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: buf.read_i64()?,
        })
    }
}

// -----------------------------------------------------------------------
// Clientbound
// -----------------------------------------------------------------------

/// Clientbound status reply: a single JSON string. The shape of the JSON
/// is documented at <https://minecraft.wiki/w/Java_Edition_protocol/Status>.
/// We keep it stringy here so the network layer is free to compose the
/// JSON however it likes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResponse {
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.json, 32_767)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            json: buf.read_string(32_767)?,
        })
    }
}

/// Clientbound pong, payload echoed verbatim from [`PingRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PongResponse {
    pub payload: i64,
}

impl Packet for PongResponse {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.payload);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: buf.read_i64()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<P: Packet + PartialEq + std::fmt::Debug>(p: P) {
        let mut buf = Vec::new();
        p.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded: P = P::decode(&mut cursor).unwrap();
        assert_eq!(decoded, p);
        assert!(cursor.is_empty());
    }

    #[test]
    fn status_request_round_trip() {
        round_trip(StatusRequest);
    }

    #[test]
    fn ping_request_round_trip() {
        round_trip(PingRequest { payload: 0 });
        round_trip(PingRequest { payload: i64::MIN });
        round_trip(PingRequest { payload: i64::MAX });
        round_trip(PingRequest { payload: -1 });
    }

    #[test]
    fn status_response_round_trip() {
        round_trip(StatusResponse {
            json: r#"{"version":{"name":"26.1.2","protocol":775}}"#.into(),
        });
    }

    #[test]
    fn pong_response_round_trip() {
        round_trip(PongResponse {
            payload: 1234567890,
        });
    }
}
