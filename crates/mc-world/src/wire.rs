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
//! The network storage is vanilla `SimpleBitStorage`: entries do not
//! cross `i64` boundaries, so each word may have unused high bits when
//! `64 % bits_per_entry != 0`.

use std::io::Cursor;

use mc_data::items::ItemRegistry;
use mc_data::{Identifier, Registry};
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use crate::block::BlockRegistry;
use crate::chunk::{
    BIOME_VOLUME, BiomeSection, BlockPos, ChestBlockEntity, Chunk, FurnaceBlockEntity, FurnaceSlot,
    LIGHT_LAYER_BYTES, SECTION_COUNT,
};
use crate::light::{ChunkLight, LightLayer};
use crate::section::{ChunkSection, PackedBitArray, SECTION_VOLUME};

/// Bits per entry vanilla 26.1.2 expects in `GlobalPalette` (direct)
/// mode for the block-state container. Sized to comfortably hold the
/// 29 873 states the bundled jar enumerates
/// (`ceil(log2(29 873)) = 15`); when the registry grows in a future
/// patch the encoder uses the wider of this constant and the actual
/// ceiling computed from the registry size.
const DIRECT_BITS: u8 = 15;
/// Above this bit-width the wire switches from the LinearPalette /
/// HashMapPalette indirect formats to GlobalPalette (direct). Matches
/// vanilla's threshold (`Strategy.getConfigurationForBitCount`).
const DIRECT_BITS_THRESHOLD: u8 = 9;
const BIOME_DIRECT_BITS_THRESHOLD: u8 = 4;

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

#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityWireRecord {
    pub pos: BlockPos,
    pub type_name: Identifier,
    pub nbt: Tag,
}

pub fn client_block_entities(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: &ItemRegistry,
) -> Vec<BlockEntityWireRecord> {
    let mut records = Vec::new();
    let mut opaque_entries: Vec<_> = chunk
        .block_entities
        .iter()
        .filter(|(pos, _)| !chunk.furnaces.contains_key(pos) && !chunk.chests.contains_key(pos))
        .collect();
    opaque_entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, bytes) in opaque_entries {
        let mut cur = Cursor::new(bytes.as_slice());
        let Ok(tag) = mc_nbt::read_network(&mut cur) else {
            continue;
        };
        let Some(type_name) = block_entity_type_name(&tag) else {
            continue;
        };
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: strip_persistent_block_entity_fields(tag),
        });
    }

    let mut furnaces: Vec<_> = chunk.furnaces.iter().collect();
    furnaces.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, furnace) in furnaces {
        let type_name = furnace_block_entity_type_for_block(chunk, registry, *pos);
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: furnace_update_tag(furnace, items),
        });
    }

    let mut chests: Vec<_> = chunk.chests.iter().collect();
    chests.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, chest) in chests {
        let type_name = chest_block_entity_type_for_block(chunk, registry, *pos);
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: chest_update_tag(chest, items),
        });
    }

    records
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

fn block_entity_type_name(tag: &Tag) -> Option<Identifier> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    fields.iter().find_map(|(key, value)| {
        if key == "id"
            && let Tag::String(id) = value
        {
            Identifier::parse(id.clone()).ok()
        } else {
            None
        }
    })
}

fn strip_persistent_block_entity_fields(tag: Tag) -> Tag {
    let Tag::Compound(fields) = tag else {
        return tag;
    };
    Tag::Compound(
        fields
            .into_iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "id" | "x"
                        | "y"
                        | "z"
                        | "CookingTimes"
                        | "CookingTotalTimes"
                        | "solaris_cooking_remaining"
                        | "solaris_cooking_total"
                )
            })
            .collect(),
    )
}

fn furnace_block_entity_type_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> Identifier {
    let local_x = pos.x.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
    let path = chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .map(|state| state.block.id.as_str());
    match path {
        Some("minecraft:smoker") => Identifier::parse("minecraft:smoker").unwrap(),
        Some("minecraft:blast_furnace") => Identifier::parse("minecraft:blast_furnace").unwrap(),
        _ => Identifier::parse("minecraft:furnace").unwrap(),
    }
}

fn chest_block_entity_type_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> Identifier {
    let local_x = pos.x.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
    let path = chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .map(|state| state.block.id.as_str());
    match path {
        Some("minecraft:barrel") => Identifier::parse("minecraft:barrel").unwrap(),
        _ => Identifier::parse("minecraft:chest").unwrap(),
    }
}

fn furnace_update_tag(furnace: &FurnaceBlockEntity, items: &ItemRegistry) -> Tag {
    Tag::Compound(vec![
        ("Items".into(), item_list_tag(&furnace.slots, items)),
        (
            "lit_time_remaining".into(),
            Tag::Short(furnace.burn_remaining),
        ),
        ("lit_total_time".into(), Tag::Short(furnace.burn_total)),
        (
            "cooking_time_spent".into(),
            Tag::Short(furnace.cook_progress),
        ),
        ("cooking_total_time".into(), Tag::Short(furnace.cook_total)),
    ])
}

fn chest_update_tag(chest: &ChestBlockEntity, items: &ItemRegistry) -> Tag {
    Tag::Compound(vec![("Items".into(), item_list_tag(&chest.slots, items))])
}

fn item_list_tag(slots: &[FurnaceSlot], items: &ItemRegistry) -> Tag {
    let item_tags = slots
        .iter()
        .enumerate()
        .filter_map(|(slot, stack)| {
            if stack.is_empty() {
                return None;
            }
            let name = items.name_of(stack.item_id)?;
            Some(Tag::Compound(vec![
                ("Slot".into(), Tag::Int(slot as i32)),
                ("id".into(), Tag::String(name.as_str().to_string())),
                ("count".into(), Tag::Int(stack.count)),
            ]))
        })
        .collect::<Vec<_>>();
    Tag::List(ListTag {
        element_type: if item_tags.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements: item_tags,
    })
}

/// Encode all sections of `chunk` into the paletted-container blob
/// that fills `LevelChunkWithLight.data`.
///
/// `biomes` is the `worldgen/biome` registry; the chunk's biome
/// palette stores identifiers and the wire format needs numeric
/// registry indices, so the registry has to be supplied.
pub fn encode_chunk_data(chunk: &Chunk, biomes: &Registry) -> Result<Vec<u8>, WireError> {
    debug_assert_eq!(chunk.sections.len(), chunk.geometry().section_count());
    debug_assert_eq!(chunk.biomes.len(), chunk.geometry().section_count());
    let mut buf = Vec::with_capacity(chunk.geometry().section_count() * 16);
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
            if indices.bits_per_entry() >= DIRECT_BITS_THRESHOLD {
                encode_block_direct(buf, palette, indices);
            } else {
                buf.push(indices.bits_per_entry());
                write_varint(
                    buf,
                    i32::try_from(palette.len()).expect("palette len < i32::MAX"),
                );
                for state in palette {
                    write_varint(buf, i32::try_from(state.0).expect("state id < i32::MAX"));
                }
                for word in pack_fixed_longs(indices.bits_per_entry(), indices.len(), |idx| {
                    indices.get(idx)
                }) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            }
        }
        (Some(_), None) => unreachable!("indirect sections always have indices"),
    }
}

/// Emit a section's block container in vanilla's `GlobalPalette`
/// (direct) shape: a single `bits_per_entry` byte, *no* palette
/// section, then a packed long array containing every cell's raw
/// global state-id at [`DIRECT_BITS`] per entry. Triggered when our
/// internal palette has grown past the indirect threshold
/// (~256 entries → ≥ 9 bits per palette index).
fn encode_block_direct(
    buf: &mut Vec<u8>,
    palette: &[crate::block::BlockStateId],
    indices: &PackedBitArray,
) {
    let bits = DIRECT_BITS.max(indices.bits_per_entry());
    let direct = pack_fixed_longs(bits, SECTION_VOLUME, |cell| {
        let p = indices.get(cell) as usize;
        palette[p].0
    });
    buf.push(bits);
    // GlobalPalette: no palette VarInts.
    for word in direct {
        buf.extend_from_slice(&(word as i64).to_be_bytes());
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
            if indices.bits_per_entry() >= BIOME_DIRECT_BITS_THRESHOLD {
                let direct_bits = bits_for_distinct_values(registry.entries.len());
                let palette_ids = palette
                    .iter()
                    .map(|biome| registry_index_of(registry, biome).map(|id| id as u32))
                    .collect::<Result<Vec<_>, _>>()?;
                buf.push(direct_bits);
                for word in pack_fixed_longs(direct_bits, indices.len(), |idx| {
                    let p = indices.get(idx) as usize;
                    palette_ids[p]
                }) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            } else {
                buf.push(indices.bits_per_entry());
                write_varint(
                    buf,
                    i32::try_from(palette.len()).expect("palette len < i32::MAX"),
                );
                for biome in palette {
                    write_varint(buf, registry_index_of(registry, biome)?);
                }
                for word in pack_fixed_longs(indices.bits_per_entry(), indices.len(), |idx| {
                    indices.get(idx)
                }) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            }
        }
    }
    Ok(())
}

fn pack_fixed_longs<F>(bits_per_entry: u8, len: usize, mut value_at: F) -> Vec<u64>
where
    F: FnMut(usize) -> u32,
{
    let bits = bits_per_entry as usize;
    debug_assert!((1..=32).contains(&bits_per_entry));
    let entries_per_word = (64 / bits).max(1);
    let mask = (1u64 << bits) - 1;
    let mut words = vec![0u64; len.div_ceil(entries_per_word)];
    for idx in 0..len {
        let value = value_at(idx) as u64 & mask;
        let word_index = idx / entries_per_word;
        let bit_offset = (idx % entries_per_word) * bits;
        words[word_index] |= value << bit_offset;
    }
    words
}

fn bits_for_distinct_values(len: usize) -> u8 {
    if len <= 1 {
        return 1;
    }
    (len - 1).ilog2() as u8 + 1
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
/// Y-slot indexing contains one slab below the world, all in-world
/// sections, and one slab above the world. Slot `i` corresponds to
/// in-world chunk section index `i - 1`.
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
/// `LevelChunkWithLight` expects. Always emits every light slot: each
/// section is either listed in the `*_y_mask` with its 2048-byte
/// nibble layer, or in the `empty_*_y_mask`. The below-world slab is
/// emitted empty for both channels; the above-world slab is emitted
/// as `sky=15` (open sky) and `block=0` (empty for block channel).
#[must_use]
pub fn encode_chunk_light(light: &ChunkLight) -> LightWire {
    let section_count = light.section_count();
    let wire_light_sections = section_count + 2;
    assert!(wire_light_sections <= u64::BITS as usize);
    let mut sky_mask: u64 = 0;
    let mut block_mask: u64 = 0;
    let mut empty_sky_mask: u64 = 0;
    let mut empty_block_mask: u64 = 0;
    let mut sky_updates: Vec<Vec<u8>> = Vec::new();
    let mut block_updates: Vec<Vec<u8>> = Vec::new();

    // Slot 0 (Y=-5, below world): empty for both channels.
    empty_sky_mask |= 1 << 0;
    empty_block_mask |= 1 << 0;

    // In-world sections occupy slots 1..=section_count.
    for section_idx in 0..section_count {
        let slot = section_idx + 1;
        let sky_layer = pack_section_layer(&light.sky, section_idx);
        let block_layer = pack_section_layer(&light.block, section_idx);
        if let Some(sky_layer) = sky_layer {
            sky_mask |= 1 << slot;
            sky_updates.push(sky_layer);
        } else {
            empty_sky_mask |= 1 << slot;
        }
        if let Some(block_layer) = block_layer {
            block_mask |= 1 << slot;
            block_updates.push(block_layer);
        } else {
            empty_block_mask |= 1 << slot;
        }
    }

    // The slot above the world is sky=15 (open sky), block empty.
    let top_slot = section_count + 1;
    sky_mask |= 1 << top_slot;
    sky_updates.push(vec![0xFF; LIGHT_LAYER_BYTES]);
    empty_block_mask |= 1 << top_slot;

    LightWire {
        sky_y_mask: vec![sky_mask as i64],
        block_y_mask: vec![block_mask as i64],
        empty_sky_y_mask: vec![empty_sky_mask as i64],
        empty_block_y_mask: vec![empty_block_mask as i64],
        sky_updates,
        block_updates,
    }
}

/// Copy one section's 2048-byte nibble layer if it contains non-zero
/// light. Missing lazy sections are all-zero and omitted from the wire
/// present mask.
fn pack_section_layer(channel: &LightLayer, section_idx: usize) -> Option<Vec<u8>> {
    let layer = channel.section(section_idx)?;
    Some(layer.to_vec())
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
    use crate::chunk::{Chunk, ChunkGeometry, ChunkPos, FurnaceSlot, Heightmap};
    use mc_data::Registry;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::{ItemRegistry, ItemReport};
    use std::collections::BTreeMap;

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

    fn numbered_biome_registry(len: usize) -> Registry {
        Registry {
            id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
            entries: (0..len)
                .map(|i| Identifier::parse(format!("minecraft:biome_{i}")).unwrap())
                .collect(),
        }
    }

    fn empty_chunk() -> Chunk {
        Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            AIR,
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }

    fn air_chest_registry() -> BlockRegistry {
        BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:chest").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap()
    }

    fn item_registry() -> ItemRegistry {
        ItemRegistry::from_report(&[ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 9,
        }])
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
    fn custom_geometry_encodes_chunk_data_and_baked_light_sections() {
        let geometry = ChunkGeometry::new(0, 256).expect("valid custom geometry");
        let mut chunk = Chunk::empty_with_geometry(
            ChunkPos { x: 0, z: 0 },
            AIR,
            Identifier::parse("minecraft:plains").unwrap(),
            geometry,
        );

        let data = encode_chunk_data(&chunk, &biome_registry()).expect("encode chunk data");
        assert_eq!(data.len(), geometry.section_count() * 8);

        chunk.section_lights[15].block = Some(vec![0x0F; LIGHT_LAYER_BYTES]);
        let light = ChunkLight::from_section_lights(&chunk.section_lights)
            .expect("rebuild baked custom-geometry light");
        let wire = encode_chunk_light(&light);

        assert_eq!(wire.block_y_mask, vec![1 << 16]);
        assert_eq!(wire.block_updates.len(), 1);
        assert_eq!(wire.block_updates[0], vec![0x0F; LIGHT_LAYER_BYTES]);
        assert_eq!(wire.sky_y_mask, vec![1 << 17]);
        assert_eq!(wire.sky_updates, vec![vec![0xFF; LIGHT_LAYER_BYTES]]);
        assert_eq!(wire.empty_block_y_mask, vec![(1 << 18) - 1 - (1 << 16)]);
        assert_eq!(wire.empty_sky_y_mask, vec![(1 << 18) - 1 - (1 << 17)]);
    }

    #[test]
    fn client_block_entities_emit_stripped_chest_update_tag() {
        let registry = air_chest_registry();
        let items = item_registry();
        let mut chunk = empty_chunk();
        let pos = BlockPos { x: 2, y: 64, z: 3 };
        chunk.set_block(2, 64, 3, BlockStateId(1)).unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = FurnaceSlot {
            count: 2,
            item_id: 9,
            damage: None,
            enchantments: Vec::new(),
        };
        chunk.chests.insert(pos, chest);

        let records = client_block_entities(&chunk, &registry, &items);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pos, pos);
        assert_eq!(records[0].type_name.as_str(), "minecraft:chest");
        let Tag::Compound(fields) = &records[0].nbt else {
            panic!("expected compound update tag");
        };
        assert!(
            fields
                .iter()
                .all(|(key, _)| !matches!(key.as_str(), "id" | "x" | "y" | "z"))
        );
        assert!(fields.iter().any(|(key, _)| key == "Items"));
    }

    #[test]
    fn client_block_entities_strip_solaris_campfire_persistence_fields() {
        let registry = air_chest_registry();
        let items = item_registry();
        let mut chunk = empty_chunk();
        let pos = BlockPos { x: 2, y: 64, z: 3 };
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:campfire".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
            (
                "Items".into(),
                Tag::List(ListTag {
                    element_type: tag_type::END,
                    elements: Vec::new(),
                }),
            ),
            ("CookingTimes".into(), Tag::IntArray(vec![1, 0, 0, 0])),
            ("CookingTotalTimes".into(), Tag::IntArray(vec![4, 0, 0, 0])),
            (
                "solaris_cooking_remaining".into(),
                Tag::IntArray(vec![3, 0, 0, 0]),
            ),
            (
                "solaris_cooking_total".into(),
                Tag::IntArray(vec![4, 0, 0, 0]),
            ),
        ]);
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &tag).expect("encode campfire block entity");
        chunk.block_entities.insert(pos, bytes);

        let records = client_block_entities(&chunk, &registry, &items);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].type_name.as_str(), "minecraft:campfire");
        let Tag::Compound(fields) = &records[0].nbt else {
            panic!("expected compound update tag");
        };
        assert!(fields.iter().any(|(key, _)| key == "Items"));
        assert!(fields.iter().all(|(key, _)| {
            !matches!(
                key.as_str(),
                "id" | "x"
                    | "y"
                    | "z"
                    | "CookingTimes"
                    | "CookingTotalTimes"
                    | "solaris_cooking_remaining"
                    | "solaris_cooking_total"
            )
        }));
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

        let mut storage = match WorldStorage::open(&world_dir, registry) {
            Ok(storage) => storage,
            Err(err) => {
                eprintln!("skipping: {} ({err})", world_dir.display());
                return;
            }
        };
        let chunk = storage
            .get_chunk(ChunkPos { x: 0, z: 0 })
            .expect("chunk (0,0) reads without error")
            .expect("chunk (0,0) present in r.0.0.mca")
            .clone();

        let data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        let data = match mc_data::load(&data_dir) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("skipping: {} ({err})", data_dir.display());
                return;
            }
        };
        let registry = data
            .registry("worldgen/biome")
            .expect("vanilla data carries biome registry");
        let bytes = encode_chunk_data(&chunk, registry).expect("encode");

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
        // cell 0 in section 0 = (sub_y=0, z=0, x=0).
        light.set_sky_local(0, 0, 0, 1);
        // cell 1 in section 0 = (sub_y=0, z=0, x=1).
        light.set_sky_local(1, 0, 0, 2);
        let layer = pack_section_layer(&light.sky, 0).unwrap();
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
        light.set_sky_local(15, 15, 15, 15);
        // Section 1, sub_y=0 → chunk-light local_y = 16.
        light.set_sky_local(0, 16, 0, 7);

        let s0 = pack_section_layer(&light.sky, 0).unwrap();
        let s1 = pack_section_layer(&light.sky, 1).unwrap();
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
        light.set_block_local(8, 64, 8, 15); // section idx = 64/16 = 4
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

    #[test]
    fn block_palette_switches_to_direct_mode_past_256_entries() {
        // M5.c.3: build a section with 260 distinct synthetic states
        // and ensure the wire encoder emits the GlobalPalette
        // (direct) shape — bits_per_entry first byte = DIRECT_BITS,
        // VarInt palette length absent, raw packed long array follows.
        use crate::section::ChunkSection;
        let mut section = ChunkSection::filled(AIR, AIR);
        for i in 1..=260u32 {
            let cell = i - 1;
            let x = (cell & 0x0F) as u8;
            let y = ((cell >> 4) & 0x0F) as u8;
            let z = ((cell >> 8) & 0x0F) as u8;
            section.set(x, y, z, BlockStateId(i));
        }

        let mut buf = Vec::new();
        super::encode_block_palette(&mut buf, &section);

        assert_eq!(buf[0], super::DIRECT_BITS, "first byte = direct bits");
        // No VarInt palette length: the next bytes are the packed
        // long array. Compute the expected long count.
        let bits = super::DIRECT_BITS as usize;
        let expected_longs = SECTION_VOLUME.div_ceil(64 / bits);
        assert_eq!(
            buf.len() - 1,
            expected_longs * 8,
            "direct-mode body = {expected_longs} longs (no palette section)"
        );
    }

    #[test]
    fn block_palette_uses_vanilla_fixed_longs_for_five_bit_indirect() {
        use crate::section::ChunkSection;
        let mut section = ChunkSection::filled(AIR, AIR);
        for i in 1..=17u32 {
            let cell = i - 1;
            section.set(
                (cell & 0x0F) as u8,
                ((cell >> 4) & 0x0F) as u8,
                0,
                BlockStateId(i),
            );
        }

        let mut buf = Vec::new();
        super::encode_block_palette(&mut buf, &section);

        assert_eq!(buf[0], 5, "17 states require five bits");
        let mut offset = 1;
        assert_eq!(buf[offset], 18, "palette includes air plus 17 states");
        offset += 1;
        for state in 0..=17u8 {
            assert_eq!(buf[offset], state);
            offset += 1;
        }
        let expected_longs = SECTION_VOLUME.div_ceil(64 / 5);
        assert_eq!(buf.len() - offset, expected_longs * 8);

        let word0 = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap());
        let word1 = u64::from_be_bytes(buf[offset + 8..offset + 16].try_into().unwrap());
        assert_eq!(word0 >> 60, 0, "top four bits are padding");
        assert_eq!(word1 & 0x1F, 13, "entry 12 starts a fresh word");
    }

    #[test]
    fn biome_palette_uses_vanilla_fixed_longs_for_three_bit_indirect() {
        let registry = numbered_biome_registry(8);
        let palette: Vec<_> = registry.entries[..5].to_vec();
        let mut indices = PackedBitArray::zeroed(3, BIOME_VOLUME);
        for idx in 0..BIOME_VOLUME {
            indices.set(idx, (idx % palette.len()) as u32);
        }
        let section = BiomeSection::from_indirect(palette, indices);

        let mut buf = Vec::new();
        super::encode_biome_palette(&mut buf, &section, &registry).unwrap();

        assert_eq!(buf[0], 3, "five biomes stay indirect at three bits");
        let mut offset = 1;
        assert_eq!(buf[offset], 5, "palette length");
        offset += 1 + 5;
        let expected_longs = BIOME_VOLUME.div_ceil(64 / 3);
        assert_eq!(buf.len() - offset, expected_longs * 8);

        let word0 = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap());
        let word1 = u64::from_be_bytes(buf[offset + 8..offset + 16].try_into().unwrap());
        assert_eq!(word0 >> 63, 0, "top bit is padding");
        assert_eq!(word1 & 0x07, 1, "entry 21 starts a fresh word");
    }

    #[test]
    fn biome_palette_switches_to_direct_mode_above_three_bits() {
        let registry = numbered_biome_registry(10);
        let palette = registry.entries.clone();
        let mut indices = PackedBitArray::zeroed(4, BIOME_VOLUME);
        for idx in 0..BIOME_VOLUME {
            indices.set(idx, (idx % palette.len()) as u32);
        }
        let section = BiomeSection::from_indirect(palette, indices);

        let mut buf = Vec::new();
        super::encode_biome_palette(&mut buf, &section, &registry).unwrap();

        assert_eq!(buf[0], 4, "ten-entry biome registry needs four direct bits");
        let expected_longs = BIOME_VOLUME.div_ceil(64 / 4);
        assert_eq!(
            buf.len() - 1,
            expected_longs * 8,
            "direct biome body has no palette length or entries"
        );
    }

    #[test]
    fn block_palette_stays_indirect_for_small_palettes() {
        // Sanity check that the M3.b indirect path didn't regress —
        // a 3-state section still uses the LinearPalette / 4-bit form.
        use crate::section::ChunkSection;
        let mut section = ChunkSection::filled(AIR, AIR);
        section.set(0, 0, 0, BlockStateId(1));
        section.set(1, 0, 0, BlockStateId(2));

        let mut buf = Vec::new();
        super::encode_block_palette(&mut buf, &section);

        assert!(
            buf[0] < super::DIRECT_BITS_THRESHOLD,
            "small palette uses indirect bpe; got {}",
            buf[0],
        );
    }
}
