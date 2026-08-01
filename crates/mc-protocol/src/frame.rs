//! Packet framing: length-prefix on top of [`crate::codec`], optionally
//! with zlib compression once the per-connection threshold has been
//! negotiated.
//!
//! Layout on the wire **without compression** (one packet):
//!
//! ```text
//! [VarInt21 packet_length] [VarInt packet_id] [body bytes…]
//!                        \__________________________/
//!                         exactly packet_length bytes
//! ```
//!
//! Layout on the wire **with compression** (one packet, threshold T):
//!
//! ```text
//! [VarInt21 packet_length] [VarInt data_length] [packet body — see below]
//!                        \__________________________________________/
//!                         exactly packet_length bytes
//!
//!   data_length == 0   → body is raw   [VarInt packet_id][payload…]   (when uncompressed body
//!                                                                      length was below T)
//!   data_length >  0   → body is zlib-compressed bytes whose decompression
//!                        is exactly data_length long and contains
//!                        [VarInt packet_id][payload…]
//! ```
//!
//! Encryption (AES-128/CFB8) wraps the entire stream a layer below this
//! one; that scaffolding lives in [`crate::cipher`] (added in M1.d when
//! it is actually wired into login). This module deliberately deals only
//! in plaintext bytes.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use flate2::Compression as ZlibLevel;
use flate2::write::ZlibEncoder;
use flate2::{Decompress, FlushDecompress, Status};
use std::io::Write;
use thiserror::Error;

use crate::codec::{ReadMc, WriteMc, varint_encoded_len};
use crate::error::CodecError;

/// Maximum value of the outer `packet_length` VarInt21.
pub const MAX_PACKET_LENGTH: usize = (1 << 21) - 1;

/// Maximum declared size of a decompressed packet, including its packet ID.
pub const MAX_DECOMPRESSED_BYTES: usize = 8 * 1024 * 1024;

/// One decoded frame: packet ID plus the body bytes that follow it.
///
/// `body` is a `Bytes` so callers can split it into the decoded packet
/// without copying. The bytes do **not** include the leading packet ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub id: i32,
    pub body: Bytes,
}

/// Compression configuration for the framing layer.
///
/// Negotiated during the Login state; vanilla servers normally enable it
/// with a threshold of 256 bytes via the `Set Compression` packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// No `Set Compression` packet has been sent yet. The frame on the
    /// wire has no `data_length` field.
    #[default]
    Disabled,
    /// `Set Compression` has been sent with the contained threshold.
    /// Bodies whose uncompressed length is below `threshold` are still
    /// transmitted as a `data_length = 0` plaintext frame; bodies at or
    /// above the threshold are zlib-compressed.
    Threshold(usize),
    /// Same wire shape as [`Compression::Threshold`], with an explicit
    /// zlib level for server-side frame encoding.
    ThresholdWithLevel { threshold: usize, level: u32 },
}

impl Compression {
    /// Returns `true` if this configuration adds a `data_length` field to
    /// every frame.
    #[must_use]
    pub const fn header_present(self) -> bool {
        matches!(self, Self::Threshold(_) | Self::ThresholdWithLevel { .. })
    }

    #[must_use]
    pub const fn threshold(self) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::Threshold(threshold) | Self::ThresholdWithLevel { threshold, .. } => {
                Some(threshold)
            }
        }
    }

    #[must_use]
    pub const fn with_level(self, level: u32) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Threshold(threshold) | Self::ThresholdWithLevel { threshold, .. } => {
                Self::ThresholdWithLevel { threshold, level }
            }
        }
    }
}

/// Everything the framing layer can complain about.
#[derive(Debug, Error)]
pub enum FramingError {
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("frame exceeds the applicable maximum of {max} bytes (got {got})")]
    FrameTooLarge { got: usize, max: usize },

    #[error("declared decompressed payload length is negative ({0})")]
    NegativeLength(i32),

    /// `data_length` claimed the uncompressed body would be this long, but
    /// a bounded decompression pass observed a different length.
    #[error(
        "compressed payload decompressed to at least {decompressed_at_least} bytes, header claimed {claimed}"
    )]
    DecompressedLengthMismatch {
        claimed: usize,
        decompressed_at_least: usize,
    },

    /// A frame had compression enabled and a positive `data_length`, but
    /// `data_length` was smaller than the per-connection threshold, which
    /// vanilla treats as a protocol violation (such packets must be sent
    /// uncompressed with `data_length = 0`).
    #[error("compressed frame declared data_length {claimed} below threshold {threshold}")]
    CompressedBelowThreshold { claimed: usize, threshold: usize },

    #[error("uncompressed frame contained {actual} bytes at or above threshold {threshold}")]
    UncompressedAboveThreshold { actual: usize, threshold: usize },

    #[error("compressed zlib stream did not reach its end marker")]
    IncompleteCompressedStream,

    #[error("compressed zlib stream left {trailing} trailing byte(s)")]
    TrailingCompressedData { trailing: usize },

    #[error("frame size arithmetic overflowed")]
    LengthOverflow,

    #[error("zlib decompression error: {0}")]
    ZlibDecompress(#[from] flate2::DecompressError),

    #[error("zlib I/O error: {0}")]
    Zlib(#[from] std::io::Error),
}

#[derive(Debug)]
pub enum FrameDecodePlan {
    Ready(RawFrame),
    Compressed(CompressedFrame),
}

#[derive(Debug)]
pub struct CompressedFrame {
    claimed_len: usize,
    payload: Bytes,
}

impl CompressedFrame {
    pub fn decode(self) -> Result<RawFrame, FramingError> {
        decode_compressed_frame(self.claimed_len, self.payload)
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn read_packet_length(buf: &[u8]) -> Result<Option<(usize, usize)>, CodecError> {
    let mut value = 0_usize;
    for index in 0..3 {
        let Some(&byte) = buf.get(index) else {
            return Ok(None);
        };
        value |= usize::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(Some((value, index + 1)));
        }
    }
    Err(CodecError::VarIntTooLong)
}

/// Try to pull a single frame off the front of `buf`.
///
/// Returns:
///
/// - `Ok(Some(frame))` and advances `buf` past the consumed bytes.
/// - `Ok(None)` if `buf` does not yet contain a complete frame. `buf` is
///   left untouched so the caller can append more bytes and retry.
/// - `Err(_)` if the bytes that ARE there are malformed; the connection
///   should be dropped.
pub fn try_decode_frame_plan(
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<Option<FrameDecodePlan>, FramingError> {
    let (packet_length, length_bytes) = match read_packet_length(buf)? {
        None => return Ok(None),
        Some(parts) => parts,
    };
    let complete_len = length_bytes
        .checked_add(packet_length)
        .ok_or(FramingError::LengthOverflow)?;
    if buf.len() < complete_len {
        return Ok(None);
    }

    buf.advance(length_bytes);
    let mut body = buf.split_to(packet_length).freeze();
    let plan = match compression {
        Compression::Disabled => {
            let id = body.read_varint()?;
            FrameDecodePlan::Ready(RawFrame { id, body })
        }
        Compression::Threshold(threshold) | Compression::ThresholdWithLevel { threshold, .. } => {
            let data_length_signed = body.read_varint()?;
            if data_length_signed < 0 {
                return Err(FramingError::NegativeLength(data_length_signed));
            }
            let data_length = data_length_signed as usize;
            if data_length == 0 {
                let actual = body.len();
                if actual >= threshold {
                    return Err(FramingError::UncompressedAboveThreshold { actual, threshold });
                }
                let id = body.read_varint()?;
                FrameDecodePlan::Ready(RawFrame { id, body })
            } else {
                if data_length < threshold {
                    return Err(FramingError::CompressedBelowThreshold {
                        claimed: data_length,
                        threshold,
                    });
                }
                if data_length > MAX_DECOMPRESSED_BYTES {
                    return Err(FramingError::FrameTooLarge {
                        got: data_length,
                        max: MAX_DECOMPRESSED_BYTES,
                    });
                }
                FrameDecodePlan::Compressed(CompressedFrame {
                    claimed_len: data_length,
                    payload: body,
                })
            }
        }
    };
    Ok(Some(plan))
}

fn decode_compressed_frame(claimed_len: usize, payload: Bytes) -> Result<RawFrame, FramingError> {
    let output_len = claimed_len
        .checked_add(1)
        .ok_or(FramingError::LengthOverflow)?;
    let mut decompressed = vec![0_u8; output_len];
    let mut decoder = Decompress::new(true);
    let status = decoder.decompress(
        payload.as_ref(),
        decompressed.as_mut_slice(),
        FlushDecompress::Finish,
    )?;
    let produced =
        usize::try_from(decoder.total_out()).map_err(|_| FramingError::LengthOverflow)?;
    if produced != claimed_len {
        return Err(FramingError::DecompressedLengthMismatch {
            claimed: claimed_len,
            decompressed_at_least: produced,
        });
    }
    if status != Status::StreamEnd {
        return Err(FramingError::IncompleteCompressedStream);
    }
    let consumed = usize::try_from(decoder.total_in()).map_err(|_| FramingError::LengthOverflow)?;
    if consumed != payload.len() {
        return Err(FramingError::TrailingCompressedData {
            trailing: payload.len() - consumed,
        });
    }

    decompressed.truncate(claimed_len);
    let mut body = Bytes::from(decompressed);
    let id = body.read_varint()?;
    Ok(RawFrame { id, body })
}

pub fn try_decode_frame(
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<Option<RawFrame>, FramingError> {
    match try_decode_frame_plan(buf, compression)? {
        None => Ok(None),
        Some(FrameDecodePlan::Ready(frame)) => Ok(Some(frame)),
        Some(FrameDecodePlan::Compressed(frame)) => frame.decode().map(Some),
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Serialise one frame onto the wire.
///
/// `body` is the packet payload (no leading packet ID — the encoder writes
/// it). The returned `Bytes` is the exact byte sequence to push into the
/// transport (or the encryption layer that wraps the transport).
pub fn encode_frame(id: i32, body: &[u8], compression: Compression) -> Result<Bytes, FramingError> {
    let id_len = varint_encoded_len(id);
    let uncompressed_len = id_len + body.len();

    match compression {
        Compression::Disabled => {
            if uncompressed_len > MAX_PACKET_LENGTH {
                return Err(FramingError::FrameTooLarge {
                    got: uncompressed_len,
                    max: MAX_PACKET_LENGTH,
                });
            }
            let mut out = BytesMut::with_capacity(
                varint_encoded_len(uncompressed_len as i32) + uncompressed_len,
            );
            out.write_varint(uncompressed_len as i32);
            out.write_varint(id);
            out.put_slice(body);
            Ok(out.freeze())
        }
        Compression::Threshold(threshold) | Compression::ThresholdWithLevel { threshold, .. } => {
            if uncompressed_len < threshold {
                // data_length = 0 sentinel; transmit plaintext.
                let inner_len = 1 /* data_length zero */ + uncompressed_len;
                if inner_len > MAX_PACKET_LENGTH {
                    return Err(FramingError::FrameTooLarge {
                        got: inner_len,
                        max: MAX_PACKET_LENGTH,
                    });
                }
                let mut out =
                    BytesMut::with_capacity(varint_encoded_len(inner_len as i32) + inner_len);
                out.write_varint(inner_len as i32);
                out.write_varint(0);
                out.write_varint(id);
                out.put_slice(body);
                Ok(out.freeze())
            } else {
                if uncompressed_len > MAX_DECOMPRESSED_BYTES {
                    return Err(FramingError::FrameTooLarge {
                        got: uncompressed_len,
                        max: MAX_DECOMPRESSED_BYTES,
                    });
                }
                // Build [VarInt id][body] then compress it.
                let mut plain = Vec::with_capacity(uncompressed_len);
                plain.write_varint(id);
                plain.extend_from_slice(body);
                let level = match compression {
                    Compression::Threshold(_) => ZlibLevel::fast(),
                    Compression::ThresholdWithLevel { level, .. } => ZlibLevel::new(level.min(9)),
                    Compression::Disabled => unreachable!("compression branch already matched"),
                };
                let mut encoder = ZlibEncoder::new(Vec::new(), level);
                encoder.write_all(&plain)?;
                let compressed = encoder.finish()?;

                let data_length = plain.len();
                let inner_len = varint_encoded_len(data_length as i32) + compressed.len();
                if inner_len > MAX_PACKET_LENGTH {
                    return Err(FramingError::FrameTooLarge {
                        got: inner_len,
                        max: MAX_PACKET_LENGTH,
                    });
                }
                let mut out =
                    BytesMut::with_capacity(varint_encoded_len(inner_len as i32) + inner_len);
                out.write_varint(inner_len as i32);
                out.write_varint(data_length as i32);
                out.put_slice(&compressed);
                Ok(out.freeze())
            }
        }
    }
}

/// Number of bytes [`encode_frame`] would emit for an uncompressed frame.
/// Useful for the writer to reserve buffer space without serialising twice.
pub fn encoded_size_uncompressed(id: i32, body_len: usize) -> Result<usize, FramingError> {
    let inner = varint_encoded_len(id)
        .checked_add(body_len)
        .ok_or(FramingError::LengthOverflow)?;
    if inner > MAX_PACKET_LENGTH {
        return Err(FramingError::FrameTooLarge {
            got: inner,
            max: MAX_PACKET_LENGTH,
        });
    }
    let inner_i32 = i32::try_from(inner).map_err(|_| FramingError::LengthOverflow)?;
    varint_encoded_len(inner_i32)
        .checked_add(inner)
        .ok_or(FramingError::LengthOverflow)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const EXPECTED_MAX_PACKET_LENGTH: usize = 2_097_151;
    const EXPECTED_MAX_DECOMPRESSED_BYTES: usize = 8_388_608;

    fn round_trip(id: i32, body: &[u8], compression: Compression) {
        let encoded = encode_frame(id, body, compression).expect("encode");
        let mut buf = BytesMut::from(&encoded[..]);
        let frame = try_decode_frame(&mut buf, compression)
            .expect("decode")
            .expect("complete");
        assert_eq!(frame.id, id);
        assert_eq!(&frame.body[..], body);
        assert!(buf.is_empty(), "all bytes consumed");
    }

    fn incompressible_bytes(len: usize) -> Vec<u8> {
        let mut state = 0xD1B5_4A32_D192_ED03_u64;
        std::iter::repeat_with(|| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .take(len)
        .collect()
    }

    fn compressed_wire(plain: &[u8], claimed: usize, trailing: &[u8]) -> BytesMut {
        let mut encoder = ZlibEncoder::new(Vec::new(), ZlibLevel::default());
        encoder.write_all(plain).unwrap();
        let mut compressed = encoder.finish().unwrap();
        compressed.extend_from_slice(trailing);
        let inner_len = varint_encoded_len(claimed as i32) + compressed.len();
        let mut buf = BytesMut::new();
        buf.write_varint(inner_len as i32);
        buf.write_varint(claimed as i32);
        buf.put_slice(&compressed);
        buf
    }

    #[test]
    fn uncompressed_round_trip_empty_body() {
        round_trip(0x00, &[], Compression::Disabled);
    }

    #[test]
    fn uncompressed_round_trip_small_body() {
        round_trip(0x12, b"hello, world", Compression::Disabled);
    }

    #[test]
    fn uncompressed_round_trip_large_body() {
        let body: Vec<u8> = (0..10_000).map(|i| (i & 0xFF) as u8).collect();
        round_trip(0x42, &body, Compression::Disabled);
    }

    #[test]
    fn uncompressed_round_trip_negative_id_var_int() {
        // Plugin channel discriminators are sometimes large; make sure
        // negative-looking IDs survive the round trip.
        round_trip(-1, b"abc", Compression::Disabled);
    }

    #[test]
    fn compressed_below_threshold_round_trip() {
        // 5 bytes body well below the 256-byte threshold → plaintext frame
        // with data_length = 0, three VarInts on the wire.
        round_trip(0x10, b"small", Compression::Threshold(256));
    }

    #[test]
    fn compressed_above_threshold_round_trip() {
        // A repetitive body so zlib actually shrinks it.
        let body: Vec<u8> = std::iter::repeat_n(b'x', 4096).collect();
        round_trip(0x20, &body, Compression::Threshold(256));
    }

    #[test]
    fn compressed_explicit_level_round_trip() {
        let body: Vec<u8> = std::iter::repeat_n(b'x', 4096).collect();
        round_trip(0x20, &body, Compression::Threshold(256).with_level(6));
    }

    #[test]
    fn compressed_threshold_edge() {
        // Exactly at the threshold (sum of id varint + body) → must compress.
        let threshold = 16;
        let id = 0x01; // 1 byte varint
        let body: Vec<u8> = vec![0xAB; threshold - 1]; // total = threshold
        round_trip(id, &body, Compression::Threshold(threshold));
    }

    #[test]
    fn decoder_rejects_uncompressed_frame_at_threshold() {
        let threshold = 16;
        let raw_len = threshold;
        let inner_len = 1 + raw_len;
        let mut buf = BytesMut::new();
        buf.write_varint(inner_len as i32);
        buf.write_varint(0);
        buf.write_varint(1);
        buf.resize(buf.len() + raw_len - 1, 0xAB);

        assert!(matches!(
            try_decode_frame(&mut buf, Compression::Threshold(threshold)),
            Err(FramingError::UncompressedAboveThreshold {
                actual,
                threshold: actual_threshold,
            }) if actual == raw_len && actual_threshold == threshold
        ));
    }

    #[test]
    fn decoder_accepts_uncompressed_frame_below_threshold() {
        round_trip(1, &[0xAB; 14], Compression::Threshold(16));
    }

    #[test]
    fn decoder_handles_split_buffers() {
        let encoded = encode_frame(0x42, b"split me", Compression::Disabled).unwrap();
        // Feed the framer one byte at a time and verify it patiently
        // returns `Ok(None)` until the last byte arrives.
        let mut acc = BytesMut::new();
        for (i, byte) in encoded.iter().enumerate() {
            acc.put_u8(*byte);
            let res = try_decode_frame(&mut acc, Compression::Disabled).unwrap();
            if i + 1 < encoded.len() {
                assert!(res.is_none(), "premature decode at byte {i}");
            } else {
                let frame = res.expect("frame at last byte");
                assert_eq!(frame.id, 0x42);
                assert_eq!(&frame.body[..], b"split me");
            }
        }
    }

    #[test]
    fn decoder_pulls_consecutive_frames() {
        let a = encode_frame(1, b"a", Compression::Disabled).unwrap();
        let b = encode_frame(2, b"bb", Compression::Disabled).unwrap();
        let mut buf = BytesMut::new();
        buf.put_slice(&a);
        buf.put_slice(&b);

        let f1 = try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .unwrap();
        assert_eq!(f1.id, 1);
        assert_eq!(&f1.body[..], b"a");

        let f2 = try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .unwrap();
        assert_eq!(f2.id, 2);
        assert_eq!(&f2.body[..], b"bb");

        assert!(
            try_decode_frame(&mut buf, Compression::Disabled)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn decoder_accepts_complete_maximum_varint21_packet_and_advances_exactly() {
        let trailing_frame = encode_frame(7, b"next", Compression::Disabled).unwrap();
        let mut buf =
            BytesMut::with_capacity(3 + EXPECTED_MAX_PACKET_LENGTH + trailing_frame.len());
        buf.put_slice(&[0xFF, 0xFF, 0x7F]);
        buf.put_u8(0); // one-byte packet id
        buf.resize(3 + EXPECTED_MAX_PACKET_LENGTH, 0xA5);
        buf.put_slice(&trailing_frame);

        let frame = try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .expect("complete maximum-length frame");

        assert_eq!(frame.id, 0);
        assert_eq!(frame.body.len(), EXPECTED_MAX_PACKET_LENGTH - 1);
        assert_eq!(&frame.body[..4], &[0xA5; 4]);
        assert_eq!(&frame.body[frame.body.len() - 4..], &[0xA5; 4]);
        assert_eq!(&buf[..], &trailing_frame[..]);
    }

    #[test]
    fn decoder_rejects_maximum_varint21_plus_one_before_payload_arrives() {
        for compression in [Compression::Disabled, Compression::Threshold(256)] {
            let mut buf = BytesMut::from(&[0x80, 0x80, 0x80, 0x01][..]);
            let err = try_decode_frame(&mut buf, compression).unwrap_err();
            assert!(matches!(
                err,
                FramingError::Codec(CodecError::VarIntTooLong)
            ));
            assert_eq!(&buf[..], &[0x80, 0x80, 0x80, 0x01]);
        }
    }

    #[test]
    fn decoder_rejects_four_byte_outer_lengths_even_when_value_is_small() {
        for encoded in [
            &[0x80, 0x80, 0x80, 0x00][..],
            &[0x81, 0x80, 0x80, 0x00][..],
            &[0xFF, 0xFF, 0xFF, 0x00][..],
        ] {
            let mut buf = BytesMut::from(encoded);
            let err = try_decode_frame(&mut buf, Compression::Disabled).unwrap_err();
            assert!(matches!(
                err,
                FramingError::Codec(CodecError::VarIntTooLong)
            ));
            assert_eq!(&buf[..], encoded);
        }
    }

    #[test]
    fn decoder_distinguishes_truncated_and_malformed_varint21_lengths() {
        for encoded in [&[0x80][..], &[0x80, 0x80][..]] {
            let mut buf = BytesMut::from(encoded);
            assert!(
                try_decode_frame(&mut buf, Compression::Disabled)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(&buf[..], encoded);
        }

        let mut malformed = BytesMut::from(&[0x80, 0x80, 0x80][..]);
        let err = try_decode_frame(&mut malformed, Compression::Disabled).unwrap_err();
        assert!(matches!(
            err,
            FramingError::Codec(CodecError::VarIntTooLong)
        ));
        assert_eq!(&malformed[..], &[0x80, 0x80, 0x80]);
    }

    #[test]
    fn decoder_rejects_negative_length() {
        let mut buf = BytesMut::new();
        buf.write_varint(-1); // -1 packet length
        let err = try_decode_frame(&mut buf, Compression::Disabled).unwrap_err();
        assert!(matches!(
            err,
            FramingError::Codec(CodecError::VarIntTooLong)
        ));
        assert_eq!(&buf[..], &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn encoder_enforces_outer_varint21_limit_with_or_without_compression_header() {
        let disabled_max_body = vec![0; EXPECTED_MAX_PACKET_LENGTH - 1];
        let encoded = encode_frame(0, &disabled_max_body, Compression::Disabled).unwrap();
        assert_eq!(&encoded[..3], &[0xFF, 0xFF, 0x7F]);

        let disabled_too_large = vec![0; EXPECTED_MAX_PACKET_LENGTH];
        assert!(matches!(
            encode_frame(0, &disabled_too_large, Compression::Disabled),
            Err(FramingError::FrameTooLarge { got, max })
                if got == EXPECTED_MAX_PACKET_LENGTH + 1
                    && max == EXPECTED_MAX_PACKET_LENGTH
        ));

        let compression = Compression::Threshold(usize::MAX);
        let enabled_max_body = vec![0; EXPECTED_MAX_PACKET_LENGTH - 2];
        let encoded = encode_frame(0, &enabled_max_body, compression).unwrap();
        assert_eq!(&encoded[..3], &[0xFF, 0xFF, 0x7F]);

        let enabled_too_large = vec![0; EXPECTED_MAX_PACKET_LENGTH - 1];
        assert!(matches!(
            encode_frame(0, &enabled_too_large, compression),
            Err(FramingError::FrameTooLarge { got, max })
                if got == EXPECTED_MAX_PACKET_LENGTH + 1
                    && max == EXPECTED_MAX_PACKET_LENGTH
        ));
    }

    #[test]
    fn compressed_payload_keeps_independent_decompressed_ceiling() {
        let max_body = vec![0; EXPECTED_MAX_DECOMPRESSED_BYTES - 1];
        round_trip(0, &max_body, Compression::Threshold(0));

        let too_large_body = vec![0; EXPECTED_MAX_DECOMPRESSED_BYTES];
        assert!(matches!(
            encode_frame(0, &too_large_body, Compression::Threshold(0)),
            Err(FramingError::FrameTooLarge { got, max })
                if got == EXPECTED_MAX_DECOMPRESSED_BYTES + 1
                    && max == EXPECTED_MAX_DECOMPRESSED_BYTES
        ));
    }

    #[test]
    fn compressed_encoder_rejects_wire_payload_larger_than_varint21() {
        let body = incompressible_bytes(EXPECTED_MAX_PACKET_LENGTH - 1);
        assert!(matches!(
            encode_frame(0, &body, Compression::Threshold(0)),
            Err(FramingError::FrameTooLarge { got, max })
                if got > EXPECTED_MAX_PACKET_LENGTH && max == EXPECTED_MAX_PACKET_LENGTH
        ));
    }

    #[test]
    fn decoder_rejects_oversized_decompressed_length_before_zlib_payload() {
        let mut buf = BytesMut::new();
        let data_length = EXPECTED_MAX_DECOMPRESSED_BYTES + 1;
        let outer_length = varint_encoded_len(data_length as i32);
        buf.write_varint(outer_length as i32);
        buf.write_varint(data_length as i32);

        let err = try_decode_frame(&mut buf, Compression::Threshold(0)).unwrap_err();
        assert!(matches!(
            err,
            FramingError::FrameTooLarge { got, max }
                if got == data_length && max == EXPECTED_MAX_DECOMPRESSED_BYTES
        ));
    }

    #[test]
    fn decoder_rejects_compressed_below_threshold() {
        // Compression threshold 256; craft a frame that claims
        // data_length = 10 (below threshold) but is supposedly compressed.
        let plain_body = b"x".repeat(10);
        let mut plain = Vec::new();
        plain.write_varint(0); // id
        plain.extend_from_slice(&plain_body);

        let mut compressed = Vec::new();
        let mut encoder = ZlibEncoder::new(&mut compressed, ZlibLevel::default());
        encoder.write_all(&plain).unwrap();
        drop(encoder);

        let inner_len = varint_encoded_len(plain.len() as i32) + compressed.len();
        let mut buf = BytesMut::new();
        buf.write_varint(inner_len as i32);
        buf.write_varint(plain.len() as i32); // data_length below threshold
        buf.put_slice(&compressed);

        let err = try_decode_frame(&mut buf, Compression::Threshold(256)).unwrap_err();
        assert!(matches!(err, FramingError::CompressedBelowThreshold { .. }));
    }

    #[test]
    fn decoder_rejects_mismatched_decompressed_length() {
        // Claim a wrong (smaller) data_length than the actual decompressed
        // size — vanilla treats this as a fatal protocol error.
        let plain_body = b"x".repeat(500);
        let mut plain = Vec::new();
        plain.write_varint(0);
        plain.extend_from_slice(&plain_body);
        let mut buf = compressed_wire(&plain, 50, &[]);

        let err = try_decode_frame(&mut buf, Compression::Threshold(16)).unwrap_err();
        assert!(matches!(
            err,
            FramingError::DecompressedLengthMismatch {
                claimed: 50,
                decompressed_at_least: 51
            }
        ));
    }

    #[test]
    fn decoder_rejects_trailing_zlib_bytes() {
        let mut plain = Vec::new();
        plain.write_varint(0x22);
        plain.extend_from_slice(b"strict stream");
        let mut buf = compressed_wire(&plain, plain.len(), &[0xAA, 0xBB]);

        assert!(matches!(
            try_decode_frame(&mut buf, Compression::Threshold(1)),
            Err(FramingError::TrailingCompressedData { trailing: 2 })
        ));
    }

    #[test]
    fn decoder_rejects_truncated_zlib_stream() {
        let mut plain = Vec::new();
        plain.write_varint(0x22);
        plain.extend_from_slice(b"strict stream");
        let mut buf = compressed_wire(&plain, plain.len(), &[]);
        buf.truncate(buf.len() - 1);
        // Rebuild the outer length after removing one compressed byte.
        let (_, length_bytes) = read_packet_length(&buf).unwrap().unwrap();
        let payload = buf.split_off(length_bytes);
        let mut truncated = BytesMut::new();
        truncated.write_varint(payload.len() as i32);
        truncated.put_slice(&payload);

        assert!(matches!(
            try_decode_frame(&mut truncated, Compression::Threshold(1)),
            Err(FramingError::IncompleteCompressedStream)
                | Err(FramingError::DecompressedLengthMismatch { .. })
                | Err(FramingError::ZlibDecompress(_))
        ));
    }

    #[test]
    fn decoder_rejects_decompression_bomb_over_declared_cap() {
        let mut plain = vec![0_u8; EXPECTED_MAX_DECOMPRESSED_BYTES + 1];
        plain[0] = 0;
        let mut buf = compressed_wire(&plain, EXPECTED_MAX_DECOMPRESSED_BYTES, &[]);

        assert!(matches!(
            try_decode_frame(&mut buf, Compression::Threshold(0)),
            Err(FramingError::DecompressedLengthMismatch {
                claimed: EXPECTED_MAX_DECOMPRESSED_BYTES,
                decompressed_at_least,
            }) if decompressed_at_least == EXPECTED_MAX_DECOMPRESSED_BYTES + 1
        ));
    }

    #[test]
    fn encoded_size_helper_matches_actual() {
        for (id, body_len) in [(0, 0), (1, 5), (255, 1024), (-1, 7)] {
            let body = vec![0u8; body_len];
            let actual = encode_frame(id, &body, Compression::Disabled)
                .unwrap()
                .len();
            assert_eq!(actual, encoded_size_uncompressed(id, body_len).unwrap());
        }
    }

    #[test]
    fn encoded_size_helper_rejects_overflow_and_oversized_frames() {
        assert!(matches!(
            encoded_size_uncompressed(0, usize::MAX),
            Err(FramingError::LengthOverflow)
        ));
        assert!(matches!(
            encoded_size_uncompressed(0, MAX_PACKET_LENGTH),
            Err(FramingError::FrameTooLarge { got, max })
                if got == MAX_PACKET_LENGTH + 1 && max == MAX_PACKET_LENGTH
        ));
    }

    // ---- Property tests -------------------------------------------------

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn proptest_uncompressed_round_trip(
            id in any::<i32>(),
            body in proptest::collection::vec(any::<u8>(), 0..1024),
        ) {
            let encoded = encode_frame(id, &body, Compression::Disabled).unwrap();
            let mut buf = BytesMut::from(&encoded[..]);
            let frame = try_decode_frame(&mut buf, Compression::Disabled).unwrap().unwrap();
            prop_assert_eq!(frame.id, id);
            prop_assert_eq!(&frame.body[..], &body[..]);
            prop_assert!(buf.is_empty());
        }

        #[test]
        fn proptest_compressed_round_trip(
            id in any::<i32>(),
            body in proptest::collection::vec(any::<u8>(), 0..4096),
            threshold in 0_usize..1024,
        ) {
            let encoded = encode_frame(id, &body, Compression::Threshold(threshold)).unwrap();
            let mut buf = BytesMut::from(&encoded[..]);
            let frame = try_decode_frame(&mut buf, Compression::Threshold(threshold)).unwrap().unwrap();
            prop_assert_eq!(frame.id, id);
            prop_assert_eq!(&frame.body[..], &body[..]);
            prop_assert!(buf.is_empty());
        }
    }
}
