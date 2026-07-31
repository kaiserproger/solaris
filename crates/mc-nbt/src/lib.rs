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
//! NBT strings use Java's Modified UTF-8: `NUL` is encoded as `C0 80`,
//! and supplementary Unicode characters are encoded as UTF-16 surrogate
//! pairs with three bytes per code unit.

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
/// them in the same order we read them is the safer default. Duplicate names
/// are preserved rather than collapsed; every occurrence counts toward parser
/// and writer budgets, and stricter duplicate semantics remain caller-owned.
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

/// Hard ceiling for array payloads and list backing allocations.
/// Strings have their own `u16` wire-length limit.
pub const MAX_NBT_LENGTH: usize = 16 * 1024 * 1024;

/// Vanilla rejects container nesting beyond 512 levels.
pub const MAX_NBT_DEPTH: usize = 512;

/// Aggregate encoded bytes accepted or emitted for one NBT root.
pub const MAX_NBT_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Aggregate decoded tag-node ceiling for one NBT root.
pub const MAX_NBT_NODES: usize = 1_048_576;
/// Per-compound entry ceiling. Duplicate names are preserved and count separately.
pub const MAX_NBT_COMPOUND_ENTRIES: usize = 65_536;
/// Aggregate compound-entry ceiling across one NBT root.
pub const MAX_NBT_TOTAL_COMPOUND_ENTRIES: usize = 1_048_576;
/// Aggregate Modified UTF-8 payload bytes across names and string values.
pub const MAX_NBT_STRING_BYTES: usize = MAX_NBT_LENGTH;
/// Aggregate heap-allocation estimate for one decoded NBT root.
pub const MAX_NBT_ALLOCATION_BYTES: usize = 64 * 1024 * 1024;

const COMPOUND_RESERVE_CHUNK: usize = 64;
const FRAME_STACK_CAPACITY: usize = MAX_NBT_DEPTH * 2 + 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NbtError {
    #[error("ran out of NBT bytes (needed {needed} more, had {available})")]
    Underflow { needed: usize, available: usize },

    #[error("expected a root Compound tag, got tag-id {0}")]
    RootMustBeCompound(u8),

    #[error("unknown tag-id {0}")]
    UnknownTag(u8),

    #[error("NBT collection payload of {0} bytes exceeds the {MAX_NBT_LENGTH}-byte ceiling")]
    ArrayTooLong(i64),

    #[error("Modified UTF-8 string payload of {0} bytes exceeds the u16 wire-length limit")]
    StringTooLong(usize),

    #[error("negative NBT array length: {0}")]
    NegativeLength(i32),

    #[error("NBT container nesting exceeds the {MAX_NBT_DEPTH}-level limit")]
    NestingTooDeep,

    #[error("NBT {resource} budget exceeds the configured limit of {limit}")]
    BudgetExceeded {
        resource: &'static str,
        limit: usize,
    },

    #[error("failed to reserve {bytes} byte(s) for NBT {resource}")]
    AllocationFailed {
        resource: &'static str,
        bytes: usize,
    },

    #[error("string is not valid Modified UTF-8")]
    InvalidString,

    #[error("non-Compound type {0:#x} used where a Compound is required")]
    NotCompound(u8),

    #[error("list of {parent:#x}s contains an element of incompatible type {found:#x}")]
    HeterogeneousList { parent: u8, found: u8 },
}

#[derive(Debug, Clone, Copy)]
struct NbtLimits {
    wire_bytes: usize,
    nodes: usize,
    compound_entries: usize,
    string_bytes: usize,
    allocation_bytes: usize,
}

const PRODUCTION_LIMITS: NbtLimits = NbtLimits {
    wire_bytes: MAX_NBT_TOTAL_BYTES,
    nodes: MAX_NBT_NODES,
    compound_entries: MAX_NBT_TOTAL_COMPOUND_ENTRIES,
    string_bytes: MAX_NBT_STRING_BYTES,
    allocation_bytes: MAX_NBT_ALLOCATION_BYTES,
};

#[derive(Debug)]
struct NbtBudget {
    limits: NbtLimits,
    wire_bytes: usize,
    nodes: usize,
    compound_entries: usize,
    string_bytes: usize,
    allocation_bytes: usize,
}

impl NbtBudget {
    fn production() -> Self {
        Self::new(PRODUCTION_LIMITS)
    }

    fn new(limits: NbtLimits) -> Self {
        Self {
            limits,
            wire_bytes: 0,
            nodes: 0,
            compound_entries: 0,
            string_bytes: 0,
            allocation_bytes: 0,
        }
    }

    fn charge_wire(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.wire_bytes,
            amount,
            self.limits.wire_bytes,
            "wire-byte",
        )
    }

    fn charge_node(&mut self) -> Result<(), NbtError> {
        charge_budget(&mut self.nodes, 1, self.limits.nodes, "node")
    }

    fn charge_compound_entry(&mut self) -> Result<(), NbtError> {
        charge_budget(
            &mut self.compound_entries,
            1,
            self.limits.compound_entries,
            "compound-entry",
        )
    }

    fn charge_string(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.string_bytes,
            amount,
            self.limits.string_bytes,
            "string-byte",
        )
    }

    fn charge_allocation(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.allocation_bytes,
            amount,
            self.limits.allocation_bytes,
            "allocation-byte",
        )
    }
}

fn charge_budget(
    used: &mut usize,
    amount: usize,
    limit: usize,
    resource: &'static str,
) -> Result<(), NbtError> {
    let next = used
        .checked_add(amount)
        .ok_or(NbtError::BudgetExceeded { resource, limit })?;
    if next > limit {
        return Err(NbtError::BudgetExceeded { resource, limit });
    }
    *used = next;
    Ok(())
}

fn capacity_bytes<T>(capacity: usize, limit: usize) -> Result<usize, NbtError> {
    capacity
        .checked_mul(size_of::<T>())
        .ok_or(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            limit,
        })
}

fn budgeted_vec_with_capacity<T>(
    capacity: usize,
    budget: &mut NbtBudget,
    resource: &'static str,
) -> Result<Vec<T>, NbtError> {
    if capacity == 0 {
        return Ok(Vec::new());
    }
    let requested_bytes = capacity_bytes::<T>(capacity, budget.limits.allocation_bytes)?;
    budget.charge_allocation(requested_bytes)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| NbtError::AllocationFailed {
            resource,
            bytes: requested_bytes,
        })?;
    if values.capacity() > capacity {
        budget.charge_allocation(capacity_bytes::<T>(
            values.capacity() - capacity,
            budget.limits.allocation_bytes,
        )?)?;
    }
    Ok(values)
}

fn budgeted_string_with_capacity(
    capacity: usize,
    budget: &mut NbtBudget,
    resource: &'static str,
) -> Result<String, NbtError> {
    if capacity == 0 {
        return Ok(String::new());
    }
    budget.charge_allocation(capacity)?;
    let mut value = String::new();
    value
        .try_reserve_exact(capacity)
        .map_err(|_| NbtError::AllocationFailed {
            resource,
            bytes: capacity,
        })?;
    if value.capacity() > capacity {
        budget.charge_allocation(value.capacity() - capacity)?;
    }
    Ok(value)
}

fn ensure_budgeted_compound_capacity(
    entries: &mut Vec<(String, Tag)>,
    budget: &mut NbtBudget,
) -> Result<(), NbtError> {
    if entries.len() < entries.capacity() {
        return Ok(());
    }
    let remaining = MAX_NBT_COMPOUND_ENTRIES.saturating_sub(entries.capacity());
    let additional = remaining.clamp(1, COMPOUND_RESERVE_CHUNK);
    let requested_bytes =
        capacity_bytes::<(String, Tag)>(additional, budget.limits.allocation_bytes)?;
    budget.charge_allocation(requested_bytes)?;
    let old_capacity = entries.capacity();
    entries
        .try_reserve_exact(additional)
        .map_err(|_| NbtError::AllocationFailed {
            resource: "compound entries",
            bytes: requested_bytes,
        })?;
    let requested_capacity = old_capacity.saturating_add(additional);
    if entries.capacity() > requested_capacity {
        budget.charge_allocation(capacity_bytes::<(String, Tag)>(
            entries.capacity() - requested_capacity,
            budget.limits.allocation_bytes,
        )?)?;
    }
    Ok(())
}

fn push_bounded<T>(stack: &mut Vec<T>, value: T) -> Result<(), NbtError> {
    if stack.len() == stack.capacity() {
        return Err(NbtError::BudgetExceeded {
            resource: "frame",
            limit: stack.capacity(),
        });
    }
    stack.push(value);
    Ok(())
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

fn consume<B: Buf + ?Sized>(buf: &B, budget: &mut NbtBudget, bytes: usize) -> Result<(), NbtError> {
    ensure_remaining(buf, bytes)?;
    budget.charge_wire(bytes)
}

fn read_string_with_budget<B: Buf>(
    buf: &mut B,
    budget: &mut NbtBudget,
) -> Result<String, NbtError> {
    consume(buf, budget, 2)?;
    let len = buf.get_u16() as usize;
    consume(buf, budget, len)?;
    budget.charge_string(len)?;

    let mut bytes = budgeted_vec_with_capacity(len, budget, "string input")?;
    bytes.resize(len, 0);
    buf.copy_to_slice(&mut bytes);

    let mut code_units = budgeted_vec_with_capacity(len, budget, "string code units")?;
    let mut offset = 0;
    while offset < bytes.len() {
        let first = bytes[offset];
        match first {
            0x01..=0x7F => {
                code_units.push(u16::from(first));
                offset += 1;
            }
            0xC0..=0xDF => {
                let second = *bytes.get(offset + 1).ok_or(NbtError::InvalidString)?;
                if second & 0xC0 != 0x80 {
                    return Err(NbtError::InvalidString);
                }

                let code_unit = (u16::from(first & 0x1F) << 6) | u16::from(second & 0x3F);
                if code_unit == 0 {
                    if first != 0xC0 || second != 0x80 {
                        return Err(NbtError::InvalidString);
                    }
                } else if code_unit < 0x80 {
                    return Err(NbtError::InvalidString);
                }

                code_units.push(code_unit);
                offset += 2;
            }
            0xE0..=0xEF => {
                let second = *bytes.get(offset + 1).ok_or(NbtError::InvalidString)?;
                let third = *bytes.get(offset + 2).ok_or(NbtError::InvalidString)?;
                if second & 0xC0 != 0x80 || third & 0xC0 != 0x80 {
                    return Err(NbtError::InvalidString);
                }

                let code_unit = (u16::from(first & 0x0F) << 12)
                    | (u16::from(second & 0x3F) << 6)
                    | u16::from(third & 0x3F);
                if code_unit < 0x800 {
                    return Err(NbtError::InvalidString);
                }

                code_units.push(code_unit);
                offset += 3;
            }
            _ => return Err(NbtError::InvalidString),
        }
    }

    let mut decoded = budgeted_string_with_capacity(len, budget, "decoded string")?;
    for scalar in char::decode_utf16(code_units) {
        decoded.push(scalar.map_err(|_| NbtError::InvalidString)?);
    }
    Ok(decoded)
}

#[cfg(test)]
fn read_string<B: Buf>(buf: &mut B) -> Result<String, NbtError> {
    let mut budget = NbtBudget::production();
    read_string_with_budget(buf, &mut budget)
}

fn modified_utf8_len(s: &str) -> Result<usize, NbtError> {
    s.encode_utf16().try_fold(0usize, |total, code_unit| {
        let encoded = match code_unit {
            0x0001..=0x007F => 1usize,
            0x0000..=0x07FF => 2,
            _ => 3,
        };
        total
            .checked_add(encoded)
            .ok_or(NbtError::StringTooLong(usize::MAX))
    })
}

fn write_string<B: BufMut>(buf: &mut B, s: &str) -> Result<(), NbtError> {
    let encoded_len = modified_utf8_len(s)?;
    let len = u16::try_from(encoded_len).map_err(|_| NbtError::StringTooLong(encoded_len))?;
    buf.put_u16(len);

    for code_unit in s.encode_utf16() {
        match code_unit {
            0x0001..=0x007F => buf.put_u8(code_unit as u8),
            0x0000..=0x07FF => {
                buf.put_u8(0xC0 | ((code_unit >> 6) as u8));
                buf.put_u8(0x80 | ((code_unit & 0x3F) as u8));
            }
            _ => {
                buf.put_u8(0xE0 | ((code_unit >> 12) as u8));
                buf.put_u8(0x80 | (((code_unit >> 6) & 0x3F) as u8));
                buf.put_u8(0x80 | ((code_unit & 0x3F) as u8));
            }
        }
    }

    Ok(())
}

fn collection_payload_bytes(len: usize, element_size: usize) -> Result<usize, NbtError> {
    let bytes = len
        .checked_mul(element_size)
        .ok_or(NbtError::ArrayTooLong(i64::MAX))?;
    if bytes > MAX_NBT_LENGTH {
        return Err(NbtError::ArrayTooLong(
            i64::try_from(bytes).unwrap_or(i64::MAX),
        ));
    }
    Ok(bytes)
}

fn read_length<B: Buf>(
    buf: &mut B,
    budget: &mut NbtBudget,
    element_size: usize,
) -> Result<usize, NbtError> {
    consume(buf, budget, 4)?;
    let len = buf.get_i32();
    if len < 0 {
        return Err(NbtError::NegativeLength(len));
    }
    let len = len as usize;
    collection_payload_bytes(len, element_size)?;
    Ok(len)
}

fn write_length<B: BufMut>(buf: &mut B, len: usize, element_size: usize) -> Result<(), NbtError> {
    collection_payload_bytes(len, element_size)?;
    let len = i32::try_from(len).map_err(|_| NbtError::ArrayTooLong(i64::MAX))?;
    buf.put_i32(len);
    Ok(())
}

fn ensure_container_depth(depth: usize) -> Result<(), NbtError> {
    if depth > MAX_NBT_DEPTH {
        return Err(NbtError::NestingTooDeep);
    }
    Ok(())
}

fn read_scalar_payload<B: Buf>(
    buf: &mut B,
    tag_id: u8,
    budget: &mut NbtBudget,
) -> Result<Tag, NbtError> {
    if !(tag_type::BYTE..=tag_type::LONG_ARRAY).contains(&tag_id) {
        return Err(NbtError::UnknownTag(tag_id));
    }
    budget.charge_node()?;
    match tag_id {
        tag_type::BYTE => {
            consume(buf, budget, 1)?;
            Ok(Tag::Byte(buf.get_i8()))
        }
        tag_type::SHORT => {
            consume(buf, budget, 2)?;
            Ok(Tag::Short(buf.get_i16()))
        }
        tag_type::INT => {
            consume(buf, budget, 4)?;
            Ok(Tag::Int(buf.get_i32()))
        }
        tag_type::LONG => {
            consume(buf, budget, 8)?;
            Ok(Tag::Long(buf.get_i64()))
        }
        tag_type::FLOAT => {
            consume(buf, budget, 4)?;
            Ok(Tag::Float(buf.get_f32()))
        }
        tag_type::DOUBLE => {
            consume(buf, budget, 8)?;
            Ok(Tag::Double(buf.get_f64()))
        }
        tag_type::BYTE_ARRAY => {
            let len = read_length(buf, budget, size_of::<i8>())?;
            consume(buf, budget, len)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "byte array")?;
            data.resize(len, 0);
            // copy_to_slice wants u8; reinterpret via from_raw_parts is
            // unsafe and forbidden here, so do the cast element-by-element.
            for slot in &mut data {
                *slot = buf.get_i8();
            }
            Ok(Tag::ByteArray(data))
        }
        tag_type::STRING => Ok(Tag::String(read_string_with_budget(buf, budget)?)),
        tag_type::LIST => unreachable!("containers use the explicit-stack reader"),
        tag_type::COMPOUND => unreachable!("containers use the explicit-stack reader"),
        tag_type::INT_ARRAY => {
            let len = read_length(buf, budget, size_of::<i32>())?;
            let bytes = collection_payload_bytes(len, size_of::<i32>())?;
            consume(buf, budget, bytes)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "int array")?;
            for _ in 0..len {
                data.push(buf.get_i32());
            }
            Ok(Tag::IntArray(data))
        }
        tag_type::LONG_ARRAY => {
            let len = read_length(buf, budget, size_of::<i64>())?;
            let bytes = collection_payload_bytes(len, size_of::<i64>())?;
            consume(buf, budget, bytes)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "long array")?;
            for _ in 0..len {
                data.push(buf.get_i64());
            }
            Ok(Tag::LongArray(data))
        }
        _ => unreachable!("tag id range was checked above"),
    }
}

fn minimum_payload_bytes(tag_id: u8) -> Option<usize> {
    match tag_id {
        tag_type::BYTE => Some(1),
        tag_type::SHORT => Some(2),
        tag_type::INT | tag_type::FLOAT => Some(4),
        tag_type::LONG | tag_type::DOUBLE => Some(8),
        tag_type::BYTE_ARRAY | tag_type::INT_ARRAY | tag_type::LONG_ARRAY => Some(4),
        tag_type::STRING => Some(2),
        tag_type::LIST => Some(5),
        tag_type::COMPOUND => Some(1),
        _ => None,
    }
}

fn ensure_list_feasible<B: Buf>(buf: &B, len: usize, element_type: u8) -> Result<(), NbtError> {
    if len == 0 {
        return Ok(());
    }
    let minimum = minimum_payload_bytes(element_type).ok_or(NbtError::UnknownTag(element_type))?;
    let available = buf.remaining();
    if len > available / minimum {
        let required = len.saturating_mul(minimum);
        return Err(NbtError::Underflow {
            needed: required.saturating_sub(available),
            available,
        });
    }
    Ok(())
}

enum ReadFrame {
    Value {
        tag_id: u8,
        depth: usize,
    },
    List {
        element_type: u8,
        remaining: usize,
        elements: Vec<Tag>,
        depth: usize,
    },
    ListValue {
        element_type: u8,
        remaining: usize,
        elements: Vec<Tag>,
        depth: usize,
    },
    Compound {
        entries: Vec<(String, Tag)>,
        depth: usize,
    },
    CompoundValue {
        entries: Vec<(String, Tag)>,
        name: String,
        depth: usize,
    },
}

fn read_tag<B: Buf>(
    buf: &mut B,
    tag_id: u8,
    depth: usize,
    budget: &mut NbtBudget,
) -> Result<Tag, NbtError> {
    let mut stack = budgeted_vec_with_capacity(FRAME_STACK_CAPACITY, budget, "reader frames")?;
    push_bounded(&mut stack, ReadFrame::Value { tag_id, depth })?;
    let mut completed = None;

    while let Some(frame) = stack.pop() {
        match frame {
            ReadFrame::Value { tag_id, depth } => match tag_id {
                tag_type::LIST => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    consume(buf, budget, 1)?;
                    let element_type = buf.get_u8();
                    let len = read_length(buf, budget, size_of::<Tag>())?;
                    if element_type == tag_type::END && len > 0 {
                        return Err(NbtError::HeterogeneousList {
                            parent: tag_type::LIST,
                            found: tag_type::END,
                        });
                    }
                    ensure_list_feasible(buf, len, element_type)?;
                    let elements = budgeted_vec_with_capacity(len, budget, "list elements")?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::List {
                            element_type,
                            remaining: len,
                            elements,
                            depth,
                        },
                    )?;
                }
                tag_type::COMPOUND => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::Compound {
                            entries: Vec::new(),
                            depth,
                        },
                    )?;
                }
                _ => completed = Some(read_scalar_payload(buf, tag_id, budget)?),
            },
            ReadFrame::List {
                element_type,
                remaining,
                elements,
                depth,
            } => {
                if remaining == 0 {
                    completed = Some(Tag::List(ListTag {
                        element_type,
                        elements,
                    }));
                } else {
                    push_bounded(
                        &mut stack,
                        ReadFrame::ListValue {
                            element_type,
                            remaining,
                            elements,
                            depth,
                        },
                    )?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::Value {
                            tag_id: element_type,
                            depth: depth + 1,
                        },
                    )?;
                }
            }
            ReadFrame::ListValue {
                element_type,
                remaining,
                mut elements,
                depth,
            } => {
                let value = completed
                    .take()
                    .expect("list child frame completes one tag");
                elements.push(value);
                push_bounded(
                    &mut stack,
                    ReadFrame::List {
                        element_type,
                        remaining: remaining - 1,
                        elements,
                        depth,
                    },
                )?;
            }
            ReadFrame::Compound { mut entries, depth } => {
                consume(buf, budget, 1)?;
                let child_type = buf.get_u8();
                if child_type == tag_type::END {
                    completed = Some(Tag::Compound(entries));
                    continue;
                }
                if entries.len() >= MAX_NBT_COMPOUND_ENTRIES {
                    return Err(NbtError::BudgetExceeded {
                        resource: "compound-entry-per-container",
                        limit: MAX_NBT_COMPOUND_ENTRIES,
                    });
                }
                budget.charge_compound_entry()?;
                ensure_budgeted_compound_capacity(&mut entries, budget)?;
                let name = read_string_with_budget(buf, budget)?;
                push_bounded(
                    &mut stack,
                    ReadFrame::CompoundValue {
                        entries,
                        name,
                        depth,
                    },
                )?;
                push_bounded(
                    &mut stack,
                    ReadFrame::Value {
                        tag_id: child_type,
                        depth: depth + 1,
                    },
                )?;
            }
            ReadFrame::CompoundValue {
                mut entries,
                name,
                depth,
            } => {
                let value = completed
                    .take()
                    .expect("compound child frame completes one tag");
                entries.push((name, value));
                push_bounded(&mut stack, ReadFrame::Compound { entries, depth })?;
            }
        }
    }

    completed.ok_or(NbtError::Underflow {
        needed: 1,
        available: 0,
    })
}

fn validate_string(s: &str, budget: &mut NbtBudget) -> Result<(), NbtError> {
    let encoded_len = modified_utf8_len(s)?;
    u16::try_from(encoded_len).map_err(|_| NbtError::StringTooLong(encoded_len))?;
    budget.charge_string(encoded_len)?;
    budget.charge_wire(2usize.saturating_add(encoded_len))
}

fn validate_scalar_payload(tag: &Tag, budget: &mut NbtBudget) -> Result<(), NbtError> {
    budget.charge_node()?;
    match tag {
        Tag::Byte(_) => budget.charge_wire(1),
        Tag::Short(_) => budget.charge_wire(2),
        Tag::Int(_) | Tag::Float(_) => budget.charge_wire(4),
        Tag::Long(_) | Tag::Double(_) => budget.charge_wire(8),
        Tag::ByteArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i8>())?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
        Tag::String(s) => validate_string(s, budget),
        Tag::List(_) => unreachable!("containers use the explicit-stack validator"),
        Tag::Compound(_) => unreachable!("containers use the explicit-stack validator"),
        Tag::IntArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i32>())?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
        Tag::LongArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i64>())?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
    }
}

enum ValidationFrame<'a> {
    Tag {
        tag: &'a Tag,
        depth: usize,
    },
    List {
        list: &'a ListTag,
        index: usize,
        depth: usize,
    },
    Compound {
        entries: &'a [(String, Tag)],
        index: usize,
        depth: usize,
    },
}

fn validate_payload(tag: &Tag, depth: usize, budget: &mut NbtBudget) -> Result<(), NbtError> {
    let mut stack = vec![ValidationFrame::Tag { tag, depth }];
    while let Some(frame) = stack.pop() {
        match frame {
            ValidationFrame::Tag { tag, depth } => match tag {
                Tag::List(list) => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    collection_payload_bytes(list.elements.len(), size_of::<Tag>())?;
                    if list.element_type == tag_type::END && !list.elements.is_empty() {
                        return Err(NbtError::HeterogeneousList {
                            parent: tag_type::LIST,
                            found: tag_type::END,
                        });
                    }
                    budget.charge_wire(5)?;
                    stack.push(ValidationFrame::List {
                        list,
                        index: 0,
                        depth,
                    });
                }
                Tag::Compound(entries) => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    if entries.len() > MAX_NBT_COMPOUND_ENTRIES {
                        return Err(NbtError::BudgetExceeded {
                            resource: "compound-entry-per-container",
                            limit: MAX_NBT_COMPOUND_ENTRIES,
                        });
                    }
                    stack.push(ValidationFrame::Compound {
                        entries,
                        index: 0,
                        depth,
                    });
                }
                _ => validate_scalar_payload(tag, budget)?,
            },
            ValidationFrame::List { list, index, depth } => {
                let Some(element) = list.elements.get(index) else {
                    continue;
                };
                if element.type_id() != list.element_type {
                    return Err(NbtError::HeterogeneousList {
                        parent: list.element_type,
                        found: element.type_id(),
                    });
                }
                stack.push(ValidationFrame::List {
                    list,
                    index: index + 1,
                    depth,
                });
                stack.push(ValidationFrame::Tag {
                    tag: element,
                    depth: depth + 1,
                });
            }
            ValidationFrame::Compound {
                entries,
                index,
                depth,
            } => {
                let Some((name, value)) = entries.get(index) else {
                    budget.charge_wire(1)?;
                    continue;
                };
                budget.charge_compound_entry()?;
                budget.charge_wire(1)?;
                validate_string(name, budget)?;
                stack.push(ValidationFrame::Compound {
                    entries,
                    index: index + 1,
                    depth,
                });
                stack.push(ValidationFrame::Tag {
                    tag: value,
                    depth: depth + 1,
                });
            }
        }
    }
    Ok(())
}

fn write_scalar_payload<B: BufMut>(buf: &mut B, tag: &Tag) {
    match tag {
        Tag::Byte(v) => buf.put_i8(*v),
        Tag::Short(v) => buf.put_i16(*v),
        Tag::Int(v) => buf.put_i32(*v),
        Tag::Long(v) => buf.put_i64(*v),
        Tag::Float(v) => buf.put_f32(*v),
        Tag::Double(v) => buf.put_f64(*v),
        Tag::ByteArray(data) => {
            write_length(buf, data.len(), size_of::<i8>()).expect("NBT write preflight");
            for v in data {
                buf.put_i8(*v);
            }
        }
        Tag::String(s) => write_string(buf, s).expect("NBT write preflight"),
        Tag::List(_) => unreachable!("containers use the explicit-stack writer"),
        Tag::Compound(_) => unreachable!("containers use the explicit-stack writer"),
        Tag::IntArray(data) => {
            write_length(buf, data.len(), size_of::<i32>()).expect("NBT write preflight");
            for v in data {
                buf.put_i32(*v);
            }
        }
        Tag::LongArray(data) => {
            write_length(buf, data.len(), size_of::<i64>()).expect("NBT write preflight");
            for v in data {
                buf.put_i64(*v);
            }
        }
    }
}

enum WriteFrame<'a> {
    Tag(&'a Tag),
    List {
        elements: &'a [Tag],
        index: usize,
    },
    Compound {
        entries: &'a [(String, Tag)],
        index: usize,
    },
}

fn write_payload_unchecked<B: BufMut>(buf: &mut B, tag: &Tag) {
    let mut stack = vec![WriteFrame::Tag(tag)];
    while let Some(frame) = stack.pop() {
        match frame {
            WriteFrame::Tag(tag) => match tag {
                Tag::List(list) => {
                    buf.put_u8(list.element_type);
                    write_length(buf, list.elements.len(), size_of::<Tag>())
                        .expect("NBT write preflight");
                    stack.push(WriteFrame::List {
                        elements: &list.elements,
                        index: 0,
                    });
                }
                Tag::Compound(entries) => stack.push(WriteFrame::Compound { entries, index: 0 }),
                _ => write_scalar_payload(buf, tag),
            },
            WriteFrame::List { elements, index } => {
                let Some(element) = elements.get(index) else {
                    continue;
                };
                stack.push(WriteFrame::List {
                    elements,
                    index: index + 1,
                });
                stack.push(WriteFrame::Tag(element));
            }
            WriteFrame::Compound { entries, index } => {
                let Some((name, value)) = entries.get(index) else {
                    buf.put_u8(tag_type::END);
                    continue;
                };
                buf.put_u8(value.type_id());
                write_string(buf, name).expect("NBT write preflight");
                stack.push(WriteFrame::Compound {
                    entries,
                    index: index + 1,
                });
                stack.push(WriteFrame::Tag(value));
            }
        }
    }
}

// -----------------------------------------------------------------------
// Public entry points
// -----------------------------------------------------------------------

fn read_network_with_budget<B: Buf>(buf: &mut B, budget: &mut NbtBudget) -> Result<Tag, NbtError> {
    consume(buf, budget, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    read_tag(buf, tag_id, 1, budget)
}

/// Read a *network-format* NBT root. The root must be a [`Tag::Compound`];
/// modern protocol packets always wrap data in one.
pub fn read_network<B: Buf>(buf: &mut B) -> Result<Tag, NbtError> {
    let mut budget = NbtBudget::production();
    read_network_with_budget(buf, &mut budget)
}

/// Write a *network-format* NBT root. `root` must be a [`Tag::Compound`].
///
/// The complete tree is validated and budgeted before the first byte is written,
/// so any semantic error leaves the caller's buffer unchanged.
pub fn write_network<B: BufMut>(buf: &mut B, root: &Tag) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    let mut budget = NbtBudget::production();
    budget.charge_wire(1)?;
    validate_payload(root, 1, &mut budget)?;

    buf.put_u8(tag_type::COMPOUND);
    write_payload_unchecked(buf, root);
    Ok(())
}

/// Read a *named* NBT root: the disk format used by region files and
/// `.nbt` files. Returns the root tag's name alongside its value.
pub fn read_named<B: Buf>(buf: &mut B) -> Result<(String, Tag), NbtError> {
    let mut budget = NbtBudget::production();
    consume(buf, &mut budget, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    let name = read_string_with_budget(buf, &mut budget)?;
    let value = read_tag(buf, tag_id, 1, &mut budget)?;
    Ok((name, value))
}

/// Write a *named* NBT root. Validation is atomic with respect to the caller's
/// buffer: semantic failures are reported before the first byte is emitted.
pub fn write_named<B: BufMut>(buf: &mut B, name: &str, root: &Tag) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    let mut budget = NbtBudget::production();
    budget.charge_wire(1)?;
    validate_string(name, &mut budget)?;
    validate_payload(root, 1, &mut budget)?;

    buf.put_u8(tag_type::COMPOUND);
    write_string(buf, name).expect("NBT write preflight");
    write_payload_unchecked(buf, root);
    Ok(())
}

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

    fn read_network_with_limits(bytes: &[u8], limits: NbtLimits) -> Result<Tag, NbtError> {
        let mut cursor = bytes;
        let mut budget = NbtBudget::new(limits);
        read_network_with_budget(&mut cursor, &mut budget)
    }

    fn validate_network_with_limits(root: &Tag, limits: NbtLimits) -> Result<(), NbtError> {
        let mut budget = NbtBudget::new(limits);
        budget.charge_wire(1)?;
        validate_payload(root, 1, &mut budget)
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
            ("modified".into(), Tag::String("nul:\0 face:😀".into())),
            ("empty".into(), Tag::String(String::new())),
        ]));
    }

    #[test]
    fn modified_utf8_encodes_ascii_exactly() {
        let mut buf = Vec::new();
        write_string(&mut buf, "ASCII").unwrap();

        assert_eq!(buf, [0x00, 0x05, b'A', b'S', b'C', b'I', b'I']);

        let mut cursor = buf.as_slice();
        assert_eq!(read_string(&mut cursor).unwrap(), "ASCII");
        assert!(cursor.is_empty());
    }

    #[test]
    fn modified_utf8_encodes_embedded_nul_as_c0_80() {
        let mut buf = Vec::new();
        write_string(&mut buf, "A\0B").unwrap();

        assert_eq!(buf, [0x00, 0x04, b'A', 0xC0, 0x80, b'B']);

        let mut cursor = buf.as_slice();
        assert_eq!(read_string(&mut cursor).unwrap(), "A\0B");
        assert!(cursor.is_empty());
    }

    #[test]
    fn modified_utf8_encodes_bmp_boundaries_exactly() {
        let mut buf = Vec::new();
        write_string(&mut buf, "\u{0001}\u{007f}\u{0080}\u{07ff}\u{0800}\u{ffff}").unwrap();

        assert_eq!(
            buf,
            vec![
                0x00, 0x0C, 0x01, 0x7F, 0xC2, 0x80, 0xDF, 0xBF, 0xE0, 0xA0, 0x80, 0xEF, 0xBF, 0xBF,
            ]
        );

        let mut cursor = buf.as_slice();
        assert_eq!(
            read_string(&mut cursor).unwrap(),
            "\u{0001}\u{007f}\u{0080}\u{07ff}\u{0800}\u{ffff}"
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn modified_utf8_encodes_supplementary_code_point_as_surrogate_pair() {
        let mut buf = Vec::new();
        write_string(&mut buf, "😀").unwrap();

        assert_eq!(buf, [0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);

        let mut cursor = buf.as_slice();
        assert_eq!(read_string(&mut cursor).unwrap(), "😀");
        assert!(cursor.is_empty());
    }

    #[test]
    fn modified_utf8_rejects_standard_four_byte_sequence() {
        let mut cursor: &[u8] = &[0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80];
        assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
    }

    #[test]
    fn modified_utf8_rejects_noncanonical_and_truncated_sequences() {
        for bytes in [
            &[0x00][..],
            &[0x80],
            &[0xC1, 0x81],
            &[0xC2],
            &[0xC2, b'A'],
            &[0xE0, 0x80, 0x80],
            &[0xE1, 0x80],
            &[0xE1, b'A', 0x80],
        ] {
            let mut encoded = Vec::with_capacity(bytes.len() + 2);
            encoded.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            encoded.extend_from_slice(bytes);
            let mut cursor = encoded.as_slice();
            assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
        }

        let mut truncated_payload: &[u8] = &[0x00, 0x02, 0xC2];
        assert_eq!(
            read_string(&mut truncated_payload),
            Err(NbtError::Underflow {
                needed: 1,
                available: 1,
            })
        );
    }

    #[test]
    fn modified_utf8_rejects_unpaired_and_reversed_surrogates() {
        for bytes in [
            &[0xED, 0xA0, 0x80][..],
            &[0xED, 0xB0, 0x80],
            &[0xED, 0xB0, 0x80, 0xED, 0xA0, 0x80],
            &[0xED, 0xA0, 0x80, 0xED, 0xA0, 0x80],
        ] {
            let mut encoded = Vec::with_capacity(bytes.len() + 2);
            encoded.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            encoded.extend_from_slice(bytes);
            let mut cursor = encoded.as_slice();
            assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
        }
    }

    #[test]
    fn modified_utf8_enforces_encoded_u16_wire_length() {
        let exact_limit = "a".repeat(usize::from(u16::MAX));
        let mut output = Vec::new();
        write_string(&mut output, &exact_limit).unwrap();
        assert_eq!(&output[..2], &[0xFF, 0xFF]);
        assert_eq!(output.len(), usize::from(u16::MAX) + 2);
        let mut cursor = output.as_slice();
        assert_eq!(read_string(&mut cursor).unwrap(), exact_limit);
        assert!(cursor.is_empty());

        let too_long = "\0".repeat(32_768);
        output.clear();
        assert_eq!(
            write_string(&mut output, &too_long),
            Err(NbtError::StringTooLong(65_536))
        );
        assert!(output.is_empty());
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
    fn nested_lists_and_empty_unknown_element_type_round_trip() {
        round_trip_network(Tag::Compound(vec![
            (
                "nested".into(),
                Tag::List(ListTag {
                    element_type: tag_type::LIST,
                    elements: vec![
                        Tag::List(ListTag {
                            element_type: tag_type::INT,
                            elements: vec![Tag::Int(1)],
                        }),
                        Tag::List(ListTag {
                            element_type: tag_type::INT,
                            elements: vec![Tag::Int(2), Tag::Int(3)],
                        }),
                    ],
                }),
            ),
            (
                "empty_unknown".into(),
                Tag::List(ListTag {
                    element_type: 0x7f,
                    elements: Vec::new(),
                }),
            ),
        ]));
    }

    #[test]
    fn non_empty_unknown_list_element_type_is_rejected() {
        let mut encoded = vec![tag_type::COMPOUND, tag_type::LIST, 0, 0, 0x7f];
        encoded.extend_from_slice(&1_i32.to_be_bytes());
        encoded.push(0);

        assert_eq!(
            read_network(&mut encoded.as_slice()),
            Err(NbtError::UnknownTag(0x7f))
        );
    }

    #[test]
    fn truncated_list_fails_feasibility_before_element_allocation() {
        let mut encoded = vec![tag_type::COMPOUND, tag_type::LIST, 0, 0, tag_type::INT];
        encoded.extend_from_slice(&2_i32.to_be_bytes());
        encoded.extend_from_slice(&1_i32.to_be_bytes());

        assert_eq!(
            read_network(&mut encoded.as_slice()),
            Err(NbtError::Underflow {
                needed: 4,
                available: 4,
            })
        );
    }

    #[test]
    fn duplicate_compound_names_round_trip_in_order() {
        round_trip_network(Tag::Compound(vec![
            ("same".into(), Tag::Byte(1)),
            ("same".into(), Tag::Byte(2)),
        ]));
    }

    #[test]
    fn reader_enforces_aggregate_wire_node_entry_string_and_allocation_budgets() {
        let simple = Tag::Compound(vec![("x".into(), Tag::Byte(1))]);
        let mut simple_bytes = Vec::new();
        write_network(&mut simple_bytes, &simple).unwrap();
        let wire_limit = simple_bytes.len() - 1;
        assert_eq!(
            read_network_with_limits(
                &simple_bytes,
                NbtLimits {
                    wire_bytes: wire_limit,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "wire-byte",
                limit: wire_limit,
            })
        );

        let list = Tag::Compound(vec![(
            "list".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
            }),
        )]);
        let mut list_bytes = Vec::new();
        write_network(&mut list_bytes, &list).unwrap();
        assert_eq!(
            read_network_with_limits(
                &list_bytes,
                NbtLimits {
                    nodes: 4,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "node",
                limit: 4,
            })
        );

        let entries = Tag::Compound(vec![("a".into(), Tag::Byte(1)), ("b".into(), Tag::Byte(2))]);
        let mut entry_bytes = Vec::new();
        write_network(&mut entry_bytes, &entries).unwrap();
        assert_eq!(
            read_network_with_limits(
                &entry_bytes,
                NbtLimits {
                    compound_entries: 1,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "compound-entry",
                limit: 1,
            })
        );

        let strings = Tag::Compound(vec![("name".into(), Tag::String("value".into()))]);
        let mut string_bytes = Vec::new();
        write_network(&mut string_bytes, &strings).unwrap();
        assert_eq!(
            read_network_with_limits(
                &string_bytes,
                NbtLimits {
                    string_bytes: 8,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "string-byte",
                limit: 8,
            })
        );

        let mut baseline_budget = NbtBudget::production();
        let mut baseline_cursor = list_bytes.as_slice();
        read_network_with_budget(&mut baseline_cursor, &mut baseline_budget).unwrap();
        let allocation_limit = baseline_budget.allocation_bytes - 1;
        assert_eq!(
            read_network_with_limits(
                &list_bytes,
                NbtLimits {
                    allocation_bytes: allocation_limit,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "allocation-byte",
                limit: allocation_limit,
            })
        );
        assert!(
            read_network_with_limits(
                &list_bytes,
                NbtLimits {
                    allocation_bytes: baseline_budget.allocation_bytes,
                    ..PRODUCTION_LIMITS
                },
            )
            .is_ok()
        );
    }

    #[test]
    fn writer_preflight_enforces_aggregate_budgets() {
        let root = Tag::Compound(vec![
            (
                "list".into(),
                Tag::List(ListTag {
                    element_type: tag_type::INT,
                    elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
                }),
            ),
            ("value".into(), Tag::String("payload".into())),
        ]);

        assert_eq!(
            validate_network_with_limits(
                &root,
                NbtLimits {
                    nodes: 5,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "node",
                limit: 5,
            })
        );
        assert_eq!(
            validate_network_with_limits(
                &root,
                NbtLimits {
                    compound_entries: 1,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "compound-entry",
                limit: 1,
            })
        );
        assert_eq!(
            validate_network_with_limits(
                &root,
                NbtLimits {
                    string_bytes: 15,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "string-byte",
                limit: 15,
            })
        );
        assert_eq!(
            validate_network_with_limits(
                &root,
                NbtLimits {
                    wire_bytes: 24,
                    ..PRODUCTION_LIMITS
                },
            ),
            Err(NbtError::BudgetExceeded {
                resource: "wire-byte",
                limit: 24,
            })
        );
    }

    #[test]
    fn reader_and_writer_enforce_per_compound_entry_limit() {
        let mut encoded = vec![tag_type::COMPOUND];
        for _ in 0..=MAX_NBT_COMPOUND_ENTRIES {
            encoded.extend_from_slice(&[tag_type::BYTE, 0, 0, 0]);
        }
        encoded.push(tag_type::END);
        assert_eq!(
            read_network(&mut encoded.as_slice()),
            Err(NbtError::BudgetExceeded {
                resource: "compound-entry-per-container",
                limit: MAX_NBT_COMPOUND_ENTRIES,
            })
        );

        let root = Tag::Compound(
            (0..=MAX_NBT_COMPOUND_ENTRIES)
                .map(|_| (String::new(), Tag::Byte(0)))
                .collect(),
        );
        let mut output = vec![0xA5];
        assert_eq!(
            write_network(&mut output, &root),
            Err(NbtError::BudgetExceeded {
                resource: "compound-entry-per-container",
                limit: MAX_NBT_COMPOUND_ENTRIES,
            })
        );
        assert_eq!(output, vec![0xA5]);
    }

    #[test]
    fn compound_capacity_growth_is_charged_before_reserve() {
        let mut budget = NbtBudget::production();
        let mut entries = Vec::new();
        ensure_budgeted_compound_capacity(&mut entries, &mut budget).unwrap();
        let initial_capacity = entries.capacity();
        assert!(initial_capacity >= COMPOUND_RESERVE_CHUNK);
        while entries.len() < initial_capacity {
            entries.push((String::new(), Tag::Byte(0)));
        }

        budget.limits.allocation_bytes = budget.allocation_bytes;
        let before_capacity = entries.capacity();
        assert_eq!(
            ensure_budgeted_compound_capacity(&mut entries, &mut budget),
            Err(NbtError::BudgetExceeded {
                resource: "allocation-byte",
                limit: budget.allocation_bytes,
            })
        );
        assert_eq!(entries.capacity(), before_capacity);
    }

    #[test]
    fn compound_capacity_growth_charges_only_final_capacity() {
        let mut budget = NbtBudget::production();
        let mut entries = Vec::new();

        for _ in 0..4 {
            ensure_budgeted_compound_capacity(&mut entries, &mut budget).unwrap();
            let capacity = entries.capacity();
            while entries.len() < capacity {
                entries.push((String::new(), Tag::Byte(0)));
            }
        }

        assert_eq!(
            budget.allocation_bytes,
            capacity_bytes::<(String, Tag)>(entries.capacity(), budget.limits.allocation_bytes)
                .unwrap()
        );
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
    fn writer_errors_leave_existing_output_unchanged() {
        let heterogeneous = Tag::Compound(vec![(
            "k".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(1), Tag::Byte(2)],
            }),
        )]);
        let mut network_output = vec![0xA5, 0x5A];
        assert!(matches!(
            write_network(&mut network_output, &heterogeneous),
            Err(NbtError::HeterogeneousList { .. })
        ));
        assert_eq!(network_output, vec![0xA5, 0x5A]);

        let long_name = "x".repeat(usize::from(u16::MAX) + 1);
        let mut named_output = vec![0x11, 0x22];
        assert!(matches!(
            write_named(&mut named_output, &long_name, &Tag::Compound(Vec::new())),
            Err(NbtError::StringTooLong(_))
        ));
        assert_eq!(named_output, vec![0x11, 0x22]);
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
    fn int_array_limit_counts_payload_bytes() {
        let too_many_ints = MAX_NBT_LENGTH / size_of::<i32>() + 1;
        let mut encoded = vec![tag_type::COMPOUND, tag_type::INT_ARRAY, 0, 1, b'x'];
        encoded.extend_from_slice(&(too_many_ints as i32).to_be_bytes());
        let mut cursor = encoded.as_slice();

        assert!(matches!(
            read_network(&mut cursor),
            Err(NbtError::ArrayTooLong(_))
        ));

        let tag = Tag::Compound(vec![("x".into(), Tag::IntArray(vec![0; too_many_ints]))]);
        let mut output = Vec::new();
        assert!(matches!(
            write_network(&mut output, &tag),
            Err(NbtError::ArrayTooLong(_))
        ));
    }

    #[test]
    fn reader_and_writer_accept_exact_depth_boundary() {
        let nested_containers = MAX_NBT_DEPTH - 1;
        let mut tag = Tag::Compound(Vec::new());
        for _ in 0..nested_containers {
            tag = Tag::Compound(vec![(String::new(), tag)]);
        }
        round_trip_network(tag);
    }

    #[test]
    fn reader_and_writer_reject_excessive_nesting() {
        let nested_containers = MAX_NBT_DEPTH;
        let mut encoded = vec![tag_type::COMPOUND];
        for _ in 0..nested_containers {
            encoded.extend_from_slice(&[tag_type::COMPOUND, 0, 0]);
        }
        encoded.resize(encoded.len() + nested_containers + 1, tag_type::END);
        let mut cursor = encoded.as_slice();
        assert_eq!(read_network(&mut cursor), Err(NbtError::NestingTooDeep));

        let mut tag = Tag::Compound(Vec::new());
        for _ in 0..nested_containers {
            tag = Tag::Compound(vec![(String::new(), tag)]);
        }
        let mut output = Vec::new();
        assert_eq!(
            write_network(&mut output, &tag),
            Err(NbtError::NestingTooDeep)
        );
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

        #[test]
        fn proptest_modified_utf8_round_trip(value in any::<String>()) {
            let mut buf = Vec::new();
            write_string(&mut buf, &value).unwrap();
            let mut cursor = buf.as_slice();
            let decoded = read_string(&mut cursor).unwrap();
            prop_assert_eq!(decoded, value);
            prop_assert!(cursor.is_empty());
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
