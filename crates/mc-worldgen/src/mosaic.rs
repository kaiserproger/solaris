//! Deterministic PNG mosaics sampled from the production terrain planner.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::TerrainGenerator;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MosaicConfig {
    pub center_x: i32,
    pub center_z: i32,
    pub extent_blocks: u32,
    pub blocks_per_pixel: u32,
}

impl MosaicConfig {
    pub fn dimensions(&self) -> Result<(u32, u32), MosaicError> {
        if self.extent_blocks == 0 {
            return Err(MosaicError::InvalidConfig(
                "extent must be greater than zero",
            ));
        }
        if self.blocks_per_pixel == 0 {
            return Err(MosaicError::InvalidConfig(
                "blocks per pixel must be greater than zero",
            ));
        }
        if !self.extent_blocks.is_multiple_of(self.blocks_per_pixel) {
            return Err(MosaicError::InvalidConfig(
                "extent must be divisible by blocks per pixel",
            ));
        }
        let side = self.extent_blocks / self.blocks_per_pixel;
        Ok((side, side))
    }

    fn minimum(&self) -> Result<(i32, i32), MosaicError> {
        let half = i64::from(self.extent_blocks) / 2;
        let min_x = i64::from(self.center_x) - half;
        let min_z = i64::from(self.center_z) - half;
        let last_offset = i64::from(self.extent_blocks.saturating_sub(1));
        if min_x < i64::from(i32::MIN)
            || min_z < i64::from(i32::MIN)
            || min_x + last_offset > i64::from(i32::MAX)
            || min_z + last_offset > i64::from(i32::MAX)
        {
            return Err(MosaicError::InvalidConfig(
                "requested mosaic exceeds supported world coordinates",
            ));
        }
        Ok((min_x as i32, min_z as i32))
    }
}

#[derive(Debug)]
pub struct MosaicImages {
    pub width: u32,
    pub height: u32,
    pub height_png: Vec<u8>,
    pub biome_png: Vec<u8>,
    pub vegetation_png: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum MosaicError {
    #[error("invalid mosaic configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("mosaic image is too large")]
    ImageTooLarge,
    #[error("failed to encode PNG: {0}")]
    Encode(#[source] std::io::Error),
    #[error("failed to create mosaic output directory {path}: {source}")]
    CreateOutput {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write mosaic image {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn render_mosaic(
    generator: &TerrainGenerator,
    config: &MosaicConfig,
) -> Result<MosaicImages, MosaicError> {
    let (width, height) = config.dimensions()?;
    let (min_x, min_z) = config.minimum()?;
    let pixel_count = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| MosaicError::ImageTooLarge)?;
    let byte_count = pixel_count
        .checked_mul(3)
        .ok_or(MosaicError::ImageTooLarge)?;
    let mut heights = Vec::with_capacity(byte_count);
    let mut biomes = Vec::with_capacity(byte_count);
    let mut vegetation = Vec::with_capacity(byte_count);
    let sample_offset = config.blocks_per_pixel / 2;

    for pixel_z in 0..height {
        for pixel_x in 0..width {
            let block_x = i64::from(min_x)
                + i64::from(pixel_x) * i64::from(config.blocks_per_pixel)
                + i64::from(sample_offset);
            let block_z = i64::from(min_z)
                + i64::from(pixel_z) * i64::from(config.blocks_per_pixel)
                + i64::from(sample_offset);
            let sample = generator.diagnostic_sample(block_x as i32, block_z as i32);
            heights.extend_from_slice(&height_color(sample.surface_y));
            biomes.extend_from_slice(&biome_color(sample.biome.path()));
            vegetation.extend_from_slice(&vegetation_color(sample.vegetation_density));
        }
    }

    Ok(MosaicImages {
        width,
        height,
        height_png: encode_rgb_png(width, height, &heights)?,
        biome_png: encode_rgb_png(width, height, &biomes)?,
        vegetation_png: encode_rgb_png(width, height, &vegetation)?,
    })
}

pub fn write_mosaic(images: &MosaicImages, output_dir: &Path) -> Result<(), MosaicError> {
    fs::create_dir_all(output_dir).map_err(|source| MosaicError::CreateOutput {
        path: output_dir.to_path_buf(),
        source,
    })?;
    for (name, bytes) in [
        ("height.png", images.height_png.as_slice()),
        ("biome.png", images.biome_png.as_slice()),
        ("vegetation.png", images.vegetation_png.as_slice()),
    ] {
        let path = output_dir.join(name);
        fs::write(&path, bytes).map_err(|source| MosaicError::Write { path, source })?;
    }
    Ok(())
}

fn height_color(surface_y: i32) -> [u8; 3] {
    if surface_y <= 63 {
        let depth = (63 - surface_y).clamp(0, 127) as u8;
        [
            18,
            80_u8.saturating_sub(depth / 3),
            150_u8.saturating_sub(depth / 2),
        ]
    } else {
        let altitude = (surface_y - 63).clamp(0, 192) as u8;
        [
            46_u8.saturating_add(altitude),
            96_u8.saturating_add(altitude / 2),
            38_u8.saturating_add(altitude / 3),
        ]
    }
}

fn biome_color(path: &str) -> [u8; 3] {
    if path.contains("deep_ocean") {
        [20, 45, 120]
    } else if path.contains("ocean") {
        [35, 90, 170]
    } else if path.contains("river") {
        [55, 125, 205]
    } else if path.contains("beach") || path.contains("shore") {
        [226, 207, 130]
    } else if path.contains("badlands") {
        [184, 86, 45]
    } else if path.contains("desert") {
        [224, 196, 96]
    } else if path.contains("jungle") {
        [28, 118, 48]
    } else if path.contains("swamp") {
        [67, 91, 54]
    } else if path.contains("snow") || path.contains("frozen") || path.contains("grove") {
        [205, 224, 230]
    } else if path.contains("taiga") {
        [70, 116, 92]
    } else if path.contains("mountain")
        || path.contains("peak")
        || path.contains("stony")
        || path.contains("windswept")
    {
        [125, 126, 120]
    } else if path.contains("forest") {
        [44, 126, 55]
    } else if path.contains("savanna") {
        [164, 173, 68]
    } else {
        [103, 171, 74]
    }
}

fn vegetation_color(density: Option<f64>) -> [u8; 3] {
    let Some(density) = density else {
        return [52, 49, 43];
    };
    let intensity = (((density.clamp(-1.0, 1.0) + 1.0) * 0.5) * 255.0).round() as u8;
    [
        24_u8.saturating_add(intensity / 8),
        48_u8.saturating_add((u16::from(intensity) * 3 / 4) as u8),
        27_u8.saturating_add(intensity / 4),
    ]
}

fn encode_rgb_png(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, MosaicError> {
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(3))
        .ok_or(MosaicError::ImageTooLarge)?;
    let raw_capacity = row_bytes
        .checked_add(1)
        .and_then(|row| row.checked_mul(height as usize))
        .ok_or(MosaicError::ImageTooLarge)?;
    let mut raw = Vec::with_capacity(raw_capacity);
    for row in pixels.chunks_exact(row_bytes) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).map_err(MosaicError::Encode)?;
    let compressed = encoder.finish().map_err(MosaicError::Encode)?;

    let mut png = Vec::with_capacity(compressed.len() + 57);
    png.extend_from_slice(PNG_SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    append_chunk(&mut png, b"IHDR", &ihdr);
    append_chunk(&mut png, b"IDAT", &compressed);
    append_chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

fn append_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for &byte in kind.iter().chain(data) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    output.extend_from_slice(&(!crc).to_be_bytes());
}
