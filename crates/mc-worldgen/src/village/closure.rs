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
    /// `start_height` resolved to an absolute block Y.
    ///
    /// Only `Constant`/`Absolute` is accepted ([`ClosureError::UnsupportedStartHeight`]):
    /// the other providers sample the growth random, and the solver has no
    /// height-provider sampler, so a structure carrying one must fail the load
    /// rather than place at Y 0 with a shifted stream.
    pub start_height: i32,
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
    /// `StructureTemplatePool.getMaxSize`: the tallest bounding box any
    /// non-`empty_pool_element` element of the pool produces at
    /// `Rotation.NONE`, or 0 for a pool of empty elements only.
    ///
    /// `use_expansion_hack` reads it for every jigsaw of a candidate whose
    /// front position lands inside the candidate's own box, so a village pool
    /// carrying only `feature_pool_element` entries reports 1 (a feature
    /// element is a degenerate one-block box) rather than 0.
    pub max_size: i32,
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

    /// The entity types the closure's pieces place that the village lane does
    /// not spawn, in ascending id order.
    ///
    /// The lane spawns a piece's `minecraft:villager` as a chunk inhabitant
    /// marker ([`SPAWNED_PIECE_MOB`]). Every other entity a village authors — the
    /// zombie villager of a zombie village's houses, the animal pens' livestock
    /// and their cats, the meeting point's iron golem, a desert camel, an armour
    /// stand — would need its own entity state and spawn path, so the lane leaves
    /// them unplaced and startup reports them by name.
    #[must_use]
    pub fn unspawned_piece_mobs(&self) -> Vec<&str> {
        let mut mobs = self
            .pieces
            .values()
            .flat_map(StructureTemplate::entities)
            .map(|entity| entity.entity_type.as_str())
            .filter(|entity_type| *entity_type != SPAWNED_PIECE_MOB)
            .collect::<Vec<_>>();
        mobs.sort_unstable();
        mobs.dedup();
        mobs
    }
}

/// The one entity type the village lane spawns: a piece template's villager
/// becomes a chunk inhabitant marker, which the runtime turns into a villager.
pub const SPAWNED_PIECE_MOB: &str = "minecraft:villager";

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
    #[error(
        "structure {structure} declares a start_height provider the engine does not sample: \\
         {provider}; only a constant absolute height is implemented"
    )]
    UnsupportedStartHeight {
        structure: Identifier,
        provider: String,
    },
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
        let spec = loader.load_structure(&entry.structure)?;
        let start_height = match &spec.start_height {
            mc_data::village_data::HeightProviderSpec::Constant(
                mc_data::village_data::VerticalAnchor::Absolute(value),
            ) => *value,
            other => {
                return Err(ClosureError::UnsupportedStartHeight {
                    structure: entry.structure.clone(),
                    provider: format!("{other:?}"),
                });
            }
        };
        structures.push(ClosureStructure {
            id: entry.structure.clone(),
            weight: u32::try_from(entry.weight).unwrap_or(1),
            spec,
            start_height,
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
        let max_size = max_size_of(&elements, &pieces);
        pools.insert(
            pool_id.clone(),
            ClosurePool {
                id: pool_id,
                fallback: spec.fallback,
                elements,
                max_size,
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

/// `StructureTemplatePool.getMaxSize`, over the elements the closure kept.
///
/// The pool's templates at `Rotation.NONE` produce the template's own height for
/// a piece element and a one-block box for a feature element
/// (`FeaturePoolElement.getBoundingBox` is `position..position`); empty elements
/// are filtered out before the maximum, and a pool with nothing else reports 0.
fn max_size_of(
    elements: &[(u32, ClosureElement)],
    pieces: &BTreeMap<Identifier, StructureTemplate>,
) -> i32 {
    let mut max = 0;
    for (_, element) in elements {
        let height = match element {
            ClosureElement::Empty => continue,
            ClosureElement::Feature { .. } => 1,
            ClosureElement::Single { piece, .. } => {
                pieces.get(piece).map_or(1, |template| template.size()[1])
            }
        };
        max = max.max(height);
    }
    max
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
