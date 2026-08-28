//! Errors produced by the codec and framing layers.

use mc_nbt::NbtError;
use thiserror::Error;

/// All errors that the encode/decode layer can produce.
///
/// Variants are kept narrow so callers can match on them: the framing layer
/// in particular needs to distinguish "the buffer just does not have the
/// bytes yet" ([`CodecError::Underflow`]) from "the bytes that are there
/// are malformed" (everything else).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    /// The reader needed more bytes than the buffer currently holds.
    ///
    /// In a framing context this means "wait for more network data and try
    /// again". In a packet-decoding context, where the framing layer has
    /// already guaranteed the whole packet is present, it means the packet
    /// is malformed (truncated).
    #[error("buffer underflow: needed {needed} more byte(s), only {available} available")]
    Underflow {
        /// How many additional bytes the read needed.
        needed: usize,
        /// How many bytes the buffer actually had at the point of the read.
        available: usize,
    },

    /// A VarInt occupied more than 5 bytes — the continuation bit stayed
    /// set on the fifth byte. Vanilla rejects this.
    #[error("VarInt is longer than 5 bytes")]
    VarIntTooLong,

    /// A VarLong occupied more than 10 bytes.
    #[error("VarLong is longer than 10 bytes")]
    VarLongTooLong,

    /// A length-prefixed string declared a length greater than the
    /// caller-supplied maximum. Many vanilla packets carry per-field
    /// maxima; exceeding them is a protocol violation.
    #[error("string length {len} exceeds maximum {max}")]
    StringTooLong {
        /// The declared length.
        len: usize,
        /// The maximum the caller is willing to accept.
        max: usize,
    },

    /// A length-prefixed byte array exceeds the caller or wire-format ceiling.
    #[error("byte array length {len} exceeds maximum {max}")]
    ArrayTooLong { len: usize, max: usize },

    /// A length-prefixed string contained bytes that are not valid UTF-8.
    #[error("invalid UTF-8 in string")]
    InvalidUtf8,

    /// A negative VarInt was supplied where an unsigned length / count
    /// was expected.
    #[error("negative length: {0}")]
    NegativeLength(i32),

    /// An identifier failed `namespace:path` validation. The string is
    /// echoed back for debugging.
    #[error("invalid identifier: {0:?}")]
    InvalidIdentifier(String),

    /// An NBT field embedded in a packet could not be encoded or decoded.
    /// `mc-nbt`'s own error variant is preserved.
    #[error("nbt error: {0}")]
    Nbt(#[from] NbtError),

    /// The decoder met a wire shape it is not equipped to handle in the
    /// current milestone scope (e.g. an ItemStack with DataComponentPatch
    /// entries before M7+ wires that path).
    #[error("not supported: {0}")]
    NotSupported(&'static str),
}
