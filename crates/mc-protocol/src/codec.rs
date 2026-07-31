//! Encode/decode primitives for the Minecraft Java Edition wire protocol.
//!
//! Reading is exposed via [`ReadMc`], an extension trait on `bytes::Buf`;
//! writing via [`WriteMc`], an extension trait on `bytes::BufMut`. This
//! lets callers reuse whatever buffer type is most convenient (`&[u8]`,
//! `bytes::Bytes`, `bytes::BytesMut`, `Vec<u8>`, …) without conversion.
//!
//! Two important properties:
//!
//! - **All-or-something semantics inside the trait methods.** When a
//!   method returns `Err`, the underlying buffer may have already advanced
//!   past some of the field's bytes — partial reads are not rolled back.
//!   This is fine for *post-framing* packet decoding (the framer hands us
//!   exactly one packet's worth of bytes; an error there means malformed
//!   input, and we bail on the connection anyway).
//! - **The framing layer uses [`read_varint_partial`] instead**, which
//!   peeks at a slice and reports "need more bytes" without consuming
//!   anything. That is the only place in the codebase that needs the
//!   peek-and-rewind dance.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::CodecError;

/// Maximum number of bytes a Minecraft VarInt may occupy on the wire.
pub const MAX_VARINT_BYTES: usize = 5;

/// Maximum number of bytes a Minecraft VarLong may occupy on the wire.
pub const MAX_VARLONG_BYTES: usize = 10;

/// VarInt-prefixed strings cap at this many UTF-16 code units in vanilla.
pub const DEFAULT_MAX_STRING_LEN: usize = 32_767;

/// Count-prefixed collections grow beyond this only after elements are decoded.
/// This prevents a valid large declared count from becoming an eager allocation.
const MAX_INITIAL_COLLECTION_CAPACITY: usize = 1_024;

fn utf16_code_units(s: &str) -> usize {
    s.encode_utf16().count()
}

// -----------------------------------------------------------------------
// VarInt / VarLong as standalone functions (used by the framing layer).
// -----------------------------------------------------------------------

/// Try to decode a VarInt out of `buf` without consuming any bytes.
///
/// Returns:
///
/// - `Ok(Some((value, bytes_read)))` on success. The caller is expected
///   to advance its own cursor by `bytes_read`.
/// - `Ok(None)` if the buffer does not yet contain a complete VarInt.
///   The caller should wait for more bytes and retry.
/// - `Err(CodecError::VarIntTooLong)` if the encoding is malformed.
///
/// This is the only varint API that distinguishes "incomplete" from
/// "malformed"; the trait-based [`ReadMc::read_varint`] folds both into
/// errors because by that point framing has guaranteed enough bytes.
pub fn read_varint_partial(buf: &[u8]) -> Result<Option<(i32, usize)>, CodecError> {
    let mut value: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let Some(&byte) = buf.get(i) else {
            return Ok(None);
        };
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(Some((value as i32, i + 1)));
        }
    }
    Err(CodecError::VarIntTooLong)
}

/// Compute how many bytes [`WriteMc::write_varint`] will emit for `value`.
///
/// Used by the framing layer to size a length prefix without serialising
/// twice.
#[must_use]
pub const fn varint_encoded_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Same as [`varint_encoded_len`] but for VarLong.
#[must_use]
pub const fn varlong_encoded_len(value: i64) -> usize {
    let mut v = value as u64;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

// -----------------------------------------------------------------------
// ReadMc: extension trait over bytes::Buf.
// -----------------------------------------------------------------------

/// Decoding side of the codec layer.
///
/// All methods consume bytes from the underlying buffer on success.
/// On error the buffer is in an unspecified state — callers should
/// discard the connection.
pub trait ReadMc: Buf {
    fn read_u8(&mut self) -> Result<u8, CodecError> {
        ensure_remaining(self, 1)?;
        Ok(self.get_u8())
    }

    fn read_i8(&mut self) -> Result<i8, CodecError> {
        ensure_remaining(self, 1)?;
        Ok(self.get_i8())
    }

    fn read_u16(&mut self) -> Result<u16, CodecError> {
        ensure_remaining(self, 2)?;
        Ok(self.get_u16())
    }

    fn read_i16(&mut self) -> Result<i16, CodecError> {
        ensure_remaining(self, 2)?;
        Ok(self.get_i16())
    }

    fn read_i32(&mut self) -> Result<i32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_i32())
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_u32())
    }

    fn read_i64(&mut self) -> Result<i64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_i64())
    }

    fn read_u64(&mut self) -> Result<u64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_u64())
    }

    fn read_f32(&mut self) -> Result<f32, CodecError> {
        ensure_remaining(self, 4)?;
        Ok(self.get_f32())
    }

    fn read_f64(&mut self) -> Result<f64, CodecError> {
        ensure_remaining(self, 8)?;
        Ok(self.get_f64())
    }

    fn read_bool(&mut self) -> Result<bool, CodecError> {
        Ok(self.read_u8()? != 0)
    }

    fn read_varint(&mut self) -> Result<i32, CodecError> {
        let mut value: u32 = 0;
        for i in 0..MAX_VARINT_BYTES {
            let byte = self.read_u8()?;
            value |= u32::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(CodecError::VarIntTooLong)
    }

    fn read_varlong(&mut self) -> Result<i64, CodecError> {
        let mut value: u64 = 0;
        for i in 0..MAX_VARLONG_BYTES {
            let byte = self.read_u8()?;
            value |= u64::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(CodecError::VarLongTooLong)
    }

    /// Length-prefixed string. `max` is the maximum permitted number of
    /// UTF-16 code units, matching Java's `String.length()`.
    fn read_string(&mut self, max: usize) -> Result<String, CodecError> {
        let len_signed = self.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        let len = len_signed as usize;
        // Netty bounds UTF-8 at three bytes per permitted UTF-16 code unit.
        // Rejecting the declared byte length here gates payload allocation.
        let max_bytes = max.saturating_mul(3);
        if len > max_bytes {
            return Err(CodecError::StringTooLong {
                len,
                max: max_bytes,
            });
        }
        ensure_remaining(self, len)?;
        let mut bytes = vec![0u8; len];
        self.copy_to_slice(&mut bytes);
        let s = String::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
        let code_units = utf16_code_units(&s);
        if code_units > max {
            return Err(CodecError::StringTooLong {
                len: code_units,
                max,
            });
        }
        Ok(s)
    }

    /// Reads a `namespace:path` identifier as a length-prefixed string and
    /// validates its shape.
    fn read_identifier(&mut self) -> Result<Identifier, CodecError> {
        let s = self.read_string(DEFAULT_MAX_STRING_LEN)?;
        Identifier::parse(s).map_err(|e| CodecError::InvalidIdentifier(e.0))
    }

    /// 128-bit big-endian UUID.
    fn read_uuid(&mut self) -> Result<Uuid, CodecError> {
        let mut bytes = [0u8; 16];
        ensure_remaining(self, 16)?;
        self.copy_to_slice(&mut bytes);
        Ok(Uuid::from_bytes(bytes))
    }

    /// Length-prefixed (VarInt) byte array.
    fn read_byte_array(&mut self, max: usize) -> Result<Vec<u8>, CodecError> {
        let len_signed = self.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        let len = len_signed as usize;
        if len > max {
            return Err(CodecError::StringTooLong { len, max });
        }
        ensure_remaining(self, len)?;
        let mut bytes = vec![0u8; len];
        self.copy_to_slice(&mut bytes);
        Ok(bytes)
    }
}

impl<T: Buf + ?Sized> ReadMc for T {}

fn ensure_remaining<B: Buf + ?Sized>(buf: &B, needed: usize) -> Result<(), CodecError> {
    let available = buf.remaining();
    if available < needed {
        return Err(CodecError::Underflow {
            needed: needed - available,
            available,
        });
    }
    Ok(())
}

/// Decode one VarInt-counted collection with allocation and wire-feasibility fences.
///
/// `min_wire_size` is a conservative lower bound for one encoded element. The
/// body-size check runs before the first collection allocation or item callback.
/// Large valid collections begin with a bounded allocation and grow only as
/// their elements are successfully decoded.
pub(crate) fn read_bounded_count<B: Buf>(
    buf: &mut B,
    semantic_max: usize,
    min_wire_size: usize,
) -> Result<usize, CodecError> {
    assert!(
        min_wire_size > 0,
        "collection minimum wire size must be non-zero"
    );
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    let count = count as usize;
    if count > semantic_max {
        return Err(CodecError::StringTooLong {
            len: count,
            max: semantic_max,
        });
    }
    let available = buf.remaining();
    if count > available / min_wire_size {
        let minimum_body_bytes = count.saturating_mul(min_wire_size);
        return Err(CodecError::Underflow {
            needed: minimum_body_bytes.saturating_sub(available),
            available,
        });
    }
    Ok(count)
}

pub(crate) fn bounded_vec<T>(count: usize) -> Vec<T> {
    Vec::with_capacity(count.min(MAX_INITIAL_COLLECTION_CAPACITY))
}

pub(crate) fn read_bounded_vec<B, T, F>(
    buf: &mut B,
    semantic_max: usize,
    min_wire_size: usize,
    mut read_item: F,
) -> Result<Vec<T>, CodecError>
where
    B: Buf,
    F: FnMut(&mut B) -> Result<T, CodecError>,
{
    let count = read_bounded_count(buf, semantic_max, min_wire_size)?;
    let mut values = bounded_vec(count);
    for _ in 0..count {
        values.push(read_item(buf)?);
    }
    Ok(values)
}

// -----------------------------------------------------------------------
// WriteMc: extension trait over bytes::BufMut.
// -----------------------------------------------------------------------

/// Encoding side of the codec layer.
///
/// Methods that cannot fail return `()` directly; the only fallible
/// writers are the length-bounded ones (`write_string`, …).
pub trait WriteMc: BufMut {
    fn write_u8(&mut self, v: u8) {
        self.put_u8(v);
    }

    fn write_i8(&mut self, v: i8) {
        self.put_i8(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.put_u16(v);
    }

    fn write_i16(&mut self, v: i16) {
        self.put_i16(v);
    }

    fn write_i32(&mut self, v: i32) {
        self.put_i32(v);
    }

    fn write_u32(&mut self, v: u32) {
        self.put_u32(v);
    }

    fn write_i64(&mut self, v: i64) {
        self.put_i64(v);
    }

    fn write_u64(&mut self, v: u64) {
        self.put_u64(v);
    }

    fn write_f32(&mut self, v: f32) {
        self.put_f32(v);
    }

    fn write_f64(&mut self, v: f64) {
        self.put_f64(v);
    }

    fn write_bool(&mut self, v: bool) {
        self.put_u8(u8::from(v));
    }

    fn write_varint(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    fn write_varlong(&mut self, value: i64) {
        let mut v = value as u64;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    fn write_string(&mut self, s: &str, max: usize) -> Result<(), CodecError> {
        let code_units = utf16_code_units(s);
        if code_units > max {
            return Err(CodecError::StringTooLong {
                len: code_units,
                max,
            });
        }
        self.write_varint(
            i32::try_from(s.len()).map_err(|_| CodecError::StringTooLong { len: s.len(), max })?,
        );
        self.put_slice(s.as_bytes());
        Ok(())
    }

    fn write_identifier(&mut self, id: &Identifier) -> Result<(), CodecError> {
        self.write_string(id.as_str(), DEFAULT_MAX_STRING_LEN)
    }

    fn write_uuid(&mut self, uuid: Uuid) {
        self.put_slice(uuid.as_bytes());
    }

    fn write_byte_array(&mut self, bytes: &[u8]) {
        self.write_varint(i32::try_from(bytes.len()).expect("byte array length fits in an i32"));
        self.put_slice(bytes);
    }
}

impl<T: BufMut + ?Sized> WriteMc for T {}

// -----------------------------------------------------------------------
// Identifier — re-exported from mc-data, where the type now lives.
// -----------------------------------------------------------------------

pub use mc_data::Identifier;

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ---- VarInt: golden vectors from the wiki.vg Data Types page. ----
    // (value, expected bytes)
    const VARINT_GOLDEN: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (25_565, &[0xDD, 0xC7, 0x01]),
        (2_097_151, &[0xFF, 0xFF, 0x7F]),
        (2_147_483_647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        (-2_147_483_648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    const VARLONG_GOLDEN: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (2_147_483_647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (
            9_223_372_036_854_775_807,
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
        ),
        (
            -1,
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
        ),
        (
            -2_147_483_648,
            &[0x80, 0x80, 0x80, 0x80, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
        ),
        (
            -9_223_372_036_854_775_808,
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
        ),
    ];

    #[test]
    fn varint_golden_encode() {
        for (value, expected) in VARINT_GOLDEN {
            let mut buf = Vec::new();
            buf.write_varint(*value);
            assert_eq!(&buf[..], *expected, "encode({value})");
            assert_eq!(varint_encoded_len(*value), expected.len(), "len({value})");
        }
    }

    #[test]
    fn varint_golden_decode() {
        for (value, encoded) in VARINT_GOLDEN {
            let mut buf: &[u8] = encoded;
            let decoded = buf.read_varint().expect("decode");
            assert_eq!(decoded, *value, "decode({encoded:?})");
            assert!(buf.is_empty(), "all bytes consumed");
        }
    }

    #[test]
    fn varlong_golden_encode() {
        for (value, expected) in VARLONG_GOLDEN {
            let mut buf = Vec::new();
            buf.write_varlong(*value);
            assert_eq!(&buf[..], *expected, "encode({value})");
            assert_eq!(varlong_encoded_len(*value), expected.len(), "len({value})");
        }
    }

    #[test]
    fn varlong_golden_decode() {
        for (value, encoded) in VARLONG_GOLDEN {
            let mut buf: &[u8] = encoded;
            let decoded = buf.read_varlong().expect("decode");
            assert_eq!(decoded, *value, "decode({encoded:?})");
        }
    }

    #[test]
    fn read_varint_partial_handles_split_buffers() {
        let encoded: &[u8] = &[0xDD, 0xC7, 0x01]; // 25565
        for split in 0..encoded.len() {
            let partial = read_varint_partial(&encoded[..split]).expect("no error");
            assert!(partial.is_none(), "incomplete at {split}");
        }
        let (value, consumed) = read_varint_partial(encoded).expect("ok").expect("complete");
        assert_eq!(value, 25_565);
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn read_varint_partial_rejects_overlong() {
        // Six continuation bytes in a row — never legal.
        let buf: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert_eq!(read_varint_partial(buf), Err(CodecError::VarIntTooLong));
    }

    #[test]
    fn read_varint_via_trait_rejects_overlong() {
        let buf: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        let mut cursor: &[u8] = buf;
        assert_eq!(cursor.read_varint(), Err(CodecError::VarIntTooLong));
    }

    #[test]
    fn underflow_is_reported_as_underflow() {
        let mut empty: &[u8] = &[];
        assert!(matches!(
            empty.read_varint(),
            Err(CodecError::Underflow { .. })
        ));
        let mut short: &[u8] = &[0x01, 0x02];
        assert!(matches!(
            short.read_i64(),
            Err(CodecError::Underflow { .. })
        ));
    }

    #[test]
    fn string_round_trip_basic() {
        let mut buf = Vec::new();
        buf.write_string("hello", 32).unwrap();
        let mut cursor: &[u8] = &buf;
        assert_eq!(cursor.read_string(32).unwrap(), "hello");
    }

    #[test]
    fn string_round_trip_unicode() {
        let s = "Hello, мир! 🌍";
        let mut buf = Vec::new();
        buf.write_string(s, 64).unwrap();
        let mut cursor: &[u8] = &buf;
        assert_eq!(cursor.read_string(64).unwrap(), s);
    }

    #[test]
    fn string_read_accepts_exact_three_byte_ceiling() {
        let s = "\u{0800}\u{0800}";
        assert_eq!(s.len(), 6);

        let mut encoded = Vec::new();
        encoded.write_varint(6);
        encoded.extend_from_slice(s.as_bytes());

        let mut cursor: &[u8] = &encoded;
        assert_eq!(cursor.read_string(2).unwrap(), s);
        assert!(cursor.is_empty());
    }

    #[test]
    fn string_read_rejects_ceiling_plus_one_before_payload_access() {
        let mut encoded = Vec::new();
        encoded.write_varint(7);

        let mut cursor: &[u8] = &encoded;
        assert_eq!(
            cursor.read_string(2).unwrap_err(),
            CodecError::StringTooLong { len: 7, max: 6 }
        );
        assert!(cursor.is_empty(), "only the declared length is consumed");
    }

    #[test]
    fn string_round_trips_ascii_bmp_and_supplementary_scalars_at_their_limits() {
        for (s, max) in [("abc", 3), ("\u{0800}", 1), ("\u{1F600}", 2)] {
            let mut encoded = Vec::new();
            encoded.write_string(s, max).unwrap();
            assert!(s.len() <= max.saturating_mul(3));

            let mut cursor: &[u8] = &encoded;
            assert_eq!(cursor.read_string(max).unwrap(), s);
            assert!(cursor.is_empty());
        }
    }

    #[test]
    fn string_limit_zero_accepts_only_empty_strings() {
        let mut encoded = Vec::new();
        encoded.write_string("", 0).unwrap();
        let mut cursor: &[u8] = &encoded;
        assert_eq!(cursor.read_string(0).unwrap(), "");
        assert!(cursor.is_empty());

        assert_eq!(
            encoded.write_string("a", 0).unwrap_err(),
            CodecError::StringTooLong { len: 1, max: 0 }
        );
        assert_eq!(encoded, vec![0]);

        let mut declared_nonempty: &[u8] = &[1];
        assert_eq!(
            declared_nonempty.read_string(0).unwrap_err(),
            CodecError::StringTooLong { len: 1, max: 0 }
        );
    }

    #[test]
    fn string_byte_ceiling_arithmetic_does_not_overflow() {
        let max = usize::MAX / 3 + 1;
        let mut encoded: &[u8] = &[1, b'a'];
        assert_eq!(encoded.read_string(max).unwrap(), "a");
        assert!(encoded.is_empty());
    }

    #[test]
    fn string_rejected_when_too_long_on_write() {
        let mut buf = Vec::new();
        let err = buf.write_string("abcdef", 3).unwrap_err();
        assert!(matches!(err, CodecError::StringTooLong { .. }));
    }

    #[test]
    fn string_rejected_when_too_long_on_read() {
        // Hand-craft: varint length=6, then 6 ASCII bytes.
        let mut buf = vec![6u8];
        buf.extend_from_slice(b"abcdef");
        let mut cursor: &[u8] = &buf;
        let err = cursor.read_string(3).unwrap_err();
        assert!(matches!(err, CodecError::StringTooLong { .. }));
    }

    #[test]
    fn string_write_limit_counts_utf16_code_units() {
        let mut buf = Vec::new();
        assert_eq!(
            buf.write_string("\u{1F600}", 1).unwrap_err(),
            CodecError::StringTooLong { len: 2, max: 1 }
        );
        assert!(buf.is_empty());

        buf.write_string("\u{1F600}", 2).unwrap();
    }

    #[test]
    fn string_write_and_read_limits_count_utf16_code_units() {
        let mut output = Vec::new();
        assert_eq!(
            output.write_string("\u{1F600}a", 2).unwrap_err(),
            CodecError::StringTooLong { len: 3, max: 2 }
        );
        assert!(output.is_empty());

        let encoded = [5, 0xF0, 0x9F, 0x98, 0x80, b'a'];
        let mut cursor: &[u8] = &encoded;
        assert_eq!(
            cursor.read_string(2).unwrap_err(),
            CodecError::StringTooLong { len: 3, max: 2 }
        );

        let mut cursor: &[u8] = &encoded;
        assert_eq!(cursor.read_string(3).unwrap(), "\u{1F600}a");
    }

    #[test]
    fn string_rejected_on_negative_length() {
        // VarInt -1 = 0xFF 0xFF 0xFF 0xFF 0x0F, then no bytes.
        let buf: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        let mut cursor: &[u8] = buf;
        assert_eq!(
            cursor.read_string(32).unwrap_err(),
            CodecError::NegativeLength(-1)
        );
    }

    #[test]
    fn string_rejected_on_malformed_varint_length() {
        let mut encoded: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80];
        assert_eq!(
            encoded.read_string(32).unwrap_err(),
            CodecError::VarIntTooLong
        );
    }

    #[test]
    fn string_rejected_on_truncated_varint_length() {
        let mut encoded: &[u8] = &[0x80];
        assert_eq!(
            encoded.read_string(32).unwrap_err(),
            CodecError::Underflow {
                needed: 1,
                available: 0,
            }
        );
    }

    #[test]
    fn string_rejected_on_bad_utf8() {
        let mut buf = vec![2u8, 0xFF, 0xFF];
        let mut cursor: &[u8] = &mut buf;
        assert_eq!(cursor.read_string(32).unwrap_err(), CodecError::InvalidUtf8);
    }

    #[test]
    fn bounded_vec_rejects_over_max_before_item_decode() {
        let mut encoded = Vec::new();
        encoded.write_varint(3);
        encoded.extend_from_slice(&[1, 2, 3]);
        let mut cursor = encoded.as_slice();
        let mut decoded = 0;

        assert_eq!(
            read_bounded_vec(&mut cursor, 2, 1, |buf| {
                decoded += 1;
                buf.read_u8()
            })
            .unwrap_err(),
            CodecError::StringTooLong { len: 3, max: 2 }
        );
        assert_eq!(decoded, 0);
    }

    #[test]
    fn bounded_vec_rejects_tiny_body_before_item_decode() {
        let mut encoded = Vec::new();
        encoded.write_varint(100_000);
        encoded.push(0);
        let mut cursor = encoded.as_slice();
        let mut decoded = 0;

        assert!(matches!(
            read_bounded_vec(&mut cursor, 100_000, 4, |buf| {
                decoded += 1;
                buf.read_u8()
            }),
            Err(CodecError::Underflow { .. })
        ));
        assert_eq!(decoded, 0);
    }

    #[test]
    fn bounded_vec_feasibility_arithmetic_does_not_overflow() {
        let mut encoded = Vec::new();
        encoded.write_varint(2);
        let mut cursor = encoded.as_slice();

        assert!(matches!(
            read_bounded_vec::<_, u8, _>(&mut cursor, 2, usize::MAX, |_| unreachable!()),
            Err(CodecError::Underflow {
                needed: usize::MAX,
                available: 0,
            })
        ));
    }

    #[test]
    fn bounded_vec_caps_initial_capacity() {
        let values = bounded_vec::<u64>(1_000_000);
        assert_eq!(values.capacity(), MAX_INITIAL_COLLECTION_CAPACITY);
        assert!(values.is_empty());
    }

    #[test]
    fn bounded_vec_accepts_a_large_complete_collection() {
        let count = MAX_INITIAL_COLLECTION_CAPACITY * 2;
        let mut encoded = Vec::new();
        encoded.write_varint(i32::try_from(count).unwrap());
        encoded.extend(std::iter::repeat_n(7, count));
        let mut cursor = encoded.as_slice();

        let decoded = read_bounded_vec(&mut cursor, count, 1, ReadMc::read_u8).unwrap();
        assert_eq!(decoded.len(), count);
        assert!(decoded.iter().all(|value| *value == 7));
        assert!(cursor.is_empty());
    }

    #[test]
    fn uuid_round_trip() {
        let uuid = Uuid::from_u128(0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210);
        let mut buf = Vec::new();
        buf.write_uuid(uuid);
        assert_eq!(buf.len(), 16);
        let mut cursor: &[u8] = &buf;
        assert_eq!(cursor.read_uuid().unwrap(), uuid);
    }

    #[test]
    fn bool_round_trip() {
        for value in [true, false] {
            let mut buf = Vec::new();
            buf.write_bool(value);
            let mut cursor: &[u8] = &buf;
            assert_eq!(cursor.read_bool().unwrap(), value);
        }
        // Any non-zero byte is `true`.
        let mut cursor: &[u8] = &[0x42];
        assert!(cursor.read_bool().unwrap());
    }

    #[test]
    fn primitive_round_trips() {
        let mut buf = Vec::new();
        buf.write_i8(-7);
        buf.write_u8(200);
        buf.write_i16(-1234);
        buf.write_u16(40_000);
        buf.write_i32(-2_000_000_000);
        buf.write_u32(3_000_000_000);
        buf.write_i64(i64::MIN);
        buf.write_u64(u64::MAX);
        buf.write_f32(core::f32::consts::PI);
        buf.write_f64(core::f64::consts::E);

        let mut cursor: &[u8] = &buf;
        assert_eq!(cursor.read_i8().unwrap(), -7);
        assert_eq!(cursor.read_u8().unwrap(), 200);
        assert_eq!(cursor.read_i16().unwrap(), -1234);
        assert_eq!(cursor.read_u16().unwrap(), 40_000);
        assert_eq!(cursor.read_i32().unwrap(), -2_000_000_000);
        assert_eq!(cursor.read_u32().unwrap(), 3_000_000_000);
        assert_eq!(cursor.read_i64().unwrap(), i64::MIN);
        assert_eq!(cursor.read_u64().unwrap(), u64::MAX);
        assert!((cursor.read_f32().unwrap() - core::f32::consts::PI).abs() < f32::EPSILON);
        assert!((cursor.read_f64().unwrap() - core::f64::consts::E).abs() < f64::EPSILON);
    }

    #[test]
    fn identifier_parses_with_and_without_namespace() {
        let id = Identifier::parse("stone").unwrap();
        assert_eq!(id.namespace(), "minecraft");
        assert_eq!(id.path(), "stone");

        let id = Identifier::parse("solaris:musket").unwrap();
        assert_eq!(id.namespace(), "solaris");
        assert_eq!(id.path(), "musket");

        let id = Identifier::parse("minecraft:block/oak_door").unwrap();
        assert_eq!(id.path(), "block/oak_door");
    }

    #[test]
    fn identifier_rejects_invalid_shapes() {
        for bad in [
            "",
            ":",
            "Solaris:musket",  // uppercase namespace
            "solaris:Musket",  // uppercase path
            "solaris::",       // empty path
            ":foo",            // empty namespace
            "solaris:mu sket", // space in path
        ] {
            assert!(
                Identifier::parse(bad).is_err(),
                "expected {bad:?} to be rejected",
            );
        }
    }

    #[test]
    fn identifier_round_trip() {
        let id = Identifier::parse("solaris:musket").unwrap();
        let mut buf = Vec::new();
        buf.write_identifier(&id).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded = cursor.read_identifier().unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn oversized_identifier_write_returns_error_without_mutating_buffer() {
        let id =
            Identifier::parse(format!("minecraft:{}", "a".repeat(DEFAULT_MAX_STRING_LEN))).unwrap();
        let expected_len = id.as_str().len();
        let mut buf = vec![0xAA];

        assert_eq!(
            buf.write_identifier(&id).unwrap_err(),
            CodecError::StringTooLong {
                len: expected_len,
                max: DEFAULT_MAX_STRING_LEN,
            }
        );
        assert_eq!(buf, vec![0xAA]);
    }

    #[test]
    fn byte_array_round_trip() {
        let data: Vec<u8> = (0..200).map(|i| (i ^ 0x55) as u8).collect();
        let mut buf = Vec::new();
        buf.write_byte_array(&data);
        let mut cursor: &[u8] = &buf;
        let read = cursor.read_byte_array(256).unwrap();
        assert_eq!(read, data);
    }

    // ---- Property tests ---------------------------------------------------

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn proptest_varint_round_trip(v in any::<i32>()) {
            let mut buf = Vec::new();
            buf.write_varint(v);
            prop_assert_eq!(buf.len(), varint_encoded_len(v));
            let mut cursor: &[u8] = &buf;
            prop_assert_eq!(cursor.read_varint().unwrap(), v);
            prop_assert!(cursor.is_empty());

            let (decoded, consumed) = read_varint_partial(&buf).unwrap().unwrap();
            prop_assert_eq!(decoded, v);
            prop_assert_eq!(consumed, buf.len());
        }

        #[test]
        fn proptest_varlong_round_trip(v in any::<i64>()) {
            let mut buf = Vec::new();
            buf.write_varlong(v);
            prop_assert_eq!(buf.len(), varlong_encoded_len(v));
            let mut cursor: &[u8] = &buf;
            prop_assert_eq!(cursor.read_varlong().unwrap(), v);
        }

        #[test]
        fn proptest_string_round_trip(s in "[\\PC]{0,256}") {
            let mut buf = Vec::new();
            buf.write_string(&s, 1024).unwrap();
            let mut cursor: &[u8] = &buf;
            prop_assert_eq!(cursor.read_string(1024).unwrap(), s);
        }

        #[test]
        fn proptest_partial_varint_resumes_on_full_buffer(v in any::<i32>()) {
            let mut buf = Vec::new();
            buf.write_varint(v);
            // Every short prefix is incomplete.
            for split in 0..buf.len() {
                prop_assert_eq!(read_varint_partial(&buf[..split]).unwrap(), None);
            }
            let (decoded, consumed) = read_varint_partial(&buf).unwrap().unwrap();
            prop_assert_eq!(decoded, v);
            prop_assert_eq!(consumed, buf.len());
        }

        #[test]
        fn proptest_uuid_round_trip(hi in any::<u64>(), lo in any::<u64>()) {
            let bytes = ((u128::from(hi)) << 64) | u128::from(lo);
            let uuid = Uuid::from_u128(bytes);
            let mut buf = Vec::new();
            buf.write_uuid(uuid);
            let mut cursor: &[u8] = &buf;
            prop_assert_eq!(cursor.read_uuid().unwrap(), uuid);
        }
    }
}
