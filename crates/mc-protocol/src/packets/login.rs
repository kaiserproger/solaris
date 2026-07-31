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
//! Online-mode uses [`EncryptionRequest`] and [`EncryptionResponse`] before
//! login success.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use super::Packet;
use crate::codec::{ReadMc, WriteMc, read_bounded_vec};
use crate::error::CodecError;

/// Maximum length of a player name, in characters, per vanilla.
pub const MAX_NAME_LEN: usize = 16;

/// Maximum RSA ciphertext size accepted in each encrypted login response field.
const MAX_ENCRYPTED_RESPONSE_BYTES: usize = 128;
/// Vanilla-authenticated profiles carry a small signed property set; keep the
/// same ceiling used by the Play player-info projection.
const MAX_GAME_PROFILE_PROPERTIES: usize = 16;
const MIN_GAME_PROFILE_PROPERTY_BYTES: usize = 3;

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

/// Encrypted shared secret and challenge returned by an online-mode client.
///
/// Solaris uses a 1024-bit RSA key, so each ciphertext is capped at 128 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionResponse {
    pub encrypted_shared_secret: Vec<u8>,
    pub encrypted_verify_token: Vec<u8>,
}

impl Packet for EncryptionResponse {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        for ciphertext in [
            self.encrypted_shared_secret.as_slice(),
            self.encrypted_verify_token.as_slice(),
        ] {
            if ciphertext.len() > MAX_ENCRYPTED_RESPONSE_BYTES {
                return Err(CodecError::StringTooLong {
                    len: ciphertext.len(),
                    max: MAX_ENCRYPTED_RESPONSE_BYTES,
                });
            }
        }
        buf.write_byte_array(&self.encrypted_shared_secret);
        buf.write_byte_array(&self.encrypted_verify_token);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let encrypted_shared_secret = buf.read_byte_array(MAX_ENCRYPTED_RESPONSE_BYTES)?;
        let encrypted_verify_token = buf.read_byte_array(MAX_ENCRYPTED_RESPONSE_BYTES)?;
        Ok(Self {
            encrypted_shared_secret,
            encrypted_verify_token,
        })
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

/// Requests online-mode encryption from the client.
///
/// Vanilla 26.1.2 `ClientboundHelloPacket` decodes the server ID with a
/// 20 UTF-16-unit limit, followed by two byte arrays and one boolean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionRequest {
    pub server_id: String,
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
    pub should_authenticate: bool,
}

impl Packet for EncryptionRequest {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        // Vanilla's writer uses writeUtf(String), whose default limit is 32,767.
        buf.write_string(&self.server_id, 32_767)?;
        buf.write_byte_array(&self.public_key);
        buf.write_byte_array(&self.verify_token);
        buf.write_bool(self.should_authenticate);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let server_id = buf.read_string(20)?;
        let public_key = buf.read_byte_array(buf.remaining())?;
        let verify_token = buf.read_byte_array(buf.remaining())?;
        let should_authenticate = buf.read_bool()?;
        Ok(Self {
            server_id,
            public_key,
            verify_token,
            should_authenticate,
        })
    }
}

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
        if self.properties.len() > MAX_GAME_PROFILE_PROPERTIES {
            return Err(CodecError::StringTooLong {
                len: self.properties.len(),
                max: MAX_GAME_PROFILE_PROPERTIES,
            });
        }
        buf.write_uuid(self.uuid);
        buf.write_string(&self.name, MAX_NAME_LEN)?;
        buf.write_varint(i32::try_from(self.properties.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.properties.len(),
                max: MAX_GAME_PROFILE_PROPERTIES,
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
        let properties = read_bounded_vec(
            buf,
            MAX_GAME_PROFILE_PROPERTIES,
            MIN_GAME_PROFILE_PROPERTY_BYTES,
            |buf| {
                let name = buf.read_string(32_767)?;
                let value = buf.read_string(32_767)?;
                let signed = buf.read_bool()?;
                let signature = if signed {
                    Some(buf.read_string(32_767)?)
                } else {
                    None
                };
                Ok(GameProfileProperty {
                    name,
                    value,
                    signature,
                })
            },
        )?;
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
    fn encryption_packet_ids_match_vanilla_registration_order() {
        assert_eq!(EncryptionRequest::ID, 0x01);
        assert_eq!(EncryptionResponse::ID, 0x01);
    }

    #[test]
    fn encryption_request_exact_body_layout() {
        let packet = EncryptionRequest {
            server_id: String::new(),
            public_key: vec![0x30, 0x82],
            verify_token: vec![0x01, 0x02, 0x03, 0x04],
            should_authenticate: true,
        };
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();

        assert_eq!(
            body,
            vec![0x00, 0x02, 0x30, 0x82, 0x04, 0x01, 0x02, 0x03, 0x04, 0x01]
        );
    }

    #[test]
    fn encryption_request_round_trip() {
        round_trip(EncryptionRequest {
            server_id: String::new(),
            public_key: vec![0x30, 0x82, 0x01, 0x0A],
            verify_token: vec![0xDE, 0xAD, 0xBE, 0xEF],
            should_authenticate: true,
        });
    }

    #[test]
    fn encryption_request_rejects_server_id_over_twenty_utf16_units() {
        let mut body = Vec::new();
        body.write_string("123456789012345678901", 32_767).unwrap();
        body.write_byte_array(&[]);
        body.write_byte_array(&[]);
        body.write_bool(true);

        let error = EncryptionRequest::decode(&mut body.as_slice()).unwrap_err();
        assert_eq!(error, CodecError::StringTooLong { len: 21, max: 20 });
    }

    #[test]
    fn encryption_response_exact_body_layout_and_round_trip() {
        let packet = EncryptionResponse {
            encrypted_shared_secret: vec![0xAA, 0xBB],
            encrypted_verify_token: vec![0xCC],
        };
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();
        assert_eq!(body, vec![0x02, 0xAA, 0xBB, 0x01, 0xCC]);

        round_trip(packet);
    }

    #[test]
    fn encryption_response_reports_truncated_ciphertext() {
        let body = [0x04, 0xAA, 0xBB];
        let error = EncryptionResponse::decode(&mut body.as_slice()).unwrap_err();
        assert_eq!(
            error,
            CodecError::Underflow {
                needed: 2,
                available: 2,
            }
        );
    }

    #[test]
    fn encryption_response_rejects_129_byte_shared_secret_before_copy() {
        let mut body = Vec::new();
        body.write_varint(129);
        body.extend_from_slice(&[0xAA; 129]);
        body.write_byte_array(&[]);
        let mut cursor = body.as_slice();

        let error = EncryptionResponse::decode(&mut cursor).unwrap_err();

        assert_eq!(error, CodecError::StringTooLong { len: 129, max: 128 });
        assert_eq!(cursor.len(), 130);
        assert_eq!(&cursor[..129], &[0xAA; 129]);
    }

    #[test]
    fn encryption_response_accepts_two_independent_128_byte_ciphertexts() {
        let expected = EncryptionResponse {
            encrypted_shared_secret: vec![0xAA; 128],
            encrypted_verify_token: vec![0xBB; 128],
        };
        let mut body = Vec::new();
        expected.encode(&mut body).unwrap();
        let mut cursor = body.as_slice();

        let decoded = EncryptionResponse::decode(&mut cursor).unwrap();

        assert_eq!(decoded, expected);
        assert!(cursor.is_empty());
    }

    #[test]
    fn encryption_response_rejects_129_byte_verify_token_before_copy() {
        let mut body = Vec::new();
        body.write_byte_array(&[0xAA; 128]);
        body.write_varint(129);
        body.extend_from_slice(&[0xBB; 129]);
        let mut cursor = body.as_slice();

        let error = EncryptionResponse::decode(&mut cursor).unwrap_err();

        assert_eq!(error, CodecError::StringTooLong { len: 129, max: 128 });
        assert_eq!(cursor, &[0xBB; 129]);
    }

    #[test]
    fn encryption_response_rejects_absurd_declared_length_before_allocation() {
        let mut body = Vec::new();
        body.write_varint(i32::MAX);
        let mut cursor = body.as_slice();

        let error = EncryptionResponse::decode(&mut cursor).unwrap_err();

        assert_eq!(
            error,
            CodecError::StringTooLong {
                len: i32::MAX as usize,
                max: 128,
            }
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn encryption_response_rejects_overlong_varint() {
        let mut cursor: &[u8] = &[0x80; 5];

        let error = EncryptionResponse::decode(&mut cursor).unwrap_err();

        assert_eq!(error, CodecError::VarIntTooLong);
        assert!(cursor.is_empty());
    }

    #[test]
    fn encryption_response_rejects_negative_length() {
        let mut body = Vec::new();
        body.write_varint(-1);
        let mut cursor = body.as_slice();

        let error = EncryptionResponse::decode(&mut cursor).unwrap_err();

        assert_eq!(error, CodecError::NegativeLength(-1));
        assert!(cursor.is_empty());
    }

    #[test]
    fn encryption_response_encode_rejects_oversized_fields_before_write() {
        for packet in [
            EncryptionResponse {
                encrypted_shared_secret: vec![0xAA; 129],
                encrypted_verify_token: Vec::new(),
            },
            EncryptionResponse {
                encrypted_shared_secret: vec![0xAA; 128],
                encrypted_verify_token: vec![0xBB; 129],
            },
        ] {
            let mut body = vec![0xCC];

            let error = packet.encode(&mut body).unwrap_err();

            assert_eq!(error, CodecError::StringTooLong { len: 129, max: 128 });
            assert_eq!(body, vec![0xCC]);
        }
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
    fn login_success_rejects_infeasible_property_count_before_decode() {
        let mut body = Vec::new();
        body.write_uuid(Uuid::from_u128(3));
        body.write_string("Player", MAX_NAME_LEN).unwrap();
        body.write_varint(MAX_GAME_PROFILE_PROPERTIES as i32);
        body.extend_from_slice(&[0, 0]);

        assert_eq!(
            LoginSuccess::decode(&mut body.as_slice()).unwrap_err(),
            CodecError::Underflow {
                needed: MAX_GAME_PROFILE_PROPERTIES * MIN_GAME_PROFILE_PROPERTY_BYTES - 2,
                available: 2,
            }
        );
    }

    #[test]
    fn login_success_encode_rejects_property_count_over_decode_cap() {
        let packet = LoginSuccess {
            uuid: Uuid::from_u128(4),
            name: "Player".into(),
            properties: vec![
                GameProfileProperty {
                    name: "x".into(),
                    value: "y".into(),
                    signature: None,
                };
                MAX_GAME_PROFILE_PROPERTIES + 1
            ],
        };
        let mut body = Vec::new();

        assert_eq!(
            packet.encode(&mut body).unwrap_err(),
            CodecError::StringTooLong {
                len: MAX_GAME_PROFILE_PROPERTIES + 1,
                max: MAX_GAME_PROFILE_PROPERTIES,
            }
        );
        assert!(body.is_empty());
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
