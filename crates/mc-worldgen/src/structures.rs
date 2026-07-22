use std::io::Read;
use std::path::Path;

use bytes::Bytes;
use flate2::read::GzDecoder;
use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_data::worldgen_structures::StructureSetFacts;
use mc_nbt::{ListTag, Tag};
use mc_world::{BlockRegistry, BlockStateId, ChestBlockEntity, FurnaceSlot};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StructureError {
    #[error("reading structure template {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("decoding NBT structure template {path}: {source}")]
    Nbt {
        path: String,
        #[source]
        source: mc_nbt::NbtError,
    },
    #[error("structure template {path} is missing {field}")]
    MissingField { path: String, field: &'static str },
    #[error("structure template {path} has invalid {field}")]
    InvalidField { path: String, field: &'static str },
    #[error("structure template {path} references unknown block {block}")]
    UnknownBlock { path: String, block: Identifier },
    #[error("structure template {path} references unresolved block state {block}")]
    UnknownState { path: String, block: Identifier },
    #[error("Solaris playable ruin references missing item {item}")]
    MissingPlayableRuinItem { item: Identifier },
}

#[derive(Debug, Clone)]
pub struct StructureTemplate {
    size: [i32; 3],
    blocks: Vec<TemplateBlock>,
    chests: Vec<TemplateChest>,
}

#[derive(Debug, Clone, Copy)]
pub struct TemplateBlock {
    pub pos: [i32; 3],
    pub state: BlockStateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateChest {
    pub pos: [i32; 3],
    pub chest: ChestBlockEntity,
}

impl StructureTemplate {
    #[must_use]
    pub fn new(size: [i32; 3], blocks: Vec<TemplateBlock>) -> Self {
        Self {
            size,
            blocks,
            chests: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_chests(mut self, chests: Vec<TemplateChest>) -> Self {
        self.chests = chests;
        self
    }

    pub fn from_nbt_file(
        path: impl AsRef<Path>,
        registry: &BlockRegistry,
    ) -> Result<Self, StructureError> {
        let path = path.as_ref();
        let display = path.display().to_string();
        let raw = std::fs::read(path).map_err(|source| StructureError::Io {
            path: display.clone(),
            source,
        })?;
        let mut decoded = Vec::new();
        if raw.starts_with(&[0x1f, 0x8b]) {
            GzDecoder::new(raw.as_slice())
                .read_to_end(&mut decoded)
                .map_err(|source| StructureError::Io {
                    path: display.clone(),
                    source,
                })?;
        } else {
            decoded = raw;
        }

        let mut bytes = Bytes::from(decoded);
        let (_name, root) =
            mc_nbt::read_named(&mut bytes).map_err(|source| StructureError::Nbt {
                path: display.clone(),
                source,
            })?;
        Self::from_tag(&display, &root, registry)
    }

    #[must_use]
    pub fn size(&self) -> [i32; 3] {
        self.size
    }

    #[must_use]
    pub fn blocks(&self) -> &[TemplateBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn chests(&self) -> &[TemplateChest] {
        &self.chests
    }

    fn from_tag(path: &str, root: &Tag, registry: &BlockRegistry) -> Result<Self, StructureError> {
        let compound = expect_compound(path, root, "root")?;
        let size = expect_int_triplet(path, require(compound, path, "size")?, "size")?;
        let palette = parse_palette(path, require(compound, path, "palette")?, registry)?;
        let blocks = parse_blocks(path, require(compound, path, "blocks")?, &palette)?;
        Ok(Self::new(size, blocks))
    }
}

#[derive(Debug, Clone)]
pub struct StructureRules {
    templates: Vec<StructureTemplate>,
    grid_chunks: i32,
    separation_chunks: i32,
    salt: u64,
    fixed_center: Option<(i32, i32)>,
}

impl StructureRules {
    #[must_use]
    pub fn none() -> Self {
        Self {
            templates: Vec::new(),
            grid_chunks: 34,
            separation_chunks: 8,
            salt: 0x9E37_8731_2B17,
            fixed_center: None,
        }
    }

    #[must_use]
    pub fn plains_village_markers(templates: Vec<StructureTemplate>) -> Self {
        Self {
            templates,
            // Mirrors the vanilla village spacing as data, while Solaris owns
            // the placement hash and filtering below.
            grid_chunks: 34,
            separation_chunks: 8,
            salt: 10_387_312,
            fixed_center: None,
        }
    }

    /// A Solaris-owned reward ruin for the seed-zero playable loop.
    ///
    /// Its block states and item protocol ids are resolved from startup
    /// registries; no protocol values are embedded here.
    pub fn solaris_playable_ruin(
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<Self, StructureError> {
        let cobblestone = playable_ruin_block(blocks, "minecraft:cobblestone")?;
        let chest_state = playable_ruin_block(blocks, "minecraft:chest")?;
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = playable_ruin_slot(items, "minecraft:diamond", 1)?;
        chest.slots[1] = playable_ruin_slot(items, "minecraft:lapis_lazuli", 4)?;
        chest.slots[2] = playable_ruin_slot(items, "minecraft:bread", 2)?;

        let mut ruin_blocks = Vec::new();
        for x in 0..5 {
            for z in 0..5 {
                ruin_blocks.push(TemplateBlock {
                    pos: [x, 0, z],
                    state: cobblestone,
                });
            }
        }
        for x in [0, 4] {
            for z in [0, 4] {
                for y in 1..4 {
                    ruin_blocks.push(TemplateBlock {
                        pos: [x, y, z],
                        state: cobblestone,
                    });
                }
            }
        }
        ruin_blocks.push(TemplateBlock {
            pos: [2, 1, 2],
            state: chest_state,
        });

        let template =
            StructureTemplate::new([5, 4, 5], ruin_blocks).with_chests(vec![TemplateChest {
                pos: [2, 1, 2],
                chest,
            }]);
        Ok(Self {
            templates: vec![template],
            grid_chunks: 34,
            separation_chunks: 8,
            salt: 0x0053_4F4C_4152_4953,
            // Chunk (4, 0): 4.5 chunks east of spawn, inside one generated chunk.
            fixed_center: Some((72, 8)),
        })
    }

    #[must_use]
    pub fn with_structure_set_facts(mut self, facts: &[StructureSetFacts]) -> Self {
        let Some(villages) = facts.iter().find(|set| {
            set.structures
                .iter()
                .any(|structure| structure.as_str() == "minecraft:village_plains")
        }) else {
            return self;
        };
        if let Some(spacing) = villages.spacing {
            self.grid_chunks = spacing.max(1);
        }
        if let Some(separation) = villages.separation {
            self.separation_chunks = separation.max(0).min(self.grid_chunks - 1);
        }
        if let Some(salt) = villages.salt {
            self.salt = salt;
        }
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    #[must_use]
    pub fn templates(&self) -> &[StructureTemplate] {
        &self.templates
    }

    #[must_use]
    pub fn grid_chunks(&self) -> i32 {
        self.grid_chunks
    }

    #[must_use]
    pub fn separation_chunks(&self) -> i32 {
        self.separation_chunks
    }

    #[must_use]
    pub fn salt(&self) -> u64 {
        self.salt
    }

    pub(crate) fn fixed_center(&self) -> Option<(i32, i32)> {
        self.fixed_center
    }

    #[cfg(test)]
    pub(crate) fn fixed_for_test(template: StructureTemplate, center: (i32, i32)) -> Self {
        Self {
            templates: vec![template],
            grid_chunks: 34,
            separation_chunks: 8,
            salt: 0,
            fixed_center: Some(center),
        }
    }
}

fn playable_ruin_block(blocks: &BlockRegistry, name: &str) -> Result<BlockStateId, StructureError> {
    let block = Identifier::parse(name).expect("static playable ruin block identifier");
    blocks
        .block(&block)
        .map(|entry| entry.default)
        .ok_or(StructureError::UnknownBlock {
            path: "Solaris playable ruin".to_string(),
            block,
        })
}

fn playable_ruin_slot(
    items: &ItemRegistry,
    name: &str,
    count: i32,
) -> Result<FurnaceSlot, StructureError> {
    let item = Identifier::parse(name).expect("static playable ruin item identifier");
    let item_id = items
        .id_of(&item)
        .ok_or_else(|| StructureError::MissingPlayableRuinItem { item: item.clone() })?;
    Ok(FurnaceSlot {
        item_id,
        count,
        damage: None,
        enchantments: Vec::new(),
    })
}

impl Default for StructureRules {
    fn default() -> Self {
        Self::none()
    }
}

fn parse_palette(
    path: &str,
    tag: &Tag,
    registry: &BlockRegistry,
) -> Result<Vec<Option<BlockStateId>>, StructureError> {
    let list = expect_list(path, tag, "palette")?;
    list.elements
        .iter()
        .map(|entry| parse_palette_entry(path, entry, registry))
        .collect()
}

fn parse_palette_entry(
    path: &str,
    entry: &Tag,
    registry: &BlockRegistry,
) -> Result<Option<BlockStateId>, StructureError> {
    let compound = expect_compound(path, entry, "palette[]")?;
    let name = expect_string(path, require(compound, path, "Name")?, "Name")?;
    let id = Identifier::parse(name.clone()).map_err(|_| StructureError::InvalidField {
        path: path.to_string(),
        field: "palette[].Name",
    })?;
    if matches!(
        id.as_str(),
        "minecraft:air" | "minecraft:jigsaw" | "minecraft:structure_void"
    ) {
        return Ok(None);
    }
    let props = match compound.iter().find(|(key, _)| key == "Properties") {
        Some((_, tag)) => parse_properties(path, tag)?,
        None => Vec::new(),
    };
    if registry.block(&id).is_none() {
        return Err(StructureError::UnknownBlock {
            path: path.to_string(),
            block: id,
        });
    }
    registry
        .by_name_and_props(&id, &props)
        .map(Some)
        .ok_or_else(|| StructureError::UnknownState {
            path: path.to_string(),
            block: id,
        })
}

fn parse_properties(path: &str, tag: &Tag) -> Result<Vec<(String, String)>, StructureError> {
    let compound = expect_compound(path, tag, "Properties")?;
    compound
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                expect_string(path, value, "Properties[]")?.clone(),
            ))
        })
        .collect()
}

fn parse_blocks(
    path: &str,
    tag: &Tag,
    palette: &[Option<BlockStateId>],
) -> Result<Vec<TemplateBlock>, StructureError> {
    let list = expect_list(path, tag, "blocks")?;
    let mut blocks = Vec::new();
    for entry in &list.elements {
        let compound = expect_compound(path, entry, "blocks[]")?;
        let pos = expect_int_triplet(path, require(compound, path, "pos")?, "blocks[].pos")?;
        let state_idx = expect_int(path, require(compound, path, "state")?, "blocks[].state")?;
        let state =
            palette
                .get(state_idx as usize)
                .ok_or_else(|| StructureError::InvalidField {
                    path: path.to_string(),
                    field: "blocks[].state",
                })?;
        if let Some(state) = state {
            blocks.push(TemplateBlock { pos, state: *state });
        }
    }
    Ok(blocks)
}

fn require<'a>(
    compound: &'a [(String, Tag)],
    path: &str,
    field: &'static str,
) -> Result<&'a Tag, StructureError> {
    compound
        .iter()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value)
        .ok_or_else(|| StructureError::MissingField {
            path: path.to_string(),
            field,
        })
}

fn expect_compound<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a [(String, Tag)], StructureError> {
    match tag {
        Tag::Compound(entries) => Ok(entries),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_list<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a ListTag, StructureError> {
    match tag {
        Tag::List(list) => Ok(list),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_string<'a>(
    path: &str,
    tag: &'a Tag,
    field: &'static str,
) -> Result<&'a String, StructureError> {
    match tag {
        Tag::String(value) => Ok(value),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_int(path: &str, tag: &Tag, field: &'static str) -> Result<i32, StructureError> {
    match tag {
        Tag::Int(value) => Ok(*value),
        _ => Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        }),
    }
}

fn expect_int_triplet(
    path: &str,
    tag: &Tag,
    field: &'static str,
) -> Result<[i32; 3], StructureError> {
    let list = expect_list(path, tag, field)?;
    if list.elements.len() != 3 {
        return Err(StructureError::InvalidField {
            path: path.to_string(),
            field,
        });
    }
    Ok([
        expect_int(path, &list.elements[0], field)?,
        expect_int(path, &list.elements[1], field)?,
        expect_int(path, &list.elements[2], field)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    #[test]
    fn loads_real_plains_fountain_template_when_present() {
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        let template_path = workspace_path(
            "data/vanilla/data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt",
        );
        if !blocks_path.exists() || !template_path.exists() {
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let template = StructureTemplate::from_nbt_file(&template_path, &registry).unwrap();

        assert_eq!(template.size(), [9, 4, 9]);
        assert!(template.blocks().len() > 100);
        assert!(
            template
                .blocks()
                .iter()
                .all(|block| registry.by_id(block.state).is_some())
        );
    }

    #[test]
    fn plains_village_rules_accept_multiple_templates() {
        let a = StructureTemplate::new(
            [1, 1, 1],
            vec![TemplateBlock {
                pos: [0, 0, 0],
                state: BlockStateId(1),
            }],
        );
        let b = StructureTemplate::new(
            [1, 2, 1],
            vec![TemplateBlock {
                pos: [0, 1, 0],
                state: BlockStateId(2),
            }],
        );

        let rules = StructureRules::plains_village_markers(vec![a, b]);

        assert_eq!(rules.templates().len(), 2);
        assert_eq!(rules.grid_chunks(), 34);
        assert_eq!(rules.separation_chunks(), 8);
    }

    #[test]
    fn structure_set_facts_adjust_plains_village_spacing() {
        let template = StructureTemplate::new(
            [1, 1, 1],
            vec![TemplateBlock {
                pos: [0, 0, 0],
                state: BlockStateId(1),
            }],
        );
        let facts = vec![StructureSetFacts {
            id: Identifier::parse("minecraft:villages").unwrap(),
            structures: vec![Identifier::parse("minecraft:village_plains").unwrap()],
            placement_type: Some(Identifier::parse("minecraft:random_spread").unwrap()),
            spacing: Some(20),
            separation: Some(5),
            salt: Some(1234),
        }];

        let rules =
            StructureRules::plains_village_markers(vec![template]).with_structure_set_facts(&facts);

        assert_eq!(rules.grid_chunks(), 20);
        assert_eq!(rules.separation_chunks(), 5);
        assert_eq!(rules.salt(), 1234);
    }
}
