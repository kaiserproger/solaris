//! Authored settlement blueprints: a closed TOML schema, registry-validated
//! palettes, and quarter-turn structure projection.
//!
//! The authored format is frozen by `settlement-blueprint-freeze.md`; this
//! module is the only decoder for it. Every hard limit fails closed: a rejected
//! file yields a typed [`BlueprintError`] and no catalog is produced.
//!
//! Rotation is applied through the block registry's property model
//! ([`rotate_block_state`]), never by blanket-replacing a block: a state that
//! does not resolve after a turn is rejected instead of being committed
//! half-applied.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Blueprints a deployed plugin may ship.
pub const MAX_BLUEPRINTS: usize = 128;
/// Ruined/expansion variants across the catalog.
pub const MAX_SETTLEMENT_VARIANTS: usize = 64;
/// Blocks in one blueprint, summed over the body and every stage.
pub const MAX_BLOCKS_PER_BLUEPRINT: usize = 65_536;
/// Largest allowed footprint extent on any axis.
pub const MAX_FOOTPRINT_AXIS: i32 = 64;
/// Total decoded catalog text (the authored TOML bytes).
pub const MAX_CATALOG_DECODED_BYTES: usize = 16 * 1024 * 1024;
/// Palette entries in one blueprint.
pub const MAX_PALETTE_ENTRIES: usize = 256;
/// Points of interest in one blueprint.
pub const MAX_POI_PER_BLUEPRINT: usize = 32;
/// Street connections in one blueprint.
pub const MAX_STREET_CONNECTIONS_PER_BLUEPRINT: usize = 32;
/// Construction stages in one blueprint (body and restoration counted apart).
pub const MAX_STAGES_PER_BLUEPRINT: usize = 32;
/// Initial block-entity seeds in one blueprint.
pub const MAX_BLOCK_ENTITIES_PER_BLUEPRINT: usize = 64;
/// Buildings a single settlement layout may place (exposed for layout callers).
pub const MAX_PLACEMENTS_PER_SETTLEMENT: usize = 128;

/// Everything that can be wrong with one authored blueprint.
#[derive(Debug, thiserror::Error)]
pub enum BlueprintError {
    #[error("blueprint {id}: unknown key {key}")]
    UnknownKey { id: String, key: String },
    #[error("blueprint {id}: id namespace is not owned by {owner}")]
    ForeignId { id: String, owner: String },
    #[error("blueprint id {id} is defined more than once")]
    DuplicateId { id: String },
    #[error("blueprint {id}: malformed id {raw:?}")]
    InvalidId { id: String, raw: String },
    #[error("blueprint {id}: invalid TOML: {reason}")]
    Parse { id: String, reason: String },
    #[error("blueprint {id}: unknown block {block}")]
    UnknownBlock { id: String, block: String },
    #[error("blueprint {id}: block {block} does not declare property {property}")]
    UnknownProperty {
        id: String,
        block: String,
        property: String,
    },
    #[error("blueprint {id}: block {block} leaves declared property {property} unset")]
    MissingProperty {
        id: String,
        block: String,
        property: String,
    },
    #[error("blueprint {id}: block {block} property {property}={value} is not declared")]
    OutOfRangeProperty {
        id: String,
        block: String,
        property: String,
        value: String,
    },
    #[error("blueprint {id}: block {block} has no state for the declared property values")]
    UnresolvedState { id: String, block: String },
    #[error("blueprint {id}: block {block} has no registered state after a {degrees} degree turn")]
    UnrotatableState {
        id: String,
        block: String,
        degrees: u16,
    },
    #[error("blueprint {id}: palette index {index} is defined more than once")]
    DuplicatePaletteIndex { id: String, index: u16 },
    #[error("blueprint {id}: has {count} blocks, limit is {max}")]
    TooManyBlocks {
        id: String,
        count: usize,
        max: usize,
    },
    #[error("blueprint {id}: footprint {size:?} is invalid or exceeds {max} per axis")]
    FootprintTooLarge {
        id: String,
        size: [i32; 3],
        max: i32,
    },
    #[error("blueprint {id}: anchor {anchor:?} is outside footprint {size:?}")]
    AnchorOutOfBounds {
        id: String,
        anchor: [i32; 3],
        size: [i32; 3],
    },
    #[error("blueprint {id}: cell {pos:?} is outside footprint {size:?}")]
    BlockOutOfBounds {
        id: String,
        pos: [i32; 3],
        size: [i32; 3],
    },
    #[error("blueprint {id}: palette index {index} is not defined")]
    UnknownPalette { id: String, index: u16 },
    #[error("blueprint {id}: cell {pos:?} is set more than once in {section}")]
    DuplicateCell {
        id: String,
        section: &'static str,
        pos: [i32; 3],
    },
    #[error("blueprint {id}: POI {poi} at {pos:?} is outside footprint {size:?}")]
    PoiOutOfBounds {
        id: String,
        poi: String,
        pos: [i32; 3],
        size: [i32; 3],
    },
    #[error("blueprint {id}: POI {poi} has unknown kind {kind}")]
    UnknownPoiKind {
        id: String,
        poi: String,
        kind: String,
    },
    #[error("blueprint {id}: POI id {poi} is defined more than once")]
    DuplicatePoi { id: String, poi: String },
    #[error("blueprint {id}: home POI {poi} has no physical entrance")]
    MissingEntrance { id: String, poi: String },
    #[error("blueprint {id}: street connection facing {facing} is not a cardinal direction")]
    InvalidFacing { id: String, facing: String },
    #[error("blueprint {id}: stage {stage} block {pos:?} is outside footprint {size:?}")]
    StageOutOfBounds {
        id: String,
        stage: String,
        pos: [i32; 3],
        size: [i32; 3],
    },
    #[error("blueprint {id}: stage {stage} is defined more than once")]
    DuplicateStage { id: String, stage: String },
    #[error("blueprint {id}: block entity at {pos:?} is outside footprint {size:?}")]
    BlockEntityOutOfBounds {
        id: String,
        pos: [i32; 3],
        size: [i32; 3],
    },
    #[error("blueprint {id}: block entity kind {kind} is not permitted")]
    UnknownBlockEntityKind { id: String, kind: String },
    #[error("blueprint {id}: has {count} {what}, limit is {max}")]
    TooManyEntries {
        id: String,
        what: &'static str,
        count: usize,
        max: usize,
    },
    #[error("blueprint {id}: content hash {actual} does not match expected {expected}")]
    HashMismatch {
        id: String,
        actual: String,
        expected: String,
    },
    #[error("blueprint {id}: variant base {base} is not in the catalog")]
    UnknownVariant { id: String, base: String },
    #[error(
        "blueprint {id}: variant footprint {size:?} differs from base {base} footprint {base_size:?}"
    )]
    VariantFootprintMismatch {
        id: String,
        base: String,
        size: [i32; 3],
        base_size: [i32; 3],
    },
    #[error("blueprint {id}: ruined variant must declare at least one restoration stage")]
    MissingRestorationStage { id: String },
    #[error("blueprint {id}: only a ruined variant may declare restoration stages")]
    UnexpectedRestorationStage { id: String },
    #[error("blueprint {id}: decoded size {bytes} exceeds limit {max}")]
    TooLarge {
        id: String,
        bytes: usize,
        max: usize,
    },
}

/// Everything that can be wrong with a catalog as a whole.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("blueprint file {file} is not a .toml file")]
    StrayFile { file: String },
    // Boxed so `Result` stays cheap to return on the hot path.
    #[error("blueprint file {file}: {source}")]
    Blueprint {
        file: String,
        #[source]
        source: Box<BlueprintError>,
    },
    #[error("catalog contains {count} blueprints, limit is {max}")]
    TooManyBlueprints { count: usize, max: usize },
    #[error("catalog contains {count} settlement variants, limit is {max}")]
    TooManyVariants { count: usize, max: usize },
    #[error("settlement places {count} buildings, limit is {max}")]
    TooManyPlacements { count: usize, max: usize },
    #[error("catalog decoded size {bytes} bytes exceeds limit {max}")]
    CatalogTooLarge { bytes: usize, max: usize },
    #[error("catalog is missing expected blueprint {id}")]
    MissingBlueprint { id: String },
    #[error("reading blueprint file {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
}

/// A quarter turn about the vertical axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarterTurn {
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl QuarterTurn {
    /// Quarter turn for a rotation in degrees; anything else is rejected.
    #[must_use]
    pub fn from_degrees(degrees: u16) -> Option<Self> {
        match degrees {
            0 => Some(Self::None),
            90 => Some(Self::Cw90),
            180 => Some(Self::Cw180),
            270 => Some(Self::Cw270),
            _ => None,
        }
    }

    #[must_use]
    pub fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// Quarter turns, `0..=3`.
    #[must_use]
    pub fn turns(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Cw90 => 1,
            Self::Cw180 => 2,
            Self::Cw270 => 3,
        }
    }

    /// Rotate local `(x, z)` about the footprint origin; `size` is the
    /// authored footprint. The rotated box has `x` and `z` extents swapped.
    #[must_use]
    pub fn rotate_offset(self, offset: [i32; 3], size: [i32; 3]) -> [i32; 3] {
        let [x, y, z] = offset;
        let [size_x, _, size_z] = size;
        match self {
            Self::None => [x, y, z],
            Self::Cw90 => [size_z - 1 - z, y, x],
            Self::Cw180 => [size_x - 1 - x, y, size_z - 1 - z],
            Self::Cw270 => [z, y, size_x - 1 - x],
        }
    }
}

/// The settlement roles a blueprint can serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoiKind {
    Home,
    Work,
    Meeting,
    Guard,
}

impl PoiKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Work => "work",
            Self::Meeting => "meeting",
            Self::Guard => "guard",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "home" => Some(Self::Home),
            "work" => Some(Self::Work),
            "meeting" => Some(Self::Meeting),
            "guard" => Some(Self::Guard),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Home => 0,
            Self::Work => 1,
            Self::Meeting => 2,
            Self::Guard => 3,
        }
    }
}

/// A cardinal direction on the horizontal plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinal {
    North,
    East,
    South,
    West,
}

impl Cardinal {
    #[must_use]
    pub fn rotate(self, turn: QuarterTurn) -> Self {
        let mut cardinal = self;
        for _ in 0..turn.turns() {
            cardinal = match cardinal {
                Self::North => Self::East,
                Self::East => Self::South,
                Self::South => Self::West,
                Self::West => Self::North,
            };
        }
        cardinal
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::East => "east",
            Self::South => "south",
            Self::West => "west",
        }
    }

    #[must_use]
    fn parse(s: &str) -> Option<Self> {
        match s {
            "north" => Some(Self::North),
            "east" => Some(Self::East),
            "south" => Some(Self::South),
            "west" => Some(Self::West),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::North => 0,
            Self::East => 1,
            Self::South => 2,
            Self::West => 3,
        }
    }
}

/// Explicit whitelist of permitted initial block-entity state. No arbitrary
/// NBT, no loot tables, no commands, no spawners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEntitySeedKind {
    EmptyContainer,
    Bed,
    Sign,
}

impl BlockEntitySeedKind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "empty_container" => Some(Self::EmptyContainer),
            "bed" => Some(Self::Bed),
            "sign" => Some(Self::Sign),
            _ => None,
        }
    }

    fn code(&self) -> u8 {
        match self {
            Self::EmptyContainer => 0,
            Self::Bed => 1,
            Self::Sign => 2,
        }
    }
}

/// One initial block-entity seed in blueprint-local coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntitySeed {
    pub at: [i32; 3],
    pub kind: BlockEntitySeedKind,
}

/// One palette entry: a registry block state with its canonical property list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub index: u16,
    pub block: Identifier,
    /// Canonical, in registry schema order.
    pub properties: Vec<(String, String)>,
    pub state: BlockStateId,
}

/// One body or stage cell, in blueprint-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub palette: u16,
}

/// A named point of interest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintPoi {
    pub id: String,
    pub kind: PoiKind,
    pub at: [i32; 3],
    pub capacity: u16,
}

/// A place the settlement road network may attach to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreetConnection {
    pub at: [i32; 3],
    pub facing: Cardinal,
}

/// One construction stage: an ordered batch of cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintStage {
    pub id: String,
    pub blocks: Vec<BlueprintBlock>,
}

/// Rotated state per quarter turn, keyed by palette index.
type PaletteTurns = BTreeMap<u16, [BlockStateId; 4]>;

/// A validated authored blueprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blueprint {
    id: String,
    revision: u32,
    variant_of: Option<String>,
    size: [i32; 3],
    anchor: [i32; 3],
    content_hash: String,
    palette: Vec<PaletteEntry>,
    palette_turns: PaletteTurns,
    blocks: Vec<BlueprintBlock>,
    pois: Vec<BlueprintPoi>,
    street_connections: Vec<StreetConnection>,
    stages: Vec<BlueprintStage>,
    restoration_stages: Vec<BlueprintStage>,
    block_entities: Vec<BlockEntitySeed>,
}

impl Blueprint {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn revision(&self) -> u32 {
        self.revision
    }

    #[must_use]
    pub fn variant_of(&self) -> Option<&str> {
        self.variant_of.as_deref()
    }

    #[must_use]
    pub fn size(&self) -> [i32; 3] {
        self.size
    }

    #[must_use]
    pub fn anchor(&self) -> [i32; 3] {
        self.anchor
    }

    /// `sha256` over the canonical content encoding, 64 lowercase hex digits.
    #[must_use]
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    #[must_use]
    pub fn palette(&self) -> &[PaletteEntry] {
        &self.palette
    }

    #[must_use]
    pub fn blocks(&self) -> &[BlueprintBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn pois(&self) -> &[BlueprintPoi] {
        &self.pois
    }

    #[must_use]
    pub fn street_connections(&self) -> &[StreetConnection] {
        &self.street_connections
    }

    #[must_use]
    pub fn stages(&self) -> &[BlueprintStage] {
        &self.stages
    }

    #[must_use]
    pub fn restoration_stages(&self) -> &[BlueprintStage] {
        &self.restoration_stages
    }

    #[must_use]
    pub fn block_entities(&self) -> &[BlockEntitySeed] {
        &self.block_entities
    }

    /// A ruined variant points at a base and carries restoration work.
    #[must_use]
    pub fn is_ruined_variant(&self) -> bool {
        self.variant_of.is_some() && !self.restoration_stages.is_empty()
    }

    fn rotated_state(&self, palette: u16, turn: QuarterTurn) -> BlockStateId {
        self.palette_turns
            .get(&palette)
            .expect("validated palettes cover every block index")[turn.turns() as usize]
    }
}

/// A blueprint placed into the world at a rotation and origin.
#[derive(Debug, Clone)]
pub struct BlueprintInstance {
    blueprint: Arc<Blueprint>,
    turn: QuarterTurn,
    origin: [i32; 3],
}

impl BlueprintInstance {
    #[must_use]
    pub fn new(blueprint: Arc<Blueprint>, turn: QuarterTurn, origin: [i32; 3]) -> Self {
        Self {
            blueprint,
            turn,
            origin,
        }
    }

    #[must_use]
    pub fn blueprint(&self) -> &Blueprint {
        &self.blueprint
    }

    #[must_use]
    pub fn turn(&self) -> QuarterTurn {
        self.turn
    }

    #[must_use]
    pub fn origin(&self) -> [i32; 3] {
        self.origin
    }

    /// World position of the blueprint's own anchor: the authored point the base
    /// row is built around, rotated with the instance. It coincides with one of
    /// the blueprint's street connections for most shipped buildings.
    #[must_use]
    pub fn placed_anchor(&self) -> [i32; 3] {
        self.place(self.blueprint.anchor())
    }

    fn place(&self, local: [i32; 3]) -> [i32; 3] {
        let rotated = self.turn.rotate_offset(local, self.blueprint.size());
        [
            self.origin[0] + rotated[0],
            self.origin[1] + rotated[1],
            self.origin[2] + rotated[2],
        ]
    }

    fn place_blocks(&self, blocks: &[BlueprintBlock]) -> Vec<PlacedBlock> {
        let mut placed: Vec<PlacedBlock> = blocks
            .iter()
            .map(|block| PlacedBlock {
                pos: self.place([block.x, block.y, block.z]),
                state: self.blueprint.rotated_state(block.palette, self.turn),
            })
            .collect();
        placed.sort_by_key(|block| block.pos);
        placed
    }

    #[must_use]
    pub fn placed_blocks(&self) -> Vec<PlacedBlock> {
        self.place_blocks(self.blueprint.blocks())
    }

    #[must_use]
    pub fn placed_stage_blocks(&self, stage: &BlueprintStage) -> Vec<PlacedBlock> {
        self.place_blocks(&stage.blocks)
    }

    #[must_use]
    pub fn placed_pois(&self) -> Vec<PlacedPoi> {
        let mut placed: Vec<PlacedPoi> = self
            .blueprint
            .pois()
            .iter()
            .map(|poi| PlacedPoi {
                id: poi.id.clone(),
                kind: poi.kind,
                at: self.place(poi.at),
                capacity: poi.capacity,
            })
            .collect();
        placed.sort_by(|left, right| left.at.cmp(&right.at).then_with(|| left.id.cmp(&right.id)));
        placed
    }

    #[must_use]
    pub fn placed_street_connections(&self) -> Vec<([i32; 3], Cardinal)> {
        let mut placed: Vec<([i32; 3], Cardinal)> = self
            .blueprint
            .street_connections()
            .iter()
            .map(|connection| {
                (
                    self.place(connection.at),
                    connection.facing.rotate(self.turn),
                )
            })
            .collect();
        placed.sort_by_key(|(at, facing)| (*at, facing.rank()));
        placed
    }

    #[must_use]
    pub fn placed_block_entities(&self) -> Vec<PlacedBlockEntity> {
        let mut placed: Vec<PlacedBlockEntity> = self
            .blueprint
            .block_entities()
            .iter()
            .map(|entity| PlacedBlockEntity {
                at: self.place(entity.at),
                kind: entity.kind.clone(),
            })
            .collect();
        placed.sort_by_key(|entity| (entity.at, entity.kind.code()));
        placed
    }

    /// `sha256` over the canonical projection: blueprint content hash, turn,
    /// origin, and the placed blocks and POIs.
    #[must_use]
    pub fn projection_hash(&self) -> String {
        let mut out = Vec::new();
        out.extend_from_slice(b"mc-worldgen.settlement.instance.v1");
        push_str(&mut out, self.blueprint.content_hash());
        out.extend_from_slice(&self.turn.degrees().to_le_bytes());
        push_i32s(&mut out, &self.origin);
        let blocks = self.placed_blocks();
        push_u32(&mut out, blocks.len());
        for block in &blocks {
            push_i32s(&mut out, &block.pos);
            out.extend_from_slice(&block.state.0.to_le_bytes());
        }
        let pois = self.placed_pois();
        push_u32(&mut out, pois.len());
        for poi in &pois {
            push_str(&mut out, &poi.id);
            out.push(poi.kind.code());
            push_i32s(&mut out, &poi.at);
            out.extend_from_slice(&poi.capacity.to_le_bytes());
        }
        hex(&Sha256::digest(&out))
    }
}

/// A block projected into world coordinates with its rotated state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedBlock {
    pub pos: [i32; 3],
    pub state: BlockStateId,
}

/// A POI projected into world coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedPoi {
    pub id: String,
    pub kind: PoiKind,
    pub at: [i32; 3],
    pub capacity: u16,
}

/// A block-entity seed projected into world coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedBlockEntity {
    pub at: [i32; 3],
    pub kind: BlockEntitySeedKind,
}

/// Every validated blueprint of one deployed plugin package.
#[derive(Debug, Default)]
pub struct BlueprintCatalog {
    blueprints: BTreeMap<String, Arc<Blueprint>>,
}

impl BlueprintCatalog {
    /// Decode and validate a package's authored blueprint files.
    ///
    /// `owner` is the package plugin id; every blueprint id must be
    /// `<owner>:<name>`. `files` are `(file_name, toml_text)` pairs.
    pub fn from_files(
        registry: &BlockRegistry,
        owner: &str,
        files: &[(String, String)],
    ) -> Result<Self, CatalogError> {
        Self::from_files_with_hashes(registry, owner, files, &BTreeMap::new())
    }

    /// As [`BlueprintCatalog::from_files`], also checking every blueprint
    /// against the content hash recorded by the deployed manifest.
    pub fn from_files_with_hashes(
        registry: &BlockRegistry,
        owner: &str,
        files: &[(String, String)],
        expected_hashes: &BTreeMap<String, String>,
    ) -> Result<Self, CatalogError> {
        if files.len() > MAX_BLUEPRINTS {
            return Err(CatalogError::TooManyBlueprints {
                count: files.len(),
                max: MAX_BLUEPRINTS,
            });
        }
        let decoded: usize = files.iter().map(|(_, text)| text.len()).sum();
        if decoded > MAX_CATALOG_DECODED_BYTES {
            return Err(CatalogError::CatalogTooLarge {
                bytes: decoded,
                max: MAX_CATALOG_DECODED_BYTES,
            });
        }

        let mut blueprints: BTreeMap<String, Arc<Blueprint>> = BTreeMap::new();
        let mut file_of: BTreeMap<String, String> = BTreeMap::new();
        for (file, text) in files {
            if !file.ends_with(".toml") {
                return Err(CatalogError::StrayFile { file: file.clone() });
            }
            let blueprint = parse_blueprint(registry, owner, file, text)?;
            if blueprints.contains_key(blueprint.id()) {
                return Err(blueprint_error(
                    file,
                    BlueprintError::DuplicateId {
                        id: blueprint.id().to_owned(),
                    },
                ));
            }
            file_of.insert(blueprint.id().to_owned(), file.clone());
            blueprints.insert(blueprint.id().to_owned(), Arc::new(blueprint));
        }

        let variants = blueprints
            .values()
            .filter(|blueprint| blueprint.variant_of().is_some())
            .count();
        if variants > MAX_SETTLEMENT_VARIANTS {
            return Err(CatalogError::TooManyVariants {
                count: variants,
                max: MAX_SETTLEMENT_VARIANTS,
            });
        }
        for variant in blueprints
            .values()
            .filter(|blueprint| blueprint.variant_of().is_some())
        {
            let base_id = variant
                .variant_of()
                .expect("filtered on variant_of being present");
            let base = blueprints
                .get(base_id)
                .filter(|base| base.id() != variant.id());
            let Some(base) = base else {
                return Err(blueprint_error(
                    file_of
                        .get(variant.id())
                        .expect("every parsed blueprint recorded its file"),
                    BlueprintError::UnknownVariant {
                        id: variant.id().to_owned(),
                        base: base_id.to_owned(),
                    },
                ));
            };
            if base.size() != variant.size() {
                return Err(blueprint_error(
                    file_of
                        .get(variant.id())
                        .expect("every parsed blueprint recorded its file"),
                    BlueprintError::VariantFootprintMismatch {
                        id: variant.id().to_owned(),
                        base: base_id.to_owned(),
                        size: variant.size(),
                        base_size: base.size(),
                    },
                ));
            }
        }

        for (id, expected) in expected_hashes {
            let Some(blueprint) = blueprints.get(id) else {
                return Err(CatalogError::MissingBlueprint { id: id.clone() });
            };
            if blueprint.content_hash() != expected {
                return Err(blueprint_error(
                    file_of
                        .get(id)
                        .expect("every parsed blueprint recorded its file"),
                    BlueprintError::HashMismatch {
                        id: id.clone(),
                        actual: blueprint.content_hash().to_owned(),
                        expected: expected.clone(),
                    },
                ));
            }
        }

        Ok(Self { blueprints })
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<Blueprint>> {
        self.blueprints.get(id)
    }

    pub fn blueprints(&self) -> impl Iterator<Item = &Arc<Blueprint>> {
        self.blueprints.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.blueprints.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blueprints.is_empty()
    }

    /// Ruined/expansion variants that point at `base`.
    #[must_use]
    pub fn variants_of(&self, base: &str) -> Vec<&Arc<Blueprint>> {
        self.blueprints
            .values()
            .filter(|blueprint| blueprint.variant_of() == Some(base))
            .collect()
    }
}

fn blueprint_error(file: &str, source: BlueprintError) -> CatalogError {
    CatalogError::Blueprint {
        file: file.to_owned(),
        source: Box::new(source),
    }
}

/// Rotate one registry state through a quarter turn.
///
/// Dependent properties move through the registry schema:
/// - `facing` cardinal values rotate (`up`/`down` stay);
/// - boolean `north`/`east`/`south`/`west` properties rotate as one group;
/// - `axis` swaps `x`/`z` on 90/270;
/// - `rotation` (0..15) advances by 4 per turn, mod 16;
/// - `orientation` rotates its cardinal token;
/// - `hinge`, `shape`, `half`, `part`, `open`, `waterlogged`, `powered`, and
///   unknown properties are preserved.
///
/// `None` when the rotated property combination is not a registered state.
#[must_use]
pub fn rotate_block_state(
    registry: &BlockRegistry,
    state: BlockStateId,
    turn: QuarterTurn,
) -> Option<BlockStateId> {
    let state = registry.by_id(state)?;
    if turn == QuarterTurn::None {
        return Some(state.id);
    }
    let block = Arc::clone(&state.block);
    let original = state.properties.clone();
    let mut properties = original.clone();
    for (key, value) in &mut properties {
        match key.as_str() {
            "facing" => {
                if let Some(cardinal) = Cardinal::parse(value) {
                    *value = cardinal.rotate(turn).as_str().to_owned();
                }
            }
            "north" | "east" | "south" | "west" => {
                let source = rotated_direction_key(key, turn);
                let source_value = original
                    .iter()
                    .find(|(name, _)| name == source)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or("false");
                *value = source_value.to_owned();
            }
            "axis" => {
                if turn != QuarterTurn::Cw180 {
                    match value.as_str() {
                        "x" => *value = "z".to_owned(),
                        "z" => *value = "x".to_owned(),
                        _ => {}
                    }
                }
            }
            "rotation" => {
                if let Ok(rotation) = value.parse::<u16>() {
                    *value = ((rotation + 4 * u16::from(turn.turns())) % 16).to_string();
                }
            }
            "orientation" => {
                *value = value
                    .split('_')
                    .map(|token| {
                        Cardinal::parse(token).map_or_else(
                            || token.to_owned(),
                            |cardinal| cardinal.rotate(turn).as_str().to_owned(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("_");
            }
            _ => {}
        }
    }
    registry.by_name_and_props(&block.id, &properties)
}

const DIRECTIONS: [&str; 4] = ["north", "east", "south", "west"];

fn rotated_direction_key(key: &str, turn: QuarterTurn) -> &'static str {
    let index = DIRECTIONS
        .iter()
        .position(|direction| *direction == key)
        .expect("caller matched a cardinal direction key");
    DIRECTIONS[(index + 4 - turn.turns() as usize) % 4]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlueprint {
    id: String,
    revision: u32,
    #[serde(default)]
    variant_of: Option<String>,
    footprint: RawFootprint,
    #[serde(default)]
    palette: Vec<RawPalette>,
    #[serde(default)]
    blocks: Vec<RawBlock>,
    #[serde(default)]
    poi: Vec<RawPoi>,
    #[serde(default)]
    street_connection: Vec<RawConnection>,
    #[serde(default)]
    stage: Vec<RawStage>,
    #[serde(default)]
    restoration_stage: Vec<RawStage>,
    #[serde(default)]
    block_entity: Vec<RawBlockEntity>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFootprint {
    size: [i32; 3],
    anchor: [i32; 3],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPalette {
    index: u16,
    block: String,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    x: i32,
    y: i32,
    z: i32,
    palette: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPoi {
    id: String,
    kind: String,
    at: [i32; 3],
    #[serde(default)]
    capacity: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConnection {
    at: [i32; 3],
    facing: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    id: String,
    #[serde(default)]
    blocks: Vec<RawBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlockEntity {
    at: [i32; 3],
    kind: String,
}

enum CellScan {
    OutOfBounds { pos: [i32; 3] },
    UnknownPalette { index: u16 },
    Duplicate { pos: [i32; 3] },
}

fn parse_blueprint(
    registry: &BlockRegistry,
    owner: &str,
    file: &str,
    text: &str,
) -> Result<Blueprint, CatalogError> {
    if text.len() > MAX_CATALOG_DECODED_BYTES {
        return Err(blueprint_error(
            file,
            BlueprintError::TooLarge {
                id: file.to_owned(),
                bytes: text.len(),
                max: MAX_CATALOG_DECODED_BYTES,
            },
        ));
    }
    let value: toml::Value = toml::from_str(text).map_err(|error| {
        blueprint_error(
            file,
            BlueprintError::Parse {
                id: file.to_owned(),
                reason: error.to_string(),
            },
        )
    })?;
    let raw_id = value
        .get("id")
        .and_then(toml::Value::as_str)
        .unwrap_or(file)
        .to_owned();
    check_closed_keys(&value, &raw_id).map_err(|error| blueprint_error(file, error))?;
    let raw: RawBlueprint = toml::from_str(text).map_err(|error| {
        blueprint_error(
            file,
            BlueprintError::Parse {
                id: raw_id.clone(),
                reason: error.to_string(),
            },
        )
    })?;
    build_blueprint(registry, owner, &raw).map_err(|error| blueprint_error(file, error))
}

fn check_closed_keys(value: &toml::Value, id: &str) -> Result<(), BlueprintError> {
    let Some(top) = value.as_table() else {
        return Ok(());
    };
    reject_unknown_keys(top, &TOP_LEVEL_KEYS, id)?;
    if let Some(footprint) = top.get("footprint").and_then(toml::Value::as_table) {
        reject_unknown_keys(footprint, &["size", "anchor"], id)?;
    }
    for entry in table_array(top, "palette") {
        reject_unknown_keys(entry, &["index", "block", "properties"], id)?;
    }
    for entry in table_array(top, "blocks") {
        reject_unknown_keys(entry, &BLOCK_KEYS, id)?;
    }
    for entry in table_array(top, "poi") {
        reject_unknown_keys(entry, &["id", "kind", "at", "capacity"], id)?;
    }
    for entry in table_array(top, "street_connection") {
        reject_unknown_keys(entry, &["at", "facing"], id)?;
    }
    for section in ["stage", "restoration_stage"] {
        for entry in table_array(top, section) {
            reject_unknown_keys(entry, &["id", "blocks"], id)?;
            for block in nested_array(entry, "blocks") {
                reject_unknown_keys(block, &BLOCK_KEYS, id)?;
            }
        }
    }
    for entry in table_array(top, "block_entity") {
        reject_unknown_keys(entry, &["at", "kind"], id)?;
    }
    Ok(())
}

const TOP_LEVEL_KEYS: [&str; 11] = [
    "id",
    "revision",
    "variant_of",
    "footprint",
    "palette",
    "blocks",
    "poi",
    "street_connection",
    "stage",
    "restoration_stage",
    "block_entity",
];

const BLOCK_KEYS: [&str; 4] = ["x", "y", "z", "palette"];

fn reject_unknown_keys(
    table: &toml::Table,
    allowed: &[&str],
    id: &str,
) -> Result<(), BlueprintError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(BlueprintError::UnknownKey {
                id: id.to_owned(),
                key: key.clone(),
            });
        }
    }
    Ok(())
}

fn table_array<'value>(table: &'value toml::Table, key: &str) -> Vec<&'value toml::Table> {
    nested_array(table, key)
}

fn nested_array<'value>(table: &'value toml::Table, key: &str) -> Vec<&'value toml::Table> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|items| items.iter().filter_map(toml::Value::as_table).collect())
        .unwrap_or_default()
}

fn build_blueprint(
    registry: &BlockRegistry,
    owner: &str,
    raw: &RawBlueprint,
) -> Result<Blueprint, BlueprintError> {
    let id = raw.id.clone();
    let identifier = Identifier::parse(raw.id.clone()).map_err(|_| BlueprintError::InvalidId {
        id: id.clone(),
        raw: raw.id.clone(),
    })?;
    if identifier.namespace() != owner {
        return Err(BlueprintError::ForeignId {
            id,
            owner: owner.to_owned(),
        });
    }

    let size = raw.footprint.size;
    if size
        .iter()
        .any(|axis| !(1..=MAX_FOOTPRINT_AXIS).contains(axis))
    {
        return Err(BlueprintError::FootprintTooLarge {
            id,
            size,
            max: MAX_FOOTPRINT_AXIS,
        });
    }
    let anchor = raw.footprint.anchor;
    if !within(anchor, size) {
        return Err(BlueprintError::AnchorOutOfBounds { id, anchor, size });
    }

    let total_blocks = raw.blocks.len()
        + raw
            .stage
            .iter()
            .map(|stage| stage.blocks.len())
            .sum::<usize>()
        + raw
            .restoration_stage
            .iter()
            .map(|stage| stage.blocks.len())
            .sum::<usize>();
    if total_blocks > MAX_BLOCKS_PER_BLUEPRINT {
        return Err(BlueprintError::TooManyBlocks {
            id,
            count: total_blocks,
            max: MAX_BLOCKS_PER_BLUEPRINT,
        });
    }
    check_length(
        &id,
        "palette entries",
        raw.palette.len(),
        MAX_PALETTE_ENTRIES,
    )?;
    check_length(
        &id,
        "points of interest",
        raw.poi.len(),
        MAX_POI_PER_BLUEPRINT,
    )?;
    check_length(
        &id,
        "street connections",
        raw.street_connection.len(),
        MAX_STREET_CONNECTIONS_PER_BLUEPRINT,
    )?;
    check_length(&id, "stages", raw.stage.len(), MAX_STAGES_PER_BLUEPRINT)?;
    check_length(
        &id,
        "restoration stages",
        raw.restoration_stage.len(),
        MAX_STAGES_PER_BLUEPRINT,
    )?;
    check_length(
        &id,
        "block entities",
        raw.block_entity.len(),
        MAX_BLOCK_ENTITIES_PER_BLUEPRINT,
    )?;

    let (palette, palette_turns) = build_palette(registry, &id, &raw.palette)?;
    let palette_indices: BTreeSet<u16> = palette.iter().map(|entry| entry.index).collect();

    let blocks =
        scan_cells(&raw.blocks, size, &palette_indices).map_err(|problem| match problem {
            CellScan::OutOfBounds { pos } => BlueprintError::BlockOutOfBounds {
                id: id.clone(),
                pos,
                size,
            },
            CellScan::UnknownPalette { index } => BlueprintError::UnknownPalette {
                id: id.clone(),
                index,
            },
            CellScan::Duplicate { pos } => BlueprintError::DuplicateCell {
                id: id.clone(),
                section: "blocks",
                pos,
            },
        })?;

    let pois = build_pois(&id, &raw.poi, size)?;
    let street_connections = build_connections(&id, &raw.street_connection, size)?;
    check_entrances(&id, &pois, &blocks, &palette, &street_connections)?;

    let stages = build_stages(&id, &raw.stage, size, &palette_indices)?;
    let restoration_stages = build_stages(&id, &raw.restoration_stage, size, &palette_indices)?;
    if raw.variant_of.is_none() && !restoration_stages.is_empty() {
        return Err(BlueprintError::UnexpectedRestorationStage { id });
    }
    if raw.variant_of.is_some() && restoration_stages.is_empty() {
        return Err(BlueprintError::MissingRestorationStage { id });
    }

    let block_entities = build_block_entities(&id, &raw.block_entity, size)?;

    let mut blueprint = Blueprint {
        id,
        revision: raw.revision,
        variant_of: raw.variant_of.clone(),
        size,
        anchor,
        content_hash: String::new(),
        palette,
        palette_turns,
        blocks,
        pois,
        street_connections,
        stages,
        restoration_stages,
        block_entities,
    };
    blueprint.content_hash = content_hash_of(&blueprint);
    Ok(blueprint)
}

fn check_length(
    id: &str,
    what: &'static str,
    count: usize,
    max: usize,
) -> Result<(), BlueprintError> {
    if count > max {
        return Err(BlueprintError::TooManyEntries {
            id: id.to_owned(),
            what,
            count,
            max,
        });
    }
    Ok(())
}

fn build_palette(
    registry: &BlockRegistry,
    id: &str,
    raw: &[RawPalette],
) -> Result<(Vec<PaletteEntry>, PaletteTurns), BlueprintError> {
    let mut entries = Vec::with_capacity(raw.len());
    let mut seen = BTreeSet::new();
    for entry in raw {
        if !seen.insert(entry.index) {
            return Err(BlueprintError::DuplicatePaletteIndex {
                id: id.to_owned(),
                index: entry.index,
            });
        }
        let block =
            Identifier::parse(entry.block.clone()).map_err(|_| BlueprintError::UnknownBlock {
                id: id.to_owned(),
                block: entry.block.clone(),
            })?;
        let definition = registry
            .block(&block)
            .ok_or_else(|| BlueprintError::UnknownBlock {
                id: id.to_owned(),
                block: entry.block.clone(),
            })?;
        for key in entry.properties.keys() {
            if !definition.properties.iter().any(|(name, _)| name == key) {
                return Err(BlueprintError::UnknownProperty {
                    id: id.to_owned(),
                    block: entry.block.clone(),
                    property: key.clone(),
                });
            }
        }
        let mut canonical = Vec::with_capacity(definition.properties.len());
        for (name, allowed) in &definition.properties {
            let Some(value) = entry.properties.get(name) else {
                return Err(BlueprintError::MissingProperty {
                    id: id.to_owned(),
                    block: entry.block.clone(),
                    property: name.clone(),
                });
            };
            if !allowed.iter().any(|declared| declared == value) {
                return Err(BlueprintError::OutOfRangeProperty {
                    id: id.to_owned(),
                    block: entry.block.clone(),
                    property: name.clone(),
                    value: value.clone(),
                });
            }
            canonical.push((name.clone(), value.clone()));
        }
        let state = registry
            .by_name_and_props(&block, &canonical)
            .ok_or_else(|| BlueprintError::UnresolvedState {
                id: id.to_owned(),
                block: entry.block.clone(),
            })?;
        let mut turns = [state; 4];
        for (position, turn) in [
            QuarterTurn::None,
            QuarterTurn::Cw90,
            QuarterTurn::Cw180,
            QuarterTurn::Cw270,
        ]
        .into_iter()
        .enumerate()
        {
            let Some(rotated) = rotate_block_state(registry, state, turn) else {
                return Err(BlueprintError::UnrotatableState {
                    id: id.to_owned(),
                    block: entry.block.clone(),
                    degrees: turn.degrees(),
                });
            };
            turns[position] = rotated;
        }
        entries.push((
            entry.index,
            PaletteEntry {
                index: entry.index,
                block,
                properties: canonical,
                state,
            },
            turns,
        ));
    }
    entries.sort_by_key(|(index, _, _)| *index);
    let mut palette = Vec::with_capacity(entries.len());
    let mut palette_turns = BTreeMap::new();
    for (index, entry, turns) in entries {
        palette.push(entry);
        palette_turns.insert(index, turns);
    }
    Ok((palette, palette_turns))
}

fn build_pois(
    id: &str,
    raw: &[RawPoi],
    size: [i32; 3],
) -> Result<Vec<BlueprintPoi>, BlueprintError> {
    let mut pois = Vec::with_capacity(raw.len());
    let mut seen = BTreeSet::new();
    for entry in raw {
        let kind = PoiKind::parse(&entry.kind).ok_or_else(|| BlueprintError::UnknownPoiKind {
            id: id.to_owned(),
            poi: entry.id.clone(),
            kind: entry.kind.clone(),
        })?;
        if !within(entry.at, size) {
            return Err(BlueprintError::PoiOutOfBounds {
                id: id.to_owned(),
                poi: entry.id.clone(),
                pos: entry.at,
                size,
            });
        }
        if !seen.insert(entry.id.clone()) {
            return Err(BlueprintError::DuplicatePoi {
                id: id.to_owned(),
                poi: entry.id.clone(),
            });
        }
        pois.push(BlueprintPoi {
            id: entry.id.clone(),
            kind,
            at: entry.at,
            capacity: entry.capacity,
        });
    }
    pois.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(pois)
}

fn build_connections(
    id: &str,
    raw: &[RawConnection],
    size: [i32; 3],
) -> Result<Vec<StreetConnection>, BlueprintError> {
    let mut connections = Vec::with_capacity(raw.len());
    for entry in raw {
        if !within(entry.at, size) {
            return Err(BlueprintError::BlockOutOfBounds {
                id: id.to_owned(),
                pos: entry.at,
                size,
            });
        }
        let facing =
            Cardinal::parse(&entry.facing).ok_or_else(|| BlueprintError::InvalidFacing {
                id: id.to_owned(),
                facing: entry.facing.clone(),
            })?;
        connections.push(StreetConnection {
            at: entry.at,
            facing,
        });
    }
    connections.sort_by_key(|connection| (connection.at, connection.facing.rank()));
    Ok(connections)
}

fn check_entrances(
    id: &str,
    pois: &[BlueprintPoi],
    blocks: &[BlueprintBlock],
    palette: &[PaletteEntry],
    connections: &[StreetConnection],
) -> Result<(), BlueprintError> {
    if connections.is_empty() {
        let doors: BTreeSet<u16> = palette
            .iter()
            .filter(|entry| entry.block.path().ends_with("_door"))
            .map(|entry| entry.index)
            .collect();
        if !blocks.iter().any(|block| doors.contains(&block.palette))
            && let Some(home) = pois.iter().find(|poi| poi.kind == PoiKind::Home)
        {
            return Err(BlueprintError::MissingEntrance {
                id: id.to_owned(),
                poi: home.id.clone(),
            });
        }
    }
    Ok(())
}

fn build_stages(
    id: &str,
    raw: &[RawStage],
    size: [i32; 3],
    palette_indices: &BTreeSet<u16>,
) -> Result<Vec<BlueprintStage>, BlueprintError> {
    let mut stages = Vec::with_capacity(raw.len());
    let mut seen = BTreeSet::new();
    for entry in raw {
        if !seen.insert(entry.id.clone()) {
            return Err(BlueprintError::DuplicateStage {
                id: id.to_owned(),
                stage: entry.id.clone(),
            });
        }
        let blocks =
            scan_cells(&entry.blocks, size, palette_indices).map_err(|problem| match problem {
                CellScan::OutOfBounds { pos } => BlueprintError::StageOutOfBounds {
                    id: id.to_owned(),
                    stage: entry.id.clone(),
                    pos,
                    size,
                },
                CellScan::UnknownPalette { index } => BlueprintError::UnknownPalette {
                    id: id.to_owned(),
                    index,
                },
                CellScan::Duplicate { pos } => BlueprintError::DuplicateCell {
                    id: id.to_owned(),
                    section: "stage",
                    pos,
                },
            })?;
        stages.push(BlueprintStage {
            id: entry.id.clone(),
            blocks,
        });
    }
    Ok(stages)
}

fn build_block_entities(
    id: &str,
    raw: &[RawBlockEntity],
    size: [i32; 3],
) -> Result<Vec<BlockEntitySeed>, BlueprintError> {
    let mut entities = Vec::with_capacity(raw.len());
    for entry in raw {
        if !within(entry.at, size) {
            return Err(BlueprintError::BlockEntityOutOfBounds {
                id: id.to_owned(),
                pos: entry.at,
                size,
            });
        }
        let kind = BlockEntitySeedKind::parse(&entry.kind).ok_or_else(|| {
            BlueprintError::UnknownBlockEntityKind {
                id: id.to_owned(),
                kind: entry.kind.clone(),
            }
        })?;
        entities.push(BlockEntitySeed { at: entry.at, kind });
    }
    entities.sort_by_key(|entity| (entity.at, entity.kind.code()));
    Ok(entities)
}

fn scan_cells(
    raw: &[RawBlock],
    size: [i32; 3],
    palette_indices: &BTreeSet<u16>,
) -> Result<Vec<BlueprintBlock>, CellScan> {
    let mut blocks = Vec::with_capacity(raw.len());
    let mut seen = BTreeSet::new();
    for entry in raw {
        let pos = [entry.x, entry.y, entry.z];
        if !within(pos, size) {
            return Err(CellScan::OutOfBounds { pos });
        }
        if !palette_indices.contains(&entry.palette) {
            return Err(CellScan::UnknownPalette {
                index: entry.palette,
            });
        }
        if !seen.insert(pos) {
            return Err(CellScan::Duplicate { pos });
        }
        blocks.push(BlueprintBlock {
            x: entry.x,
            y: entry.y,
            z: entry.z,
            palette: entry.palette,
        });
    }
    blocks.sort_by_key(|block| (block.x, block.y, block.z));
    Ok(blocks)
}

fn within(pos: [i32; 3], size: [i32; 3]) -> bool {
    (0..size[0]).contains(&pos[0])
        && (0..size[1]).contains(&pos[1])
        && (0..size[2]).contains(&pos[2])
}

/// Canonical content encoding, documented in the frozen contract:
/// `id, revision, variant_of, size, anchor, palette, blocks, pois, street
/// connections, stages, restoration stages, block entities`, with maps and
/// vectors in sorted order and the derived hash excluded.
fn content_hash_of(blueprint: &Blueprint) -> String {
    let mut out = Vec::new();
    out.extend_from_slice(b"mc-worldgen.settlement.blueprint.v1");
    push_str(&mut out, &blueprint.id);
    out.extend_from_slice(&blueprint.revision.to_le_bytes());
    match &blueprint.variant_of {
        Some(base) => {
            out.push(1);
            push_str(&mut out, base);
        }
        None => out.push(0),
    }
    push_i32s(&mut out, &blueprint.size);
    push_i32s(&mut out, &blueprint.anchor);
    push_u32(&mut out, blueprint.palette.len());
    for entry in &blueprint.palette {
        out.extend_from_slice(&entry.index.to_le_bytes());
        push_str(&mut out, entry.block.as_str());
        push_u32(&mut out, entry.properties.len());
        for (key, value) in &entry.properties {
            push_str(&mut out, key);
            push_str(&mut out, value);
        }
    }
    push_blocks(&mut out, &blueprint.blocks);
    push_u32(&mut out, blueprint.pois.len());
    for poi in &blueprint.pois {
        push_str(&mut out, &poi.id);
        out.push(poi.kind.code());
        push_i32s(&mut out, &poi.at);
        out.extend_from_slice(&poi.capacity.to_le_bytes());
    }
    push_u32(&mut out, blueprint.street_connections.len());
    for connection in &blueprint.street_connections {
        push_i32s(&mut out, &connection.at);
        out.push(connection.facing.rank());
    }
    push_stages(&mut out, &blueprint.stages);
    push_stages(&mut out, &blueprint.restoration_stages);
    push_u32(&mut out, blueprint.block_entities.len());
    for entity in &blueprint.block_entities {
        push_i32s(&mut out, &entity.at);
        out.push(entity.kind.code());
    }
    hex(&Sha256::digest(&out))
}

fn push_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u32).to_le_bytes());
}

fn push_i32s(out: &mut Vec<u8>, values: &[i32; 3]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn push_str(out: &mut Vec<u8>, value: &str) {
    push_u32(out, value.len());
    out.extend_from_slice(value.as_bytes());
}

fn push_blocks(out: &mut Vec<u8>, blocks: &[BlueprintBlock]) {
    push_u32(out, blocks.len());
    for block in blocks {
        push_i32s(out, &[block.x, block.y, block.z]);
        out.extend_from_slice(&block.palette.to_le_bytes());
    }
}

fn push_stages(out: &mut Vec<u8>, stages: &[BlueprintStage]) {
    push_u32(out, stages.len());
    for stage in stages {
        push_str(out, &stage.id);
        push_blocks(out, &stage.blocks);
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}
