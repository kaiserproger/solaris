use std::io::Read;
use std::path::Path;

use bytes::Bytes;
use flate2::read::GzDecoder;
use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_data::worldgen_structures::{
    StructureDataError, StructureSetFacts, load_structure_set_facts,
};
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
    #[error(
        "plains village templates expose {available} villager slots for {requested} planned inhabitants"
    )]
    MissingVillagerSlots { requested: usize, available: usize },
    #[error(transparent)]
    StructureData(#[from] StructureDataError),
}

#[derive(Debug, Clone)]
pub struct StructureTemplate {
    size: [i32; 3],
    blocks: Vec<TemplateBlock>,
    chests: Vec<TemplateChest>,
    villager_markers: Vec<[i32; 3]>,
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
            villager_markers: Vec::new(),
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

    #[must_use]
    pub fn villager_markers(&self) -> &[[i32; 3]] {
        &self.villager_markers
    }

    fn from_tag(path: &str, root: &Tag, registry: &BlockRegistry) -> Result<Self, StructureError> {
        let compound = expect_compound(path, root, "root")?;
        let size = expect_int_triplet(path, require(compound, path, "size")?, "size")?;
        let palette = parse_palette(path, require(compound, path, "palette")?, registry)?;
        let (blocks, villager_markers) =
            parse_blocks(path, require(compound, path, "blocks")?, &palette)?;
        let mut template = Self::new(size, blocks);
        template.villager_markers = villager_markers;
        Ok(template)
    }

    fn combine(parts: Vec<(Self, [i32; 3])>) -> Self {
        let mut size = [0; 3];
        let mut blocks = Vec::new();
        let mut chests = Vec::new();
        let mut villager_markers = Vec::new();
        for (part, offset) in parts {
            for axis in 0..3 {
                size[axis] = size[axis].max(offset[axis] + part.size[axis]);
            }
            blocks.extend(part.blocks.into_iter().map(|mut block| {
                for (coordinate, delta) in block.pos.iter_mut().zip(offset.iter()) {
                    *coordinate += delta;
                }
                block
            }));
            chests.extend(part.chests.into_iter().map(|mut chest| {
                for (coordinate, delta) in chest.pos.iter_mut().zip(offset.iter()) {
                    *coordinate += delta;
                }
                chest
            }));
            villager_markers.extend(part.villager_markers.into_iter().map(|mut marker| {
                for (coordinate, delta) in marker.iter_mut().zip(offset.iter()) {
                    *coordinate += delta;
                }
                marker
            }));
        }
        Self {
            size,
            blocks,
            chests,
            villager_markers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureInhabitant {
    pub id: String,
    pub entity_type: String,
    pub villager_kind: String,
    pub profession: String,
    pub level: u8,
}

#[derive(Debug, Clone)]
pub struct StructureRules {
    templates: Vec<StructureTemplate>,
    grid_chunks: i32,
    separation_chunks: i32,
    salt: u64,
    fixed_center: Option<(i32, i32)>,
    inhabitants: Vec<StructureInhabitant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainsVillagePrototypePart {
    Fountain,
    SmallHouse,
    Toolsmith,
}

impl PlainsVillagePrototypePart {
    const fn source(self) -> (&'static str, [i32; 3]) {
        match self {
            Self::Fountain => (
                "village/plains/town_centers/plains_fountain_01.nbt",
                [0, 0, 0],
            ),
            Self::SmallHouse => ("village/plains/houses/plains_small_house_1.nbt", [12, 0, 0]),
            Self::Toolsmith => ("village/plains/houses/plains_tool_smith_1.nbt", [0, 0, 12]),
        }
    }
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
            inhabitants: Vec::new(),
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
            inhabitants: Vec::new(),
        }
    }

    /// One bounded village prototype assembled from unmodified vanilla NBT
    /// templates and vanilla village spacing data.
    pub fn plains_village_prototype(
        vanilla_data_dir: impl AsRef<Path>,
        blocks: &BlockRegistry,
    ) -> Result<Self, StructureError> {
        Self::plains_village_prototype_with_parts(
            vanilla_data_dir,
            blocks,
            &[
                PlainsVillagePrototypePart::Fountain,
                PlainsVillagePrototypePart::SmallHouse,
                PlainsVillagePrototypePart::Toolsmith,
            ],
        )
    }

    pub fn plains_village_prototype_with_parts(
        vanilla_data_dir: impl AsRef<Path>,
        blocks: &BlockRegistry,
        parts: &[PlainsVillagePrototypePart],
    ) -> Result<Self, StructureError> {
        let vanilla_data_dir = vanilla_data_dir.as_ref();
        let structure_root = vanilla_data_dir.join("data/minecraft/structure");
        let parts = parts
            .iter()
            .copied()
            .map(PlainsVillagePrototypePart::source)
            .map(|(path, offset)| {
                StructureTemplate::from_nbt_file(structure_root.join(path), blocks)
                    .map(|template| (template, offset))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let facts = load_structure_set_facts(vanilla_data_dir.join("data/minecraft/worldgen"))?;
        Ok(
            Self::plains_village_markers(vec![StructureTemplate::combine(parts)])
                .with_structure_set_facts(&facts),
        )
    }

    pub fn plains_village_prototype_with_plan(
        vanilla_data_dir: impl AsRef<Path>,
        blocks: &BlockRegistry,
        parts: &[PlainsVillagePrototypePart],
        inhabitants: Vec<StructureInhabitant>,
    ) -> Result<Self, StructureError> {
        let mut rules = Self::plains_village_prototype_with_parts(vanilla_data_dir, blocks, parts)?;
        let available = rules
            .templates
            .first()
            .map_or(0, |template| template.villager_markers.len());
        if inhabitants.len() > available {
            return Err(StructureError::MissingVillagerSlots {
                requested: inhabitants.len(),
                available,
            });
        }
        rules.inhabitants = inhabitants;
        Ok(rules)
    }

    #[must_use]
    pub fn with_fixed_center(mut self, center: (i32, i32)) -> Self {
        self.fixed_center = Some(center);
        self
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
            inhabitants: Vec::new(),
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

    #[must_use]
    pub fn inhabitants(&self) -> &[StructureInhabitant] {
        &self.inhabitants
    }

    #[cfg(test)]
    pub(crate) fn fixed_for_test(template: StructureTemplate, center: (i32, i32)) -> Self {
        Self {
            templates: vec![template],
            grid_chunks: 34,
            separation_chunks: 8,
            salt: 0,
            fixed_center: Some(center),
            inhabitants: Vec::new(),
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

#[derive(Debug, Clone, Copy)]
enum TemplatePaletteEntry {
    Block(BlockStateId),
    Jigsaw,
    Ignored,
}

fn parse_palette(
    path: &str,
    tag: &Tag,
    registry: &BlockRegistry,
) -> Result<Vec<TemplatePaletteEntry>, StructureError> {
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
) -> Result<TemplatePaletteEntry, StructureError> {
    let compound = expect_compound(path, entry, "palette[]")?;
    let name = expect_string(path, require(compound, path, "Name")?, "Name")?;
    let id = Identifier::parse(name.clone()).map_err(|_| StructureError::InvalidField {
        path: path.to_string(),
        field: "palette[].Name",
    })?;
    if id.as_str() == "minecraft:jigsaw" {
        return Ok(TemplatePaletteEntry::Jigsaw);
    }
    if matches!(id.as_str(), "minecraft:air" | "minecraft:structure_void") {
        return Ok(TemplatePaletteEntry::Ignored);
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
        .map(TemplatePaletteEntry::Block)
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
    palette: &[TemplatePaletteEntry],
) -> Result<(Vec<TemplateBlock>, Vec<[i32; 3]>), StructureError> {
    let list = expect_list(path, tag, "blocks")?;
    let mut blocks = Vec::new();
    let mut villager_markers = Vec::new();
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
        match state {
            TemplatePaletteEntry::Block(state) => {
                blocks.push(TemplateBlock { pos, state: *state });
            }
            TemplatePaletteEntry::Jigsaw if is_plains_villager_jigsaw(compound) => {
                villager_markers.push(pos);
            }
            TemplatePaletteEntry::Jigsaw | TemplatePaletteEntry::Ignored => {}
        }
    }
    Ok((blocks, villager_markers))
}

fn is_plains_villager_jigsaw(compound: &[(String, Tag)]) -> bool {
    compound
        .iter()
        .find(|(key, _)| key == "nbt")
        .and_then(|(_, tag)| match tag {
            Tag::Compound(fields) => Some(fields),
            _ => None,
        })
        .is_some_and(|fields| {
            fields.iter().any(|(key, value)| {
                key == "pool"
                    && matches!(
                        value,
                        Tag::String(pool) if pool == "minecraft:village/plains/villagers"
                    )
            })
        })
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
    fn loads_real_plains_village_prototype_when_present() {
        let vanilla = workspace_path("data/vanilla");
        let blocks_path = vanilla.join("reports/blocks.json");
        let fountain = vanilla
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
        if !blocks_path.exists() || !fountain.exists() {
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();

        let rules = StructureRules::plains_village_prototype(&vanilla, &registry).unwrap();

        assert_eq!(rules.templates().len(), 1);
        assert!(rules.templates()[0].size()[0] > 16);
        assert!(rules.templates()[0].size()[2] > 16);
        assert!(rules.templates()[0].blocks().len() > 200);
        assert_eq!(rules.templates()[0].villager_markers().len(), 4);
        assert_eq!(rules.grid_chunks(), 34);
        assert_eq!(rules.separation_chunks(), 8);
        assert_eq!(rules.salt(), 10_387_312);
    }

    #[test]
    fn plains_village_plan_selects_only_declared_building_parts_when_present() {
        let vanilla = workspace_path("data/vanilla");
        let blocks_path = vanilla.join("reports/blocks.json");
        let fountain = vanilla
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
        if !blocks_path.exists() || !fountain.exists() {
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();

        let rules = StructureRules::plains_village_prototype_with_parts(
            &vanilla,
            &registry,
            &[PlainsVillagePrototypePart::Fountain],
        )
        .unwrap();

        assert_eq!(rules.templates().len(), 1);
        assert_eq!(rules.templates()[0].size(), [9, 4, 9]);
        assert!(rules.templates()[0].blocks().len() > 100);
    }

    #[test]
    fn combined_templates_keep_stable_offsets() {
        let combined = StructureTemplate::combine(vec![
            (
                StructureTemplate::new(
                    [2, 1, 1],
                    vec![TemplateBlock {
                        pos: [1, 0, 0],
                        state: BlockStateId(1),
                    }],
                ),
                [0, 0, 0],
            ),
            (
                StructureTemplate::new(
                    [1, 2, 1],
                    vec![TemplateBlock {
                        pos: [0, 1, 0],
                        state: BlockStateId(2),
                    }],
                ),
                [4, 0, 3],
            ),
        ]);

        assert_eq!(combined.size(), [5, 2, 4]);
        assert_eq!(combined.blocks()[0].pos, [1, 0, 0]);
        assert_eq!(combined.blocks()[1].pos, [4, 1, 3]);
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
