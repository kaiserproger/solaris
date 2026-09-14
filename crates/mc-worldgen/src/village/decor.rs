//! The village decor lane: the closure's `feature_pool_element` entries,
//! compiled and placed where the solver's feature pieces are written.
//!
//! ## What vanilla places, and when
//!
//! A feature element is a *piece* like any other: the solver places it (a
//! degenerate one-block box with a synthetic jigsaw pointing at the empty pool)
//! and `StructureStart.placeInChunk` calls `PoolElementStructurePiece.place` on
//! it when the chunk it lands in is generated, which is
//! `FeaturePoolElement.place` → `PlacedFeature.place(level, generator, random,
//! position)`. `position` is the piece's own position; `rotation` is ignored.
//!
//! The random is the `FEATURES` step's: `ChunkGenerator.applyBiomeDecoration`
//! builds one `WorldgenRandom` over `XoroshiroRandomSource` per chunk, derives
//! `decorationSeed = setDecorationSeed(worldSeed, chunkX * 16, chunkZ * 16)`
//! from it, and reseeds it per structure with
//! `setFeatureSeed(decorationSeed, index, step)` — the structure's index among
//! the registered structures of its generation step (`Registration` order is
//! identifier order) and that step's ordinal. All of one structure's starts that
//! a chunk references are placed with that one stream, in piece order, so a
//! chunk two villages of the same type reach shares it.
//!
//! ## What this module owns
//!
//! [`VillageDecor::compile`] is the startup step: it compiles every
//! `feature_pool_element` in the closure through the data-driven executor
//! ([`crate::vanilla_features::compile_pool_features`]) and resolves each
//! closure structure's `(index within its step, step ordinal)` from the content
//! cache. [`VillageDecor::place`] is the generation step: one compiled feature,
//! placed at the piece's position with the random the caller keeps for that
//! structure.
//!
//! Everything is data: the feature types, states, tokens and modifiers come from
//! the derived cache, and anything the executor's partial layer does not
//! implement fails the load by name rather than placing something else.

use std::collections::BTreeMap;
use std::collections::btree_map;
use std::path::Path;

use mc_data::Identifier;
use mc_data::village_data::{VillageDataError, VillageDataLoader};
use mc_world::BlockPos;
use thiserror::Error;

use crate::vanilla_features::{
    BlockSemantics, CompiledPlacedFeature, FeatureLevel, GlueError, PlaceError, RandomSource,
    WorldgenRandom, compile_pool_features,
};
use crate::village::closure::{ClosureElement, VillageClosure};

/// The compiled village decor and the seeding vanilla places it with.
pub struct VillageDecor {
    /// Compiled features by pool and placed-feature id. Features are compiled
    /// per pool, so the map is nested rather than keyed by a pair: a lookup must
    /// not have to allocate to ask.
    features: BTreeMap<Identifier, BTreeMap<Identifier, CompiledPlacedFeature>>,
    /// Per closure structure: its index among the registered structures of its
    /// generation step, and that step's ordinal.
    seeding: BTreeMap<Identifier, (i32, i32)>,
}

impl VillageDecor {
    /// A decor with nothing compiled: the state of a closure that reaches no
    /// `feature_pool_element`.
    ///
    /// A closure that *does* reach one fails
    /// [`crate::village::plan_source::VillagePlanSource::new`]'s validation
    /// rather than silently placing nothing, so this cannot hide a gap.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            features: BTreeMap::new(),
            seeding: BTreeMap::new(),
        }
    }

    /// Compile the closure's feature elements and resolve the seeding indices.
    ///
    /// `cache_root` is the directory holding `data/minecraft/**`, the same one
    /// [`crate::village::closure::load_village_closure`] walked.
    ///
    /// # Errors
    ///
    /// [`DecorError`] when a pool's feature closure cannot be resolved or
    /// compiled, when the structure directory cannot be read, or when a closure
    /// structure is missing from its own generation step's registry list.
    pub fn compile(
        cache_root: impl AsRef<Path>,
        closure: &VillageClosure,
        semantics: &BlockSemantics<'_>,
    ) -> Result<Self, DecorError> {
        let cache_root = cache_root.as_ref();
        let worldgen_dir = cache_root.join("data").join("minecraft").join("worldgen");
        let mut features: BTreeMap<Identifier, BTreeMap<Identifier, CompiledPlacedFeature>> =
            BTreeMap::new();
        for pool in closure.pools.values() {
            if !pool
                .elements
                .iter()
                .any(|(_, element)| matches!(element, ClosureElement::Feature { .. }))
            {
                continue;
            }
            let compiled = compile_pool_features(&worldgen_dir, &pool.id, semantics)?;
            let entry = features.entry(pool.id.clone()).or_default();
            for feature in compiled {
                entry.insert(feature.placed_feature, feature.feature);
            }
        }

        let loader = VillageDataLoader::new(&worldgen_dir);
        let mut registries: BTreeMap<i32, Vec<Identifier>> = BTreeMap::new();
        let mut seeding = BTreeMap::new();
        for structure in &closure.structures {
            let step = structure.spec.step;
            let ordinal = step.decoration_ordinal();
            if let btree_map::Entry::Vacant(entry) = registries.entry(ordinal) {
                entry.insert(loader.structures_in_step(step)?);
            }
            let registry = registries
                .get(&ordinal)
                .expect("the step's list was inserted above");
            let Some(index) = registry.iter().position(|id| id == &structure.id) else {
                return Err(DecorError::StructureNotRegistered {
                    structure: structure.id.clone(),
                });
            };
            let Ok(index) = i32::try_from(index) else {
                return Err(DecorError::StructureNotRegistered {
                    structure: structure.id.clone(),
                });
            };
            seeding.insert(structure.id.clone(), (index, ordinal));
        }
        Ok(Self { features, seeding })
    }

    /// The compiled feature a pool's `feature_pool_element` names, or `None`
    /// when the closure reaches no such element.
    #[must_use]
    pub fn feature(
        &self,
        pool: &Identifier,
        placed_feature: &Identifier,
    ) -> Option<&CompiledPlacedFeature> {
        self.features.get(pool)?.get(placed_feature)
    }

    /// The random one chunk places one structure's features on.
    ///
    /// The caller keeps the returned source for the whole chunk so that every
    /// feature of the same structure advances one stream, the way vanilla's
    /// single `WorldgenRandom` does.
    ///
    /// # Panics
    ///
    /// When `structure` is not a structure of the closure `VillageDecor` was
    /// compiled from; [`VillageDecor::compile`] resolved every closure structure.
    #[must_use]
    pub fn random_for(
        &self,
        seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        structure: &Identifier,
    ) -> WorldgenRandom {
        let (index, ordinal) = *self
            .seeding
            .get(structure)
            .expect("every closure structure is seeded at compile time");
        // `ChunkGenerator.applyBiomeDecoration`: one `WorldgenRandom` per chunk,
        // `setDecorationSeed` at the chunk's minimum block coordinates, then
        // `setFeatureSeed` per structure of the step. The random is a *wrapper*,
        // so both calls draw through `BitRandomSource`'s legacy composition over
        // the Xoroshiro bits — see [`WorldgenRandom`].
        let mut random = WorldgenRandom::over_xoroshiro(seed);
        let decoration_seed = random.set_decoration_seed(seed, chunk_x * 16, chunk_z * 16);
        random.set_feature_seed(decoration_seed, index, ordinal);
        random
    }

    /// Place one feature element: vanilla `FeaturePoolElement.place` →
    /// `PlacedFeature.place(level, generator, random, position)`.
    ///
    /// The feature must be one [`VillageDecor::compile`] compiled, which
    /// [`crate::village::plan_source::VillagePlanSource::new`] proves for every
    /// element the closure reaches before a world uses the village.
    ///
    /// # Errors
    ///
    /// The executor's [`PlaceError`] when a faithful execution reaches a
    /// boundary the layer refuses to cross silently.
    pub fn place(
        &self,
        pool: &Identifier,
        placed_feature: &Identifier,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        position: BlockPos,
    ) -> Result<bool, PlaceError> {
        let feature = self
            .feature(pool, placed_feature)
            .unwrap_or_else(|| panic!("feature {placed_feature} of pool {pool} was not compiled"));
        feature.place(level, semantics, random, position)
    }
}

#[derive(Debug, Error)]
pub enum DecorError {
    #[error(transparent)]
    Glue(#[from] GlueError),
    #[error(transparent)]
    Data(#[from] VillageDataError),
    #[error(
        "structure {structure} is not in its own generation step's registry list, so the \
         random vanilla places its features with cannot be seeded"
    )]
    StructureNotRegistered { structure: Identifier },
}
