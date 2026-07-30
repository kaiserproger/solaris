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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use thiserror::Error;

/// 32 × 32 chunks per region.
pub const CHUNKS_PER_REGION_AXIS: usize = 32;
const REGION_CHUNK_COUNT: usize = CHUNKS_PER_REGION_AXIS * CHUNKS_PER_REGION_AXIS;
const SECTOR_SIZE: usize = 4096;
const HEADER_SECTORS: usize = 2; // locations + timestamps
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
}

/// Read every populated chunk out of a region file. Empty slots are
/// silently skipped. Returns chunks in slot-order
/// `(cz * 32 + cx)`.
pub fn read_region(path: impl AsRef<Path>) -> Result<Vec<ChunkPayload>, RegionError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| RegionError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    if bytes.len() < HEADER_SECTORS * SECTOR_SIZE {
        return Err(RegionError::TooShort {
            needed: HEADER_SECTORS * SECTOR_SIZE,
            got: bytes.len(),
        });
    }
    // Vanilla allocates per-chunk in sector-sized chunks but the
    // total file may not be padded to a sector boundary at EOF, so
    // don't require alignment of the file as a whole. We still
    // verify that every chunk's declared allocation fits inside.
    let total_sectors = bytes.len().div_ceil(SECTOR_SIZE);
    let mut out = Vec::new();
    for slot in 0..REGION_CHUNK_COUNT {
        let cx = (slot % CHUNKS_PER_REGION_AXIS) as u8;
        let cz = (slot / CHUNKS_PER_REGION_AXIS) as u8;

        let loc_off = slot * 4;
        let loc = u32::from_be_bytes(
            bytes[loc_off..loc_off + 4]
                .try_into()
                .expect("4-byte slice"),
        );
        let sector = loc >> 8;
        let count = (loc & 0xFF) as u8;

        if sector == 0 && count == 0 {
            continue;
        }
        if count == 0 {
            return Err(RegionError::ZeroSectorCount { cx, cz });
        }
        let start = sector as usize * SECTOR_SIZE;
        let end = (start + count as usize * SECTOR_SIZE).min(bytes.len());
        if start >= bytes.len() {
            return Err(RegionError::SectorOutOfRange {
                cx,
                cz,
                sector,
                sector_count: total_sectors,
            });
        }

        // 4-byte length, 1-byte compression, payload, trailing pad.
        let ts_off = SECTOR_SIZE + slot * 4;
        let timestamp =
            u32::from_be_bytes(bytes[ts_off..ts_off + 4].try_into().expect("4-byte slice"));

        let chunk_bytes = &bytes[start..end];
        if chunk_bytes.len() < 5 {
            return Err(RegionError::LengthOverrun {
                cx,
                cz,
                len: 0,
                bytes_available: chunk_bytes.len(),
            });
        }
        let len = u32::from_be_bytes(chunk_bytes[..4].try_into().expect("4-byte slice"));
        let comp_byte = chunk_bytes[4];
        if comp_byte & 0x80 != 0 {
            return Err(RegionError::Oversized);
        }
        let comp = CompressionType::from_byte(comp_byte)?;
        let payload_len = (len as usize)
            .checked_sub(1)
            .ok_or(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available: chunk_bytes.len(),
            })?;
        if 5 + payload_len > chunk_bytes.len() {
            return Err(RegionError::LengthOverrun {
                cx,
                cz,
                len,
                bytes_available: chunk_bytes.len(),
            });
        }
        let payload = &chunk_bytes[5..5 + payload_len];

        let uncompressed =
            decompress(comp, payload).map_err(|e| RegionError::Decompress { cx, cz, source: e })?;

        out.push(ChunkPayload {
            local_x: cx,
            local_z: cz,
            timestamp,
            uncompressed_nbt: uncompressed,
        });
    }
    Ok(out)
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

fn decompress(comp: CompressionType, payload: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    match comp {
        CompressionType::Uncompressed => Ok(payload.to_vec()),
        CompressionType::Zlib => {
            let mut out = Vec::with_capacity(payload.len() * 4);
            ZlibDecoder::new(payload).read_to_end(&mut out)?;
            Ok(out)
        }
        CompressionType::Gzip => {
            let mut out = Vec::with_capacity(payload.len() * 4);
            GzDecoder::new(payload).read_to_end(&mut out)?;
            Ok(out)
        }
        CompressionType::Lz4 => decompress_lz4_block_stream(payload),
    }
}

fn decompress_lz4_block_stream(payload: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut pos = 0;
    let mut out = Vec::with_capacity(payload.len() * 4);
    loop {
        if payload.len() - pos < LZ4_BLOCK_HEADER_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block header",
            ));
        }
        if &payload[pos..pos + LZ4_BLOCK_MAGIC.len()] != LZ4_BLOCK_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing LZ4Block magic",
            ));
        }

        let token = payload[pos + 8];
        let method = token & 0xF0;
        let compressed_len =
            u32::from_le_bytes(payload[pos + 9..pos + 13].try_into().expect("4-byte slice"))
                as usize;
        let decompressed_len = u32::from_le_bytes(
            payload[pos + 13..pos + 17]
                .try_into()
                .expect("4-byte slice"),
        ) as usize;
        pos += LZ4_BLOCK_HEADER_LEN;

        if compressed_len == 0 && decompressed_len == 0 {
            if pos != payload.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "trailing bytes after LZ4 end marker",
                ));
            }
            return Ok(out);
        }
        if payload.len() - pos < compressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated LZ4 block payload",
            ));
        }

        let block = &payload[pos..pos + compressed_len];
        match method {
            LZ4_BLOCK_METHOD_RAW => {
                if compressed_len != decompressed_len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "raw LZ4 block length mismatch",
                    ));
                }
                out.extend_from_slice(block);
            }
            LZ4_BLOCK_METHOD_COMPRESSED => {
                let decompressed = lz4_flex::block::decompress(block, decompressed_len)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if decompressed.len() != decompressed_len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "LZ4 block decompressed length mismatch",
                    ));
                }
                out.extend_from_slice(&decompressed);
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown LZ4 block token {token:#04x}"),
                ));
            }
        }
        pos += compressed_len;
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
        let body_len = 5 + payload.len();
        let sectors = body_len.div_ceil(SECTOR_SIZE).max(1);
        let mut bytes = vec![0u8; (HEADER_SECTORS + sectors) * SECTOR_SIZE];
        let location = ((HEADER_SECTORS as u32) << 8) | sectors as u32;
        bytes[0..4].copy_from_slice(&location.to_be_bytes());
        bytes[SECTOR_SIZE..SECTOR_SIZE + 4].copy_from_slice(&1_700_000_000u32.to_be_bytes());
        let chunk_start = HEADER_SECTORS * SECTOR_SIZE;
        let declared_len = (payload.len() + 1) as u32;
        bytes[chunk_start..chunk_start + 4].copy_from_slice(&declared_len.to_be_bytes());
        bytes[chunk_start + 4] = compression;
        bytes[chunk_start + 5..chunk_start + 5 + payload.len()].copy_from_slice(payload);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        tmp
    }

    fn gzip_payload(raw: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    fn raw_lz4_block_payload(raw: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(LZ4_BLOCK_MAGIC);
        payload.push(LZ4_BLOCK_METHOD_RAW);
        payload.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        payload.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(raw);
        payload.extend_from_slice(LZ4_BLOCK_MAGIC);
        payload.push(LZ4_BLOCK_METHOD_RAW);
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload
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
            (CompressionType::Uncompressed as u8, raw.to_vec()),
            (CompressionType::Lz4 as u8, raw_lz4_block_payload(raw)),
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
        let nbt = decompress_lz4_block_stream(&payload).unwrap();
        assert_eq!(nbt[0], 0x0A);

        let mut frame = lz4_flex::frame::FrameDecoder::new(payload.as_slice());
        let mut frame_out = Vec::new();
        assert!(frame.read_to_end(&mut frame_out).is_err());
    }
}
