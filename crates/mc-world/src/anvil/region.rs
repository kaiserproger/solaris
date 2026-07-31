//! Anvil `.mca` region-file binary layout.
//!
//! Per minecraft.wiki "Region file format":
//!
//! - bytes `0..4096` — locations table: 1024 entries, each 4 bytes
//!   big-endian, holding `(offset_in_sectors << 8) | sector_count`.
//!   Sector size is 4096. `0u32` means the chunk slot is empty.
//! - bytes `4096..8192` — timestamps table: 1024 `u32` BE values
//!   (epoch seconds of last save).
//! - bytes `8192..` — chunk data, 4096-aligned. Each chunk is
//!   `[len: i32 BE][compression: u8][payload …]` where `len` counts
//!   the compression byte plus the payload (so `payload_len = len - 1`).
//!
//! Compression types: 1 = gzip, 2 = zlib, 3 = uncompressed, 4 = LZ4.
//! The high bit (`0x80`) on the compression byte marks an *oversized*
//! chunk whose payload lives in a sibling `c.X.Z.mcc` file; we don't
//! emit that variant on write but read returns an error rather than
//! silently dropping it.
//!
//! Slot index for chunk-local `(cx, cz)` in `0..32`:
//!     idx = (cz as usize) * 32 + (cx as usize)
//! Locations table entry sits at `idx * 4`.

use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use thiserror::Error;

/// 32 × 32 chunks per region.
pub const CHUNKS_PER_REGION_AXIS: usize = 32;
const REGION_CHUNK_COUNT: usize = CHUNKS_PER_REGION_AXIS * CHUNKS_PER_REGION_AXIS;
const SECTOR_SIZE: usize = 4096;
const HEADER_SECTORS: usize = 2; // locations + timestamps
const HEADER_BYTES: usize = HEADER_SECTORS * SECTOR_SIZE;
const MAX_REGION_FILE_BYTES: u64 =
    (HEADER_SECTORS as u64 + REGION_CHUNK_COUNT as u64 * u8::MAX as u64) * SECTOR_SIZE as u64;
const MAX_DECOMPRESSED_CHUNK_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECOMPRESSED_REGION_BYTES: usize = 256 * 1024 * 1024;
const LZ4_BLOCK_MAGIC: &[u8; 8] = b"LZ4Block";
const LZ4_BLOCK_HEADER_LEN: usize = 21;
const LZ4_BLOCK_METHOD_RAW: u8 = 0x10;
const LZ4_BLOCK_METHOD_COMPRESSED: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompressionType {
    Gzip = 1,
    Zlib = 2,
    Uncompressed = 3,
    Lz4 = 4,
}

impl CompressionType {
    fn from_byte(b: u8) -> Result<Self, RegionError> {
        match b {
            1 => Ok(Self::Gzip),
            2 => Ok(Self::Zlib),
            3 => Ok(Self::Uncompressed),
            4 => Ok(Self::Lz4),
            other => Err(RegionError::UnknownCompression(other)),
        }
    }
}

/// One chunk slot's full content.
#[derive(Debug, Clone)]
pub struct ChunkPayload {
    /// Local chunk x within the region, `0..32`.
    pub local_x: u8,
    /// Local chunk z within the region, `0..32`.
    pub local_z: u8,
    /// Epoch seconds from the timestamps table.
    pub timestamp: u32,
    /// Uncompressed Java-standard NBT bytes (a single named root
    /// compound, as `mc_nbt::read_named` accepts).
    pub uncompressed_nbt: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum RegionError {
    #[error("region io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("region file too short: needs at least {needed} bytes, got {got}")]
    TooShort { needed: usize, got: usize },
    #[error("region file is {bytes} bytes, exceeding limit {max}")]
    RegionTooLarge { bytes: u64, max: u64 },
    #[error("chunk coordinates ({cx},{cz}) are outside the local region range 0..32")]
    InvalidChunkCoordinates { cx: u8, cz: u8 },
    #[error(
        "chunk slot ({cx},{cz}) points at sector {sector} which is past the file's {sector_count} sectors"
    )]
    SectorOutOfRange {
        cx: u8,
        cz: u8,
        sector: u32,
        sector_count: usize,
    },
    #[error("chunk slot ({cx},{cz}) has nonzero offset but zero sector count")]
    ZeroSectorCount { cx: u8, cz: u8 },
    #[error(
        "chunk slot ({cx},{cz}) declared length {len} runs past its allocated {bytes_available} bytes"
    )]
    LengthOverrun {
        cx: u8,
        cz: u8,
        len: u32,
        bytes_available: usize,
    },
    #[error("unknown compression byte {0:#04x}")]
    UnknownCompression(u8),
    #[error("oversized chunks (.mcc sidecars) are not supported")]
    Oversized,
    #[error("decompression failed for chunk ({cx},{cz}): {source}")]
    Decompress {
        cx: u8,
        cz: u8,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "chunk ({cx},{cz}) decompressed payload is at least {bytes} bytes, exceeding limit {max}"
    )]
    DecompressedChunkTooLarge {
        cx: u8,
        cz: u8,
        bytes: usize,
        max: usize,
    },
    #[error("region decompressed payload total is {bytes} bytes, exceeding limit {max}")]
    DecompressedRegionTooLarge { bytes: usize, max: usize },
}

#[derive(Debug, Clone, Copy)]
struct RegionLimits {
    max_file_bytes: u64,
    max_chunk_bytes: usize,
    max_region_bytes: usize,
}

const DEFAULT_REGION_LIMITS: RegionLimits = RegionLimits {
    max_file_bytes: MAX_REGION_FILE_BYTES,
    max_chunk_bytes: MAX_DECOMPRESSED_CHUNK_BYTES,
    max_region_bytes: MAX_DECOMPRESSED_REGION_BYTES,
};

struct RegionReader {
    path: PathBuf,
    file: File,
    file_len: u64,
    total_sectors: usize,
    header: [u8; HEADER_BYTES],
}

impl RegionReader {
    fn open(path: &Path, limits: RegionLimits) -> Result<Self, RegionError> {
        let mut file = File::open(path).map_err(|source| RegionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_len = file
            .metadata()
            .map_err(|source| RegionError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        if file_len > limits.max_file_bytes {
            return Err(RegionError::RegionTooLarge {
                bytes: file_len,
                max: limits.max_file_bytes,
            });
        }
        if file_len < HEADER_BYTES as u64 {
            return Err(RegionError::TooShort {
                needed: HEADER_BYTES,
                got: file_len as usize,
            });
        }

        let mut header = [0_u8; HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|source| RegionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let total_sectors = usize::try_from(file_len.div_ceil(SECTOR_SIZE as u64))
            .expect("bounded region sector count fits usize");
        Ok(Self {
            path: path.to_path_buf(),
            file,
            file_len,
            total_sectors,
            header,
        })
    }

    fn read_slot(
        &mut self,
        slot: usize,
        decoded_so_far: usize,
        limits: RegionLimits,
    ) -> Result<Option<ChunkPayload>, RegionError> {
        let cx = (slot % CHUNKS_PER_REGION_AXIS) as u8;
        let cz = (slot / CHUNKS_PER_REGION_AXIS) as u8;
        let loc_off = slot * 4;
        let loc = u32::from_be_bytes(
            self.header[loc_off..loc_off + 4]
                .try_into()
                .expect("4-byte slice"),
        );
        let sector = loc >> 8;
        let count = (loc & 0xFF) as u8;
        if sector == 0 && count == 0 {
            return Ok(None);
        }
        if count == 0 {
            return Err(RegionError::ZeroSectorCount { cx, cz });
        }

        let start = u64::from(sector) * SECTOR_SIZE as u64;
        if start >= self.file_len {
            return Err(RegionError::SectorOutOfRange {
                cx,
                cz,
                sector,
                sector_count: self.total_sectors,
            });
        }
        let allocated_end = start
            .checked_add(u64::from(count) * SECTOR_SIZE as u64)
            .expect("u24 sector offset plus u8 count fits u64");
        let end = allocated_end.min(self.file_len);
        let bytes_available =
            usize::try_from(end - start).expect("one Anvil chunk allocation fits usize");
        if bytes_available < 5 {
            return Err(RegionError::LengthOverrun {
                cx,
                cz,
                len: 0,
                bytes_available,
            });
        }

        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut chunk_header = [0_u8; 5];
        self.file
            .read_exact(&mut chunk_header)
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;
        let len = u32::from_be_bytes(chunk_header[..4].try_into().expect("4-byte slice"));
        let comp_byte = chunk_header[4];
        if comp_byte & 0x80 != 0 {
            return Err(RegionError::Oversized);
        }
        let comp = CompressionType::from_byte(comp_byte)?;
        let payload_len = usize::try_from(len)
            .expect("u32 chunk length fits usize")
            .checked_sub(1)
            .ok_or(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            })?;
        let encoded_len = 5_usize
            .checked_add(payload_len)
            .ok_or(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            })?;
        if encoded_len > bytes_available {
            return Err(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available,
            });
        }

        let mut payload =
            allocate_exact_bytes(payload_len, "compressed Anvil chunk").map_err(|source| {
                RegionError::Io {
                    path: self.path.clone(),
                    source,
                }
            })?;
        self.file
            .read_exact(&mut payload)
            .map_err(|source| RegionError::Io {
                path: self.path.clone(),
                source,
            })?;

        let uncompressed_nbt = decompress_bounded(comp, &payload, cx, cz, decoded_so_far, limits)?;
        let ts_off = SECTOR_SIZE + slot * 4;
        let timestamp = u32::from_be_bytes(
            self.header[ts_off..ts_off + 4]
                .try_into()
                .expect("4-byte slice"),
        );
        Ok(Some(ChunkPayload {
            local_x: cx,
            local_z: cz,
            timestamp,
            uncompressed_nbt,
        }))
    }
}

/// Read every populated chunk out of a region file. Empty slots are
/// silently skipped. Returns chunks in slot-order
/// `(cz * 32 + cx)`.
pub fn read_region(path: impl AsRef<Path>) -> Result<Vec<ChunkPayload>, RegionError> {
    let path = path.as_ref();
    let mut out = Vec::new();
    out.try_reserve_exact(REGION_CHUNK_COUNT)
        .map_err(|error| RegionError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("reserve region chunk list: {error}")),
        })?;
    read_region_with_limits(path, DEFAULT_REGION_LIMITS, |payload| out.push(payload))?;
    Ok(out)
}

/// Visit every populated chunk without retaining all decoded payloads.
/// The same per-chunk and aggregate decode budgets as [`read_region`]
/// are enforced before each output allocation.
pub fn visit_region(
    path: impl AsRef<Path>,
    visitor: impl FnMut(ChunkPayload),
) -> Result<(), RegionError> {
    read_region_with_limits(path.as_ref(), DEFAULT_REGION_LIMITS, visitor)
}

pub(crate) fn read_chunk(
    path: impl AsRef<Path>,
    local_x: u8,
    local_z: u8,
) -> Result<Option<ChunkPayload>, RegionError> {
    if usize::from(local_x) >= CHUNKS_PER_REGION_AXIS
        || usize::from(local_z) >= CHUNKS_PER_REGION_AXIS
    {
        return Err(RegionError::InvalidChunkCoordinates {
            cx: local_x,
            cz: local_z,
        });
    }
    let mut reader = RegionReader::open(path.as_ref(), DEFAULT_REGION_LIMITS)?;
    let slot = usize::from(local_z) * CHUNKS_PER_REGION_AXIS + usize::from(local_x);
    reader.read_slot(slot, 0, DEFAULT_REGION_LIMITS)
}

fn read_region_with_limits(
    path: &Path,
    limits: RegionLimits,
    mut visitor: impl FnMut(ChunkPayload),
) -> Result<(), RegionError> {
    let mut reader = RegionReader::open(path, limits)?;
    let mut decoded_total = 0_usize;
    for slot in 0..REGION_CHUNK_COUNT {
        if let Some(payload) = reader.read_slot(slot, decoded_total, limits)? {
            decoded_total = decoded_total
                .checked_add(payload.uncompressed_nbt.len())
                .expect("aggregate limit prevents decoded byte overflow");
            visitor(payload);
        }
    }
    Ok(())
}

/// Write a fresh region file at `path`, packing each chunk with zlib.
/// Slots not represented in `chunks` are left empty (`(0, 0)` in the
/// locations table).
pub fn write_region(path: impl AsRef<Path>, chunks: &[ChunkPayload]) -> Result<(), RegionError> {
    write_region_with_options(path.as_ref(), chunks, false)
}

/// Write a fresh region file at `path`, failing if it already exists.
pub fn write_region_create_new(
    path: impl AsRef<Path>,
    chunks: &[ChunkPayload],
) -> Result<(), RegionError> {
    write_region_with_options(path.as_ref(), chunks, true)
}

fn write_region_with_options(
    path: &Path,
    chunks: &[ChunkPayload],
    create_new: bool,
) -> Result<(), RegionError> {
    let mut locations = vec![0u32; REGION_CHUNK_COUNT];
    let mut timestamps = vec![0u32; REGION_CHUNK_COUNT];
    // Body begins at sector HEADER_SECTORS; next_sector tracks growth.
    let mut next_sector = HEADER_SECTORS as u32;
    let mut body: Vec<u8> = Vec::new();

    for c in chunks {
        let slot = (c.local_z as usize) * CHUNKS_PER_REGION_AXIS + (c.local_x as usize);
        let mut zlib = ZlibEncoder::new(
            Vec::with_capacity(c.uncompressed_nbt.len()),
            Compression::default(),
        );
        zlib.write_all(&c.uncompressed_nbt)
            .map_err(|e| RegionError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;
        let compressed = zlib.finish().map_err(|e| RegionError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        // [len:4][comp:1][payload …] padded to a sector.
        let len = compressed.len() as u32 + 1; // +1 for compression byte
        let raw_len = 4 + 1 + compressed.len();
        let padded_len = raw_len.div_ceil(SECTOR_SIZE) * SECTOR_SIZE;
        let pad = padded_len - raw_len;

        body.extend_from_slice(&len.to_be_bytes());
        body.push(CompressionType::Zlib as u8);
        body.extend_from_slice(&compressed);
        body.extend(std::iter::repeat_n(0u8, pad));

        let sectors_used = (padded_len / SECTOR_SIZE) as u32;
        locations[slot] = (next_sector << 8) | (sectors_used & 0xFF);
        timestamps[slot] = c.timestamp;
        next_sector += sectors_used;
    }

    let mut out = Vec::with_capacity(HEADER_SECTORS * SECTOR_SIZE + body.len());
    for &loc in &locations {
        out.extend_from_slice(&loc.to_be_bytes());
    }
    for &ts in &timestamps {
        out.extend_from_slice(&ts.to_be_bytes());
    }
    out.extend_from_slice(&body);

    let mut file = if create_new {
        OpenOptions::new().write(true).create_new(true).open(path)
    } else {
        File::create(path)
    }
    .map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    file.write_all(&out).map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    file.sync_all().map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedLength {
    Exact(usize),
    TooLarge { at_least: usize },
}

fn decompress_bounded(
    comp: CompressionType,
    payload: &[u8],
    cx: u8,
    cz: u8,
    decoded_so_far: usize,
    limits: RegionLimits,
) -> Result<Vec<u8>, RegionError> {
    let decoded_len = match count_decoded_bytes(comp, payload, limits.max_chunk_bytes)
        .map_err(|source| RegionError::Decompress { cx, cz, source })?
    {
        BoundedLength::Exact(len) => len,
        BoundedLength::TooLarge { at_least } => {
            return Err(RegionError::DecompressedChunkTooLarge {
                cx,
                cz,
                bytes: at_least,
                max: limits.max_chunk_bytes,
            });
        }
    };
    let aggregate = decoded_so_far.saturating_add(decoded_len);
    if aggregate > limits.max_region_bytes {
        return Err(RegionError::DecompressedRegionTooLarge {
            bytes: aggregate,
            max: limits.max_region_bytes,
        });
    }
    decompress_exact(comp, payload, decoded_len).map_err(|source| RegionError::Decompress {
        cx,
        cz,
        source,
    })
}

fn count_decoded_bytes(
    comp: CompressionType,
    payload: &[u8],
    max: usize,
) -> Result<BoundedLength, std::io::Error> {
    match comp {
        CompressionType::Uncompressed => Ok(if payload.len() > max {
            BoundedLength::TooLarge {
                at_least: payload.len(),
            }
        } else {
            BoundedLength::Exact(payload.len())
        }),
        CompressionType::Zlib => count_zlib_bytes(payload, max),
        CompressionType::Gzip => count_gzip_bytes(payload, max),
        CompressionType::Lz4 => count_lz4_bytes(payload, max),
    }
}

fn count_zlib_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut decoder = Decompress::new(true);
    let mut input_pos = 0_usize;
    let mut total = 0_usize;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let remaining = max.saturating_sub(total);
        let output_cap = remaining.saturating_add(1).min(buffer.len());
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let flush = if input_pos == payload.len() {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        let status = decoder
            .decompress(&payload[input_pos..], &mut buffer[..output_cap], flush)
            .map_err(decompress_io_error)?;
        let consumed = usize::try_from(decoder.total_in() - before_input)
            .expect("bounded zlib input progress fits usize");
        let produced = usize::try_from(decoder.total_out() - before_output)
            .expect("bounded zlib output progress fits usize");
        input_pos += consumed;
        total = total.checked_add(produced).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib decompressed length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
        if status == Status::StreamEnd {
            ensure_compressed_consumed(input_pos, payload.len(), "zlib stream")?;
            return Ok(BoundedLength::Exact(total));
        }
        if consumed == 0 && produced == 0 {
            return Err(std::io::Error::new(
                if input_pos == payload.len() {
                    std::io::ErrorKind::UnexpectedEof
                } else {
                    std::io::ErrorKind::InvalidData
                },
                if input_pos == payload.len() {
                    "truncated zlib stream"
                } else {
                    "zlib decoder made no progress"
                },
            ));
        }
    }
}

fn count_gzip_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    let len = count_bounded_bytes(&mut decoder, max)?;
    if matches!(len, BoundedLength::Exact(_)) {
        ensure_compressed_consumed(
            decoder.into_inner().position() as usize,
            payload.len(),
            "gzip member",
        )?;
    }
    Ok(len)
}

fn count_bounded_bytes(
    reader: &mut impl Read,
    max: usize,
) -> Result<BoundedLength, std::io::Error> {
    let mut total = 0_usize;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let remaining = max.saturating_sub(total);
        let read_cap = remaining.saturating_add(1).min(buffer.len());
        let read = reader.read(&mut buffer[..read_cap])?;
        if read == 0 {
            return Ok(BoundedLength::Exact(total));
        }
        total = total.checked_add(read).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "decompressed Anvil length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
    }
}

fn decompress_exact(
    comp: CompressionType,
    payload: &[u8],
    decoded_len: usize,
) -> Result<Vec<u8>, std::io::Error> {
    match comp {
        CompressionType::Uncompressed => {
            let mut out = allocate_exact_bytes(decoded_len, "uncompressed Anvil chunk")?;
            out.copy_from_slice(payload);
            Ok(out)
        }
        CompressionType::Zlib => decompress_zlib_exact(payload, decoded_len),
        CompressionType::Gzip => decompress_gzip_exact(payload, decoded_len),
        CompressionType::Lz4 => decompress_lz4_exact(payload, decoded_len),
    }
}

fn decompress_zlib_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed zlib Anvil chunk")?;
    let mut decoder = Decompress::new(true);
    let mut input_pos = 0_usize;
    let mut output_pos = 0_usize;
    let mut extra = [0_u8; 1];
    loop {
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let flush = if input_pos == payload.len() {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        let status = if output_pos < out.len() {
            decoder.decompress(&payload[input_pos..], &mut out[output_pos..], flush)
        } else {
            decoder.decompress(&payload[input_pos..], &mut extra, flush)
        }
        .map_err(decompress_io_error)?;
        let consumed = usize::try_from(decoder.total_in() - before_input)
            .expect("bounded zlib input progress fits usize");
        let produced = usize::try_from(decoder.total_out() - before_output)
            .expect("bounded zlib output progress fits usize");
        input_pos += consumed;
        if output_pos == out.len() && produced != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output changed between bounded passes",
            ));
        }
        output_pos = output_pos.checked_add(produced).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output length overflow",
            )
        })?;
        if output_pos > out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib output changed between bounded passes",
            ));
        }
        if status == Status::StreamEnd {
            ensure_compressed_consumed(input_pos, payload.len(), "zlib stream")?;
            if output_pos != decoded_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "zlib output changed between bounded passes",
                ));
            }
            return Ok(out);
        }
        if consumed == 0 && produced == 0 {
            return Err(std::io::Error::new(
                if input_pos == payload.len() {
                    std::io::ErrorKind::UnexpectedEof
                } else {
                    std::io::ErrorKind::InvalidData
                },
                if input_pos == payload.len() {
                    "truncated zlib stream"
                } else {
                    "zlib decoder made no progress"
                },
            ));
        }
    }
}

fn decompress_gzip_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed gzip Anvil chunk")?;
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    decoder.read_exact(&mut out)?;
    ensure_decoder_eof(&mut decoder, "gzip output changed between bounded passes")?;
    ensure_compressed_consumed(
        decoder.into_inner().position() as usize,
        payload.len(),
        "gzip member",
    )?;
    Ok(out)
}

fn ensure_decoder_eof(
    decoder: &mut impl Read,
    message: &'static str,
) -> Result<(), std::io::Error> {
    let mut extra = [0_u8; 1];
    if decoder.read(&mut extra)? != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        ));
    }
    Ok(())
}

fn ensure_compressed_consumed(
    consumed: usize,
    expected: usize,
    kind: &'static str,
) -> Result<(), std::io::Error> {
    if consumed != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "trailing bytes after {kind}: {}",
                expected.saturating_sub(consumed)
            ),
        ));
    }
    Ok(())
}

fn decompress_io_error(error: flate2::DecompressError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}

fn allocate_exact_bytes(len: usize, context: &'static str) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|error| {
        std::io::Error::other(format!("reserve {context} buffer of {len} bytes: {error}"))
    })?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn count_lz4_bytes(payload: &[u8], max: usize) -> Result<BoundedLength, std::io::Error> {
    let mut pos = 0_usize;
    let mut total = 0_usize;
    loop {
        let (token, compressed_len, decompressed_len) = read_lz4_block_header(payload, &mut pos)?;
        if compressed_len == 0 && decompressed_len == 0 {
            if pos != payload.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "trailing bytes after LZ4 end marker",
                ));
            }
            return Ok(BoundedLength::Exact(total));
        }
        validate_lz4_method(token, compressed_len, decompressed_len)?;
        total = total.checked_add(decompressed_len).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LZ4 decompressed length overflow",
            )
        })?;
        if total > max {
            return Ok(BoundedLength::TooLarge { at_least: total });
        }
        if payload.len() - pos < compressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block payload",
            ));
        }
        pos += compressed_len;
    }
}

fn decompress_lz4_exact(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut out = allocate_exact_bytes(decoded_len, "decompressed LZ4 Anvil chunk")?;
    let mut pos = 0_usize;
    let mut out_pos = 0_usize;
    loop {
        let (token, compressed_len, decompressed_len) = read_lz4_block_header(payload, &mut pos)?;
        if compressed_len == 0 && decompressed_len == 0 {
            if pos != payload.len() || out_pos != decoded_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LZ4 output changed between bounded passes",
                ));
            }
            return Ok(out);
        }
        validate_lz4_method(token, compressed_len, decompressed_len)?;
        if payload.len() - pos < compressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block payload",
            ));
        }
        let out_end = out_pos.checked_add(decompressed_len).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LZ4 output length overflow",
            )
        })?;
        if out_end > out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "LZ4 output changed between bounded passes",
            ));
        }
        let block = &payload[pos..pos + compressed_len];
        match token & 0xF0 {
            LZ4_BLOCK_METHOD_RAW => out[out_pos..out_end].copy_from_slice(block),
            LZ4_BLOCK_METHOD_COMPRESSED => {
                let written = lz4_flex::block::decompress_into(block, &mut out[out_pos..out_end])
                    .map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                })?;
                if written != decompressed_len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "LZ4 block decompressed length mismatch",
                    ));
                }
            }
            _ => unreachable!("LZ4 method validated before decode"),
        }
        pos += compressed_len;
        out_pos = out_end;
    }
}

fn read_lz4_block_header(
    payload: &[u8],
    pos: &mut usize,
) -> Result<(u8, usize, usize), std::io::Error> {
    if payload.len() - *pos < LZ4_BLOCK_HEADER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "truncated LZ4 block header",
        ));
    }
    if &payload[*pos..*pos + LZ4_BLOCK_MAGIC.len()] != LZ4_BLOCK_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing LZ4Block magic",
        ));
    }
    let token = payload[*pos + 8];
    let compressed_len = u32::from_le_bytes(
        payload[*pos + 9..*pos + 13]
            .try_into()
            .expect("4-byte slice"),
    ) as usize;
    let decompressed_len = u32::from_le_bytes(
        payload[*pos + 13..*pos + 17]
            .try_into()
            .expect("4-byte slice"),
    ) as usize;
    *pos += LZ4_BLOCK_HEADER_LEN;
    Ok((token, compressed_len, decompressed_len))
}

fn validate_lz4_method(
    token: u8,
    compressed_len: usize,
    decompressed_len: usize,
) -> Result<(), std::io::Error> {
    match token & 0xF0 {
        LZ4_BLOCK_METHOD_RAW if compressed_len == decompressed_len => Ok(()),
        LZ4_BLOCK_METHOD_RAW => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "raw LZ4 block length mismatch",
        )),
        LZ4_BLOCK_METHOD_COMPRESSED => Ok(()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown LZ4 block token {token:#04x}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    fn first_populated_chunk(path: &Path) -> Option<(u8, Vec<u8>)> {
        let bytes = std::fs::read(path).unwrap();
        for slot in 0..REGION_CHUNK_COUNT {
            let loc_off = slot * 4;
            let loc = u32::from_be_bytes(
                bytes[loc_off..loc_off + 4]
                    .try_into()
                    .expect("4-byte slice"),
            );
            if loc == 0 {
                continue;
            }
            let sector = loc >> 8;
            let start = sector as usize * SECTOR_SIZE;
            let len = u32::from_be_bytes(bytes[start..start + 4].try_into().expect("4-byte slice"))
                as usize;
            let compression = bytes[start + 4];
            let payload = bytes[start + 5..start + 4 + len].to_vec();
            return Some((compression, payload));
        }
        None
    }

    fn synthetic_region(compression: u8, payload: &[u8]) -> tempfile::NamedTempFile {
        synthetic_region_chunks(&[(0, compression, payload.to_vec())])
    }

    fn synthetic_region_chunks(chunks: &[(usize, u8, Vec<u8>)]) -> tempfile::NamedTempFile {
        let mut bytes = vec![0_u8; HEADER_BYTES];
        let mut next_sector = HEADER_SECTORS;
        for (slot, compression, payload) in chunks {
            assert!(*slot < REGION_CHUNK_COUNT);
            let body_len = 5 + payload.len();
            let sectors = body_len.div_ceil(SECTOR_SIZE).max(1);
            assert!(sectors <= usize::from(u8::MAX));
            let location = ((next_sector as u32) << 8) | sectors as u32;
            let loc_off = slot * 4;
            bytes[loc_off..loc_off + 4].copy_from_slice(&location.to_be_bytes());
            let ts_off = SECTOR_SIZE + slot * 4;
            bytes[ts_off..ts_off + 4]
                .copy_from_slice(&(1_700_000_000_u32 + *slot as u32).to_be_bytes());

            let chunk_start = next_sector * SECTOR_SIZE;
            bytes.resize((next_sector + sectors) * SECTOR_SIZE, 0);
            let declared_len = (payload.len() + 1) as u32;
            bytes[chunk_start..chunk_start + 4].copy_from_slice(&declared_len.to_be_bytes());
            bytes[chunk_start + 4] = *compression;
            bytes[chunk_start + 5..chunk_start + 5 + payload.len()].copy_from_slice(payload);
            next_sector += sectors;
        }

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        tmp
    }

    fn collect_with_limits(
        path: &Path,
        limits: RegionLimits,
    ) -> Result<Vec<ChunkPayload>, RegionError> {
        let mut chunks = Vec::new();
        read_region_with_limits(path, limits, |payload| chunks.push(payload))?;
        Ok(chunks)
    }

    fn test_limits(max_chunk_bytes: usize, max_region_bytes: usize) -> RegionLimits {
        RegionLimits {
            max_file_bytes: MAX_REGION_FILE_BYTES,
            max_chunk_bytes,
            max_region_bytes,
        }
    }

    fn gzip_payload(raw: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    fn zlib_payload(raw: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    fn lz4_block_payload(method: u8, block: &[u8], decompressed_len: usize) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(LZ4_BLOCK_MAGIC);
        payload.push(method);
        payload.extend_from_slice(&(block.len() as u32).to_le_bytes());
        payload.extend_from_slice(&(decompressed_len as u32).to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload.extend_from_slice(block);
        payload.extend_from_slice(LZ4_BLOCK_MAGIC);
        payload.push(LZ4_BLOCK_METHOD_RAW);
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload
    }

    fn raw_lz4_block_payload(raw: &[u8]) -> Vec<u8> {
        lz4_block_payload(LZ4_BLOCK_METHOD_RAW, raw, raw.len())
    }

    fn compressed_lz4_block_payload(raw: &[u8]) -> Vec<u8> {
        let compressed = lz4_flex::block::compress(raw);
        lz4_block_payload(LZ4_BLOCK_METHOD_COMPRESSED, &compressed, raw.len())
    }

    #[test]
    fn round_trip_synthetic_region() {
        let payloads = vec![
            ChunkPayload {
                local_x: 0,
                local_z: 0,
                timestamp: 1_700_000_000,
                uncompressed_nbt: b"hello".to_vec(),
            },
            ChunkPayload {
                local_x: 31,
                local_z: 31,
                timestamp: 1_700_000_500,
                uncompressed_nbt: vec![0xAA; 10_000],
            },
        ];

        let tmp = tempfile::NamedTempFile::new().unwrap();
        write_region(tmp.path(), &payloads).unwrap();

        let read = read_region(tmp.path()).unwrap();
        assert_eq!(read.len(), 2);
        let by_slot: std::collections::HashMap<(u8, u8), &ChunkPayload> =
            read.iter().map(|c| ((c.local_x, c.local_z), c)).collect();
        for orig in &payloads {
            let got = by_slot[&(orig.local_x, orig.local_z)];
            assert_eq!(got.timestamp, orig.timestamp);
            assert_eq!(got.uncompressed_nbt, orig.uncompressed_nbt);
        }
    }

    #[test]
    fn reads_synthetic_gzip_uncompressed_and_lz4_regions() {
        let raw = b"synthetic chunk payload bytes";
        for (compression, payload) in [
            (CompressionType::Gzip as u8, gzip_payload(raw)),
            (CompressionType::Zlib as u8, zlib_payload(raw)),
            (CompressionType::Uncompressed as u8, raw.to_vec()),
            (CompressionType::Lz4 as u8, raw_lz4_block_payload(raw)),
            (
                CompressionType::Lz4 as u8,
                compressed_lz4_block_payload(raw),
            ),
        ] {
            let region = synthetic_region(compression, &payload);

            let chunks = read_region(region.path()).unwrap();

            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].local_x, 0);
            assert_eq!(chunks[0].local_z, 0);
            assert_eq!(chunks[0].timestamp, 1_700_000_000);
            assert_eq!(chunks[0].uncompressed_nbt, raw);
        }
    }

    #[test]
    fn slot_reader_and_visitor_do_not_require_retaining_the_region() {
        let region = synthetic_region_chunks(&[
            (0, CompressionType::Uncompressed as u8, b"zero".to_vec()),
            (33, CompressionType::Uncompressed as u8, b"target".to_vec()),
        ]);

        let target = read_chunk(region.path(), 1, 1).unwrap().unwrap();
        assert_eq!(target.uncompressed_nbt, b"target");
        assert!(read_chunk(region.path(), 2, 2).unwrap().is_none());
        assert!(matches!(
            read_chunk(region.path(), 32, 0),
            Err(RegionError::InvalidChunkCoordinates { cx: 32, cz: 0 })
        ));

        let mut visited = Vec::new();
        visit_region(region.path(), |payload| {
            visited.push((payload.local_x, payload.local_z, payload.uncompressed_nbt));
        })
        .unwrap();
        assert_eq!(
            visited,
            vec![(0, 0, b"zero".to_vec()), (1, 1, b"target".to_vec())]
        );
    }

    #[test]
    fn slot_reader_isolated_from_unrelated_corrupt_slot() {
        let region = synthetic_region_chunks(&[
            (0, 0x7F, b"corrupt".to_vec()),
            (33, CompressionType::Uncompressed as u8, b"target".to_vec()),
        ]);

        let target = read_chunk(region.path(), 1, 1).unwrap().unwrap();
        assert_eq!(target.uncompressed_nbt, b"target");
        assert!(matches!(
            read_region(region.path()),
            Err(RegionError::UnknownCompression(0x7F))
        ));
    }

    #[test]
    fn rejects_region_file_above_sector_geometry_limit_before_body_read() {
        let region = tempfile::NamedTempFile::new().unwrap();
        region.as_file().set_len((HEADER_BYTES + 1) as u64).unwrap();
        let limits = RegionLimits {
            max_file_bytes: HEADER_BYTES as u64,
            max_chunk_bytes: 64,
            max_region_bytes: 64,
        };

        let error = collect_with_limits(region.path(), limits).unwrap_err();
        assert!(matches!(
            error,
            RegionError::RegionTooLarge { bytes, max }
                if bytes == (HEADER_BYTES + 1) as u64 && max == HEADER_BYTES as u64
        ));
    }

    #[test]
    fn rejects_decompression_bombs_for_every_codec_before_output_allocation() {
        let raw = vec![0xA5; 65];
        let declared_lz4_bomb = lz4_block_payload(LZ4_BLOCK_METHOD_COMPRESSED, &[0], raw.len());
        for (compression, payload) in [
            (CompressionType::Gzip as u8, gzip_payload(&raw)),
            (CompressionType::Zlib as u8, zlib_payload(&raw)),
            (CompressionType::Uncompressed as u8, raw.clone()),
            (CompressionType::Lz4 as u8, raw_lz4_block_payload(&raw)),
            (CompressionType::Lz4 as u8, declared_lz4_bomb),
        ] {
            let region = synthetic_region(compression, &payload);
            let error = collect_with_limits(region.path(), test_limits(64, 128)).unwrap_err();
            assert!(matches!(
                error,
                RegionError::DecompressedChunkTooLarge {
                    cx: 0,
                    cz: 0,
                    bytes: 65,
                    max: 64,
                }
            ));
        }
    }

    #[test]
    fn accepts_exact_chunk_limit_for_every_codec() {
        let raw = vec![0x5A; 64];
        for (compression, payload) in [
            (CompressionType::Gzip as u8, gzip_payload(&raw)),
            (CompressionType::Zlib as u8, zlib_payload(&raw)),
            (CompressionType::Uncompressed as u8, raw.clone()),
            (CompressionType::Lz4 as u8, raw_lz4_block_payload(&raw)),
            (
                CompressionType::Lz4 as u8,
                compressed_lz4_block_payload(&raw),
            ),
        ] {
            let region = synthetic_region(compression, &payload);
            let chunks = collect_with_limits(region.path(), test_limits(64, 64)).unwrap();
            assert_eq!(chunks[0].uncompressed_nbt, raw);
        }
    }

    #[test]
    fn rejects_aggregate_many_chunk_payloads_before_second_allocation() {
        let region = synthetic_region_chunks(&[
            (0, CompressionType::Uncompressed as u8, vec![1; 40]),
            (1, CompressionType::Uncompressed as u8, vec![2; 40]),
        ]);
        let mut visited = 0_usize;
        let error = read_region_with_limits(region.path(), test_limits(64, 64), |_| {
            visited += 1;
        })
        .unwrap_err();

        assert_eq!(visited, 1);
        assert!(matches!(
            error,
            RegionError::DecompressedRegionTooLarge { bytes: 80, max: 64 }
        ));
    }

    #[test]
    fn rejects_truncated_and_trailing_compressed_streams() {
        let raw = b"bounded compressed stream";
        let mut gzip = gzip_payload(raw);
        gzip.truncate(gzip.len() / 2);
        let mut zlib = zlib_payload(raw);
        zlib.truncate(zlib.len() / 2);
        let mut lz4 = raw_lz4_block_payload(raw);
        lz4.pop();
        for (compression, payload) in [
            (CompressionType::Gzip as u8, gzip),
            (CompressionType::Zlib as u8, zlib),
            (CompressionType::Lz4 as u8, lz4),
        ] {
            let region = synthetic_region(compression, &payload);
            let result = read_region(region.path());
            assert!(
                matches!(result, Err(RegionError::Decompress { cx: 0, cz: 0, .. })),
                "compression {compression} unexpectedly returned {result:?}"
            );
        }

        let mut gzip_trailing = gzip_payload(raw);
        gzip_trailing.push(0);
        let mut zlib_trailing = zlib_payload(raw);
        zlib_trailing.push(0);
        for (compression, payload) in [
            (CompressionType::Gzip as u8, gzip_trailing),
            (CompressionType::Zlib as u8, zlib_trailing),
        ] {
            let region = synthetic_region(compression, &payload);
            let result = read_region(region.path());
            assert!(
                matches!(result, Err(RegionError::Decompress { cx: 0, cz: 0, .. })),
                "compression {compression} unexpectedly returned {result:?}"
            );
        }
    }

    #[test]
    fn rejects_chunk_stream_truncated_inside_allocated_sector() {
        let region = synthetic_region(CompressionType::Uncompressed as u8, b"payload");
        region.as_file().set_len((HEADER_BYTES + 6) as u64).unwrap();

        assert!(matches!(
            read_region(region.path()),
            Err(RegionError::LengthOverrun {
                cx: 0,
                cz: 0,
                bytes_available: 6,
                ..
            })
        ));
    }

    #[test]
    fn rejects_unknown_and_oversized_compression_flags() {
        let unknown = synthetic_region(0x7F, b"payload");
        match read_region(unknown.path()) {
            Err(RegionError::UnknownCompression(0x7F)) => {}
            other => panic!("expected unknown compression, got {other:?}"),
        }

        let oversized = synthetic_region(0x80 | CompressionType::Zlib as u8, b"payload");
        match read_region(oversized.path()) {
            Err(RegionError::Oversized) => {}
            other => panic!("expected oversized rejection, got {other:?}"),
        }
    }

    /// Read every chunk out of the real vanilla r.0.0.mca, decompress
    /// each, and assert the payload starts with the NBT named-root
    /// header (`0x0a` = Compound, followed by a length-prefixed name).
    /// Opt-in because the vanilla test world is a local artifact.
    #[test]
    #[ignore = "requires local .analysis/test-world vanilla region"]
    fn reads_real_vanilla_region() {
        let path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        assert!(
            path.is_file(),
            "{} not present; run tools/generate-test-world.sh",
            path.display()
        );
        let chunks = read_region(&path).unwrap();
        assert!(!chunks.is_empty(), "spawn region should have chunks");
        for c in &chunks {
            assert!(c.uncompressed_nbt.len() > 100, "NBT should be non-trivial");
            assert_eq!(c.uncompressed_nbt[0], 0x0A, "root tag must be Compound");
        }
    }

    /// Round-trip every chunk in the real region: read, then write
    /// out to a fresh file, then read that back, then confirm the
    /// decompressed payloads match byte-for-byte.
    #[test]
    #[ignore = "requires local .analysis/test-world vanilla region"]
    fn round_trip_real_vanilla_region() {
        let path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        assert!(
            path.is_file(),
            "{} not present; run tools/generate-test-world.sh",
            path.display()
        );
        let original = read_region(&path).unwrap();
        assert!(!original.is_empty(), "vanilla oracle region has no chunks");
        let tmp = tempfile::NamedTempFile::new().unwrap();
        write_region(tmp.path(), &original).unwrap();
        let reread = read_region(tmp.path()).unwrap();
        assert_eq!(reread.len(), original.len());
        let by_slot: std::collections::HashMap<(u8, u8), &ChunkPayload> =
            reread.iter().map(|c| ((c.local_x, c.local_z), c)).collect();
        for orig in &original {
            let got = by_slot[&(orig.local_x, orig.local_z)];
            assert_eq!(got.timestamp, orig.timestamp);
            assert_eq!(
                got.uncompressed_nbt, orig.uncompressed_nbt,
                "decompressed NBT must round-trip for slot ({},{})",
                orig.local_x, orig.local_z
            );
        }
    }

    /// Prove the local vanilla oracle was actually written with
    /// compression byte 4 before exercising the Anvil reader. Skipped
    /// against the opt-in local LZ4 oracle world.
    #[test]
    #[ignore = "requires local .analysis/test-world-lz4 vanilla region"]
    fn reads_real_vanilla_lz4_region() {
        let path = workspace_path(".analysis/test-world-lz4/region/r.0.0.mca");
        assert!(
            path.is_file(),
            "{} not present; run OUT_DIR=.analysis/test-world-lz4 REGION_FILE_COMPRESSION=lz4 tools/generate-test-world.sh",
            path.display()
        );

        let (compression, payload) = first_populated_chunk(&path).unwrap();
        assert_eq!(compression, CompressionType::Lz4 as u8);
        assert_eq!(&payload[..LZ4_BLOCK_MAGIC.len()], LZ4_BLOCK_MAGIC);

        let chunks = read_region(&path).unwrap();
        assert!(!chunks.is_empty(), "spawn region should have chunks");
        for c in &chunks {
            assert!(c.uncompressed_nbt.len() > 100, "NBT should be non-trivial");
            assert_eq!(c.uncompressed_nbt[0], 0x0A, "root tag must be Compound");
        }
    }

    #[test]
    #[ignore = "requires local .analysis/test-world-lz4 vanilla region"]
    fn lz4_block_api_matches_real_vanilla_lz4_payload() {
        let path = workspace_path(".analysis/test-world-lz4/region/r.0.0.mca");
        assert!(
            path.is_file(),
            "{} not present; run OUT_DIR=.analysis/test-world-lz4 REGION_FILE_COMPRESSION=lz4 tools/generate-test-world.sh",
            path.display()
        );

        let (compression, payload) = first_populated_chunk(&path).unwrap();
        assert_eq!(compression, CompressionType::Lz4 as u8);
        let decoded_len = match count_lz4_bytes(&payload, MAX_DECOMPRESSED_CHUNK_BYTES).unwrap() {
            BoundedLength::Exact(len) => len,
            BoundedLength::TooLarge { at_least } => {
                panic!("real LZ4 chunk unexpectedly exceeds limit: {at_least}")
            }
        };
        let nbt = decompress_lz4_exact(&payload, decoded_len).unwrap();
        assert_eq!(nbt[0], 0x0A);

        let mut frame = lz4_flex::frame::FrameDecoder::new(payload.as_slice());
        let mut frame_out = Vec::new();
        assert!(frame.read_to_end(&mut frame_out).is_err());
    }
}
