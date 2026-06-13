//! Packet framing: length-prefix on top of [`crate::codec`], optionally
//! with zlib compression once the per-connection threshold has been
//! negotiated.
//!
//! Layout on the wire **without compression** (one packet):
//!
//! ```text
//! [VarInt packet_length] [VarInt packet_id] [body bytes…]
//!                        \__________________________/
//!                         exactly packet_length bytes
//! ```
//!
//! Layout on the wire **with compression** (one packet, threshold T):
//!
//! ```text
//! [VarInt packet_length] [VarInt data_length] [packet body — see below]
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
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{Read, Write};
use thiserror::Error;

use crate::codec::{ReadMc, WriteMc, read_varint_partial, varint_encoded_len};
use crate::error::CodecError;

/// Hard ceiling on the size of any single frame's payload, in bytes.
///
/// Vanilla's effective ceiling is around 2 MB for normal packets and
/// significantly higher for a handful of bulk packets. We pick 8 MB as a
/// generous DoS guard; if a real packet legitimately exceeds this we'll
/// revisit per-packet limits in a later milestone.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

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

    #[error("frame exceeds the configured maximum of {MAX_FRAME_BYTES} bytes (got {got})")]
    FrameTooLarge { got: usize },

    #[error("declared frame length is negative ({0})")]
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

    #[error("zlib I/O error: {0}")]
    Zlib(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Try to pull a single frame off the front of `buf`.
///
/// Returns:
///
/// - `Ok(Some(frame))` and advances `buf` past the consumed bytes.
/// - `Ok(None)` if `buf` does not yet contain a complete frame. `buf` is
///   left untouched so the caller can append more bytes and retry.
/// - `Err(_)` if the bytes that ARE there are malformed; the connection
///   should be dropped.
pub fn try_decode_frame(
    buf: &mut BytesMut,
    compression: Compression,
) -> Result<Option<RawFrame>, FramingError> {
    // 1. Peek the leading packet_length VarInt without consuming it.
    let (packet_length_signed, length_bytes) = match read_varint_partial(buf)? {
        None => return Ok(None),
        Some(parts) => parts,
    };
    if packet_length_signed < 0 {
        return Err(FramingError::NegativeLength(packet_length_signed));
    }
    let packet_length = packet_length_signed as usize;
    if packet_length > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge { got: packet_length });
    }

    // 2. Need all `packet_length` bytes after the length VarInt itself.
    if buf.len() < length_bytes + packet_length {
        return Ok(None);
    }

    // We're committed now — consume.
    buf.advance(length_bytes);
    let mut body = buf.split_to(packet_length).freeze();

    // 3. Either uncompressed or maybe-compressed; branch.
    let frame = match compression {
        Compression::Disabled => {
            let id = body.read_varint()?;
            RawFrame { id, body }
        }
        Compression::Threshold(threshold) | Compression::ThresholdWithLevel { threshold, .. } => {
            let data_length_signed = body.read_varint()?;
            if data_length_signed < 0 {
                return Err(FramingError::NegativeLength(data_length_signed));
            }
            let data_length = data_length_signed as usize;
            if data_length == 0 {
                // Plaintext, just like the uncompressed case but with the
                // extra data_length field already stripped.
                let id = body.read_varint()?;
                RawFrame { id, body }
            } else {
                if data_length < threshold {
                    return Err(FramingError::CompressedBelowThreshold {
                        claimed: data_length,
                        threshold,
                    });
                }
                if data_length > MAX_FRAME_BYTES {
                    return Err(FramingError::FrameTooLarge { got: data_length });
                }
                // `body` here is the compressed payload.
                let decoder = ZlibDecoder::new(body.as_ref());
                let mut decoder = decoder.take(data_length as u64 + 1);
                let mut decompressed = Vec::with_capacity(data_length);
                decoder.read_to_end(&mut decompressed)?;
                if decompressed.len() != data_length {
                    return Err(FramingError::DecompressedLengthMismatch {
                        claimed: data_length,
                        decompressed_at_least: decompressed.len(),
                    });
                }
                let mut cursor: &[u8] = &decompressed;
                let id = cursor.read_varint()?;
                let payload_start = decompressed.len() - cursor.len();
                let mut owned = BytesMut::with_capacity(cursor.len());
                owned.put_slice(&decompressed[payload_start..]);
                RawFrame {
                    id,
                    body: owned.freeze(),
                }
            }
        }
    };

    Ok(Some(frame))
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
            if uncompressed_len > MAX_FRAME_BYTES {
                return Err(FramingError::FrameTooLarge {
                    got: uncompressed_len,
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
                if inner_len > MAX_FRAME_BYTES {
                    return Err(FramingError::FrameTooLarge { got: inner_len });
                }
                let mut out =
                    BytesMut::with_capacity(varint_encoded_len(inner_len as i32) + inner_len);
                out.write_varint(inner_len as i32);
                out.write_varint(0);
                out.write_varint(id);
                out.put_slice(body);
                Ok(out.freeze())
            } else {
                if uncompressed_len > MAX_FRAME_BYTES {
                    return Err(FramingError::FrameTooLarge {
                        got: uncompressed_len,
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
#[must_use]
pub fn encoded_size_uncompressed(id: i32, body_len: usize) -> usize {
    let inner = varint_encoded_len(id) + body_len;
    varint_encoded_len(inner as i32) + inner
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
    fn decoder_rejects_excess_size() {
        // Hand-build a frame claiming a > 8 MiB length.
        let mut buf = BytesMut::new();
        let huge = MAX_FRAME_BYTES + 1;
        buf.write_varint(huge as i32);
        // No actual body — we only need the length to be parsed.
        let err = try_decode_frame(&mut buf, Compression::Disabled).unwrap_err();
        assert!(matches!(err, FramingError::FrameTooLarge { .. }));
    }

    #[test]
    fn decoder_rejects_negative_length() {
        let mut buf = BytesMut::new();
        buf.write_varint(-1); // -1 packet length
        let err = try_decode_frame(&mut buf, Compression::Disabled).unwrap_err();
        assert!(matches!(err, FramingError::NegativeLength(-1)));
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

        let mut compressed = Vec::new();
        let mut encoder = ZlibEncoder::new(&mut compressed, ZlibLevel::default());
        encoder.write_all(&plain).unwrap();
        drop(encoder);

        let inner_len = varint_encoded_len(50_i32) + compressed.len();
        let mut buf = BytesMut::new();
        buf.write_varint(inner_len as i32);
        buf.write_varint(50); // lie: claim 50 bytes, actually 501
        buf.put_slice(&compressed);

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
    fn encoded_size_helper_matches_actual() {
        for (id, body_len) in [(0, 0), (1, 5), (255, 1024), (-1, 7)] {
            let body = vec![0u8; body_len];
            let actual = encode_frame(id, &body, Compression::Disabled)
                .unwrap()
                .len();
            assert_eq!(actual, encoded_size_uncompressed(id, body_len));
        }
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
