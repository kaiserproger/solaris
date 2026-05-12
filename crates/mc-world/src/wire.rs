//! Chunk → wire-payload conversion for the `LevelChunkWithLight`
//! packet. Lives in `mc-world` (and not `mc-protocol`) because the
//! conversion needs `Chunk` / `ChunkSection` / `BiomeSection`
//! internals; in exchange, the protocol crate stays unaware of the
//! world model.
//!
//! Two outputs:
//!
//! - [`encode_chunk_data`] — the section-by-section paletted-
//!   container blob that fills `LevelChunkWithLight.data`.
//! - [`client_heightmaps`] — the `(type_id, long[])` pairs that
//!   make up `LevelChunkWithLight.heightmaps`.
//!
//! Wire layout (per
//! `net.minecraft.world.level.chunk.LevelChunkSection.write`,
//! ADR 0002 javap):
//!
//! ```text
//! for each of 24 sections, concatenated:
//!     i16  non_air_block_count    (vanilla's `nonEmptyBlockCount`)
//!     i16  fluid_count            (always 0 — we don't model fluids yet)
//!     PalettedContainer<BlockState>:
//!         u8       bits_per_entry
//!         Palette  (see below)
//!         i64[N]   raw packed entries, no length prefix
//!                  (`writeFixedSizeLongArray`)
//!     PalettedContainer<Biome>:    same shape, smaller bpe range
//! ```
//!
//! Vanilla's palette dispatch by `bits_per_entry` (verified via
//! `Strategy.getConfigurationForBitCount`):
//!
//! ```text
//!  bpe   block strategy            biome strategy
//!  0     SingleValuePalette        SingleValuePalette
//!  1..3  (blocks pad to 4)         LinearPalette (1 / 2 / 3 bits)
//!  4     LinearPalette             — (uses direct above 3)
//!  5..8  HashMapPalette            — (uses direct above 3)
//!  ≥ 9   GlobalPalette (direct)    GlobalPalette (direct, ≥ 4 for biomes)
//! ```
//!
//! Wire shapes per palette kind:
//!
//! ```text
//! SingleValuePalette : VarInt single_entry
//! LinearPalette      : VarInt size, VarInt[size] entries
//! HashMapPalette     : VarInt size, VarInt[size] entries
//! GlobalPalette      : (nothing — entries are raw global ids
//!                       in the long[])
//! ```
//!
//! M3.b produces **single-value** and **indirect** outputs; "direct"
//! / GlobalPalette emission is M3+ territory and not used by any of
//! the test-world chunks we exercise.

use mc_data::{Identifier, Registry};
use thiserror::Error;

use crate::chunk::{BIOME_VOLUME, BiomeSection, Chunk, LIGHT_LAYER_BYTES, SECTION_COUNT};
use crate::light::ChunkLight;
use crate::section::{ChunkSection, SECTION_DIM, SECTION_VOLUME};

/// Heightmap type ids — the ordinals of `Heightmap$Types` in the
/// vanilla source, verified via `javap -p -c` (ADR 0002). Only the
/// three entries with `Usage.CLIENT` are sent on the wire.
pub mod heightmap_type {
    pub const WORLD_SURFACE: i32 = 1;
    pub const MOTION_BLOCKING: i32 = 4;
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// Things that can go wrong producing a `LevelChunkWithLight` body.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    /// A biome identifier in the chunk's biome palette is not in the
    /// supplied registry. Indicates a registry / world mismatch (e.g.
    /// the world was generated against a newer datapack); we surface
    /// it instead of silently substituting.
    #[error("biome {0} not present in the supplied registry")]
    UnknownBiome(Identifier),
}

/// One heightmap entry ready for `LevelChunkWithLight.heightmaps`.
/// Mirrors the structure of `mc_protocol::packets::play::ChunkHeightmap`
/// without depending on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightmapEntry {
    pub type_id: i32,
    pub data: Vec<i64>,
}

/// Build the CLIENT-usage heightmaps from a chunk in the order
/// vanilla emits them (`WORLD_SURFACE`, `MOTION_BLOCKING`,
/// `MOTION_BLOCKING_NO_LEAVES`). Entries are skipped silently when
/// the chunk has no heightmap of that name — vanilla rejects unknown
/// keys but happily accepts a subset.
#[must_use]
pub fn client_heightmaps(chunk: &Chunk) -> Vec<HeightmapEntry> {
    const ORDER: [(&str, i32); 3] = [
        ("WORLD_SURFACE", heightmap_type::WORLD_SURFACE),
        ("MOTION_BLOCKING", heightmap_type::MOTION_BLOCKING),
        (
            "MOTION_BLOCKING_NO_LEAVES",
            heightmap_type::MOTION_BLOCKING_NO_LEAVES,
        ),
    ];
    ORDER
        .iter()
        .filter_map(|(name, type_id)| {
            chunk.heightmaps.get(*name).map(|h| HeightmapEntry {
                type_id: *type_id,
                data: h.to_long_array(),
            })
        })
        .collect()
}

/// Encode all 24 sections of `chunk` into the paletted-container blob
/// that fills `LevelChunkWithLight.data`.
///
/// `biomes` is the `worldgen/biome` registry; the chunk's biome
/// palette stores identifiers and the wire format needs numeric
/// registry indices, so the registry has to be supplied.
pub fn encode_chunk_data(chunk: &Chunk, biomes: &Registry) -> Result<Vec<u8>, WireError> {
    debug_assert_eq!(chunk.sections.len(), SECTION_COUNT);
    debug_assert_eq!(chunk.biomes.len(), SECTION_COUNT);
    let mut buf = Vec::new();
    for (sec, bsec) in chunk.sections.iter().zip(chunk.biomes.iter()) {
        // i16 non_air_block_count + i16 fluid_count (we don't model
        // fluids yet, so always 0). Both big-endian on the wire.
        buf.extend_from_slice(&(sec.non_air_count() as i16).to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes());
        encode_block_palette(&mut buf, sec);
        encode_biome_palette(&mut buf, bsec, biomes)?;
    }
    Ok(buf)
}

fn encode_block_palette(buf: &mut Vec<u8>, section: &ChunkSection) {
    match (section.palette(), section.indices()) {
        (None, _) => {
            // Section is in Single mode — vanilla's
            // SingleValuePalette: bpe=0, VarInt single state id, no
            // backing storage.
            //
            // `palette() == None` implies the section was constructed
            // via `ChunkSection::filled(state, _)`; `state` is what
            // every cell holds, recoverable from `get(0,0,0)`.
            buf.push(0);
            write_varint(
                buf,
                i32::try_from(section.get(0, 0, 0).0).expect("state id < i32::MAX"),
            );
            // No bit-storage longs.
        }
        (Some(palette), Some(indices)) => {
            buf.push(indices.bits_per_entry());
            write_varint(
                buf,
                i32::try_from(palette.len()).expect("palette len < i32::MAX"),
            );
            for state in palette {
                write_varint(buf, i32::try_from(state.0).expect("state id < i32::MAX"));
            }
            for &word in indices.words() {
                buf.extend_from_slice(&(word as i64).to_be_bytes());
            }
        }
        (Some(_), None) => unreachable!("indirect sections always have indices"),
    }
}

fn encode_biome_palette(
    buf: &mut Vec<u8>,
    section: &BiomeSection,
    registry: &Registry,
) -> Result<(), WireError> {
    match section {
        BiomeSection::Single(biome) => {
            let id = registry_index_of(registry, biome)?;
            buf.push(0);
            write_varint(buf, id);
        }
        BiomeSection::Indirect { palette, indices } => {
            debug_assert_eq!(indices.len(), BIOME_VOLUME);
            buf.push(indices.bits_per_entry());
            write_varint(
                buf,
                i32::try_from(palette.len()).expect("palette len < i32::MAX"),
            );
            for biome in palette {
                write_varint(buf, registry_index_of(registry, biome)?);
            }
            for &word in indices.words() {
                buf.extend_from_slice(&(word as i64).to_be_bytes());
            }
        }
    }
    Ok(())
}

fn registry_index_of(registry: &Registry, biome: &Identifier) -> Result<i32, WireError> {
    registry
        .entries
        .iter()
        .position(|e| e == biome)
        .map(|p| p as i32)
        .ok_or_else(|| WireError::UnknownBiome(biome.clone()))
}

// ---------------------------------------------------------------------
// Light wire encoding (M4.d)
// ---------------------------------------------------------------------

/// Wire-ready light payload for one chunk. Mirrors the shape of
/// `mc_protocol::packets::play::LightData` so `mc-net::play` can lift
/// fields straight through without translation. Held here (and not in
/// `mc-protocol`) so the conversion from `ChunkLight` doesn't drag
/// world-model types into the protocol crate, matching the same
/// posture `encode_chunk_data` / `client_heightmaps` already establish.
///
/// Y-slot indexing: 26 slots covering section-Y `-5..=20` (one slab
/// below the bottom of the world, 24 in-world sections, one slab
/// above the top). Slot 0 = `Y=-5`, slot 25 = `Y=20`. Slot `i`
/// corresponds to in-world chunk section index `i - 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightWire {
    pub sky_y_mask: Vec<i64>,
    pub block_y_mask: Vec<i64>,
    pub empty_sky_y_mask: Vec<i64>,
    pub empty_block_y_mask: Vec<i64>,
    pub sky_updates: Vec<Vec<u8>>,
    pub block_updates: Vec<Vec<u8>>,
}

/// Total number of wire light slots (one below + 24 in-world + one
/// above).
pub const WIRE_LIGHT_SECTIONS: usize = SECTION_COUNT + 2;

/// Pack a per-chunk computed [`ChunkLight`] into the wire payload
/// `LevelChunkWithLight` expects. Always emits all 26 slots: each
/// section is either listed in the `*_y_mask` with its 2048-byte
/// nibble layer, or in the `empty_*_y_mask`. The below-world slab is
/// emitted empty for both channels; the above-world slab is emitted
/// as `sky=15` (open sky) and `block=0` (empty for block channel).
#[must_use]
pub fn encode_chunk_light(light: &ChunkLight) -> LightWire {
    let mut sky_mask: u64 = 0;
    let mut block_mask: u64 = 0;
    let mut empty_sky_mask: u64 = 0;
    let mut empty_block_mask: u64 = 0;
    let mut sky_updates: Vec<Vec<u8>> = Vec::new();
    let mut block_updates: Vec<Vec<u8>> = Vec::new();

    // Slot 0 (Y=-5, below world): empty for both channels.
    empty_sky_mask |= 1 << 0;
    empty_block_mask |= 1 << 0;

    // Slots 1..=24 — in-world sections 0..=23 of the ChunkLight.
    for section_idx in 0..SECTION_COUNT {
        let slot = section_idx + 1;
        let sky_layer = pack_section_layer(&light.sky, section_idx);
        let block_layer = pack_section_layer(&light.block, section_idx);
        if sky_layer.iter().any(|&b| b != 0) {
            sky_mask |= 1 << slot;
            sky_updates.push(sky_layer);
        } else {
            empty_sky_mask |= 1 << slot;
        }
        if block_layer.iter().any(|&b| b != 0) {
            block_mask |= 1 << slot;
            block_updates.push(block_layer);
        } else {
            empty_block_mask |= 1 << slot;
        }
    }

    // Slot 25 (Y=20, above world): sky=15 (open sky), block empty.
    sky_mask |= 1 << 25;
    sky_updates.push(vec![0xFF; LIGHT_LAYER_BYTES]);
    empty_block_mask |= 1 << 25;

    LightWire {
        sky_y_mask: vec![sky_mask as i64],
        block_y_mask: vec![block_mask as i64],
        empty_sky_y_mask: vec![empty_sky_mask as i64],
        empty_block_y_mask: vec![empty_block_mask as i64],
        sky_updates,
        block_updates,
    }
}

/// Pack one section's 4096 per-cell `u8` values (0..=15 each) into
/// the 2048-byte nibble layer vanilla expects: cells are paired by
/// the linear `(y, z, x)` index, low nibble first.
fn pack_section_layer(channel: &[u8], section_idx: usize) -> Vec<u8> {
    debug_assert!(section_idx < SECTION_COUNT);
    let base = section_idx * SECTION_VOLUME;
    let mut layer = vec![0u8; LIGHT_LAYER_BYTES];
    // section-local cell index = (sub_y * 256) + (z * 16) + x, sub_y in 0..16.
    for cell in 0..SECTION_VOLUME {
        let sub_y = cell / (SECTION_DIM * SECTION_DIM);
        let rem = cell % (SECTION_DIM * SECTION_DIM);
        let z = rem / SECTION_DIM;
        let x = rem % SECTION_DIM;
        // ChunkLight indexes by absolute y_local first, so reconstruct
        // the source index: (section_base_y + sub_y) * 256 + z * 16 + x.
        let world_local_y = section_idx * SECTION_DIM + sub_y;
        let src = world_local_y * (SECTION_DIM * SECTION_DIM) + z * SECTION_DIM + x;
        debug_assert!(src >= base);
        debug_assert!(src < base + SECTION_VOLUME);
        let value = channel[src] & 0x0F;
        let byte_idx = cell / 2;
        let shift = (cell & 1) * 4;
        layer[byte_idx] |= value << shift;
    }
    layer
}

fn write_varint(buf: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            buf.push(v as u8);
            return;
        }
        buf.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockStateId;
    use crate::chunk::{Chunk, ChunkPos, Heightmap};
    use mc_data::Registry;

    const AIR: BlockStateId = BlockStateId(0);
    const BEDROCK: BlockStateId = BlockStateId(74);

    fn biome_registry() -> Registry {
        Registry {
            id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
            entries: vec![
                Identifier::parse("minecraft:badlands").unwrap(),
                Identifier::parse("minecraft:plains").unwrap(),
                Identifier::parse("minecraft:the_void").unwrap(),
            ],
        }
    }

    fn empty_chunk() -> Chunk {
        Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            AIR,
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }

    // ---- byte-level assertions ----

    #[test]
    fn empty_chunk_emits_single_value_air_sections() {
        let chunk = empty_chunk();
        let bytes = encode_chunk_data(&chunk, &biome_registry()).unwrap();

        // Per section: i16(0) i16(0) | u8(0) VarInt(0) | u8(0) VarInt(1)
        //            = 2 + 2 + 1 + 1 + 1 + 1 = 8 bytes.
        // 24 sections × 8 = 192 bytes total.
        assert_eq!(bytes.len(), 24 * 8);

        let expected: [u8; 8] = [
            0x00, 0x00, // non_air_count = 0
            0x00, 0x00, // fluid_count = 0
            0x00, // block bpe = 0
            0x00, // block VarInt(0) = AIR
            0x00, // biome bpe = 0
            0x01, // biome VarInt(1) = plains
        ];
        for sec in 0..24 {
            let start = sec * 8;
            assert_eq!(
                &bytes[start..start + 8],
                &expected,
                "section {sec} mismatches expected all-air/all-plains layout"
            );
        }
    }

    #[test]
    fn single_non_air_block_picks_up_promoted_palette() {
        // Place one bedrock at the section-local origin of section 0.
        let mut chunk = empty_chunk();
        chunk.set_block(0, -64, 0, BEDROCK).unwrap();

        let bytes = encode_chunk_data(&chunk, &biome_registry()).unwrap();

        // Section 0 is now indirect:
        //   i16 non_air = 1, i16 fluid = 0          (4 bytes)
        //   u8 bpe = 4                              (1 byte)
        //   VarInt palette_len = 2                  (1 byte)
        //   VarInt(AIR = 0)                         (1 byte)
        //   VarInt(BEDROCK = 74)                    (1 byte)
        //   long[256]: 4096 entries × 4 bits / 64 bits_per_word
        //              = 256 words × 8 bytes_per_word
        //                                          (2048 bytes)
        //   Biome single-value: u8(0) + VarInt(plains = 1)
        //                                           (2 bytes)
        let block_long_bytes = 256 * 8;
        let want_len = 4 + 1 + 1 + 1 + 1 + block_long_bytes + 2;

        assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]); // counts
        assert_eq!(bytes[4], 4, "blocks bpe should be 4");
        assert_eq!(bytes[5], 2, "palette size = 2");
        assert_eq!(bytes[6], 0, "palette[0] = AIR (state 0)");
        // BEDROCK=74 is 0x4A, fits in one VarInt byte.
        assert_eq!(bytes[7], 0x4A);

        // The first 4-bit entry in the packed array must be palette
        // index 1 (BEDROCK). In vanilla's "non-crossing" layout the
        // first entry occupies bits 0..3 of word 0.
        let word0 = i64::from_be_bytes(bytes[8..16].try_into().unwrap()) as u64;
        assert_eq!(word0 & 0xF, 1, "cell (0,0,0) maps to palette[1]");

        // Biomes still single-value.
        let biome_off = 4 + 1 + 1 + 1 + 1 + block_long_bytes;
        assert_eq!(bytes[biome_off], 0, "biome bpe = 0");
        assert_eq!(bytes[biome_off + 1], 1, "biome = plains (index 1)");

        // Sections 1..24 are still single-value air + plains
        // (8 bytes each, see `empty_chunk_emits_single_value_air_sections`).
        assert_eq!(bytes.len(), want_len + 23 * 8);
    }

    // ---- client_heightmaps ordering ----

    #[test]
    fn client_heightmaps_skip_worldgen_only_kinds() {
        let mut chunk = empty_chunk();
        // Worldgen-only heightmap that the client should NOT see.
        chunk
            .heightmaps
            .insert("OCEAN_FLOOR_WG".into(), Heightmap::zeroed());
        // Two client-visible heightmaps in arbitrary insertion order.
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

        let entries = client_heightmaps(&chunk);
        assert_eq!(entries.len(), 2, "WG-only key excluded");
        // Output is in WORLD_SURFACE, MOTION_BLOCKING, … order
        // regardless of HashMap insertion order.
        assert_eq!(entries[0].type_id, heightmap_type::WORLD_SURFACE);
        assert_eq!(entries[1].type_id, heightmap_type::MOTION_BLOCKING);
    }

    #[test]
    fn client_heightmaps_passes_long_array_through_unchanged() {
        let mut chunk = empty_chunk();
        let mut hm = Heightmap::zeroed();
        hm.set(3, 5, 42);
        let expected = hm.to_long_array();
        chunk.heightmaps.insert("MOTION_BLOCKING".into(), hm);

        let entries = client_heightmaps(&chunk);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data, expected);
    }

    // ---- error cases ----

    #[test]
    fn unknown_biome_surfaces_as_error() {
        let chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            AIR,
            Identifier::parse("minecraft:nope").unwrap(),
        );
        let err = encode_chunk_data(&chunk, &biome_registry()).unwrap_err();
        assert_eq!(
            err,
            WireError::UnknownBiome(Identifier::parse("minecraft:nope").unwrap())
        );
    }

    // ---- VarInt helper sanity ----

    #[test]
    fn write_varint_matches_known_encodings() {
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (25565, &[0xDD, 0xC7, 0x01]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ];
        for (value, expected) in cases {
            let mut buf = Vec::new();
            write_varint(&mut buf, *value);
            assert_eq!(&buf, expected, "VarInt({value})");
        }
    }

    // ---- real-world chunk smoke test ----
    //
    // Skipped silently when the test world is absent (same pattern as
    // M2's round-trip oracle).

    #[test]
    fn encodes_real_test_world_chunk_zero_zero() {
        use crate::WorldStorage;
        use std::path::PathBuf;

        let world_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.analysis/test-world");
        if !world_dir.exists() {
            eprintln!(
                "skipping: {} missing (run tools/generate-test-world.sh)",
                world_dir.display()
            );
            return;
        }

        let report_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/vanilla/reports/blocks.json");
        if !report_path.exists() {
            eprintln!(
                "skipping: {} missing (run tools/extract-vanilla-data.sh --reports)",
                report_path.display()
            );
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&report_path).expect("blocks.json loads");
        let registry = std::sync::Arc::new(
            crate::BlockRegistry::from_report(&report).expect("block registry builds"),
        );

        let mut storage = WorldStorage::open(&world_dir, registry).expect("test world opens");
        let chunk = storage
            .get_chunk(ChunkPos { x: 0, z: 0 })
            .expect("chunk (0,0) reads without error")
            .expect("chunk (0,0) present in r.0.0.mca")
            .clone();

        // Synthesise a 3-entry biome registry with plains at index 1
        // (matching the test-world generator's default).
        let registry = biome_registry();
        let bytes = encode_chunk_data(&chunk, &registry).expect("encode");

        // Structural checks: 24 sections, each ≥ minimum length.
        // Minimum per section = 4 (counts) + 2 (single-value block:
        // u8 bpe + VarInt id, both 1 byte for state 0) + 2 (single-
        // value biome) = 8 bytes. Real sections with mixed content
        // are larger.
        assert!(bytes.len() >= 24 * 8);

        // The bottom section of the flat test world must report a
        // non-zero non_air_count (bedrock + dirt + grass occupy the
        // lowest few Y layers).
        let first_count = i16::from_be_bytes([bytes[0], bytes[1]]);
        assert!(
            first_count > 0,
            "section 0 of the flat preset should have non-air blocks"
        );
    }

    fn chunk_light_idx(x: usize, local_y: usize, z: usize) -> usize {
        local_y * (SECTION_DIM * SECTION_DIM) + z * SECTION_DIM + x
    }

    #[test]
    fn encodes_zero_chunk_as_all_empty_with_open_top() {
        let light = ChunkLight::zeroed();
        let wire = encode_chunk_light(&light);

        // Slot 25 (above world) is the only present sky slot;
        // everything else is empty in the sky channel.
        assert_eq!(wire.sky_y_mask.len(), 1);
        assert_eq!(wire.sky_y_mask[0], 1 << 25);
        assert_eq!(wire.sky_updates.len(), 1);
        assert!(wire.sky_updates[0].iter().all(|&b| b == 0xFF));
        assert_eq!(wire.empty_sky_y_mask.len(), 1);
        // All 26 slots except slot 25 are empty.
        let expected_empty_sky = ((1u64 << WIRE_LIGHT_SECTIONS) - 1) & !(1 << 25);
        assert_eq!(wire.empty_sky_y_mask[0], expected_empty_sky as i64);

        // Block channel: all 26 slots empty, no layers shipped.
        assert!(wire.block_updates.is_empty());
        assert_eq!(wire.block_y_mask[0], 0);
        let expected_empty_block = (1u64 << WIRE_LIGHT_SECTIONS) - 1;
        assert_eq!(wire.empty_block_y_mask[0], expected_empty_block as i64);
    }

    #[test]
    fn nibble_packing_round_trips_low_first() {
        // Build a ChunkLight whose section 0 has cell index 0 = 1,
        // cell index 1 = 2, both in the layer's first byte. Then
        // verify pack_section_layer puts them at low (0x01) and
        // high (0x20) nibbles → 0x21 in byte 0.
        let mut light = ChunkLight::zeroed();
        // cell 0 in section 0 = (sub_y=0, z=0, x=0) → chunk-light idx 0.
        light.sky[chunk_light_idx(0, 0, 0)] = 1;
        // cell 1 in section 0 = (sub_y=0, z=0, x=1).
        light.sky[chunk_light_idx(1, 0, 0)] = 2;
        let layer = pack_section_layer(&light.sky, 0);
        assert_eq!(layer[0], 0x21);
        // Other bytes are untouched.
        assert!(layer[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn pack_layer_sees_each_section_independently() {
        // Place value 15 at the very last cell of section 0
        // (sub_y=15, z=15, x=15) and value 7 at the very first cell
        // of section 1 (sub_y=0, z=0, x=0). pack_section_layer(_, 0)
        // must see only the 15; pack_section_layer(_, 1) only the 7.
        let mut light = ChunkLight::zeroed();
        // Section 0, sub_y=15 → chunk-light local_y = 15.
        light.sky[chunk_light_idx(15, 15, 15)] = 15;
        // Section 1, sub_y=0 → chunk-light local_y = 16.
        light.sky[chunk_light_idx(0, 16, 0)] = 7;

        let s0 = pack_section_layer(&light.sky, 0);
        let s1 = pack_section_layer(&light.sky, 1);
        // Section 0: last cell is index 4095 in the section, byte 2047,
        // high nibble.
        assert_eq!(s0[2047], 0xF0);
        let zeros_in_s0: usize = s0[..2047].iter().filter(|&&b| b == 0).count();
        assert_eq!(zeros_in_s0, 2047);
        // Section 1: first cell is index 0, byte 0, low nibble = 7.
        assert_eq!(s1[0], 0x07);
        assert!(s1[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn encodes_non_empty_block_section_into_present_mask() {
        // Drop a single glowstone-equivalent cell into section 4
        // (around world Y=0). Expect: section 4 + 1 = wire slot 5 in
        // the present mask; slot 0 + slots 1..=4 + slots 6..=24 +
        // slot 25 in the empty mask; one layer in block_updates.
        let mut light = ChunkLight::zeroed();
        light.block[chunk_light_idx(8, 64, 8)] = 15; // section idx = 64/16 = 4
        let wire = encode_chunk_light(&light);

        assert_eq!(wire.block_updates.len(), 1, "single non-zero section");
        assert_eq!(wire.block_y_mask[0], 1 << 5);

        let layer = &wire.block_updates[0];
        // Cell index within section 4: sub_y=0, z=8, x=8 →
        // 0 * 256 + 8 * 16 + 8 = 136. Byte 68, low nibble (136 even).
        assert_eq!(layer[68], 0x0F);
        let nonzero_bytes: usize = layer.iter().filter(|&&b| b != 0).count();
        assert_eq!(nonzero_bytes, 1, "only one cell lit");
    }
}
