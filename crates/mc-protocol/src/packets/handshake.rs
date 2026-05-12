//! Handshake state — exactly one serverbound packet, then state moves on.

use bytes::{Buf, BufMut};

use super::Packet;
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;

/// What the client is asking to do after the handshake.
///
/// Vanilla numbers these 1/2/3 in the wire protocol. 1.20.5 added
/// `Transfer`; we accept it but route it identically to `Login` until
/// transfer support is wired in a later milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
    Transfer,
}

impl NextState {
    fn from_i32(value: i32) -> Result<Self, CodecError> {
        match value {
            1 => Ok(Self::Status),
            2 => Ok(Self::Login),
            3 => Ok(Self::Transfer),
            _ => Err(CodecError::InvalidIdentifier(format!(
                "unknown next_state {value}"
            ))),
        }
    }

    fn as_i32(self) -> i32 {
        match self {
            Self::Status => 1,
            Self::Login => 2,
            Self::Transfer => 3,
        }
    }
}

/// The single serverbound handshake packet (ID `0x00`).
///
/// The client tells us which protocol version it is speaking, the hostname
/// and port it connected to (used by routing proxies to virtual-host
/// multiple servers behind one socket — we currently just log it), and
/// what it wants to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub next_state: NextState,
}

impl Packet for Handshake {
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.protocol_version);
        buf.write_string(&self.server_address, 255)?;
        buf.write_u16(self.server_port);
        buf.write_varint(self.next_state.as_i32());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let protocol_version = buf.read_varint()?;
        let server_address = buf.read_string(255)?;
        let server_port = buf.read_u16()?;
        let next_state = NextState::from_i32(buf.read_varint()?)?;
        Ok(Self {
            protocol_version,
            server_address,
            server_port,
            next_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_status_intent() {
        let pkt = Handshake {
            protocol_version: 775,
            server_address: "play.example.com".into(),
            server_port: 25565,
            next_state: NextState::Status,
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        assert_eq!(Handshake::decode(&mut cursor).unwrap(), pkt);
    }

    #[test]
    fn round_trip_login_intent() {
        let pkt = Handshake {
            protocol_version: 775,
            server_address: "localhost".into(),
            server_port: 25565,
            next_state: NextState::Login,
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        assert_eq!(Handshake::decode(&mut cursor).unwrap(), pkt);
    }

    #[test]
    fn rejects_unknown_next_state() {
        // Encode a manually-malformed packet with next_state = 99.
        let mut buf: Vec<u8> = Vec::new();
        buf.write_varint(775);
        buf.write_string("localhost", 255).unwrap();
        buf.write_u16(25565);
        buf.write_varint(99);
        let mut cursor: &[u8] = &buf;
        assert!(Handshake::decode(&mut cursor).is_err());
    }
}
