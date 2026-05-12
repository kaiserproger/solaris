//! # mc-nbt
//!
//! Solaris' own NBT codec. The Named Binary Tag format used by Minecraft
//! comes in two flavours:
//!
//! - **Named NBT** (the original disk format): every tag carries its
//!   name. The root is `[type:u8][name_len:u16][name…][payload…]`.
//!   This is what region files and `.nbt` files on disk use.
//! - **Network NBT** (since 1.20.2): the root tag's name is stripped —
//!   `[type:u8][payload…]`. This is what packets like `Registry Data`
//!   and `Login (Play)` carry.
//!
//! Both flavours are exposed via [`read_named`] / [`write_named`] and
//! [`read_network`] / [`write_network`]; the payload format is shared.
//!
//! String encoding: Mojang's spec calls for *Modified UTF-8* (a.k.a.
//! CESU-8). For ASCII strings — which is what every key and most values
//! in vanilla data are — Modified UTF-8 is identical to UTF-8 and our
//! implementation interoperates correctly. Non-ASCII BMP characters
//! also pass through unchanged. The two divergences are the `NUL`
//! character (vanilla encodes it as `0xC0 0x80`; we emit a plain `0x00`)
//! and supplementary characters above `U+FFFF` (vanilla emits a CESU-8
//! surrogate pair; we emit standard UTF-8 four-byte sequences). Neither
//! case turns up in any vanilla data file Solaris currently reads, but
//! they will need fixing before we support user-supplied text content.
//! Flipping both paths to strict MUTF-8 is a tracked follow-up; it has
//! no caller today.

use bytes::{Buf, BufMut};
use thiserror::Error;

// -----------------------------------------------------------------------
// Tag types
// -----------------------------------------------------------------------

/// Tag-type discriminator on the wire.
pub mod tag_type {
    pub const END: u8 = 0;
    pub const BYTE: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const INT: u8 = 3;
    pub const LONG: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const BYTE_ARRAY: u8 = 7;
    pub const STRING: u8 = 8;
    pub const LIST: u8 = 9;
    pub const COMPOUND: u8 = 10;
    pub const INT_ARRAY: u8 = 11;
    pub const LONG_ARRAY: u8 = 12;
}

/// A typed NBT value.
///
/// Compounds preserve insertion order — vanilla treats compound order
/// as ordered for hashing/equality purposes in some places, so emitting
/// them in the same order we read them is the safer default.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(ListTag),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// The on-wire tag-type byte for this value.
    #[must_use]
    pub fn type_id(&self) -> u8 {
        match self {
            Self::Byte(_) => tag_type::BYTE,
            Self::Short(_) => tag_type::SHORT,
            Self::Int(_) => tag_type::INT,
            Self::Long(_) => tag_type::LONG,
            Self::Float(_) => tag_type::FLOAT,
            Self::Double(_) => tag_type::DOUBLE,
            Self::ByteArray(_) => tag_type::BYTE_ARRAY,
            Self::String(_) => tag_type::STRING,
            Self::List(_) => tag_type::LIST,
            Self::Compound(_) => tag_type::COMPOUND,
            Self::IntArray(_) => tag_type::INT_ARRAY,
            Self::LongArray(_) => tag_type::LONG_ARRAY,
        }
    }
}

/// A homogenously-typed list. All elements share `element_type`; for an
/// empty list `element_type` is conventionally `End` (0) in vanilla.
#[derive(Debug, Clone, PartialEq)]
pub struct ListTag {
    pub element_type: u8,
    pub elements: Vec<Tag>,
}

impl ListTag {
    /// Empty list with the conventional `End` element type.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            element_type: tag_type::END,
            elements: Vec::new(),
        }
    }
}

// -----------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------

/// Hard ceiling we apply to every length-prefixed array (`ByteArray`,
/// `String`, `List`, `IntArray`, `LongArray`). Picked generously enough
/// that no legitimate NBT we care about gets near it; tight enough that
/// a malicious peer cannot make us allocate gigabytes.
pub const MAX_NBT_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum NbtError {
    #[error("ran out of NBT bytes (needed {needed} more, had {available})")]
    Underflow { needed: usize, available: usize },

    #[error("expected a root Compound tag, got tag-id {0}")]
    RootMustBeCompound(u8),

    #[error("unknown tag-id {0}")]
    UnknownTag(u8),

    #[error("NBT array of length {0} exceeds the {MAX_NBT_LENGTH}-byte ceiling")]
    ArrayTooLong(i64),

    #[error("negative NBT array length: {0}")]
    NegativeLength(i32),

    #[error("string is not valid UTF-8")]
    InvalidString,

    #[error("non-Compound type {0:#x} used where a Compound is required")]
    NotCompound(u8),

    #[error("list of {parent:#x}s contains an element of incompatible type {found:#x}")]
    HeterogeneousList { parent: u8, found: u8 },
}

// -----------------------------------------------------------------------
// Reader / writer
// -----------------------------------------------------------------------

fn ensure_remaining<B: Buf + ?Sized>(buf: &B, needed: usize) -> Result<(), NbtError> {
    let available = buf.remaining();
    if available < needed {
        return Err(NbtError::Underflow {
            needed: needed - available,
            available,
        });
    }
    Ok(())
}

fn read_string<B: Buf>(buf: &mut B) -> Result<String, NbtError> {
    ensure_remaining(buf, 2)?;
    let len = buf.get_u16() as usize;
    ensure_remaining(buf, len)?;
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    String::from_utf8(bytes).map_err(|_| NbtError::InvalidString)
}

fn write_string<B: BufMut>(buf: &mut B, s: &str) -> Result<(), NbtError> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len()).map_err(|_| NbtError::ArrayTooLong(bytes.len() as i64))?;
    buf.put_u16(len);
    buf.put_slice(bytes);
    Ok(())
}

fn read_length<B: Buf>(buf: &mut B) -> Result<usize, NbtError> {
    ensure_remaining(buf, 4)?;
    let len = buf.get_i32();
    if len < 0 {
        return Err(NbtError::NegativeLength(len));
    }
    let len = len as usize;
    if len > MAX_NBT_LENGTH {
        return Err(NbtError::ArrayTooLong(len as i64));
    }
    Ok(len)
}

fn read_payload<B: Buf>(buf: &mut B, tag_id: u8) -> Result<Tag, NbtError> {
    match tag_id {
        tag_type::BYTE => {
            ensure_remaining(buf, 1)?;
            Ok(Tag::Byte(buf.get_i8()))
        }
        tag_type::SHORT => {
            ensure_remaining(buf, 2)?;
            Ok(Tag::Short(buf.get_i16()))
        }
        tag_type::INT => {
            ensure_remaining(buf, 4)?;
            Ok(Tag::Int(buf.get_i32()))
        }
        tag_type::LONG => {
            ensure_remaining(buf, 8)?;
            Ok(Tag::Long(buf.get_i64()))
        }
        tag_type::FLOAT => {
            ensure_remaining(buf, 4)?;
            Ok(Tag::Float(buf.get_f32()))
        }
        tag_type::DOUBLE => {
            ensure_remaining(buf, 8)?;
            Ok(Tag::Double(buf.get_f64()))
        }
        tag_type::BYTE_ARRAY => {
            let len = read_length(buf)?;
            ensure_remaining(buf, len)?;
            let mut data = vec![0i8; len];
            // copy_to_slice wants u8; reinterpret via from_raw_parts is
            // unsafe and forbidden here, so do the cast element-by-element.
            for slot in &mut data {
                *slot = buf.get_i8();
            }
            Ok(Tag::ByteArray(data))
        }
        tag_type::STRING => Ok(Tag::String(read_string(buf)?)),
        tag_type::LIST => {
            ensure_remaining(buf, 1)?;
            let element_type = buf.get_u8();
            let len = read_length(buf)?;
            if element_type == tag_type::END && len > 0 {
                return Err(NbtError::HeterogeneousList {
                    parent: tag_type::LIST,
                    found: tag_type::END,
                });
            }
            let mut elements = Vec::with_capacity(len);
            for _ in 0..len {
                elements.push(read_payload(buf, element_type)?);
            }
            Ok(Tag::List(ListTag {
                element_type,
                elements,
            }))
        }
        tag_type::COMPOUND => read_compound_payload(buf),
        tag_type::INT_ARRAY => {
            let len = read_length(buf)?;
            ensure_remaining(buf, len.saturating_mul(4))?;
            let mut data = Vec::with_capacity(len);
            for _ in 0..len {
                data.push(buf.get_i32());
            }
            Ok(Tag::IntArray(data))
        }
        tag_type::LONG_ARRAY => {
            let len = read_length(buf)?;
            ensure_remaining(buf, len.saturating_mul(8))?;
            let mut data = Vec::with_capacity(len);
            for _ in 0..len {
                data.push(buf.get_i64());
            }
            Ok(Tag::LongArray(data))
        }
        other => Err(NbtError::UnknownTag(other)),
    }
}

fn read_compound_payload<B: Buf>(buf: &mut B) -> Result<Tag, NbtError> {
    let mut entries = Vec::new();
    loop {
        ensure_remaining(buf, 1)?;
        let tag_id = buf.get_u8();
        if tag_id == tag_type::END {
            return Ok(Tag::Compound(entries));
        }
        let name = read_string(buf)?;
        let value = read_payload(buf, tag_id)?;
        entries.push((name, value));
    }
}

fn write_payload<B: BufMut>(buf: &mut B, tag: &Tag) -> Result<(), NbtError> {
    match tag {
        Tag::Byte(v) => buf.put_i8(*v),
        Tag::Short(v) => buf.put_i16(*v),
        Tag::Int(v) => buf.put_i32(*v),
        Tag::Long(v) => buf.put_i64(*v),
        Tag::Float(v) => buf.put_f32(*v),
        Tag::Double(v) => buf.put_f64(*v),
        Tag::ByteArray(data) => {
            let len =
                i32::try_from(data.len()).map_err(|_| NbtError::ArrayTooLong(data.len() as i64))?;
            buf.put_i32(len);
            for v in data {
                buf.put_i8(*v);
            }
        }
        Tag::String(s) => write_string(buf, s)?,
        Tag::List(list) => {
            buf.put_u8(list.element_type);
            let len = i32::try_from(list.elements.len())
                .map_err(|_| NbtError::ArrayTooLong(list.elements.len() as i64))?;
            buf.put_i32(len);
            for element in &list.elements {
                if element.type_id() != list.element_type {
                    return Err(NbtError::HeterogeneousList {
                        parent: list.element_type,
                        found: element.type_id(),
                    });
                }
                write_payload(buf, element)?;
            }
        }
        Tag::Compound(entries) => {
            for (name, value) in entries {
                buf.put_u8(value.type_id());
                write_string(buf, name)?;
                write_payload(buf, value)?;
            }
            buf.put_u8(tag_type::END);
        }
        Tag::IntArray(data) => {
            let len =
                i32::try_from(data.len()).map_err(|_| NbtError::ArrayTooLong(data.len() as i64))?;
            buf.put_i32(len);
            for v in data {
                buf.put_i32(*v);
            }
        }
        Tag::LongArray(data) => {
            let len =
                i32::try_from(data.len()).map_err(|_| NbtError::ArrayTooLong(data.len() as i64))?;
            buf.put_i32(len);
            for v in data {
                buf.put_i64(*v);
            }
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------
// Public entry points
// -----------------------------------------------------------------------

/// Read a *network-format* NBT root. The root must be a [`Tag::Compound`];
/// modern protocol packets always wrap data in one.
pub fn read_network<B: Buf>(buf: &mut B) -> Result<Tag, NbtError> {
    ensure_remaining(buf, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    read_compound_payload(buf)
}

/// Write a *network-format* NBT root. `root` must be a [`Tag::Compound`].
pub fn write_network<B: BufMut>(buf: &mut B, root: &Tag) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    buf.put_u8(tag_type::COMPOUND);
    write_payload(buf, root)
}

/// Read a *named* NBT root: the disk format used by region files and
/// `.nbt` files. Returns the root tag's name alongside its value.
pub fn read_named<B: Buf>(buf: &mut B) -> Result<(String, Tag), NbtError> {
    ensure_remaining(buf, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    let name = read_string(buf)?;
    let value = read_compound_payload(buf)?;
    Ok((name, value))
}

/// Write a *named* NBT root.
pub fn write_named<B: BufMut>(buf: &mut B, name: &str, root: &Tag) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    buf.put_u8(tag_type::COMPOUND);
    write_string(buf, name)?;
    write_payload(buf, root)
}

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn round_trip_network(tag: Tag) {
        let mut buf = Vec::new();
        write_network(&mut buf, &tag).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded = read_network(&mut cursor).unwrap();
        assert_eq!(decoded, tag);
        assert!(cursor.is_empty(), "all bytes consumed");
    }

    fn round_trip_named(name: &str, tag: Tag) {
        let mut buf = Vec::new();
        write_named(&mut buf, name, &tag).unwrap();
        let mut cursor: &[u8] = &buf;
        let (decoded_name, decoded) = read_named(&mut cursor).unwrap();
        assert_eq!(decoded_name, name);
        assert_eq!(decoded, tag);
        assert!(cursor.is_empty());
    }

    #[test]
    fn empty_compound_round_trip() {
        round_trip_network(Tag::Compound(vec![]));
        round_trip_named("", Tag::Compound(vec![]));
        round_trip_named("hello", Tag::Compound(vec![]));
    }

    #[test]
    fn primitives_round_trip() {
        round_trip_network(Tag::Compound(vec![
            ("b".into(), Tag::Byte(-1)),
            ("s".into(), Tag::Short(-32_000)),
            ("i".into(), Tag::Int(0x7FFF_FFFF)),
            ("l".into(), Tag::Long(i64::MIN)),
            ("f".into(), Tag::Float(core::f32::consts::PI)),
            ("d".into(), Tag::Double(core::f64::consts::E)),
        ]));
    }

    #[test]
    fn strings_round_trip() {
        round_trip_network(Tag::Compound(vec![
            ("ascii".into(), Tag::String("hello, world".into())),
            ("bmp".into(), Tag::String("ümlauts, шифры, 漢字".into())),
            ("empty".into(), Tag::String(String::new())),
        ]));
    }

    #[test]
    fn arrays_round_trip() {
        round_trip_network(Tag::Compound(vec![
            (
                "ba".into(),
                Tag::ByteArray((0..50).map(|i| i as i8).collect()),
            ),
            (
                "ia".into(),
                Tag::IntArray((0..30).map(|i| i * 1_000_000 - 15_000_000).collect()),
            ),
            (
                "la".into(),
                // Span both signs, cover MIN/MAX, but stay inside i64.
                Tag::LongArray(vec![
                    0,
                    1,
                    -1,
                    i64::MIN,
                    i64::MAX,
                    i64::MAX / 19,
                    -(i64::MAX / 7),
                ]),
            ),
        ]));
    }

    #[test]
    fn list_round_trip() {
        round_trip_network(Tag::Compound(vec![
            (
                "ints".into(),
                Tag::List(ListTag {
                    element_type: tag_type::INT,
                    elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
                }),
            ),
            ("empty".into(), Tag::List(ListTag::empty())),
            (
                "nested_compounds".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![
                        Tag::Compound(vec![("k".into(), Tag::Byte(1))]),
                        Tag::Compound(vec![("k".into(), Tag::Byte(2))]),
                    ],
                }),
            ),
        ]));
    }

    #[test]
    fn deeply_nested_compound() {
        let leaf = Tag::String("hi".into());
        let mut tag = Tag::Compound(vec![("leaf".into(), leaf)]);
        for i in 0..32 {
            tag = Tag::Compound(vec![(format!("level{i}"), tag)]);
        }
        round_trip_network(tag);
    }

    #[test]
    fn write_rejects_heterogeneous_list() {
        let bad = Tag::Compound(vec![(
            "k".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(1), Tag::Byte(2)],
            }),
        )]);
        let mut buf = Vec::new();
        let err = write_network(&mut buf, &bad).unwrap_err();
        assert!(matches!(err, NbtError::HeterogeneousList { .. }));
    }

    #[test]
    fn write_rejects_non_compound_root() {
        let mut buf = Vec::new();
        let err = write_network(&mut buf, &Tag::Int(42)).unwrap_err();
        assert!(matches!(err, NbtError::RootMustBeCompound(_)));
    }

    #[test]
    fn read_rejects_non_compound_root() {
        let buf: &[u8] = &[tag_type::INT, 0, 0, 0, 0]; // tag-id 3, four payload bytes
        let mut cursor = buf;
        let err = read_network(&mut cursor).unwrap_err();
        assert!(matches!(err, NbtError::NotCompound(tag_type::INT)));
    }

    #[test]
    fn read_rejects_negative_length() {
        // type=Compound, then BYTE_ARRAY field "x" with length=-1
        let mut buf: Vec<u8> = vec![tag_type::COMPOUND, tag_type::BYTE_ARRAY, 0, 1, b'x'];
        buf.extend_from_slice(&(-1i32).to_be_bytes());
        let mut cursor: &[u8] = &buf;
        let err = read_network(&mut cursor).unwrap_err();
        assert!(matches!(err, NbtError::NegativeLength(-1)));
    }

    #[test]
    fn read_rejects_oversized_length() {
        // BYTE_ARRAY field with length just over the ceiling.
        let mut buf: Vec<u8> = vec![tag_type::COMPOUND, tag_type::BYTE_ARRAY, 0, 1, b'x'];
        buf.extend_from_slice(&((MAX_NBT_LENGTH as i32 + 1).to_be_bytes()));
        let mut cursor: &[u8] = &buf;
        let err = read_network(&mut cursor).unwrap_err();
        assert!(matches!(err, NbtError::ArrayTooLong(_)));
    }

    #[test]
    fn read_rejects_truncated_buffer() {
        // Compound header but no payload bytes follow.
        let buf: &[u8] = &[tag_type::COMPOUND, tag_type::INT, 0, 1, b'x'];
        let mut cursor = buf;
        assert!(matches!(
            read_network(&mut cursor),
            Err(NbtError::Underflow { .. })
        ));
    }

    // ---- Property tests ---------------------------------------------------

    fn arb_simple_tag() -> impl Strategy<Value = Tag> {
        prop_oneof![
            any::<i8>().prop_map(Tag::Byte),
            any::<i16>().prop_map(Tag::Short),
            any::<i32>().prop_map(Tag::Int),
            any::<i64>().prop_map(Tag::Long),
            // Use `from_bits` to also exercise NaN/infinity values.
            any::<u32>().prop_map(|b| Tag::Float(f32::from_bits(b))),
            any::<u64>().prop_map(|b| Tag::Double(f64::from_bits(b))),
            "[\\x20-\\x7E]{0,128}".prop_map(Tag::String),
            proptest::collection::vec(any::<i8>(), 0..256).prop_map(Tag::ByteArray),
            proptest::collection::vec(any::<i32>(), 0..128).prop_map(Tag::IntArray),
            proptest::collection::vec(any::<i64>(), 0..128).prop_map(Tag::LongArray),
        ]
    }

    fn arb_compound() -> impl Strategy<Value = Tag> {
        proptest::collection::vec(("[a-z][a-z_0-9]{0,16}", arb_simple_tag()), 0..16)
            .prop_map(Tag::Compound)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn proptest_compound_round_trip(tag in arb_compound()) {
            // Re-implement round_trip_network inline so we can compare with
            // bitwise equality for floats (NaN != NaN otherwise).
            let mut buf = Vec::new();
            write_network(&mut buf, &tag).unwrap();
            let mut cursor: &[u8] = &buf;
            let decoded = read_network(&mut cursor).unwrap();
            prop_assert!(tags_bitwise_eq(&decoded, &tag));
            prop_assert!(cursor.is_empty());
        }

        #[test]
        fn proptest_named_round_trip(
            name in "[a-z]{0,32}",
            tag in arb_compound(),
        ) {
            let mut buf = Vec::new();
            write_named(&mut buf, &name, &tag).unwrap();
            let mut cursor: &[u8] = &buf;
            let (decoded_name, decoded) = read_named(&mut cursor).unwrap();
            prop_assert_eq!(decoded_name, name);
            prop_assert!(tags_bitwise_eq(&decoded, &tag));
        }
    }

    /// Compare tags with bitwise float equality so NaN round-trips do
    /// not spuriously fail.
    fn tags_bitwise_eq(a: &Tag, b: &Tag) -> bool {
        match (a, b) {
            (Tag::Float(x), Tag::Float(y)) => x.to_bits() == y.to_bits(),
            (Tag::Double(x), Tag::Double(y)) => x.to_bits() == y.to_bits(),
            (Tag::List(x), Tag::List(y)) => {
                x.element_type == y.element_type
                    && x.elements.len() == y.elements.len()
                    && x.elements
                        .iter()
                        .zip(&y.elements)
                        .all(|(p, q)| tags_bitwise_eq(p, q))
            }
            (Tag::Compound(x), Tag::Compound(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .zip(y)
                        .all(|((kx, vx), (ky, vy))| kx == ky && tags_bitwise_eq(vx, vy))
            }
            _ => a == b,
        }
    }
}
