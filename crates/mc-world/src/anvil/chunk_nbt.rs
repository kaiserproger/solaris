//! Anvil chunk schema ↔ [`Chunk`] translation.
//!
//! Decodes the chunk NBT compound vanilla 26.1 writes into a `Chunk`,
//! and emits the same shape back. The translation is **lossy**: fields
//! we don't model (structures, block/fluid ticks, PostProcessing,
//! InhabitedTime, LastUpdate, DataVersion) are dropped on decode and
//! not re-emitted. The modelled subset round-trips losslessly:
//!
//! - block states (palette + packed indices)
//! - biomes (palette + packed indices)
//! - heightmaps (long-array view preserved)
//! - block entities (re-encoded as opaque network NBT and stored in
//!   the chunk's `block_entities` map, byte-identical on round-trip)
//! - Status string, xPos / zPos
//! - per-section `BlockLight` / `SkyLight` nibble arrays (decoded
//!   into `Chunk::section_lights`; **not** re-emitted by
//!   `chunk_to_nbt` — the on-disk light is informative for the wire
//!   path, not load-bearing, so the encoder stays light-less to keep
//!   the existing round-trip oracle scoped to block/biome data)

use std::io::Cursor;

use mc_data::Identifier;
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BIOME_VOLUME, BiomeSection, BlockPos, Chunk, ChunkPos, Heightmap, LIGHT_LAYER_BYTES,
    MIN_SECTION_Y, SECTION_COUNT, SectionLight,
};
use crate::section::{ChunkSection, PackedBitArray, SECTION_VOLUME};

#[derive(Debug, Error)]
pub enum ChunkNbtError {
    #[error("NBT error: {0}")]
    Nbt(#[from] mc_nbt::NbtError),
    #[error("expected root Compound, got tag {0:#x}")]
    NotCompound(u8),
    #[error("missing required field {0}")]
    MissingField(&'static str),
    #[error("field {field} has wrong type: expected {expected}, got tag {got:#x}")]
    WrongType {
        field: &'static str,
        expected: &'static str,
        got: u8,
    },
    #[error("invalid identifier {0:?}")]
    InvalidIdentifier(String),
    #[error("block state {name}[{props:?}] not in registry")]
    UnknownBlockState {
        name: String,
        props: Vec<(String, String)>,
    },
    #[error("section Y={0} is outside the {SECTION_COUNT}-section range")]
    SectionOutOfRange(i32),
    #[error("packed bit-array length mismatch: expected {expected} words, got {got}")]
    PackedWordMismatch { expected: usize, got: usize },
    #[error("{field} has wrong byte length: expected {expected}, got {got}")]
    LightLengthMismatch {
        field: &'static str,
        expected: usize,
        got: usize,
    },
}

// ---------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------

pub fn chunk_from_nbt(nbt: &Tag, registry: &BlockRegistry) -> Result<Chunk, ChunkNbtError> {
    let root = expect_compound(nbt, "root")?;

    let x = get_int(root, "xPos")?;
    let z = get_int(root, "zPos")?;
    let status = get_string(root, "Status")?.to_string();

    let air = registry
        .block(&id("minecraft:air"))
        .map(|b| b.default)
        .unwrap_or(BlockStateId(0));
    let default_biome = id("minecraft:plains");

    let mut chunk = Chunk::empty(ChunkPos { x, z }, air, default_biome);
    chunk.status = status;

    if let Some(sections) = get_optional_list(root, "sections")? {
        for s in &sections.elements {
            let cmp = expect_compound(s, "sections[]")?;
            let y = get_byte(cmp, "Y")? as i32;
            let idx = (y - MIN_SECTION_Y) as usize;
            if idx >= SECTION_COUNT {
                return Err(ChunkNbtError::SectionOutOfRange(y));
            }
            let bs = get_compound(cmp, "block_states")?;
            chunk.sections[idx] = decode_block_section(bs, registry, air)?;
            let bi = get_compound(cmp, "biomes")?;
            chunk.biomes[idx] = decode_biome_section(bi)?;
            chunk.section_lights[idx] = decode_section_light(cmp)?;
        }
    }

    if let Some(hms) = get_optional_compound(root, "Heightmaps")? {
        for (name, tag) in hms {
            if let Tag::LongArray(longs) = tag {
                chunk
                    .heightmaps
                    .insert(name.clone(), Heightmap::from_long_array(longs));
            } else {
                return Err(ChunkNbtError::WrongType {
                    field: "Heightmaps.*",
                    expected: "LongArray",
                    got: tag.type_id(),
                });
            }
        }
    }

    if let Some(be_list) = get_optional_list(root, "block_entities")? {
        for be in &be_list.elements {
            let cmp = expect_compound(be, "block_entities[]")?;
            let bx = get_int(cmp, "x")?;
            let by = get_int(cmp, "y")?;
            let bz = get_int(cmp, "z")?;
            // Round-trip the entry verbatim by re-encoding it through
            // the network NBT path ([type][payload], no name). Read
            // back via read_network when needed.
            let mut buf = Vec::with_capacity(64);
            mc_nbt::write_network(&mut buf, be)?;
            chunk.block_entities.insert(
                BlockPos {
                    x: bx,
                    y: by,
                    z: bz,
                },
                buf,
            );
        }
    }

    // M5.c.2: capture every other root-level field verbatim so a
    // load → save round-trip preserves the unmodelled subset
    // (PostProcessing, block_ticks, fluid_ticks, structures,
    // InhabitedTime, LastUpdate, DataVersion, ...). Order is the
    // original compound's insertion order — vanilla doesn't require
    // any particular ordering, but stable ordering keeps byte-diff
    // workflows clean.
    for (key, value) in root {
        if !MODELLED_ROOT_KEYS.contains(&key.as_str()) {
            chunk.extras.push((key.clone(), value.clone()));
        }
    }

    Ok(chunk)
}

/// Root-level NBT keys `chunk_from_nbt` decodes into typed fields.
/// Anything outside this set ends up on `Chunk.extras` so the
/// codec round-trips byte-stably on the unmodelled subset.
const MODELLED_ROOT_KEYS: &[&str] = &[
    "xPos",
    "zPos",
    "yPos",
    "Status",
    "sections",
    "Heightmaps",
    "block_entities",
];

fn decode_block_section(
    nbt: &[(String, Tag)],
    registry: &BlockRegistry,
    air: BlockStateId,
) -> Result<ChunkSection, ChunkNbtError> {
    let palette_list = get_list(nbt, "palette")?;
    let palette: Vec<BlockStateId> = palette_list
        .elements
        .iter()
        .map(|p| {
            let entry = expect_compound(p, "palette[]")?;
            let name_str: String = get_string(entry, "Name")?.clone();
            let name = Identifier::parse(name_str.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(name_str.clone()))?;
            let props = if let Some(props_cmp) = get_optional_compound(entry, "Properties")? {
                props_cmp
                    .iter()
                    .map(|(k, v)| match v {
                        Tag::String(s) => Ok((k.clone(), s.clone())),
                        other => Err(ChunkNbtError::WrongType {
                            field: "Properties.*",
                            expected: "String",
                            got: other.type_id(),
                        }),
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            registry
                .by_name_and_props(&name, &props)
                .ok_or(ChunkNbtError::UnknownBlockState {
                    name: name_str,
                    props,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    match get_optional_long_array(nbt, "data")? {
        None => {
            // Single-state section.
            Ok(ChunkSection::filled(palette[0], air))
        }
        Some(longs) => {
            let bits = block_states_bits_per_entry(palette.len());
            let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
            let epw = 64 / bits as usize;
            let expected_words = SECTION_VOLUME.div_ceil(epw);
            if words.len() != expected_words {
                return Err(ChunkNbtError::PackedWordMismatch {
                    expected: expected_words,
                    got: words.len(),
                });
            }
            let indices = PackedBitArray::from_words(bits, SECTION_VOLUME, words);
            Ok(ChunkSection::from_indirect(palette, indices, air))
        }
    }
}

fn decode_biome_section(nbt: &[(String, Tag)]) -> Result<BiomeSection, ChunkNbtError> {
    let palette_list = get_list(nbt, "palette")?;
    let palette: Vec<Identifier> = palette_list
        .elements
        .iter()
        .map(|p| match p {
            Tag::String(s) => Identifier::parse(s.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(s.clone())),
            other => Err(ChunkNbtError::WrongType {
                field: "biomes.palette[]",
                expected: "String",
                got: other.type_id(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;

    match get_optional_long_array(nbt, "data")? {
        None => Ok(BiomeSection::filled(
            palette
                .into_iter()
                .next()
                .expect("vanilla biome palette always non-empty"),
        )),
        Some(longs) => {
            let bits = biome_bits_per_entry(palette.len());
            let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
            let epw = 64 / bits as usize;
            let expected_words = BIOME_VOLUME.div_ceil(epw);
            if words.len() != expected_words {
                return Err(ChunkNbtError::PackedWordMismatch {
                    expected: expected_words,
                    got: words.len(),
                });
            }
            let indices = PackedBitArray::from_words(bits, BIOME_VOLUME, words);
            Ok(BiomeSection::from_indirect(palette, indices))
        }
    }
}

fn decode_section_light(cmp: &[(String, Tag)]) -> Result<SectionLight, ChunkNbtError> {
    Ok(SectionLight {
        block: decode_light_layer(cmp, "BlockLight")?,
        sky: decode_light_layer(cmp, "SkyLight")?,
    })
}

fn decode_light_layer(
    cmp: &[(String, Tag)],
    field: &'static str,
) -> Result<Option<Vec<u8>>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == field).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::ByteArray(bytes) => {
            if bytes.len() != LIGHT_LAYER_BYTES {
                return Err(ChunkNbtError::LightLengthMismatch {
                    field,
                    expected: LIGHT_LAYER_BYTES,
                    got: bytes.len(),
                });
            }
            Ok(Some(bytes.iter().map(|&b| b as u8).collect()))
        }
        other => Err(ChunkNbtError::WrongType {
            field,
            expected: "ByteArray",
            got: other.type_id(),
        }),
    }
}

/// Per Anvil: `max(4, ceil(log2(palette.len())))`.
fn block_states_bits_per_entry(palette_len: usize) -> u8 {
    if palette_len <= 1 {
        return 4;
    }
    let raw = (palette_len - 1).ilog2() as u8 + 1;
    raw.max(4)
}

/// Biome bits per entry: `max(1, ceil(log2(palette.len())))`, no
/// minimum-4 quirk (biomes only have 64 cells per section, the
/// minimum-4 padding would waste 75% of every entry).
fn biome_bits_per_entry(palette_len: usize) -> u8 {
    if palette_len <= 1 {
        return 1;
    }
    (palette_len - 1).ilog2() as u8 + 1
}

// ---------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------

pub fn chunk_to_nbt(chunk: &Chunk, registry: &BlockRegistry) -> Result<Tag, ChunkNbtError> {
    let mut root: Vec<(String, Tag)> = Vec::with_capacity(8);
    root.push(("xPos".into(), Tag::Int(chunk.pos.x)));
    root.push(("zPos".into(), Tag::Int(chunk.pos.z)));
    root.push(("yPos".into(), Tag::Int(MIN_SECTION_Y)));
    root.push(("Status".into(), Tag::String(chunk.status.clone())));

    // sections
    let mut sections = Vec::with_capacity(SECTION_COUNT);
    for (i, sec) in chunk.sections.iter().enumerate() {
        let y = (i as i32 + MIN_SECTION_Y) as i8;
        let s_cmp = vec![
            ("Y".into(), Tag::Byte(y)),
            ("block_states".into(), encode_block_section(sec, registry)?),
            ("biomes".into(), encode_biome_section(&chunk.biomes[i])),
        ];
        sections.push(Tag::Compound(s_cmp));
    }
    root.push((
        "sections".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: sections,
        }),
    ));

    // Heightmaps
    let mut hms: Vec<(String, Tag)> = chunk
        .heightmaps
        .iter()
        .map(|(name, h)| (name.clone(), Tag::LongArray(h.to_long_array())))
        .collect();
    // Stable order for byte-similar emission.
    hms.sort_by(|a, b| a.0.cmp(&b.0));
    root.push(("Heightmaps".into(), Tag::Compound(hms)));

    // block_entities — read back the opaque network NBT into Tags.
    let mut be_list: Vec<Tag> = Vec::with_capacity(chunk.block_entities.len());
    let mut be_entries: Vec<(&BlockPos, &Vec<u8>)> = chunk.block_entities.iter().collect();
    be_entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (_, bytes) in be_entries {
        let mut cur = Cursor::new(&bytes[..]);
        let tag = mc_nbt::read_network(&mut cur)?;
        be_list.push(tag);
    }
    root.push((
        "block_entities".into(),
        Tag::List(ListTag {
            element_type: if be_list.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: be_list,
        }),
    ));

    // M5.c.2: re-emit every root-level field decode kept in
    // `extras` (PostProcessing, block_ticks, fluid_ticks,
    // structures, InhabitedTime, LastUpdate, DataVersion, etc.).
    // Order is the decode-time insertion order so the round-trip
    // stays stable.
    for (key, value) in &chunk.extras {
        root.push((key.clone(), value.clone()));
    }

    Ok(Tag::Compound(root))
}

fn encode_block_section(
    section: &ChunkSection,
    registry: &BlockRegistry,
) -> Result<Tag, ChunkNbtError> {
    let mut out: Vec<(String, Tag)> = Vec::with_capacity(2);
    match section.palette() {
        None => {
            // Single mode: palette has exactly one entry, no data.
            let id = section.get(0, 0, 0);
            let entry = block_state_to_palette_entry(id, registry);
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![entry],
                }),
            ));
        }
        Some(palette) => {
            let pal_tags = palette
                .iter()
                .map(|&id| block_state_to_palette_entry(id, registry))
                .collect();
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: pal_tags,
                }),
            ));
            let indices = section.indices().expect("indirect mode has indices");
            let longs: Vec<i64> = indices.words().iter().map(|&w| w as i64).collect();
            out.push(("data".into(), Tag::LongArray(longs)));
        }
    }
    Ok(Tag::Compound(out))
}

fn block_state_to_palette_entry(id: BlockStateId, registry: &BlockRegistry) -> Tag {
    let state = registry
        .by_id(id)
        .expect("registry must contain every state id present in a chunk");
    let mut entry: Vec<(String, Tag)> = Vec::with_capacity(2);
    entry.push((
        "Name".into(),
        Tag::String(state.block.id.as_str().to_string()),
    ));
    if !state.properties.is_empty() {
        let props: Vec<(String, Tag)> = state
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), Tag::String(v.clone())))
            .collect();
        entry.push(("Properties".into(), Tag::Compound(props)));
    }
    Tag::Compound(entry)
}

fn encode_biome_section(section: &BiomeSection) -> Tag {
    let mut out: Vec<(String, Tag)> = Vec::with_capacity(2);
    match section {
        BiomeSection::Single(id) => {
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: vec![Tag::String(id.as_str().to_string())],
                }),
            ));
        }
        BiomeSection::Indirect { palette, indices } => {
            let pal_tags: Vec<Tag> = palette
                .iter()
                .map(|id| Tag::String(id.as_str().to_string()))
                .collect();
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: pal_tags,
                }),
            ));
            let longs: Vec<i64> = indices.words().iter().map(|&w| w as i64).collect();
            out.push(("data".into(), Tag::LongArray(longs)));
        }
    }
    Tag::Compound(out)
}

// ---------------------------------------------------------------------
// Small NBT helpers
// ---------------------------------------------------------------------

fn id(s: &str) -> Identifier {
    Identifier::parse(s).expect("static identifier")
}

fn expect_compound<'a>(tag: &'a Tag, name: &str) -> Result<&'a [(String, Tag)], ChunkNbtError> {
    match tag {
        Tag::Compound(entries) => Ok(entries),
        other => Err(if name == "root" {
            ChunkNbtError::NotCompound(other.type_id())
        } else {
            ChunkNbtError::WrongType {
                field: leak_field_name(name),
                expected: "Compound",
                got: other.type_id(),
            }
        }),
    }
}

fn get_field<'a>(cmp: &'a [(String, Tag)], name: &'static str) -> Result<&'a Tag, ChunkNbtError> {
    cmp.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
        .ok_or(ChunkNbtError::MissingField(name))
}

fn get_int(cmp: &[(String, Tag)], name: &'static str) -> Result<i32, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::Int(v) => Ok(*v),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Int",
            got: other.type_id(),
        }),
    }
}

fn get_byte(cmp: &[(String, Tag)], name: &'static str) -> Result<i8, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::Byte(v) => Ok(*v),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Byte",
            got: other.type_id(),
        }),
    }
}

fn get_string<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<&'a String, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::String(s) => Ok(s),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "String",
            got: other.type_id(),
        }),
    }
}

fn get_list<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<&'a ListTag, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::List(l) => Ok(l),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "List",
            got: other.type_id(),
        }),
    }
}

fn get_compound<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<&'a [(String, Tag)], ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::Compound(e) => Ok(e),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Compound",
            got: other.type_id(),
        }),
    }
}

fn get_optional_list<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a ListTag>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::List(l) => Ok(Some(l)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "List",
            got: other.type_id(),
        }),
    }
}

fn get_optional_compound<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a [(String, Tag)]>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Compound(e) => Ok(Some(e)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Compound",
            got: other.type_id(),
        }),
    }
}

fn get_optional_long_array<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a [i64]>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::LongArray(a) => Ok(Some(a)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "LongArray",
            got: other.type_id(),
        }),
    }
}

fn leak_field_name(s: &str) -> &'static str {
    // Used only on error paths.
    Box::leak(s.to_string().into_boxed_str())
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil::region;
    use crate::chunk::{MAX_Y, MIN_Y};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    /// The big M2 acceptance test: load every chunk in the real
    /// .mca, decode it, re-encode it, decode again, and verify the
    /// modelled state is bit-identical between the two decoded
    /// chunks. Skipped when the prerequisites aren't on disk.
    #[test]
    fn round_trip_real_vanilla_chunks() {
        let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !region_path.is_file() || !blocks_path.is_file() {
            eprintln!(
                "skipping: need both {} and {}",
                region_path.display(),
                blocks_path.display()
            );
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let payloads = region::read_region(&region_path).unwrap();

        let mut probed = 0usize;
        for payload in &payloads {
            let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
            let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
            let chunk1 = chunk_from_nbt(&root, &registry).expect("decode #1");

            let root2 = chunk_to_nbt(&chunk1, &registry).expect("encode");
            let mut buf2 = Vec::new();
            mc_nbt::write_named(&mut buf2, "", &root2).unwrap();
            let mut cur2 = Cursor::new(&buf2[..]);
            let (_, root3) = mc_nbt::read_named(&mut cur2).unwrap();
            let chunk2 = chunk_from_nbt(&root3, &registry).expect("decode #2");

            assert_eq!(chunk1.pos, chunk2.pos, "ChunkPos");
            assert_eq!(chunk1.status, chunk2.status, "Status");

            for y in MIN_Y..MAX_Y {
                for z in 0..16u8 {
                    for x in 0..16u8 {
                        assert_eq!(
                            chunk1.get_block(x, y, z),
                            chunk2.get_block(x, y, z),
                            "block at ({x},{y},{z}) in chunk {:?}",
                            chunk1.pos
                        );
                    }
                }
            }

            for (i, (b1, b2)) in chunk1.biomes.iter().zip(&chunk2.biomes).enumerate() {
                for y in 0..4u8 {
                    for z in 0..4u8 {
                        for x in 0..4u8 {
                            assert_eq!(
                                b1.get(x, y, z),
                                b2.get(x, y, z),
                                "biome at ({x},{y},{z}) in section {i} of {:?}",
                                chunk1.pos
                            );
                        }
                    }
                }
            }

            let hm1: HashSet<&String> = chunk1.heightmaps.keys().collect();
            let hm2: HashSet<&String> = chunk2.heightmaps.keys().collect();
            assert_eq!(hm1, hm2, "Heightmaps key set");
            for (name, h1) in &chunk1.heightmaps {
                let h2 = chunk2.heightmaps.get(name).unwrap();
                assert_eq!(
                    h1.to_long_array(),
                    h2.to_long_array(),
                    "heightmap {name} in {:?}",
                    chunk1.pos,
                );
            }

            assert_eq!(
                chunk1.block_entities, chunk2.block_entities,
                "block entities in {:?}",
                chunk1.pos
            );

            // M5.c.2: extras must round-trip key-and-value.
            assert_eq!(
                chunk1.extras.len(),
                chunk2.extras.len(),
                "extras count in {:?}",
                chunk1.pos,
            );
            for (e1, e2) in chunk1.extras.iter().zip(&chunk2.extras) {
                assert_eq!(e1.0, e2.0, "extras key order in {:?}", chunk1.pos);
                assert_eq!(e1.1, e2.1, "extras value for {} in {:?}", e1.0, chunk1.pos);
            }

            probed += 1;
        }
        assert!(probed > 0, "test region must have at least one chunk");
        eprintln!("round-tripped {probed} chunks");
    }

    #[test]
    fn real_test_world_carries_dropped_root_fields_in_extras() {
        let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !region_path.is_file() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let payloads = region::read_region(&region_path).unwrap();

        let mut chunks_with_extras = 0usize;
        let mut seen_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for payload in &payloads {
            let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
            let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
            let chunk = chunk_from_nbt(&root, &registry).unwrap();
            if !chunk.extras.is_empty() {
                chunks_with_extras += 1;
                for (k, _) in &chunk.extras {
                    seen_keys.insert(k.clone());
                }
            }
        }

        // Every real chunk has at least DataVersion + InhabitedTime
        // + LastUpdate (vanilla writes them on every save).
        assert!(
            chunks_with_extras > 0,
            "expected ≥1 chunk with unmodelled root fields",
        );
        for required in &["DataVersion", "InhabitedTime", "LastUpdate"] {
            assert!(
                seen_keys.contains(*required),
                "extras key set should contain {required}; got {:?}",
                seen_keys,
            );
        }
    }

    fn nibble_pattern() -> Vec<i8> {
        // Distinct values per byte so we can pin the byte order.
        // Cast through u8 so the literal 0..=255 wraps cleanly into
        // i8's -128..=127 range.
        (0..LIGHT_LAYER_BYTES)
            .map(|i| (i & 0xFF) as u8 as i8)
            .collect()
    }

    fn build_section_with_light(
        y: i8,
        block_light: Option<Vec<i8>>,
        sky_light: Option<Vec<i8>>,
    ) -> Tag {
        let mut s: Vec<(String, Tag)> = vec![
            ("Y".into(), Tag::Byte(y)),
            (
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: vec![Tag::Compound(vec![(
                            "Name".into(),
                            Tag::String("minecraft:air".into()),
                        )])],
                    }),
                )]),
            ),
            (
                "biomes".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::STRING,
                        elements: vec![Tag::String("minecraft:plains".into())],
                    }),
                )]),
            ),
        ];
        if let Some(bl) = block_light {
            s.push(("BlockLight".into(), Tag::ByteArray(bl)));
        }
        if let Some(sl) = sky_light {
            s.push(("SkyLight".into(), Tag::ByteArray(sl)));
        }
        Tag::Compound(s)
    }

    fn build_chunk_root_with_sections(sections: Vec<Tag>) -> Tag {
        Tag::Compound(vec![
            ("xPos".into(), Tag::Int(0)),
            ("zPos".into(), Tag::Int(0)),
            ("yPos".into(), Tag::Int(MIN_SECTION_Y)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            (
                "sections".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: sections,
                }),
            ),
        ])
    }

    fn tiny_registry() -> BlockRegistry {
        // Two blocks suffice for the M4.a tests: air and stone, each
        // with a single default state. Built inline so the test
        // doesn't depend on the full vanilla report being on disk.
        use std::collections::BTreeMap;
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ];
        BlockRegistry::from_report(&report).unwrap()
    }

    #[test]
    fn decodes_per_section_light_arrays_when_present() {
        let pattern = nibble_pattern();
        let root = build_chunk_root_with_sections(vec![
            build_section_with_light(-4, Some(pattern.clone()), Some(pattern.clone())),
            build_section_with_light(0, None, Some(pattern.clone())),
        ]);
        let chunk = chunk_from_nbt(&root, &tiny_registry()).expect("decode");

        let expected: Vec<u8> = pattern.iter().map(|&b| b as u8).collect();

        // Section Y=-4 → idx 0: both layers present.
        assert_eq!(
            chunk.section_lights[0].block.as_deref(),
            Some(&expected[..])
        );
        assert_eq!(chunk.section_lights[0].sky.as_deref(), Some(&expected[..]));

        // Section Y=0 → idx 4: only sky present.
        assert_eq!(chunk.section_lights[4].block, None);
        assert_eq!(chunk.section_lights[4].sky.as_deref(), Some(&expected[..]));

        // Untouched sections keep the default (no light).
        assert_eq!(chunk.section_lights[1], SectionLight::default());
        assert_eq!(chunk.section_lights[23], SectionLight::default());

        // Layer is exactly LIGHT_LAYER_BYTES bytes; sanity-check the
        // i8 → u8 reinterpretation didn't shift anything.
        let sky = chunk.section_lights[0].sky.as_deref().unwrap();
        assert_eq!(sky.len(), LIGHT_LAYER_BYTES);
        assert_eq!(sky[0x00], 0x00);
        assert_eq!(sky[0x7F], 0x7F);
        assert_eq!(sky[0x80], 0x80);
        assert_eq!(sky[0xFF], 0xFF);
    }

    #[test]
    fn rejects_wrong_length_light_array() {
        let truncated: Vec<i8> = vec![0; LIGHT_LAYER_BYTES - 1];
        let root = build_chunk_root_with_sections(vec![build_section_with_light(
            0,
            Some(truncated),
            None,
        )]);
        match chunk_from_nbt(&root, &tiny_registry()) {
            Err(ChunkNbtError::LightLengthMismatch {
                field,
                expected,
                got,
            }) => {
                assert_eq!(field, "BlockLight");
                assert_eq!(expected, LIGHT_LAYER_BYTES);
                assert_eq!(got, LIGHT_LAYER_BYTES - 1);
            }
            other => panic!("expected LightLengthMismatch, got {:?}", other),
        }
    }

    #[test]
    fn rejects_wrong_type_light_field() {
        let root = build_chunk_root_with_sections(vec![Tag::Compound(vec![
            ("Y".into(), Tag::Byte(0)),
            (
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: vec![Tag::Compound(vec![(
                            "Name".into(),
                            Tag::String("minecraft:air".into()),
                        )])],
                    }),
                )]),
            ),
            (
                "biomes".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::STRING,
                        elements: vec![Tag::String("minecraft:plains".into())],
                    }),
                )]),
            ),
            ("SkyLight".into(), Tag::Int(0)),
        ])]);
        match chunk_from_nbt(&root, &tiny_registry()) {
            Err(ChunkNbtError::WrongType { field, .. }) => assert_eq!(field, "SkyLight"),
            other => panic!("expected WrongType for SkyLight, got {:?}", other),
        }
    }

    #[test]
    fn real_test_world_carries_some_baked_skylight() {
        let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !region_path.is_file() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let payloads = region::read_region(&region_path).unwrap();

        let mut chunks_with_sky = 0usize;
        let mut sections_with_sky = 0usize;
        for payload in &payloads {
            let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
            let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
            let chunk = chunk_from_nbt(&root, &registry).expect("decode");

            let chunk_has_sky = chunk.section_lights.iter().any(|sl| sl.sky.is_some());
            if chunk_has_sky {
                chunks_with_sky += 1;
            }
            for sl in &chunk.section_lights {
                if let Some(layer) = &sl.sky {
                    assert_eq!(layer.len(), LIGHT_LAYER_BYTES);
                    sections_with_sky += 1;
                }
                if let Some(layer) = &sl.block {
                    assert_eq!(layer.len(), LIGHT_LAYER_BYTES);
                }
            }
        }

        // The bundled test world has at least one `minecraft:full`
        // chunk with baked sky-light on at least one section. This
        // pins the decode path against real bytes; loosen the bound
        // when the test world is regenerated.
        assert!(
            chunks_with_sky >= 1,
            "expected ≥1 chunk with baked SkyLight, got {chunks_with_sky}",
        );
        assert!(
            sections_with_sky >= 1,
            "expected ≥1 section with baked SkyLight, got {sections_with_sky}",
        );
    }
}
