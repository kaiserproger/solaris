//! Anvil chunk schema ↔ [`Chunk`] translation.
//!
//! Decodes the chunk NBT compound vanilla 26.1 writes into a `Chunk`,
//! and emits the supported normalized Anvil shape back. Typed fields are
//! modelled where runtime code consumes them. Root-level fields outside
//! the modelled set are kept in `Chunk::extras` and re-emitted on save
//! when possible. The modelled subset round-trips losslessly:
//!
//! - block states (palette + packed indices)
//! - biomes (palette + packed indices)
//! - heightmaps (long-array view preserved)
//! - opaque block entities (stored in `Chunk::block_entities` and
//!   byte-identical on round-trip); typed furnace/chest entities are
//!   normalized through runtime models
//! - scheduled block and fluid ticks
//! - Status string, xPos / zPos
//! - per-section `BlockLight` / `SkyLight` nibble arrays (decoded
//!   into `Chunk::section_lights` and re-emitted when present)
//!
//! Additionally, unmodelled root-level extras such as structures,
//! PostProcessing, InhabitedTime, LastUpdate, and DataVersion are
//! best-effort preserved verbatim.

use std::io::Cursor;

use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use crate::SECTION_DIM;
use crate::anvil::ChunkPayload;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BIOME_VOLUME, BiomeSection, BlockPos, ChestBlockEntity, Chunk, ChunkGeometry, ChunkPos,
    FurnaceBlockEntity, FurnaceSlot, Heightmap, HopperBlockEntity, LIGHT_LAYER_BYTES,
    ScheduledBlockTick, ScheduledFluidTick, SectionLight,
};
use crate::section::{ChunkSection, PackedBitArray, SECTION_VOLUME};

const REGION_AXIS_CHUNKS: i32 = 32;
const DAMAGE_COMPONENT: &str = "minecraft:damage";
const ENCHANTMENTS_COMPONENT: &str = "minecraft:enchantments";

/// Serialise a chunk to a [`ChunkPayload`] ready for
/// [`write_region`](crate::anvil::write_region). Used by the M6.b
/// persistence flush path: encodes the chunk to NBT, serialises with
/// `mc_nbt::write_named` (Anvil chunks are stored as an unnamed root,
/// `name = ""`), and bundles with the chunk's region-local slot
/// coordinates plus the supplied epoch-seconds timestamp.
pub fn chunk_to_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    timestamp: u32,
) -> Result<ChunkPayload, ChunkNbtError> {
    chunk_to_payload_with_items(chunk, registry, None, timestamp)
}

pub fn chunk_to_payload_with_items(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    timestamp: u32,
) -> Result<ChunkPayload, ChunkNbtError> {
    chunk_to_payload_with_items_at_tick(chunk, registry, items, timestamp, 0)
}

pub fn chunk_to_payload_with_items_at_tick(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    timestamp: u32,
    current_tick: u64,
) -> Result<ChunkPayload, ChunkNbtError> {
    let nbt = chunk_to_nbt_with_items_at_tick(chunk, registry, items, current_tick)?;
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    mc_nbt::write_named(&mut buf, "", &nbt)?;
    let local_x = chunk.pos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    let local_z = chunk.pos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    Ok(ChunkPayload {
        local_x,
        local_z,
        timestamp,
        uncompressed_nbt: buf,
    })
}

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
    #[error("block state id {0} not in registry")]
    UnknownBlockStateId(u32),
    #[error("item {0} not in registry")]
    UnknownItem(String),
    #[error("invalid enchantment component {0:?}")]
    InvalidEnchantment(String),
    #[error("section Y={0} is outside the chunk geometry declared by yPos and sections")]
    SectionOutOfRange(i32),
    #[error("section Y={0} is duplicated or leaves a gap in the chunk geometry")]
    InvalidSectionShape(i32),
    #[error("yPos={y_pos} and {section_count} sections do not define a supported chunk geometry")]
    InvalidChunkGeometry { y_pos: i32, section_count: usize },
    #[error("section Y={0} does not fit the vanilla byte field")]
    SectionYOutOfByteRange(i32),
    #[error("packed bit-array length mismatch: expected {expected} words, got {got}")]
    PackedWordMismatch { expected: usize, got: usize },
    #[error("{field} has wrong byte length: expected {expected}, got {got}")]
    LightLengthMismatch {
        field: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("scheduled block tick delay is negative: {0}")]
    NegativeTickDelay(i32),
    #[error("scheduled block tick delay {0} does not fit in Anvil int")]
    TickDelayOutOfRange(u64),
}

// ---------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------

pub fn chunk_from_nbt(nbt: &Tag, registry: &BlockRegistry) -> Result<Chunk, ChunkNbtError> {
    chunk_from_nbt_with_items(nbt, registry, None)
}

pub fn chunk_from_nbt_with_items(
    nbt: &Tag,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
) -> Result<Chunk, ChunkNbtError> {
    let root = expect_compound(nbt, "root")?;

    let x = get_int(root, "xPos")?;
    let z = get_int(root, "zPos")?;
    let status = get_string(root, "Status")?.to_string();

    let air = registry
        .block(&id("minecraft:air"))
        .map(|b| b.default)
        .unwrap_or(BlockStateId(0));
    let default_biome = id("minecraft:plains");

    let y_pos = get_int(root, "yPos")?;
    let sections = get_list(root, "sections")?;
    let section_count = sections.elements.len();
    let min_y =
        y_pos
            .checked_mul(SECTION_DIM as i32)
            .ok_or(ChunkNbtError::InvalidChunkGeometry {
                y_pos,
                section_count,
            })?;
    let height = i32::try_from(section_count)
        .ok()
        .and_then(|count| count.checked_mul(SECTION_DIM as i32))
        .ok_or(ChunkNbtError::InvalidChunkGeometry {
            y_pos,
            section_count,
        })?;
    let geometry =
        ChunkGeometry::new(min_y, height).ok_or(ChunkNbtError::InvalidChunkGeometry {
            y_pos,
            section_count,
        })?;

    let mut chunk = Chunk::empty_with_geometry(ChunkPos { x, z }, air, default_biome, geometry);
    chunk.status = status;

    let mut seen_sections = vec![false; section_count];
    for s in &sections.elements {
        let cmp = expect_compound(s, "sections[]")?;
        let y = get_byte(cmp, "Y")? as i32;
        let Some(idx) = y
            .checked_sub(y_pos)
            .and_then(|idx| usize::try_from(idx).ok())
            .filter(|idx| *idx < section_count)
        else {
            return Err(ChunkNbtError::SectionOutOfRange(y));
        };
        if seen_sections[idx] {
            return Err(ChunkNbtError::InvalidSectionShape(y));
        }
        seen_sections[idx] = true;
        let bs = get_compound(cmp, "block_states")?;
        chunk.sections[idx] = decode_block_section(bs, registry, air)?;
        let bi = get_compound(cmp, "biomes")?;
        chunk.biomes[idx] = decode_biome_section(bi)?;
        chunk.section_lights[idx] = decode_section_light(cmp)?;
    }
    if let Some(missing_idx) = seen_sections.iter().position(|seen| !seen) {
        return Err(ChunkNbtError::InvalidSectionShape(
            y_pos + missing_idx as i32,
        ));
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
    if let Some(heightmap) = chunk
        .heightmaps
        .get("MOTION_BLOCKING")
        .or_else(|| chunk.heightmaps.get("WORLD_SURFACE"))
    {
        chunk.highest_opaque = heightmap.clone();
    }

    if let Some(be_list) = get_optional_list(root, "block_entities")? {
        for be in &be_list.elements {
            let cmp = expect_compound(be, "block_entities[]")?;
            let bx = get_int(cmp, "x")?;
            let by = get_int(cmp, "y")?;
            let bz = get_int(cmp, "z")?;
            let block_entity_id = get_string(cmp, "id").ok();
            if block_entity_id.is_some_and(|id| is_furnace_block_entity_id(id))
                && let Some(items) = items
            {
                chunk.furnaces.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_furnace(cmp, items)?,
                );
                continue;
            }
            if block_entity_id.is_some_and(|id| is_chest_storage_block_entity_id(id))
                && let Some(items) = items
            {
                chunk.chests.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_chest(cmp, items)?,
                );
                continue;
            }
            if block_entity_id.is_some_and(|id| is_hopper_block_entity_id(id))
                && let Some(items) = items
            {
                chunk.hoppers.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_hopper(cmp, items)?,
                );
                continue;
            }

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

    if let Some(block_ticks) = get_optional_list(root, "block_ticks")? {
        chunk.load_scheduled_block_ticks(decode_scheduled_block_ticks(block_ticks, 0)?);
    }
    if let Some(fluid_ticks) = get_optional_list(root, "fluid_ticks")? {
        chunk.load_scheduled_fluid_ticks(decode_scheduled_fluid_ticks(fluid_ticks, 0)?);
    }

    // M5.c.2: capture every other root-level field verbatim so a
    // load → save round-trip preserves the unmodelled subset
    // (PostProcessing, structures,
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
    "block_ticks",
    "fluid_ticks",
];

fn decode_scheduled_block_ticks(
    list: &ListTag,
    current_tick: u64,
) -> Result<Vec<ScheduledBlockTick>, ChunkNbtError> {
    list.elements
        .iter()
        .enumerate()
        .map(|(sequence, tag)| {
            let tick = expect_compound(tag, "block_ticks[]")?;
            let block_name = get_string(tick, "i")?;
            let block = Identifier::parse(block_name.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(block_name.clone()))?;
            let delay = get_int(tick, "t")?;
            if delay < 0 {
                return Err(ChunkNbtError::NegativeTickDelay(delay));
            }
            Ok(ScheduledBlockTick::from_storage(
                BlockPos {
                    x: get_int(tick, "x")?,
                    y: get_int(tick, "y")?,
                    z: get_int(tick, "z")?,
                },
                block,
                current_tick + delay as u64,
                get_int(tick, "p")?,
                sequence as u64,
            ))
        })
        .collect()
}

fn decode_scheduled_fluid_ticks(
    list: &ListTag,
    current_tick: u64,
) -> Result<Vec<ScheduledFluidTick>, ChunkNbtError> {
    list.elements
        .iter()
        .enumerate()
        .map(|(sequence, tag)| {
            let tick = expect_compound(tag, "fluid_ticks[]")?;
            let fluid_name = get_string(tick, "i")?;
            let fluid = Identifier::parse(fluid_name.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(fluid_name.clone()))?;
            let delay = get_int(tick, "t")?;
            if delay < 0 {
                return Err(ChunkNbtError::NegativeTickDelay(delay));
            }
            Ok(ScheduledFluidTick::from_storage(
                BlockPos {
                    x: get_int(tick, "x")?,
                    y: get_int(tick, "y")?,
                    z: get_int(tick, "z")?,
                },
                fluid,
                current_tick + delay as u64,
                get_int(tick, "p")?,
                sequence as u64,
            ))
        })
        .collect()
}

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
    chunk_to_nbt_with_items(chunk, registry, None)
}

pub fn chunk_to_nbt_with_items(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
) -> Result<Tag, ChunkNbtError> {
    chunk_to_nbt_with_items_at_tick(chunk, registry, items, 0)
}

pub fn chunk_to_nbt_with_items_at_tick(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    let mut root: Vec<(String, Tag)> = Vec::with_capacity(8);
    root.push(("xPos".into(), Tag::Int(chunk.pos.x)));
    root.push(("zPos".into(), Tag::Int(chunk.pos.z)));
    let min_section_y = chunk.geometry().min_y() / SECTION_DIM as i32;
    root.push(("yPos".into(), Tag::Int(min_section_y)));
    root.push(("Status".into(), Tag::String(chunk.status.clone())));

    // sections
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for (i, sec) in chunk.sections.iter().enumerate() {
        let section_y = min_section_y + i as i32;
        let y = i8::try_from(section_y)
            .map_err(|_| ChunkNbtError::SectionYOutOfByteRange(section_y))?;
        let mut s_cmp = vec![
            ("Y".into(), Tag::Byte(y)),
            ("block_states".into(), encode_block_section(sec, registry)?),
            ("biomes".into(), encode_biome_section(&chunk.biomes[i])),
        ];
        if let Some(tag) = encode_light_layer("BlockLight", &chunk.section_lights[i].block)? {
            s_cmp.push(("BlockLight".into(), tag));
        }
        if let Some(tag) = encode_light_layer("SkyLight", &chunk.section_lights[i].sky)? {
            s_cmp.push(("SkyLight".into(), tag));
        }
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
    let mut be_entries: Vec<(&BlockPos, &Vec<u8>)> = chunk
        .block_entities
        .iter()
        .filter(|(pos, _)| {
            !chunk.furnaces.contains_key(pos)
                && !chunk.chests.contains_key(pos)
                && !chunk.hoppers.contains_key(pos)
        })
        .collect();
    be_entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (_, bytes) in be_entries {
        let mut cur = Cursor::new(&bytes[..]);
        let tag = mc_nbt::read_network(&mut cur)?;
        be_list.push(tag);
    }
    if let Some(items) = items {
        let mut furnaces: Vec<(&BlockPos, &FurnaceBlockEntity)> = chunk.furnaces.iter().collect();
        furnaces.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, furnace) in furnaces {
            be_list.push(encode_furnace(
                pos,
                furnace,
                items,
                furnace_block_entity_id_for_block(chunk, registry, *pos),
            )?);
        }
        let mut chests: Vec<(&BlockPos, &ChestBlockEntity)> = chunk.chests.iter().collect();
        chests.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, chest) in chests {
            be_list.push(encode_chest(
                pos,
                chest,
                items,
                chest_storage_block_entity_id_for_block(chunk, registry, *pos),
            )?);
        }
        let mut hoppers: Vec<(&BlockPos, &HopperBlockEntity)> = chunk.hoppers.iter().collect();
        hoppers.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, hopper) in hoppers {
            be_list.push(encode_hopper(pos, hopper, items)?);
        }
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

    root.push((
        "block_ticks".into(),
        encode_scheduled_block_ticks(&chunk.scheduled_block_ticks, current_tick)?,
    ));
    root.push((
        "fluid_ticks".into(),
        encode_scheduled_fluid_ticks(&chunk.scheduled_fluid_ticks, current_tick)?,
    ));

    // M5.c.2: re-emit every root-level field decode kept in
    // `extras` (PostProcessing,
    // structures, InhabitedTime, LastUpdate, DataVersion, etc.).
    // Order is the decode-time insertion order so the round-trip
    // stays stable.
    for (key, value) in &chunk.extras {
        if !MODELLED_ROOT_KEYS.contains(&key.as_str()) {
            root.push((key.clone(), value.clone()));
        }
    }

    Ok(Tag::Compound(root))
}

fn encode_light_layer(
    field: &'static str,
    layer: &Option<Vec<u8>>,
) -> Result<Option<Tag>, ChunkNbtError> {
    let Some(layer) = layer else {
        return Ok(None);
    };
    if layer.len() != LIGHT_LAYER_BYTES {
        return Err(ChunkNbtError::LightLengthMismatch {
            field,
            expected: LIGHT_LAYER_BYTES,
            got: layer.len(),
        });
    }
    Ok(Some(Tag::ByteArray(
        layer.iter().copied().map(|byte| byte as i8).collect(),
    )))
}

fn encode_scheduled_block_ticks(
    ticks: &[ScheduledBlockTick],
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    let mut elements = Vec::with_capacity(ticks.len());
    for tick in ticks {
        let delay = tick.trigger_tick.saturating_sub(current_tick);
        if delay > i32::MAX as u64 {
            return Err(ChunkNbtError::TickDelayOutOfRange(delay));
        }
        elements.push(Tag::Compound(vec![
            ("i".into(), Tag::String(tick.block.as_str().to_string())),
            ("x".into(), Tag::Int(tick.pos.x)),
            ("y".into(), Tag::Int(tick.pos.y)),
            ("z".into(), Tag::Int(tick.pos.z)),
            ("t".into(), Tag::Int(delay as i32)),
            ("p".into(), Tag::Int(tick.priority)),
        ]));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn encode_scheduled_fluid_ticks(
    ticks: &[ScheduledFluidTick],
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    let mut elements = Vec::with_capacity(ticks.len());
    for tick in ticks {
        let delay = tick.trigger_tick.saturating_sub(current_tick);
        if delay > i32::MAX as u64 {
            return Err(ChunkNbtError::TickDelayOutOfRange(delay));
        }
        elements.push(Tag::Compound(vec![
            ("i".into(), Tag::String(tick.fluid.as_str().to_string())),
            ("x".into(), Tag::Int(tick.pos.x)),
            ("y".into(), Tag::Int(tick.pos.y)),
            ("z".into(), Tag::Int(tick.pos.z)),
            ("t".into(), Tag::Int(delay as i32)),
            ("p".into(), Tag::Int(tick.priority)),
        ]));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
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
            let entry = block_state_to_palette_entry(id, registry)?;
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
                .collect::<Result<Vec<_>, _>>()?;
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

fn block_state_to_palette_entry(
    id: BlockStateId,
    registry: &BlockRegistry,
) -> Result<Tag, ChunkNbtError> {
    let state = registry
        .by_id(id)
        .ok_or(ChunkNbtError::UnknownBlockStateId(id.0))?;
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
    Ok(Tag::Compound(entry))
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
            let bits = biome_bits_per_entry(palette.len());
            let longs: Vec<i64> = if indices.bits_per_entry() == bits {
                indices.words().iter().map(|&w| w as i64).collect()
            } else {
                let mut packed = PackedBitArray::zeroed(bits, BIOME_VOLUME);
                for idx in 0..BIOME_VOLUME {
                    packed.set(idx, indices.get(idx));
                }
                packed.words().iter().map(|&w| w as i64).collect()
            };
            out.push(("data".into(), Tag::LongArray(longs)));
        }
    }
    Tag::Compound(out)
}

fn decode_furnace(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<FurnaceBlockEntity, ChunkNbtError> {
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: get_optional_short(cmp, "lit_time_remaining")?.unwrap_or(0),
        burn_total: get_optional_short(cmp, "lit_total_time")?.unwrap_or(1600),
        cook_progress: get_optional_short(cmp, "cooking_time_spent")?.unwrap_or(0),
        cook_total: get_optional_short(cmp, "cooking_total_time")?.unwrap_or(200),
        ..FurnaceBlockEntity::default()
    };

    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_int(item, "Slot")?;
            if !(0..=2).contains(&slot) {
                continue;
            }
            furnace.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    if let Some(recipes_used) = get_optional_compound(cmp, "RecipesUsed")? {
        for (recipe_id, count) in recipes_used {
            let parsed = Identifier::parse(recipe_id.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(recipe_id.clone()))?;
            let Tag::Int(count) = count else {
                return Err(ChunkNbtError::WrongType {
                    field: "RecipesUsed.*",
                    expected: "Int",
                    got: count.type_id(),
                });
            };
            if *count > 0 {
                furnace
                    .recipes_used
                    .insert(parsed.as_str().to_string(), *count);
            }
        }
    }
    Ok(furnace)
}

fn decode_chest(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<ChestBlockEntity, ChunkNbtError> {
    let mut chest = ChestBlockEntity::default();
    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_int(item, "Slot")?;
            if !(0..=26).contains(&slot) {
                continue;
            }
            chest.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    Ok(chest)
}

fn decode_hopper(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<HopperBlockEntity, ChunkNbtError> {
    let mut hopper = HopperBlockEntity {
        transfer_cooldown: get_optional_int(cmp, "TransferCooldown")?.unwrap_or(-1),
        ..Default::default()
    };
    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_int(item, "Slot")?;
            if !(0..=4).contains(&slot) {
                continue;
            }
            hopper.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    Ok(hopper)
}

fn encode_furnace(
    pos: &BlockPos,
    furnace: &FurnaceBlockEntity,
    items: &ItemRegistry,
    block_entity_id: &str,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String(block_entity_id.to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];

    let mut item_tags = Vec::new();
    for (slot, stack) in furnace.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    compound.push((
        "lit_time_remaining".into(),
        Tag::Short(furnace.burn_remaining),
    ));
    compound.push(("lit_total_time".into(), Tag::Short(furnace.burn_total)));
    compound.push((
        "cooking_time_spent".into(),
        Tag::Short(furnace.cook_progress),
    ));
    compound.push(("cooking_total_time".into(), Tag::Short(furnace.cook_total)));
    compound.push((
        "RecipesUsed".into(),
        Tag::Compound(
            furnace
                .recipes_used
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(recipe_id, count)| (recipe_id.clone(), Tag::Int(*count)))
                .collect(),
        ),
    ));
    Ok(Tag::Compound(compound))
}

fn is_furnace_block_entity_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft:furnace" | "minecraft:smoker" | "minecraft:blast_furnace"
    )
}

fn furnace_block_entity_id_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> &'static str {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    let Some(state_id) = chunk.get_block(local_x, pos.y, local_z) else {
        return "minecraft:furnace";
    };
    let Some(state) = registry.by_id(state_id) else {
        return "minecraft:furnace";
    };
    match state.block.id.as_str() {
        "minecraft:smoker" => "minecraft:smoker",
        "minecraft:blast_furnace" => "minecraft:blast_furnace",
        _ => "minecraft:furnace",
    }
}

fn encode_chest(
    pos: &BlockPos,
    chest: &ChestBlockEntity,
    items: &ItemRegistry,
    block_entity_id: &str,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String(block_entity_id.to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];

    let mut item_tags = Vec::new();
    for (slot, stack) in chest.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    Ok(Tag::Compound(compound))
}

fn is_chest_storage_block_entity_id(id: &str) -> bool {
    matches!(id, "minecraft:chest" | "minecraft:barrel")
}

fn encode_hopper(
    pos: &BlockPos,
    hopper: &HopperBlockEntity,
    items: &ItemRegistry,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String("minecraft:hopper".to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        (
            "TransferCooldown".into(),
            Tag::Int(hopper.transfer_cooldown),
        ),
    ];

    let mut item_tags = Vec::new();
    for (slot, stack) in hopper.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    Ok(Tag::Compound(compound))
}

fn is_hopper_block_entity_id(id: &str) -> bool {
    id == "minecraft:hopper"
}

fn decode_container_stack(
    item: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<FurnaceSlot, ChunkNbtError> {
    let item_name = get_string(item, "id")?;
    let parsed = Identifier::parse(item_name.clone())
        .map_err(|_| ChunkNbtError::InvalidIdentifier(item_name.clone()))?;
    let item_id = items
        .id_of(&parsed)
        .ok_or_else(|| ChunkNbtError::UnknownItem(item_name.clone()))?;
    let components = get_optional_compound(item, "components")?;
    let damage = components
        .map(|components| get_optional_int(components, DAMAGE_COMPONENT))
        .transpose()?
        .flatten();
    let mut enchantments = Vec::new();
    if let Some(components) = components
        && let Some(values) = get_optional_compound(components, ENCHANTMENTS_COMPONENT)?
    {
        enchantments.reserve(values.len());
        for (id, level) in values {
            let Tag::Int(level) = level else {
                return Err(ChunkNbtError::InvalidEnchantment(id.clone()));
            };
            let parsed = Identifier::parse(id.clone())
                .map_err(|_| ChunkNbtError::InvalidEnchantment(id.clone()))?;
            if !(1..=255).contains(level)
                || mc_data::required_registry_entry_id("enchantment", &parsed).is_none()
            {
                return Err(ChunkNbtError::InvalidEnchantment(id.clone()));
            }
            enchantments.push(mc_data::ItemEnchantment {
                id: parsed,
                level: *level,
            });
        }
        enchantments.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(FurnaceSlot {
        count: get_int(item, "count")?,
        item_id,
        damage,
        enchantments,
    })
}

fn encode_container_stack(
    slot: usize,
    stack: &FurnaceSlot,
    items: &ItemRegistry,
) -> Result<Tag, ChunkNbtError> {
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| ChunkNbtError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = vec![
        ("Slot".into(), Tag::Int(slot as i32)),
        ("id".into(), Tag::String(name.as_str().to_string())),
        ("count".into(), Tag::Int(stack.count)),
    ];
    let mut components = Vec::new();
    if let Some(damage) = stack.damage {
        components.push((DAMAGE_COMPONENT.into(), Tag::Int(damage)));
    }
    if !stack.enchantments.is_empty() {
        components.push((
            ENCHANTMENTS_COMPONENT.into(),
            Tag::Compound(
                stack
                    .enchantments
                    .iter()
                    .map(|enchantment| {
                        (
                            enchantment.id.as_str().to_string(),
                            Tag::Int(enchantment.level),
                        )
                    })
                    .collect(),
            ),
        ));
    }
    if !components.is_empty() {
        fields.push(("components".into(), Tag::Compound(components)));
    }
    Ok(Tag::Compound(fields))
}

fn chest_storage_block_entity_id_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> &'static str {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    let Some(state_id) = chunk.get_block(local_x, pos.y, local_z) else {
        return "minecraft:chest";
    };
    let Some(state) = registry.by_id(state_id) else {
        return "minecraft:chest";
    };
    match state.block.id.as_str() {
        "minecraft:barrel" => "minecraft:barrel",
        _ => "minecraft:chest",
    }
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

fn get_optional_int(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i32>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Int(v) => Ok(Some(*v)),
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

fn get_optional_short(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i16>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Short(v) => Ok(Some(*v)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Short",
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
    use crate::chunk::{MAX_Y, MIN_SECTION_Y, MIN_Y};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    fn top_non_air_y(chunk: &Chunk, x: u8, z: u8, air: BlockStateId) -> Option<i32> {
        (MIN_Y..MAX_Y)
            .rev()
            .find(|&y| chunk.get_block(x, y, z) != Some(air))
    }

    #[test]
    fn furnace_family_block_entity_ids_follow_block_state() {
        fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
            mc_data::blocks::BlockReport {
                id: Identifier::parse(name).unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            }
        }

        let registry = BlockRegistry::from_report(&[
            block_report(0, "minecraft:air"),
            block_report(1, "minecraft:furnace"),
            block_report(2, "minecraft:smoker"),
            block_report(3, "minecraft:blast_furnace"),
        ])
        .unwrap();
        let items = mc_data::items::ItemRegistry::from_report(&[]);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
        let entries = [
            (
                BlockPos { x: 1, y: 64, z: 1 },
                BlockStateId(1),
                "minecraft:furnace",
            ),
            (
                BlockPos { x: 2, y: 64, z: 1 },
                BlockStateId(2),
                "minecraft:smoker",
            ),
            (
                BlockPos { x: 3, y: 64, z: 1 },
                BlockStateId(3),
                "minecraft:blast_furnace",
            ),
        ];
        for &(pos, state, _) in &entries {
            chunk
                .set_block(pos.x as u8, pos.y, pos.z as u8, state)
                .unwrap();
            let mut furnace = FurnaceBlockEntity::default();
            furnace
                .recipes_used
                .insert("minecraft:test_smelting".to_string(), 3);
            chunk.furnaces.insert(pos, furnace);
        }

        let root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
        let Tag::Compound(root_cmp) = &root else {
            panic!("chunk root must be a compound");
        };
        let Tag::List(block_entities) = root_cmp
            .iter()
            .find(|(key, _)| key == "block_entities")
            .map(|(_, tag)| tag)
            .expect("block_entities list")
        else {
            panic!("block_entities must be a list");
        };
        let ids: Vec<&str> = block_entities
            .elements
            .iter()
            .map(|tag| {
                let Tag::Compound(cmp) = tag else {
                    panic!("block entity must be a compound");
                };
                get_string(cmp, "id").unwrap().as_str()
            })
            .collect();
        assert_eq!(
            ids,
            entries.iter().map(|(_, _, id)| *id).collect::<Vec<&str>>()
        );

        let decoded = chunk_from_nbt_with_items(&root, &registry, Some(&items)).unwrap();
        for &(pos, _, _) in &entries {
            assert_eq!(
                decoded.furnaces[&pos]
                    .recipes_used
                    .get("minecraft:test_smelting"),
                Some(&3)
            );
        }
    }

    #[test]
    fn barrel_block_entity_id_follows_block_state() {
        fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
            mc_data::blocks::BlockReport {
                id: Identifier::parse(name).unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            }
        }

        let registry = BlockRegistry::from_report(&[
            block_report(0, "minecraft:air"),
            block_report(1, "minecraft:chest"),
            block_report(2, "minecraft:barrel"),
        ])
        .unwrap();
        let items = mc_data::items::ItemRegistry::from_report(&[]);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
        let entries = [
            (
                BlockPos { x: 1, y: 64, z: 1 },
                BlockStateId(1),
                "minecraft:chest",
            ),
            (
                BlockPos { x: 2, y: 64, z: 1 },
                BlockStateId(2),
                "minecraft:barrel",
            ),
        ];
        for &(pos, state, _) in &entries {
            chunk
                .set_block(pos.x as u8, pos.y, pos.z as u8, state)
                .unwrap();
            chunk.chests.insert(pos, ChestBlockEntity::default());
        }

        let root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
        let Tag::Compound(root_cmp) = &root else {
            panic!("chunk root must be a compound");
        };
        let Tag::List(block_entities) = root_cmp
            .iter()
            .find(|(key, _)| key == "block_entities")
            .map(|(_, tag)| tag)
            .expect("block_entities list")
        else {
            panic!("block_entities must be a list");
        };
        let ids: Vec<&str> = block_entities
            .elements
            .iter()
            .map(|tag| {
                let Tag::Compound(cmp) = tag else {
                    panic!("block entity must be a compound");
                };
                get_string(cmp, "id").unwrap().as_str()
            })
            .collect();
        assert_eq!(
            ids,
            entries.iter().map(|(_, _, id)| *id).collect::<Vec<&str>>()
        );

        let decoded = chunk_from_nbt_with_items(&root, &registry, Some(&items)).unwrap();
        for &(pos, _, _) in &entries {
            assert!(decoded.chests.contains_key(&pos));
        }
    }

    #[test]
    fn hopper_block_entity_items_round_trip_through_anvil() {
        fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
            mc_data::blocks::BlockReport {
                id: Identifier::parse(name).unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            }
        }

        let registry = BlockRegistry::from_report(&[
            block_report(0, "minecraft:air"),
            block_report(1, "minecraft:hopper"),
        ])
        .unwrap();
        let items = mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:cobblestone").unwrap(),
                protocol_id: 10,
            },
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:apple").unwrap(),
                protocol_id: 11,
            },
        ]);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
        let pos = BlockPos { x: 4, y: 64, z: 5 };
        chunk.set_block(4, 64, 5, BlockStateId(1)).unwrap();
        let mut hopper = HopperBlockEntity {
            transfer_cooldown: 5,
            ..Default::default()
        };
        hopper.slots[0] = FurnaceSlot {
            count: 64,
            item_id: 10,
            damage: Some(7),
            enchantments: vec![mc_data::ItemEnchantment {
                id: Identifier::parse("minecraft:efficiency").unwrap(),
                level: 1,
            }],
        };
        hopper.slots[4] = FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
            enchantments: Vec::new(),
        };
        chunk.hoppers.insert(pos, hopper.clone());

        let root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
        let Tag::Compound(root_cmp) = &root else {
            panic!("chunk root must be a compound");
        };
        let Tag::List(block_entities) = root_cmp
            .iter()
            .find(|(key, _)| key == "block_entities")
            .map(|(_, tag)| tag)
            .expect("block_entities list")
        else {
            panic!("block_entities must be a list");
        };
        assert_eq!(block_entities.elements.len(), 1);
        let Tag::Compound(cmp) = &block_entities.elements[0] else {
            panic!("hopper block entity must be a compound");
        };
        assert_eq!(get_string(cmp, "id").unwrap(), "minecraft:hopper");
        assert_eq!(get_int(cmp, "TransferCooldown").unwrap(), 5);

        let decoded = chunk_from_nbt_with_items(&root, &registry, Some(&items)).unwrap();
        assert_eq!(decoded.hoppers.get(&pos), Some(&hopper));
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
            assert_eq!(
                chunk1.scheduled_block_ticks(),
                chunk2.scheduled_block_ticks(),
                "scheduled block ticks in {:?}",
                chunk1.pos
            );
            assert_eq!(
                chunk1.scheduled_fluid_ticks(),
                chunk2.scheduled_fluid_ticks(),
                "scheduled fluid ticks in {:?}",
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
            let vanilla_extras = chunk
                .extras
                .iter()
                .filter(|(key, _)| key != "SolarisJournalLsn")
                .collect::<Vec<_>>();
            if !vanilla_extras.is_empty() {
                chunks_with_extras += 1;
                for (k, _) in vanilla_extras {
                    seen_keys.insert(k.clone());
                }
            }
        }

        // Vanilla-generated oracle chunks have at least DataVersion +
        // InhabitedTime + LastUpdate. Solaris-generated local worlds do
        // not currently persist unmodelled root extras, so this oracle
        // assertion is skipped for that shape.
        if chunks_with_extras == 0 {
            eprintln!("skipping: test world carries no unmodelled root fields");
            return;
        }
        for required in &["DataVersion", "InhabitedTime", "LastUpdate"] {
            assert!(
                seen_keys.contains(*required),
                "extras key set should contain {required}; got {:?}",
                seen_keys,
            );
        }
    }

    /// M6.a: a chunk is decoded from the real test world, one block is
    /// mutated via `set_block_and_update`, then `chunk_to_payload` +
    /// `write_region` + `read_region` round-trips it back through a
    /// fresh `.mca` file. The mutated cell must read back as the new
    /// state, every other modelled field plus `extras` must survive.
    #[test]
    fn round_trip_modified_chunk_through_disk() {
        let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !region_path.is_file() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let payloads = region::read_region(&region_path).unwrap();

        let original_payload = payloads
            .iter()
            .find(|p| p.local_x == 0 && p.local_z == 0)
            .expect("test world has chunk (0,0)");
        let mut cur = Cursor::new(&original_payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let mut chunk = chunk_from_nbt(&root, &registry).unwrap();

        // Mutation: top block under spawn → a different solid state.
        let air = registry
            .block(&Identifier::parse("minecraft:air").unwrap())
            .map(|b| b.default)
            .unwrap();
        let stone = registry
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .map(|b| b.default)
            .unwrap();
        let dirt = registry
            .block(&Identifier::parse("minecraft:dirt").unwrap())
            .map(|b| b.default)
            .unwrap();
        let edit_y = top_non_air_y(&chunk, 0, 0, air).expect("origin column has terrain");
        let current = chunk.get_block(0, edit_y, 0).expect("edit cell present");
        let new_state = if current == stone { dirt } else { stone };
        let prev = chunk
            .set_block_and_update(0, edit_y, 0, new_state, air)
            .expect("y in range");
        assert_ne!(
            prev, new_state,
            "test world cell was already the replacement state — pick another"
        );
        assert!(chunk.dirty, "set_block_and_update marks chunk dirty");

        // Encode, write a fresh single-chunk region, read it back.
        let payload = chunk_to_payload(&chunk, &registry, 1_700_000_000).unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        region::write_region(tmp.path(), &[payload]).unwrap();
        let reread = region::read_region(tmp.path()).unwrap();
        let reread_payload = reread
            .iter()
            .find(|p| p.local_x == 0 && p.local_z == 0)
            .expect("written chunk must read back");
        let mut cur2 = Cursor::new(&reread_payload.uncompressed_nbt[..]);
        let (_, root2) = mc_nbt::read_named(&mut cur2).unwrap();
        let chunk2 = chunk_from_nbt(&root2, &registry).unwrap();

        // Mutation survived.
        assert_eq!(chunk2.get_block(0, edit_y, 0), Some(new_state));
        // Same chunk position.
        assert_eq!(chunk2.pos, chunk.pos);
        // Heightmap is whatever set_block_and_update produced.
        let hms_before: std::collections::BTreeSet<&String> = chunk.heightmaps.keys().collect();
        let hms_after: std::collections::BTreeSet<&String> = chunk2.heightmaps.keys().collect();
        assert_eq!(hms_before, hms_after, "heightmap key set must survive");
        // Extras (DataVersion / InhabitedTime / …) survived byte-stably.
        assert_eq!(chunk.extras.len(), chunk2.extras.len(), "extras count");
        for (e1, e2) in chunk.extras.iter().zip(&chunk2.extras) {
            assert_eq!(e1.0, e2.0, "extras key order");
            assert_eq!(e1.1, e2.1, "extras value for {}", e1.0);
        }
        // Timestamp survived through the write/read pair.
        assert_eq!(reread_payload.timestamp, 1_700_000_000);
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

    fn build_chunk_root_with_geometry(
        y_pos: i32,
        section_count: usize,
        section_overrides: Vec<Tag>,
    ) -> Tag {
        let mut sections = (0..section_count)
            .map(|index| {
                build_section_with_light(i8::try_from(y_pos + index as i32).unwrap(), None, None)
            })
            .collect::<Vec<_>>();
        for section in section_overrides {
            let compound = expect_compound(&section, "section override").unwrap();
            let section_y = get_byte(compound, "Y").unwrap() as i32;
            let index = usize::try_from(section_y - y_pos).unwrap();
            sections[index] = section;
        }
        Tag::Compound(vec![
            ("xPos".into(), Tag::Int(0)),
            ("zPos".into(), Tag::Int(0)),
            ("yPos".into(), Tag::Int(y_pos)),
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

    fn build_chunk_root_with_sections(section_overrides: Vec<Tag>) -> Tag {
        build_chunk_root_with_geometry(
            MIN_SECTION_Y,
            (MAX_Y - MIN_Y) as usize / SECTION_DIM,
            section_overrides,
        )
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
    fn encode_unknown_block_state_returns_error_without_panicking() {
        let registry = tiny_registry();
        let chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(7),
            Identifier::parse("minecraft:plains").unwrap(),
        );

        let encoded = std::panic::catch_unwind(|| chunk_to_nbt(&chunk, &registry));
        let error = encoded
            .expect("unknown block state encoding must not panic")
            .expect_err("unknown block state encoding must fail");

        assert!(matches!(error, ChunkNbtError::UnknownBlockStateId(7)));
    }

    fn scheduled_tick_tag(id: &str, x: i32, y: i32, z: i32, delay: i32, priority: i32) -> Tag {
        Tag::Compound(vec![
            ("i".into(), Tag::String(id.into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("t".into(), Tag::Int(delay)),
            ("p".into(), Tag::Int(priority)),
        ])
    }

    fn add_root_field(root: &mut Tag, key: &str, value: Tag) {
        let Tag::Compound(entries) = root else {
            panic!("test root must be compound");
        };
        entries.push((key.into(), value));
    }

    fn root_fields(root: &Tag) -> &[(String, Tag)] {
        match root {
            Tag::Compound(entries) => entries,
            other => panic!("expected root compound, got {other:?}"),
        }
    }

    fn scheduled_tick_list(elements: Vec<Tag>) -> Tag {
        Tag::List(ListTag {
            element_type: if elements.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements,
        })
    }

    #[test]
    fn round_trips_non_overworld_geometry_from_y_pos_and_sections() {
        let root = build_chunk_root_with_geometry(0, 16, Vec::new());

        let registry = tiny_registry();
        let chunk = chunk_from_nbt(&root, &registry).expect("decode custom geometry");
        assert_eq!(chunk.geometry().min_y(), 0);
        assert_eq!(chunk.geometry().max_y(), 256);

        let encoded = chunk_to_nbt(&chunk, &registry).expect("encode custom geometry");
        let encoded_root = expect_compound(&encoded, "root").unwrap();
        assert_eq!(get_int(encoded_root, "yPos").unwrap(), 0);

        let decoded = chunk_from_nbt(&encoded, &registry).expect("decode round-trip");
        assert_eq!(decoded.geometry(), chunk.geometry());
    }

    #[test]
    fn rejects_geometry_when_sections_do_not_define_a_height() {
        let root = build_chunk_root_with_geometry(0, 0, Vec::new());

        assert!(matches!(
            chunk_from_nbt(&root, &tiny_registry()),
            Err(ChunkNbtError::InvalidChunkGeometry {
                y_pos: 0,
                section_count: 0
            })
        ));
    }

    #[test]
    fn rejects_duplicate_section_y_in_inferred_geometry() {
        let mut root = build_chunk_root_with_geometry(0, 16, Vec::new());
        let Tag::Compound(fields) = &mut root else {
            unreachable!();
        };
        let Tag::List(sections) = &mut fields
            .iter_mut()
            .find(|(name, _)| name == "sections")
            .expect("synthetic chunk has sections")
            .1
        else {
            unreachable!();
        };
        let Tag::Compound(last_section) = sections.elements.last_mut().unwrap() else {
            unreachable!();
        };
        let y = last_section
            .iter_mut()
            .find(|(name, _)| name == "Y")
            .expect("synthetic section has Y");
        y.1 = Tag::Byte(14);

        assert!(matches!(
            chunk_from_nbt(&root, &tiny_registry()),
            Err(ChunkNbtError::InvalidSectionShape(14))
        ));
    }

    #[test]
    fn decodes_scheduled_block_ticks_from_block_ticks() {
        let mut root = build_chunk_root_with_sections(Vec::new());
        add_root_field(
            &mut root,
            "block_ticks",
            scheduled_tick_list(vec![
                scheduled_tick_tag("minecraft:stone", 2, 64, 2, 5, 0),
                scheduled_tick_tag("minecraft:air", 1, 64, 1, 5, 0),
                scheduled_tick_tag("minecraft:stone", 3, 64, 3, 1, 1),
            ]),
        );

        let mut chunk = chunk_from_nbt(&root, &tiny_registry()).expect("decode");
        let ticks = chunk.scheduled_block_ticks();
        assert_eq!(ticks.len(), 3);
        assert_eq!(ticks[0].trigger_tick, 1);
        assert_eq!(ticks[0].priority, 1);
        assert_eq!(ticks[0].block.as_str(), "minecraft:stone");

        let due = chunk.drain_due_block_ticks(5, usize::MAX);
        assert_eq!(
            due.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn decodes_scheduled_fluid_ticks_from_fluid_ticks() {
        let mut root = build_chunk_root_with_sections(Vec::new());
        add_root_field(
            &mut root,
            "fluid_ticks",
            scheduled_tick_list(vec![
                scheduled_tick_tag("minecraft:water", 2, 64, 2, 5, 0),
                scheduled_tick_tag("minecraft:lava", 1, 64, 1, 5, 0),
                scheduled_tick_tag("minecraft:water", 3, 64, 3, 1, 1),
            ]),
        );

        let mut chunk = chunk_from_nbt(&root, &tiny_registry()).expect("decode");
        let ticks = chunk.scheduled_fluid_ticks();
        assert_eq!(ticks.len(), 3);
        assert_eq!(ticks[0].trigger_tick, 1);
        assert_eq!(ticks[0].priority, 1);
        assert_eq!(ticks[0].fluid.as_str(), "minecraft:water");

        let due = chunk.drain_due_fluid_ticks(5, usize::MAX);
        assert_eq!(
            due.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn block_and_fluid_ticks_are_modelled_not_extras() {
        let mut root = build_chunk_root_with_sections(Vec::new());
        add_root_field(
            &mut root,
            "block_ticks",
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:stone", 1, 64, 1, 2, 0)]),
        );
        add_root_field(
            &mut root,
            "fluid_ticks",
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:water", 1, 64, 1, 2, 0)]),
        );

        let chunk = chunk_from_nbt(&root, &tiny_registry()).expect("decode");

        assert_eq!(chunk.scheduled_block_ticks().len(), 1);
        assert_eq!(chunk.scheduled_fluid_ticks().len(), 1);
        assert!(!chunk.extras.iter().any(|(key, _)| key == "block_ticks"));
        assert!(!chunk.extras.iter().any(|(key, _)| key == "fluid_ticks"));
    }

    #[test]
    fn encodes_scheduled_block_ticks_without_duplicate_root_key() {
        let registry = tiny_registry();
        let mut chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        assert!(chunk.schedule_block_tick(ScheduledBlockTick::new(
            BlockPos { x: 1, y: 64, z: 1 },
            Identifier::parse("minecraft:stone").unwrap(),
            7,
            -1,
        )));
        chunk.extras.push((
            "block_ticks".into(),
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:air", 2, 64, 2, 1, 0)]),
        ));

        let root = chunk_to_nbt(&chunk, &registry).expect("encode");
        let block_tick_fields: Vec<&Tag> = root_fields(&root)
            .iter()
            .filter_map(|(key, value)| (key == "block_ticks").then_some(value))
            .collect();

        assert_eq!(block_tick_fields.len(), 1);
        let Tag::List(list) = block_tick_fields[0] else {
            panic!("block_ticks must encode as list");
        };
        assert_eq!(list.element_type, tag_type::COMPOUND);
        assert_eq!(list.elements.len(), 1);
    }

    #[test]
    fn encodes_scheduled_fluid_ticks_without_duplicate_root_key() {
        let registry = tiny_registry();
        let mut chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        assert!(chunk.schedule_fluid_tick(ScheduledFluidTick::new(
            BlockPos { x: 1, y: 64, z: 1 },
            Identifier::parse("minecraft:water").unwrap(),
            7,
            -1,
        )));
        chunk.extras.push((
            "fluid_ticks".into(),
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:lava", 2, 64, 2, 1, 0)]),
        ));

        let root = chunk_to_nbt(&chunk, &registry).expect("encode");
        let fluid_tick_fields: Vec<&Tag> = root_fields(&root)
            .iter()
            .filter_map(|(key, value)| (key == "fluid_ticks").then_some(value))
            .collect();

        assert_eq!(fluid_tick_fields.len(), 1);
        let Tag::List(list) = fluid_tick_fields[0] else {
            panic!("fluid_ticks must encode as list");
        };
        assert_eq!(list.element_type, tag_type::COMPOUND);
        assert_eq!(list.elements.len(), 1);
    }

    #[test]
    fn scheduled_ticks_round_trip_through_disk() {
        let registry = tiny_registry();
        let mut chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        assert!(chunk.schedule_block_tick(ScheduledBlockTick::new(
            BlockPos { x: 1, y: 64, z: 1 },
            Identifier::parse("minecraft:stone").unwrap(),
            12,
            0,
        )));
        assert!(chunk.schedule_fluid_tick(ScheduledFluidTick::new(
            BlockPos { x: 2, y: 64, z: 2 },
            Identifier::parse("minecraft:water").unwrap(),
            8,
            -1,
        )));

        let payload = chunk_to_payload(&chunk, &registry, 1_700_000_001).unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        region::write_region(tmp.path(), &[payload]).unwrap();
        let reread = region::read_region(tmp.path()).unwrap();
        let mut cur = Cursor::new(&reread[0].uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let decoded = chunk_from_nbt(&root, &registry).unwrap();

        assert_eq!(decoded.scheduled_block_ticks().len(), 1);
        assert_eq!(decoded.scheduled_block_ticks()[0].trigger_tick, 12);
        assert_eq!(decoded.scheduled_block_ticks()[0].priority, 0);
        assert_eq!(decoded.scheduled_fluid_ticks().len(), 1);
        assert_eq!(decoded.scheduled_fluid_ticks()[0].trigger_tick, 8);
        assert_eq!(decoded.scheduled_fluid_ticks()[0].priority, -1);
        assert!(!decoded.extras.iter().any(|(key, _)| key == "fluid_ticks"));
    }

    #[test]
    fn rejects_negative_scheduled_block_tick_delay() {
        let mut root = build_chunk_root_with_sections(Vec::new());
        add_root_field(
            &mut root,
            "block_ticks",
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:stone", 1, 64, 1, -1, 0)]),
        );

        match chunk_from_nbt(&root, &tiny_registry()) {
            Err(ChunkNbtError::NegativeTickDelay(-1)) => {}
            other => panic!("expected NegativeTickDelay, got {other:?}"),
        }
    }

    #[test]
    fn rejects_negative_scheduled_fluid_tick_delay() {
        let mut root = build_chunk_root_with_sections(Vec::new());
        add_root_field(
            &mut root,
            "fluid_ticks",
            scheduled_tick_list(vec![scheduled_tick_tag("minecraft:water", 1, 64, 1, -1, 0)]),
        );

        match chunk_from_nbt(&root, &tiny_registry()) {
            Err(ChunkNbtError::NegativeTickDelay(-1)) => {}
            other => panic!("expected NegativeTickDelay, got {other:?}"),
        }
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
    fn encodes_per_section_light_arrays_when_present() {
        let registry = tiny_registry();
        let pattern = nibble_pattern();
        let expected: Vec<u8> = pattern.iter().map(|&byte| byte as u8).collect();
        let mut chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk.section_lights[0].block = Some(expected.clone());
        chunk.section_lights[0].sky = Some(expected.clone());
        chunk.section_lights[4].sky = Some(expected.clone());

        let root = chunk_to_nbt(&chunk, &registry).expect("encode");
        let sections = get_optional_list(expect_compound(&root, "root").unwrap(), "sections")
            .unwrap()
            .unwrap();

        let first = expect_compound(&sections.elements[0], "sections[0]").unwrap();
        assert_eq!(
            first
                .iter()
                .find(|(key, _)| key == "BlockLight")
                .map(|(_, tag)| tag),
            Some(&Tag::ByteArray(pattern.clone()))
        );
        assert_eq!(
            first
                .iter()
                .find(|(key, _)| key == "SkyLight")
                .map(|(_, tag)| tag),
            Some(&Tag::ByteArray(pattern.clone()))
        );
        let second = expect_compound(&sections.elements[1], "sections[1]").unwrap();
        assert!(
            second
                .iter()
                .all(|(key, _)| key != "BlockLight" && key != "SkyLight")
        );
        let y_zero = expect_compound(&sections.elements[4], "sections[4]").unwrap();
        assert!(y_zero.iter().all(|(key, _)| key != "BlockLight"));
        assert_eq!(
            y_zero
                .iter()
                .find(|(key, _)| key == "SkyLight")
                .map(|(_, tag)| tag),
            Some(&Tag::ByteArray(pattern))
        );

        let decoded = chunk_from_nbt(&root, &registry).expect("decode encoded light");
        assert_eq!(
            decoded.section_lights[0].block.as_deref(),
            Some(&expected[..])
        );
        assert_eq!(
            decoded.section_lights[0].sky.as_deref(),
            Some(&expected[..])
        );
        assert_eq!(decoded.section_lights[4].block, None);
        assert_eq!(
            decoded.section_lights[4].sky.as_deref(),
            Some(&expected[..])
        );
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

        // Vanilla-generated oracle chunks carry baked SkyLight. Solaris
        // generated local worlds do not persist light arrays yet (queued
        // for a later milestone), so skip that oracle shape.
        if chunks_with_sky == 0 {
            eprintln!("skipping: test world carries no baked SkyLight");
            return;
        }
        assert!(
            sections_with_sky >= 1,
            "expected ≥1 section with baked SkyLight, got {sections_with_sky}",
        );
    }
}
