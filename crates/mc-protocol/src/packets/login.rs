//! Login state — initial authentication and protocol-level setup.
//!
//! Wire choreography for offline-mode (M1.d):
//!
//! ```text
//! C → S  LoginStart        (0x00, name + uuid)
//!        — encryption negotiation is skipped in offline-mode —
//! S → C  LoginSuccess      (0x02, uuid + name + properties)
//! C → S  LoginAcknowledged (0x03, empty)
//!        → connection transitions into Configuration state
//! ```
//!
//! Compression negotiation uses `SetCompression (0x03 clientbound)`.
//! The packet itself is still plaintext; every following frame uses the
//! compressed framing layout described in [`crate::frame`].
//!
//! Online-mode (`EncryptionRequest`/`EncryptionResponse`) is a deliberate
//! follow-up milestone and lives outside M1.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use super::Packet;
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;

/// Maximum length of a player name, in characters, per vanilla.
pub const MAX_NAME_LEN: usize = 16;

// -----------------------------------------------------------------------
// Serverbound
// -----------------------------------------------------------------------

/// Initial packet of the Login state.
///
/// Since protocol version 761 (1.19.3 era) the UUID is non-optional and
/// always carried; 26.1 maintains the same layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    pub player_uuid: Uuid,
}

impl Packet for LoginStart {
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.name, MAX_NAME_LEN)?;
        buf.write_uuid(self.player_uuid);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let name = buf.read_string(MAX_NAME_LEN)?;
        let player_uuid = buf.read_uuid()?;
        Ok(Self { name, player_uuid })
    }
}

/// Sent by the client after [`LoginSuccess`] to acknowledge the
/// transition into the Configuration state. The body is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoginAcknowledged;

impl Packet for LoginAcknowledged {
    const ID: i32 = 0x03;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

// -----------------------------------------------------------------------
// Clientbound
// -----------------------------------------------------------------------

/// A "go away politely" packet legal only in the Login state. The body is
/// a JSON-encoded text component; in M1.d we send a plain `{"text":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDisconnect {
    pub reason_json: String,
}

impl Packet for LoginDisconnect {
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.reason_json, 262_144)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            reason_json: buf.read_string(262_144)?,
        })
    }
}

/// One signed/unsigned property in [`LoginSuccess`] (used to carry
/// skin/texture data for online-mode players).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// Final clientbound packet of the Login state. After sending this the
/// server waits for [`LoginAcknowledged`] and then is in the
/// Configuration state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
}

impl Packet for LoginSuccess {
    const ID: i32 = 0x02;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_uuid(self.uuid);
        buf.write_string(&self.name, MAX_NAME_LEN)?;
        buf.write_varint(i32::try_from(self.properties.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.properties.len(),
                max: i32::MAX as usize,
            }
        })?);
        for property in &self.properties {
            buf.write_string(&property.name, 32_767)?;
            buf.write_string(&property.value, 32_767)?;
            match &property.signature {
                Some(sig) => {
                    buf.write_bool(true);
                    buf.write_string(sig, 32_767)?;
                }
                None => buf.write_bool(false),
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let uuid = buf.read_uuid()?;
        let name = buf.read_string(MAX_NAME_LEN)?;
        let count = buf.read_varint()?;
        if count < 0 {
            return Err(CodecError::NegativeLength(count));
        }
        let mut properties = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = buf.read_string(32_767)?;
            let value = buf.read_string(32_767)?;
            let signed = buf.read_bool()?;
            let signature = if signed {
                Some(buf.read_string(32_767)?)
            } else {
                None
            };
            properties.push(GameProfileProperty {
                name,
                value,
                signature,
            });
        }
        Ok(Self {
            uuid,
            name,
            properties,
        })
    }
}

/// `SetCompression` — once sent, every subsequent frame on the wire
/// uses the compressed layout described in [`crate::frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCompression {
    pub threshold: i32,
}

impl Packet for SetCompression {
    const ID: i32 = 0x03;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.threshold);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            threshold: buf.read_varint()?,
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
    fn login_start_round_trip() {
        round_trip(LoginStart {
            name: "Notch".into(),
            player_uuid: Uuid::from_u128(0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210),
        });
    }

    #[test]
    fn login_start_rejects_overlong_name() {
        // 17 chars is over the 16-char vanilla limit.
        let mut buf = Vec::new();
        buf.write_string("0123456789ABCDEFG", 100).unwrap();
        let nil_uuid = Uuid::nil();
        buf.write_uuid(nil_uuid);
        let mut cursor: &[u8] = &buf;
        assert!(LoginStart::decode(&mut cursor).is_err());
    }

    #[test]
    fn login_acknowledged_round_trip() {
        round_trip(LoginAcknowledged);
    }

    #[test]
    fn login_success_empty_properties_round_trip() {
        round_trip(LoginSuccess {
            uuid: Uuid::from_u128(1),
            name: "Player".into(),
            properties: vec![],
        });
    }

    #[test]
    fn login_success_with_properties_round_trip() {
        round_trip(LoginSuccess {
            uuid: Uuid::from_u128(2),
            name: "Player".into(),
            properties: vec![
                GameProfileProperty {
                    name: "textures".into(),
                    value: "eyJ0aW1lc3RhbXAiOj…".into(),
                    signature: Some("MEUCIQDx…".into()),
                },
                GameProfileProperty {
                    name: "uncommon".into(),
                    value: "value".into(),
                    signature: None,
                },
            ],
        });
    }

    #[test]
    fn login_disconnect_round_trip() {
        round_trip(LoginDisconnect {
            reason_json: r#"{"text":"M1.d sample"}"#.into(),
        });
    }

    #[test]
    fn set_compression_round_trip() {
        round_trip(SetCompression { threshold: 256 });
        round_trip(SetCompression { threshold: -1 });
    }
}
