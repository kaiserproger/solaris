//! Typed packet definitions.
//!
//! Packets are organised by connection *state*. The same packet ID may
//! mean different things in different states, so dispatch is always done
//! via `(state, direction, id)`. Each individual packet type implements
//! [`Packet`] and knows its ID *within its own state/direction*.
//!
//! ```text
//! Handshake     → Status | Login        (transient, single inbound packet)
//! Status        → (closes after pong)
//! Login         → Configuration         (M1.d)
//! Configuration → Play                  (M1.e)
//! Play          → (the rest of the game)
//! ```

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::codec::{DEFAULT_MAX_STRING_LEN, Identifier, ReadMc, WriteMc};
use crate::error::CodecError;

pub mod configuration;
pub mod handshake;
pub mod login;
pub mod play;
pub mod status;

/// Direction a packet travels: serverbound (client → server) or
/// clientbound (server → client).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Serverbound,
    Clientbound,
}

/// The five top-level connection states a 26.1 connection moves through.
///
/// `Transfer` is technically a sixth `next_state` value in the handshake
/// packet (since 1.20.5), routed similarly to `Login`; we treat it as a
/// `Login` for now and revisit if needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

/// A typed packet. Every packet knows its own ID within its (state,
/// direction) pair and how to encode/decode itself.
///
/// The `Packet` trait deliberately does not expose state/direction — those
/// are properties of the call site (the state machine), not of the type.
pub trait Packet: Sized {
    /// Packet ID, scoped to this packet's (state, direction).
    const ID: i32;

    /// Encode the packet body (without the length prefix or the ID
    /// VarInt — both are added by the framing layer).
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError>;

    /// Decode the packet body. The leading ID VarInt has already been
    /// consumed by the framing layer.
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatVisibility {
    Full,
    System,
    Hidden,
}

impl ChatVisibility {
    const fn as_wire(self) -> i32 {
        match self {
            Self::Full => 0,
            Self::System => 1,
            Self::Hidden => 2,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Full,
            1 => Self::System,
            2 => Self::Hidden,
            _ => return Err(CodecError::NotSupported("unknown ChatVisibility id")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainHand {
    Left,
    Right,
}

impl MainHand {
    const fn as_wire(self) -> i32 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Left,
            1 => Self::Right,
            _ => return Err(CodecError::NotSupported("unknown HumanoidArm id")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleStatus {
    All,
    Decreased,
    Minimal,
}

impl ParticleStatus {
    const fn as_wire(self) -> i32 {
        match self {
            Self::All => 0,
            Self::Decreased => 1,
            Self::Minimal => 2,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::All,
            1 => Self::Decreased,
            2 => Self::Minimal,
            _ => return Err(CodecError::NotSupported("unknown ParticleStatus id")),
        })
    }
}

/// Common `ClientInformation` payload used in Configuration and Play.
///
/// Verified from local decompiled 26.1.2 sources under `.analysis/decompiled`:
/// `ClientInformation(FriendlyByteBuf)` reads `readUtf(16)`, byte view distance,
/// `readEnum(ChatVisiblity)`, boolean chat colors, unsigned byte model bits,
/// `readEnum(HumanoidArm)`, boolean text filtering, boolean listing, and
/// `readEnum(ParticleStatus)`. `FriendlyByteBuf.readEnum` uses VarInt ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation {
    pub language: String,
    pub view_distance: i8,
    pub chat_visibility: ChatVisibility,
    pub chat_colors: bool,
    pub model_customisation: u8,
    pub main_hand: MainHand,
    pub text_filtering_enabled: bool,
    pub allows_listing: bool,
    pub particle_status: ParticleStatus,
}

impl ClientInformation {
    pub fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.language, 16)?;
        buf.write_i8(self.view_distance);
        buf.write_varint(self.chat_visibility.as_wire());
        buf.write_bool(self.chat_colors);
        buf.write_u8(self.model_customisation);
        buf.write_varint(self.main_hand.as_wire());
        buf.write_bool(self.text_filtering_enabled);
        buf.write_bool(self.allows_listing);
        buf.write_varint(self.particle_status.as_wire());
        Ok(())
    }

    pub fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            language: buf.read_string(16)?,
            view_distance: buf.read_i8()?,
            chat_visibility: ChatVisibility::from_wire(buf.read_varint()?)?,
            chat_colors: buf.read_bool()?,
            model_customisation: buf.read_u8()?,
            main_hand: MainHand::from_wire(buf.read_varint()?)?,
            text_filtering_enabled: buf.read_bool()?,
            allows_listing: buf.read_bool()?,
            particle_status: ParticleStatus::from_wire(buf.read_varint()?)?,
        })
    }
}

/// Common custom payload body used in Configuration and Play.
///
/// Verified from local decompiled 26.1.2 sources: `CustomPacketPayload.codec`
/// reads an Identifier first, then dispatches by type; `BrandPayload.TYPE` is
/// `minecraft:brand` and its payload is one UTF string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomPayload {
    Brand(String),
    Unknown {
        channel: Identifier,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePackAction {
    SuccessfullyLoaded,
    Declined,
    FailedDownload,
    Accepted,
    Downloaded,
    InvalidUrl,
    FailedReload,
    Discarded,
}

impl ResourcePackAction {
    const fn as_wire(self) -> i32 {
        match self {
            Self::SuccessfullyLoaded => 0,
            Self::Declined => 1,
            Self::FailedDownload => 2,
            Self::Accepted => 3,
            Self::Downloaded => 4,
            Self::InvalidUrl => 5,
            Self::FailedReload => 6,
            Self::Discarded => 7,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::SuccessfullyLoaded,
            1 => Self::Declined,
            2 => Self::FailedDownload,
            3 => Self::Accepted,
            4 => Self::Downloaded,
            5 => Self::InvalidUrl,
            6 => Self::FailedReload,
            7 => Self::Discarded,
            _ => return Err(CodecError::NotSupported("unknown ResourcePack action id")),
        })
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Accepted | Self::Downloaded)
    }
}

/// Common resource-pack status payload used in Configuration and Play.
///
/// Verified from local decompiled 26.1.2
/// `ServerboundResourcePackPacket(UUID id, Action action)`: `readUUID()` then
/// `readEnum(Action.class)`; `FriendlyByteBuf.readEnum` uses VarInt ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePackStatus {
    pub id: Uuid,
    pub action: ResourcePackAction,
}

impl ResourcePackStatus {
    pub fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_uuid(self.id);
        buf.write_varint(self.action.as_wire());
        Ok(())
    }

    pub fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_uuid()?,
            action: ResourcePackAction::from_wire(buf.read_varint()?)?,
        })
    }
}

impl CustomPayload {
    const MAX_SERVERBOUND_UNKNOWN_BODY_LEN: usize = 32_767;

    pub fn channel(&self) -> &Identifier {
        match self {
            Self::Brand(_) => Self::brand_channel(),
            Self::Unknown { channel, .. } => channel,
        }
    }

    pub fn brand_channel() -> &'static Identifier {
        static BRAND: std::sync::OnceLock<Identifier> = std::sync::OnceLock::new();
        BRAND.get_or_init(|| Identifier::parse("minecraft:brand").expect("static identifier"))
    }

    pub fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Brand(brand) => {
                let brand_len = brand.encode_utf16().count();
                if brand_len > DEFAULT_MAX_STRING_LEN {
                    return Err(CodecError::StringTooLong {
                        len: brand_len,
                        max: DEFAULT_MAX_STRING_LEN,
                    });
                }
                buf.write_identifier(Self::brand_channel())?;
                buf.write_string(brand, DEFAULT_MAX_STRING_LEN)?;
            }
            Self::Unknown { channel, payload } => {
                buf.write_identifier(channel)?;
                buf.put_slice(payload);
            }
        }
        Ok(())
    }

    pub fn encode_serverbound<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Unknown { payload, .. }
                if payload.len() > Self::MAX_SERVERBOUND_UNKNOWN_BODY_LEN =>
            {
                return Err(CodecError::StringTooLong {
                    len: payload.len(),
                    max: Self::MAX_SERVERBOUND_UNKNOWN_BODY_LEN,
                });
            }
            _ => {}
        }
        self.encode(buf)
    }

    pub fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Self::decode_with_unknown_body_limit(buf, None)
    }

    pub fn decode_serverbound<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Self::decode_with_unknown_body_limit(buf, Some(Self::MAX_SERVERBOUND_UNKNOWN_BODY_LEN))
    }

    fn decode_with_unknown_body_limit<B: Buf>(
        buf: &mut B,
        max_unknown_body_len: Option<usize>,
    ) -> Result<Self, CodecError> {
        let channel = buf.read_identifier()?;
        if channel == *Self::brand_channel() {
            return Ok(Self::Brand(buf.read_string(DEFAULT_MAX_STRING_LEN)?));
        }
        let remaining = buf.remaining();
        match max_unknown_body_len {
            Some(max) if remaining > max => {
                return Err(CodecError::StringTooLong {
                    len: remaining,
                    max,
                });
            }
            _ => {}
        }
        let mut payload = vec![0; remaining];
        buf.copy_to_slice(&mut payload);
        Ok(Self::Unknown { channel, payload })
    }
}
