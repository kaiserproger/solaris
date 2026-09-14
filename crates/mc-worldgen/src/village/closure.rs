//! The village closure: everything the village structure set reaches, loaded by
//! reference and closed against the two halves of the split.
//!
//! The JSON registries come from `mc-data::village_data` (structure set,
//! structures, template pools, processor lists, per id), the piece NBT comes
//! from [`crate::structures::StructureTemplate`] (this crate, because jigsaw
//! metadata is engine-side), and the walk between them is here: a pool names
//! pieces, a piece's jigsaw blocks name the pools they connect to, a pool names
//! its fallback, and every pool element names its processor list. Nothing
//! outside that closure is read, and `list_pool_element` — which vanilla places
//! by placing each of its children — is refused rather than silently flattened,
//! because the village pools do not reach it and a silent flatten would change
//! placement.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use mc_data::village_data::{
    PlacementSpec, PoolElementSpec, RandomSpreadType, StructureProcessorSpec, TemplatePoolSpec,
    VillageDataError, VillageDataLoader,
};
use mc_data::{Identifier, ResourcePath};
use mc_world::BlockRegistry;
use thiserror::Error;

use crate::structures::StructureTemplate;

/// A structure of the set, with its weighting and its resolved settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureStructure {
    pub id: Identifier,
    pub weight: u32,
    pub spec: mc_data::village_data::StructureSpec,
}

/// One pool element the engine can place.
#[derive(Debug, Clone, PartialEq)]
pub enum ClosureElement {
    Empty,
    Single {
        piece: Identifier,
        projection: mc_data::village_data::Projection,
        processors: mc_data::village_data::ProcessorRef,
        /// `legacy_single_pool_element` rather than `single_pool_element`:
        /// vanilla's two place settings differ (legacy also installs
        /// `BlockIgnoreProcessor.STRUCTURE_AND_AIR` last), so the kind is
        /// carried, never assumed.
        legacy: bool,
    },
    Feature {
        placed_feature: Identifier,
        projection: mc_data::village_data::Projection,
    },
}

/// A pool reduced to the element types the engine places.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosurePool {
    pub id: Identifier,
    pub fallback: Identifier,
    pub elements: Vec<(u32, ClosureElement)>,
}

/// Everything the village closure reached.
#[derive(Debug, Clone)]
pub struct VillageClosure {
    pub structure_set: Identifier,
    pub placement: crate::village::placement::RandomSpreadPlacement,
    pub structures: Vec<ClosureStructure>,
    pub pools: BTreeMap<Identifier, ClosurePool>,
    pub processor_lists: BTreeMap<Identifier, Vec<StructureProcessorSpec>>,
    pub pieces: BTreeMap<Identifier, StructureTemplate>,
    /// The distinct placed features the closure's `feature_pool_element` entries
    /// name — the A1/A2 executor's input.
    pub placed_features: Vec<Identifier>,
}

impl VillageClosure {
    /// Whether the closure reaches `pool`.
    #[must_use]
    pub fn pool(&self, pool: &Identifier) -> Option<&ClosurePool> {
        self.pools.get(pool)
    }

    /// Whether the closure reaches `piece`.
    #[must_use]
    pub fn piece(&self, piece: &Identifier) -> Option<&StructureTemplate> {
        self.pieces.get(piece)
    }
}

#[derive(Debug, Error)]
pub enum ClosureError {
    #[error(transparent)]
    Data(#[from] VillageDataError),
    #[error(transparent)]
    Reader(#[from] std::io::Error),
    #[error(transparent)]
    Template(#[from] crate::structures::StructureError),
    #[error(transparent)]
    ResourcePath(#[from] mc_data::ResourcePathError),
    #[error("missing structure piece {piece} referenced by {referrer} at {path}")]
    MissingPiece {
        piece: Identifier,
        referrer: Identifier,
        path: PathBuf,
    },
    #[error(
        "unsupported template pool element in {pool}: vanilla places a list element by placing \\
         each of its children, which the engine does not implement"
    )]
    UnsupportedListElement { pool: Identifier },
}

/// Load and walk the closure from a vanilla content cache root (the directory
/// holding `data/minecraft/**`).
pub fn load_village_closure(
    cache_root: impl AsRef<Path>,
    structure_set: &Identifier,
    registry: &BlockRegistry,
) -> Result<VillageClosure, ClosureError> {
    let cache_root = cache_root.as_ref();
    let loader = VillageDataLoader::new(cache_root.join("data").join("minecraft").join("worldgen"));
    let set = loader.load_structure_set(structure_set)?;
    let mut structures = Vec::with_capacity(set.structures.len());
    for entry in &set.structures {
        structures.push(ClosureStructure {
            id: entry.structure.clone(),
            weight: u32::try_from(entry.weight).unwrap_or(1),
            spec: loader.load_structure(&entry.structure)?,
        });
    }

    let mut pools: BTreeMap<Identifier, ClosurePool> = BTreeMap::new();
    let mut processor_lists: BTreeMap<Identifier, Vec<StructureProcessorSpec>> = BTreeMap::new();
    let mut pieces: BTreeMap<Identifier, StructureTemplate> = BTreeMap::new();
    let mut placed_features: Vec<Identifier> = Vec::new();
    let mut queue: VecDeque<(Identifier, Identifier)> = structures
        .iter()
        .map(|structure| (structure.spec.start_pool.clone(), structure.id.clone()))
        .collect();

    while let Some((pool_id, _referrer)) = queue.pop_front() {
        if pools.contains_key(&pool_id) {
            continue;
        }
        let spec: TemplatePoolSpec = loader.load_template_pool(&pool_id)?;
        let mut elements = Vec::with_capacity(spec.elements.len());
        for entry in &spec.elements {
            let element = match &entry.element {
                PoolElementSpec::Empty => ClosureElement::Empty,
                PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
                    let legacy = matches!(&entry.element, PoolElementSpec::LegacySingle(_));
                    load_processors(&loader, &single.processors, &mut processor_lists)?;
                    if !pieces.contains_key(&single.location) {
                        let template =
                            load_piece(cache_root, &single.location, &pool_id, registry)?;
                        for jigsaw in template.jigsaws() {
                            queue.push_back((jigsaw.pool.clone(), single.location.clone()));
                        }
                        pieces.insert(single.location.clone(), template);
                    }
                    ClosureElement::Single {
                        piece: single.location.clone(),
                        projection: single.projection,
                        processors: single.processors.clone(),
                        legacy,
                    }
                }
                PoolElementSpec::Feature {
                    feature,
                    projection,
                } => {
                    if !placed_features.contains(feature) {
                        placed_features.push(feature.clone());
                    }
                    ClosureElement::Feature {
                        placed_feature: feature.clone(),
                        projection: *projection,
                    }
                }
                PoolElementSpec::List { .. } => {
                    return Err(ClosureError::UnsupportedListElement {
                        pool: pool_id.clone(),
                    });
                }
            };
            elements.push((u32::try_from(entry.weight).unwrap_or(1), element));
        }
        if !pools.contains_key(&spec.fallback) {
            queue.push_back((spec.fallback.clone(), pool_id.clone()));
        }
        pools.insert(
            pool_id.clone(),
            ClosurePool {
                id: pool_id,
                fallback: spec.fallback,
                elements,
            },
        );
    }

    let mc_data::village_data::RandomSpreadPlacement {
        spacing,
        separation,
        salt,
        spread_type,
        ..
    } = match &set.placement {
        PlacementSpec::RandomSpread(placement) => placement.clone(),
    };
    placed_features.sort();
    Ok(VillageClosure {
        structure_set: structure_set.clone(),
        placement: crate::village::placement::RandomSpreadPlacement {
            spacing,
            separation,
            salt: i64::from(salt),
            triangular: spread_type == RandomSpreadType::Triangular,
        },
        structures,
        pools,
        processor_lists,
        pieces,
        placed_features,
    })
}

fn load_processors(
    loader: &VillageDataLoader,
    processors: &mc_data::village_data::ProcessorRef,
    out: &mut BTreeMap<Identifier, Vec<StructureProcessorSpec>>,
) -> Result<(), ClosureError> {
    match processors {
        mc_data::village_data::ProcessorRef::List(list) => {
            if !out.contains_key(list) {
                let spec = loader.load_processor_list(list)?;
                out.insert(list.clone(), spec.processors);
            }
        }
        // Inline lists are per element and already carried on the element, so
        // only named lists enter the closure's list map.
        mc_data::village_data::ProcessorRef::Inline(_) => {}
    }
    Ok(())
}

fn load_piece(
    cache_root: &Path,
    piece: &Identifier,
    referrer: &Identifier,
    registry: &BlockRegistry,
) -> Result<StructureTemplate, ClosureError> {
    let root = cache_root.join("data").join("minecraft").join("structure");
    let mut resource = ResourcePath::from_identifier_path(piece)?;
    resource.set_extension("nbt")?;
    let path = resource.lexical_under(&root);
    if !path.is_file() {
        return Err(ClosureError::MissingPiece {
            piece: piece.clone(),
            referrer: referrer.clone(),
            path,
        });
    }
    Ok(StructureTemplate::from_nbt_file(&path, registry)?)
}
