//! The one village-plan lookup the terrain pipeline consults, and the writer
//! that turns plans into chunk contents.
//!
//! [`assemble_village`] (in [`super::solver`]) decides a village; this module is
//! the adapter that turns one plan into the things the terrain pipeline needs
//! from it:
//!
//! - the columns [`crate::village::beard::apply_columns`] moves, which the
//!   generator reads at its surface decision
//!   ([`crate::terrain::TerrainGenerator::surface_height`]), and
//! - the contents [`VillagePlanSource::write_plans`] writes at the structure
//!   step: piece blocks through [`crate::village::piece::place_piece`], and
//!   `feature_pool_element` features through [`super::decor`].
//!
//! ## One lookup for both consumers
//!
//! There is one plan computation and no second source of truth. For the chunk
//! being filled the generator asks [`VillagePlanSource::plans_for_chunk`]; the
//! [`VillagePlanSet`] it returns holds the plans the chunk's piece blocks are
//! placed from and the plans its beard columns are read from — the same objects,
//! not re-derived copies. The lookup assembles on demand by enumerating the
//! candidate structure-start chunks within [`NEIGHBOURHOOD_CHUNK_RADIUS`] of the
//! chunk and keeping every one whose affected box overlaps it, and it keeps the
//! assemblies it has already run in a bounded memo keyed by start chunk
//! ([`AssemblyCache`]), because the work is per start chunk while the questions
//! arrive per column: a column query, an ore halo and a chunk's own fill all
//! keep asking about the same candidates. A miss runs the solver and stores the
//! result; evicting is never observable, because an assembly is a pure function
//! of the world seed and its start chunk.
//!
//! A chunk can need more than one village. Vanilla's `random_spread` grid puts
//! candidates at `grid * spacing + spread` with `spread` in
//! `0..spacing - separation`, so two neighbouring cells' candidates can be
//! `spacing - (spacing - separation - 1) = separation + 1 = 9` chunks apart
//! while a plan reaches up to [`NEIGHBOURHOOD_CHUNK_RADIUS`] chunks past its
//! start; a chunk in that gap holds part of both villages. The set therefore
//! keeps every overlapping plan, in ascending start-chunk order, and the beard
//! analogue sums their contributions the way vanilla's `Beardifier` sums every
//! structure the chunk references before the terrain is decided.
//!
//! ## Why the neighbourhood is bounded
//!
//! The region a plan reaches is every placed piece's box unioned with the
//! pieces' and junctions' boxes inflated by the beard kernel radius, and a
//! village grows up to the structure's `max_distance_from_center` (80 blocks
//! for the village structures) from the start piece, whose own box is at most
//! 16 blocks wide. A piece block or a moved column can therefore only come from
//! a plan started within `ceil((80 + 16 + 12) / 16) = 7` chunks of it, and
//! [`NEIGHBOURHOOD_CHUNK_RADIUS`] keeps a chunk of margin on that bound. The
//! live proof pins the bound against real plans.
//!
//! ## What a plan carries
//!
//! [`VillagePlan`] carries the solver's [`VillageAssembly`] (pieces, junctions
//! and the RIGID beard contributions), the junctions in
//! [`BeardJunction`] form, the affected box, and one [`PlanPiece`] per placed
//! piece: the element (`single_pool_element` /
//! `legacy_single_pool_element`, or a `feature_pool_element`), the element's
//! processor list, the projection, rotation, position, reference position and
//! box `write_plans` needs. Nothing here re-derives placement — the solver owns
//! that — and nothing here applies terrain: [`VillagePlan::adjusted_surface_y`]
//! delegates to [`apply_columns`].
//!
//! ## What a chunk write does
//!
//! `StructureStart.placeInChunk` places the pieces whose box intersects the
//! chunk's writable area, in piece order, with one random per structure, so
//! [`VillagePlanSource::write_plans`] does the same: a piece whose box misses
//! the chunk is skipped (it would write nothing and, for a feature, must not
//! draw), each plan's pieces are written through one [`ChunkPieceWriter`], and a
//! feature element runs on the stream [`VillageDecor::random_for`] seeds per
//! (chunk, structure) — shared by every plan of that structure reaching the
//! chunk, exactly as vanilla shares one `WorldgenRandom` across the starts it
//! places — and a chest's `LootTableSeed` is drawn from that same stream as the
//! piece lane writes it, the way `StructureTemplate.placeInWorld` does.
//!
//! ## Terrain adaptation
//!
//! `terrain_adaptation = beard_thin` reaches the pipeline as the column-height
//! analogue in [`crate::village::beard`], never as vanilla's density
//! arithmetic; that module owns the divergence and the upgrade path. The
//! closure this source is built from is the village structure set, whose
//! structures all carry `beard_thin`, so the analogue is applied to the whole
//! plan rather than gated per structure; the solver's RIGID pieces are the
//! contributions it reads.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;

use mc_data::Identifier;
use mc_data::village_data::{HeightmapType, ProcessorRef, Projection, StructureProcessorSpec};
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, Chunk, GeneratedVillagePiece, GeneratedVillageSite,
    SettlementInhabitantMarker,
};

use crate::structures::{StructureLoot, TemplateChest};
use crate::vanilla_features::{
    BiomeTagIndex, BlockSemantics, BlockTagIndex, FeatureLevel, LegacyRandom, WorldgenRandom,
};
use crate::village::beard::{
    BeardColumn, BeardJunction, BeardSource, affected_box, apply_columns_over,
};
use crate::village::closure::{ClosureElement, VillageClosure};
use crate::village::decor::VillageDecor;
use crate::village::piece::{
    BlockClip, Mirror, PieceError, PieceSettings, PieceWriter, PlacedEntity, place_piece,
};
use crate::village::processors::{PieceElement, ProcessLevel};
use crate::village::solver::{PlacedElement, Rotation, VillageAssembly, assemble_village};

/// How far, in chunks, a plan's affected box can reach from its start chunk.
///
/// The bound is `ceil((max_distance_from_center + piece width + kernel radius) /
/// 16)` over the village structures (`(80 + 16 + 12) / 16 = 6.75`), rounded up
/// and given a chunk of margin. A column outside this neighbourhood of a
/// structure start cannot be inside that plan's affected box.
pub const NEIGHBOURHOOD_CHUNK_RADIUS: i32 = 8;

/// Component-wise minimum of two corners.
fn min_of(a: BlockPos, b: BlockPos) -> BlockPos {
    BlockPos {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        z: a.z.min(b.z),
    }
}

/// Component-wise maximum of two corners.
fn max_of(a: BlockPos, b: BlockPos) -> BlockPos {
    BlockPos {
        x: a.x.max(b.x),
        y: a.y.max(b.y),
        z: a.z.max(b.z),
    }
}

/// One piece of a plan: a template to place, or a placed feature to run.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanElement {
    /// `single_pool_element` / `legacy_single_pool_element`.
    Single {
        /// The template's id in the closure.
        template: Identifier,
        /// The two element kinds install different block-ignore processors, so
        /// the kind is carried, never assumed.
        kind: PieceElement,
        /// The element's processor list, named lists resolved against the
        /// closure.
        processors: Vec<StructureProcessorSpec>,
        /// The template pool that referred this element: the entry a processor
        /// failure is reported against.
        owner: Identifier,
        projection: Projection,
    },
    /// `feature_pool_element`: the placed feature the solver placed as a
    /// terminal leaf, run through the executor where the piece is written.
    Feature {
        placed_feature: Identifier,
        owner: Identifier,
    },
}

/// One piece of a plan, with everything placement needs for it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanPiece {
    pub element: PlanElement,
    /// World position of the element's origin.
    pub position: BlockPos,
    /// The reference position the element's position predicates read
    /// (`StructureStart.placeInChunk`'s: the first piece's box centre at its
    /// own minimum Y).
    pub reference_pos: BlockPos,
    pub rotation: Rotation,
    pub depth: i32,
    /// The piece's box, as the solver decided it (`expandTo` included). A chunk
    /// places a piece only when this box intersects the chunk's writable area,
    /// the way `StructureStart.placeInChunk` filters pieces.
    pub bounds_min: BlockPos,
    pub bounds_max: BlockPos,
}

/// One assembled village, with the placement data its pieces are written from.
#[derive(Debug, Clone, PartialEq)]
pub struct VillagePlan {
    /// Shared with the memo that holds the start chunk's assembly
    /// ([`AssemblyCache`]) and with every plan assembled from it.
    assembly: Arc<VillageAssembly>,
    junctions: Vec<BeardJunction>,
    affected_min: BlockPos,
    affected_max: BlockPos,
    pieces: Vec<PlanPiece>,
}

impl VillagePlan {
    /// The solver's assembly: pieces, junctions and RIGID beard contributions.
    #[must_use]
    pub fn assembly(&self) -> &VillageAssembly {
        self.assembly.as_ref()
    }

    /// The assembly's junctions, as the beard analogue reads them.
    #[must_use]
    pub fn junctions(&self) -> &[BeardJunction] {
        &self.junctions
    }

    /// The placeable pieces, in placement order.
    #[must_use]
    pub fn pieces(&self) -> &[PlanPiece] {
        &self.pieces
    }

    /// The chunk the placement formula started this village in.
    #[must_use]
    pub fn start_chunk(&self) -> (i32, i32) {
        self.assembly.chunk
    }

    /// The region the plan reaches: every placed piece's box, unioned with the
    /// beard-filled boxes inflated by the beard kernel radius. A chunk the
    /// region misses cannot contain a piece block or a moved column.
    #[must_use]
    pub const fn affected_box(&self) -> (BlockPos, BlockPos) {
        (self.affected_min, self.affected_max)
    }

    /// Whether the affected box reaches into `(chunk_x, chunk_z)`.
    #[must_use]
    pub fn affects_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        let min_x = chunk_x * 16;
        let min_z = chunk_z * 16;
        self.affected_min.x < min_x + 16
            && self.affected_max.x >= min_x
            && self.affected_min.z < min_z + 16
            && self.affected_max.z >= min_z
    }

    /// Whether the affected box contains `(world_x, world_z)`.
    #[must_use]
    pub fn affects_column(&self, world_x: i32, world_z: i32) -> bool {
        (self.affected_min.x..=self.affected_max.x).contains(&world_x)
            && (self.affected_min.z..=self.affected_max.z).contains(&world_z)
    }

    /// The analogue's inputs: the RIGID pieces and the junctions of this plan.
    ///
    /// A column's offset is the sum over every plan that reaches it
    /// ([`VillagePlanSet::adjusted_surface_y`]); a single plan's own offset is
    /// this source passed to [`crate::village::beard::apply_columns_over`].
    #[must_use]
    pub fn beard_source(&self) -> BeardSource<'_> {
        (&self.assembly.beard_pieces, &self.junctions)
    }

    /// This plan as the generated village a settlement owner adopts: the start
    /// chunk, the region the plan reaches, and one entry per placed piece.
    ///
    /// The gate's biome tag and the stub position stay in the plan; a consumer
    /// that adopts a village asks about the village, not about the structure the
    /// gate resolved.
    #[must_use]
    pub fn site(&self) -> GeneratedVillageSite {
        GeneratedVillageSite {
            start_chunk: self.start_chunk(),
            min: self.affected_min,
            max: self.affected_max,
            pieces: self
                .pieces
                .iter()
                .map(|piece| GeneratedVillagePiece {
                    template: match &piece.element {
                        PlanElement::Single { template, .. } => Some(template.to_string()),
                        PlanElement::Feature { .. } => None,
                    },
                    position: piece.position,
                    rotation: piece.rotation.quarter_turns(),
                })
                .collect(),
        }
    }
}

/// Every village plan whose affected box reaches one chunk, in ascending
/// start-chunk order.
///
/// Vanilla's `Beardifier` sums the contributions of all structures a chunk
/// references, so a chunk in the gap between two `random_spread` candidates
/// carries both villages: this set is that group, and both consumers (piece
/// blocks, beard columns) read it.
#[derive(Debug, Clone)]
pub struct VillagePlanSet {
    plans: Vec<Arc<VillagePlan>>,
    affected_min: BlockPos,
    affected_max: BlockPos,
}

impl VillagePlanSet {
    /// The plans, in ascending start-chunk order.
    #[must_use]
    pub fn plans(&self) -> &[Arc<VillagePlan>] {
        &self.plans
    }

    /// Whether no village reaches the chunk this set was looked up for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    /// The region the set reaches: the union of its plans' affected boxes.
    #[must_use]
    pub const fn affected_box(&self) -> (BlockPos, BlockPos) {
        (self.affected_min, self.affected_max)
    }

    /// Whether the region reaches into `(chunk_x, chunk_z)`.
    #[must_use]
    pub fn affects_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        let min_x = chunk_x * 16;
        let min_z = chunk_z * 16;
        self.affected_min.x < min_x + 16
            && self.affected_max.x >= min_x
            && self.affected_min.z < min_z + 16
            && self.affected_max.z >= min_z
    }

    /// Whether the region contains `(world_x, world_z)`.
    #[must_use]
    pub fn affects_column(&self, world_x: i32, world_z: i32) -> bool {
        (self.affected_min.x..=self.affected_max.x).contains(&world_x)
            && (self.affected_min.z..=self.affected_max.z).contains(&world_z)
    }

    /// The plans that reach one column, in the set's order.
    #[must_use]
    pub fn plans_reaching(&self, world_x: i32, world_z: i32) -> Vec<&Arc<VillagePlan>> {
        self.plans
            .iter()
            .filter(|plan| plan.affects_column(world_x, world_z))
            .collect()
    }

    /// The analogue's offset for one column: `surface_y` moved toward the rigid
    /// pieces' `box.minY() + groundLevelDelta` of every plan reaching it, summed
    /// before rounding and clamped to `min_y + 1 ..= max_y - 1`.
    #[must_use]
    pub fn adjusted_surface_y(
        &self,
        world_x: i32,
        world_z: i32,
        surface_y: i32,
        min_y: i32,
        max_y: i32,
    ) -> i32 {
        let columns = [(world_x, world_z, surface_y)];
        match self.adjusted_columns(&columns, min_y, max_y).first() {
            Some(column) => column.adjusted_y,
            None => surface_y,
        }
    }

    /// [`crate::village::beard::apply_columns_over`] over every plan in the set,
    /// for callers holding a whole chunk's worth of surfaces.
    #[must_use]
    pub fn adjusted_columns(
        &self,
        columns: &[(i32, i32, i32)],
        min_y: i32,
        max_y: i32,
    ) -> Vec<BeardColumn> {
        let sources: Vec<BeardSource<'_>> =
            self.plans.iter().map(|plan| plan.beard_source()).collect();
        apply_columns_over(&sources, columns, min_y, max_y)
    }
}

/// The per-chunk plan lookup. Constructed once per world and handed to
/// [`crate::terrain::TerrainGenerator::with_village_plans`].
///
/// Data only: the loaded closure, the world seed, the block registry and tag
/// index the processors resolve against, and the biome tag contents the
/// structure gate reads. The biome at a column is not stored here — every lookup
/// takes it from the caller, so this source can never call back into the
/// generator that holds it.
pub struct VillagePlanSource {
    closure: Arc<VillageClosure>,
    decor: Arc<VillageDecor>,
    seed: i64,
    blocks: Arc<BlockRegistry>,
    block_tags: Arc<dyn BlockTagIndex + Send + Sync>,
    biome_tags: Arc<dyn BiomeTagIndex + Send + Sync>,
    assemblies: Mutex<AssemblyCache>,
}

/// How many start chunks' assemblies the memo keeps. Placement puts a village
/// in roughly one chunk in 230 (`random_spread` spacing 34, separation 8, five
/// structures), and a village's region spans a handful of chunks, so this holds
/// the villages a sweep of the world keeps asking about and bounds what a long
/// session can keep.
const ASSEMBLY_CACHE_CAPACITY: usize = 64;

/// The assemblies the solver has already run, keyed by start chunk.
///
/// The lookup is asked about one chunk at a time, but the work is per *start*
/// chunk: a column query re-derives its chunk's plan set, a chunk's plan set
/// scans every candidate of the placement grid in reach, and each candidate that
/// can start a village runs the whole solver — hundreds of candidate tests
/// against a growing free shape. Every caller that asks about more than one
/// column of the same neighbourhood (a mosaic pixel, a settlement layout row, an
/// ore halo, a chunk being filled) would otherwise run the same assembly again
/// for each of them.
///
/// An entry can be reused because an assembly is a pure function of the world
/// seed and the start chunk: the two world facts it reads — the free height and
/// the biome at a column — are [`VillagePlanSource`]'s caller's, and every
/// caller in this engine derives them from the world's seed. A caller that
/// answers differently for one position between two queries of the same world
/// would invalidate it, which is why they are production terrain functions, not
/// per-query state.
///
/// Only the start chunks that *do* hold a village are kept: a candidate the
/// placement formula rejects costs that formula and nothing else, so the memo
/// that would hold its `None` would be mostly empty answers — and, being
/// bounded, would evict the real assemblies it exists for.
///
/// Eviction is insertion-ordered and bounded: the memo is a cache, never a
/// source of truth, so evicting an entry costs only the assembly it holds.
struct AssemblyCache {
    order: VecDeque<(i32, i32)>,
    entries: HashMap<(i32, i32), Arc<VillageAssembly>>,
}

impl AssemblyCache {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, key: (i32, i32), assembly: &Arc<VillageAssembly>) {
        if self.entries.insert(key, assembly.clone()).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > ASSEMBLY_CACHE_CAPACITY {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }
}

impl VillagePlanSource {
    /// Build the source, proving first that this registry can carry the whole
    /// village ([`VillagePlanSource::validate`]).
    ///
    /// This is the only constructor, so holding a `VillagePlanSource` *is* the
    /// proof that every piece the closure reaches places under every rotation
    /// the solver can pick. Placement inside chunk generation cannot report a
    /// failure, so the check lives where it can: at startup, before a world uses
    /// the village.
    ///
    /// `blocks` is the registry the closure's templates were loaded against:
    /// processor execution resolves rule outputs and jigsaw final states
    /// through it. `block_tags` is the block tag index those processors test
    /// against. `biome_tags` is the worldgen biome tag contents the structure
    /// gate reads. `decor` is the closure's compiled `feature_pool_element`
    /// entries, which the piece walk places where the solver put them.
    ///
    /// # Errors
    ///
    /// The first [`PieceError`] the validation pass produces.
    pub fn new(
        closure: Arc<VillageClosure>,
        decor: Arc<VillageDecor>,
        seed: i64,
        blocks: Arc<BlockRegistry>,
        block_tags: Arc<dyn BlockTagIndex + Send + Sync>,
        biome_tags: Arc<dyn BiomeTagIndex + Send + Sync>,
    ) -> Result<Self, PieceError> {
        let source = Self {
            closure,
            decor,
            seed,
            blocks,
            block_tags,
            biome_tags,
            assemblies: Mutex::new(AssemblyCache::new()),
        };
        source.validate()?;
        Ok(source)
    }

    /// The closure every plan is assembled from.
    #[must_use]
    pub fn closure(&self) -> &VillageClosure {
        &self.closure
    }

    /// The compiled decor the plan source places feature elements through.
    #[must_use]
    pub fn decor(&self) -> &VillageDecor {
        &self.decor
    }

    /// The world seed the placement formula and the solver run on.
    #[must_use]
    pub const fn seed(&self) -> i64 {
        self.seed
    }

    /// Every plan whose affected box reaches into `(chunk_x, chunk_z)`, in
    /// ascending start-chunk order, or `None` when no village reaches it.
    ///
    /// `free_height` is the `WORLD_SURFACE_WG` first-free-height query at a
    /// column, the height the solver projects the start piece to. `biome_at` is
    /// the terrain's own plan-free biome at a column, which the structure gate
    /// resolves each structure's biome tag against.
    #[must_use]
    pub fn plans_for_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
    ) -> Option<VillagePlanSet> {
        let mut plans = Vec::new();
        for offset_z in -NEIGHBOURHOOD_CHUNK_RADIUS..=NEIGHBOURHOOD_CHUNK_RADIUS {
            for offset_x in -NEIGHBOURHOOD_CHUNK_RADIUS..=NEIGHBOURHOOD_CHUNK_RADIUS {
                let start_x = chunk_x + offset_x;
                let start_z = chunk_z + offset_z;
                let Some(plan) = self.plan_for_start_chunk(start_x, start_z, free_height, biome_at)
                else {
                    continue;
                };
                if plan.affects_chunk(chunk_x, chunk_z) {
                    plans.push(Arc::new(plan));
                }
            }
        }
        if plans.is_empty() {
            return None;
        }
        // Ascending start-chunk order, so the set is a function of the chunk and
        // not of the enumeration order.
        plans.sort_by_key(|plan| plan.start_chunk());
        let mut affected_min = plans[0].affected_min;
        let mut affected_max = plans[0].affected_max;
        for plan in &plans[1..] {
            affected_min = min_of(affected_min, plan.affected_min);
            affected_max = max_of(affected_max, plan.affected_max);
        }
        Some(VillagePlanSet {
            plans,
            affected_min,
            affected_max,
        })
    }

    /// Every village whose start chunk lies in the inclusive chunk rectangle
    /// `min_chunk..=max_chunk`, in ascending start-chunk order.
    ///
    /// One [`Self::plan_for_start_chunk`] per chunk in the rectangle: a chunk
    /// the `random_spread` grid does not name costs the placement formula and
    /// nothing else. The enumeration generates no chunk and reads no world
    /// state beyond the two column queries the caller supplies, so a settlement
    /// owner can list the villages a region will hold — and adopt one — before
    /// any of its chunks exist, exactly as the generator's own lookup agrees
    /// with it later.
    ///
    /// `free_height` and `biome_at` are the generator's plan-free queries, the
    /// same pair [`Self::plans_for_chunk`] takes; a caller that answers
    /// differently for one column than the world's terrain does would describe a
    /// village the world will not generate.
    ///
    /// The rectangle is the caller's bound: this walks every chunk in it, so a
    /// caller scans one bounded page of candidate cells rather than a world.
    #[must_use]
    pub fn sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
    ) -> Vec<GeneratedVillageSite> {
        if min_chunk.0 > max_chunk.0 || min_chunk.1 > max_chunk.1 {
            return Vec::new();
        }
        let mut sites = Vec::new();
        for chunk_z in min_chunk.1..=max_chunk.1 {
            for chunk_x in min_chunk.0..=max_chunk.0 {
                let Some(plan) = self.plan_for_start_chunk(chunk_x, chunk_z, free_height, biome_at)
                else {
                    continue;
                };
                sites.push(plan.site());
            }
        }
        sites
    }

    /// The plans whose affected boxes cover `(world_x, world_z)`, for callers
    /// that hold a column rather than a chunk (the generator's
    /// [`crate::terrain::TerrainGenerator::surface_height`]). The set is the
    /// chunk's own set, so a column and its chunk cannot disagree.
    #[must_use]
    pub fn plans_for_column(
        &self,
        world_x: i32,
        world_z: i32,
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
    ) -> Option<VillagePlanSet> {
        let set = self.plans_for_chunk(
            world_x.div_euclid(16),
            world_z.div_euclid(16),
            free_height,
            biome_at,
        )?;
        set.affects_column(world_x, world_z).then_some(set)
    }

    /// `ChunkGenerator.createStructures` + `JigsawStructure.findGenerationPoint`
    /// for one candidate start chunk: the placement formula decides whether the
    /// set can start there, the solver draws the set's entries (re-drawing when
    /// one fails its biome gate), and the biome gate itself is decided at the
    /// assembly's stub position.
    fn plan_for_start_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
    ) -> Option<VillagePlan> {
        let biome_matches =
            |tag: &Identifier, biome: &Identifier| self.biome_tags.biome_in_tag(tag, biome);
        let assembly = self.assembly_for(chunk_x, chunk_z, free_height, biome_at, biome_matches)?;
        self.plan_from(assembly)
    }

    /// The start chunk's assembly, from the memo when this source has already
    /// run it ([`AssemblyCache`]), or `None` when the chunk cannot start a
    /// village.
    ///
    /// A miss is solved outside the lock: two threads that miss the same start
    /// chunk both run the solver and store equal assemblies, which is the same
    /// work the memo removes for every other caller and never a wrong answer.
    fn assembly_for(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
        biome_matches: impl Fn(&Identifier, &Identifier) -> bool,
    ) -> Option<Arc<VillageAssembly>> {
        if let Some(hit) = self
            .assemblies
            .lock()
            .expect("the assembly memo is never held across a panic")
            .entries
            .get(&(chunk_x, chunk_z))
        {
            return Some(hit.clone());
        }
        let assembly = Arc::new(assemble_village(
            &self.closure,
            self.seed,
            chunk_x,
            chunk_z,
            biome_at,
            biome_matches,
            free_height,
        )?);
        self.assemblies
            .lock()
            .expect("the assembly memo is never held across a panic")
            .insert((chunk_x, chunk_z), &assembly);
        Some(assembly)
    }

    /// Turn the solver's assembly into the plan, resolving per piece the
    /// element data placement needs. `None` when the closure cannot resolve a
    /// placed piece: the closure is loaded by walking exactly the pools the
    /// solver draws from, so this is a defect rather than a runtime case.
    fn plan_from(&self, assembly: Arc<VillageAssembly>) -> Option<VillagePlan> {
        let junctions: Vec<BeardJunction> = assembly
            .junctions
            .iter()
            .map(|junction| BeardJunction {
                x: junction.source.x,
                ground_y: junction.ground_y,
                z: junction.source.z,
            })
            .collect();
        // `StructureStart.placeInChunk`'s reference position: the first
        // (start) piece's box centre at that box's own minimum Y.
        let reference_pos = assembly.pieces.first().map_or(assembly.origin, |piece| {
            let bounds = (piece.bounds_min, piece.bounds_max);
            BlockPos {
                x: bounds.0.x + (bounds.1.x - bounds.0.x + 1) / 2,
                y: bounds.0.y,
                z: bounds.0.z + (bounds.1.z - bounds.0.z + 1) / 2,
            }
        });
        let mut pieces = Vec::with_capacity(assembly.pieces.len());
        let mut bounds: Option<(BlockPos, BlockPos)> = None;
        for piece in &assembly.pieces {
            bounds = Some(match bounds {
                None => (piece.bounds_min, piece.bounds_max),
                Some((min, max)) => (min_of(min, piece.bounds_min), max_of(max, piece.bounds_max)),
            });
            let element = match &piece.element {
                PlacedElement::Single {
                    template,
                    owner,
                    processors,
                    legacy,
                    ..
                } => PlanElement::Single {
                    template: template.clone(),
                    kind: element_kind(*legacy),
                    processors: self.element_processors(processors, owner),
                    owner: owner.clone(),
                    projection: piece.projection,
                },
                PlacedElement::Feature {
                    placed_feature,
                    owner,
                    ..
                } => PlanElement::Feature {
                    placed_feature: placed_feature.clone(),
                    owner: owner.clone(),
                },
                PlacedElement::Empty => continue,
            };
            pieces.push(PlanPiece {
                element,
                position: piece.position,
                reference_pos,
                rotation: piece.rotation,
                depth: piece.depth,
                bounds_min: piece.bounds_min,
                bounds_max: piece.bounds_max,
            });
        }
        // The region the plan reaches: the pieces themselves, unioned with the
        // beard-filled boxes. The pieces are what decides whether a chunk
        // references the plan at all — a plan whose RIGID pieces are all far
        // from a terrain-matching piece still has to write that piece — and the
        // beard boxes extend the region by the kernel radius.
        if let Some((min, max)) = affected_box(&assembly.beard_pieces, &junctions) {
            bounds = Some(match bounds {
                None => (min, max),
                Some((current_min, current_max)) => {
                    (min_of(current_min, min), max_of(current_max, max))
                }
            });
        }
        let (affected_min, affected_max) = bounds?;
        Some(VillagePlan {
            assembly,
            junctions,
            affected_min,
            affected_max,
            pieces,
        })
    }

    /// The processor list an element declared, with a named list resolved
    /// against the closure.
    ///
    /// The solver carries the element's own `ProcessorRef`, so a template two
    /// pools place differently (the four terminators are placed by four biome
    /// pools with four different processor lists) is placed with the list of the
    /// element that actually placed it, not with the first list some other pool
    /// names for the same template. A named list that is *not* in the closure is
    /// a defect: [`VillagePlanSource::new`] proves every element's list resolves
    /// before a world uses the village, so this cannot be reached.
    fn element_processors(
        &self,
        processors: &ProcessorRef,
        owner: &Identifier,
    ) -> Vec<StructureProcessorSpec> {
        match processors {
            ProcessorRef::List(list) => self
                .closure
                .processor_lists
                .get(list)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("processor list {list} named by {owner} is not in the closure")
                }),
            ProcessorRef::Inline(specs) => specs.clone(),
        }
    }

    /// Place every piece the closure reaches, under every rotation the solver
    /// can pick and under both world states the reachable processors branch on,
    /// to prove the registry can carry the village before a world uses it; and
    /// prove that every non-piece element the closure reaches resolved.
    ///
    /// Placement rewrites block states (`rotate_state`, `mirror_state`) and the
    /// waterlogging decision reads the world, so a registry missing a rotated
    /// combination fails here, loudly, instead of silently dropping piece blocks
    /// inside chunk generation where nothing can report it. Nothing is written:
    /// the probe writer discards every block.
    ///
    /// The processor check is the same class of proof: a named list the closure
    /// did not load would place a piece with only the ignore/jigsaw passes, and
    /// a `feature_pool_element` [`VillageDecor`] did not compile would have
    /// nothing to run where the solver placed it.
    ///
    /// The two passes are the two `locState` inputs the reachable processor
    /// lists branch on — air everywhere, then water everywhere (the plains
    /// street list's water-conditional rules fire only in the second) — with a
    /// fixed height so the `terrain_matching` gravity processor runs too.
    ///
    /// # Errors
    ///
    /// The first [`PieceError`] any closure piece produces.
    pub fn validate(&self) -> Result<(), PieceError> {
        let semantics = BlockSemantics::new(self.blocks.as_ref(), self.block_tags.as_ref());
        let air = self
            .blocks
            .block(&Identifier::parse("minecraft:air").expect("a static id parses"))
            .map_or(BlockStateId(0), |block| block.default);
        let water = self
            .blocks
            .block(&Identifier::parse("minecraft:water").expect("a static id parses"))
            .map_or(air, |block| block.default);
        for pool in self.closure.pools.values() {
            for (_, element) in &pool.elements {
                match element {
                    ClosureElement::Empty => {}
                    ClosureElement::Feature { placed_feature, .. } => {
                        assert!(
                            self.decor.feature(&pool.id, placed_feature).is_some(),
                            "feature element {placed_feature} of pool {} was not compiled",
                            pool.id
                        );
                    }
                    ClosureElement::Single { processors, .. } => {
                        self.element_processors(processors, &pool.id);
                    }
                }
            }
        }
        // A throwaway source: validation reads no chest seed, and the pass must
        // not touch any caller's stream.
        let mut probe_random = LegacyRandom::new(0);
        for level_state in [air, water] {
            let mut writer = ProbePieceWriter { level_state };
            for pool in self.closure.pools.values() {
                for (_, element) in &pool.elements {
                    let ClosureElement::Single {
                        piece,
                        projection,
                        processors,
                        legacy,
                    } = element
                    else {
                        continue;
                    };
                    let Some(template) = self.closure.piece(piece) else {
                        continue;
                    };
                    for rotation in [
                        Rotation::None,
                        Rotation::Clockwise90,
                        Rotation::Clockwise180,
                        Rotation::CounterClockwise90,
                    ] {
                        let settings = PieceSettings {
                            position: BlockPos { x: 0, y: 0, z: 0 },
                            reference_pos: BlockPos { x: 0, y: 0, z: 0 },
                            rotation,
                            mirror: Mirror::None,
                            projection: *projection,
                            element: element_kind(*legacy),
                            processors: &self.element_processors(processors, &pool.id),
                            owner: &pool.id,
                            clip: None,
                        };
                        place_piece(
                            &semantics,
                            template,
                            &settings,
                            &mut writer,
                            Some(&mut probe_random),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Write every plan's blocks into `chunk`, clipped to the chunk's box:
    /// vanilla places a structure into the generation region box it was
    /// referenced by, so a piece spanning several chunks writes its own part
    /// into each chunk that asks for it.
    ///
    /// Pieces are written in plan order, and a `feature_pool_element` is placed
    /// where its piece falls, with the random vanilla's `FEATURES` step seeds
    /// per (chunk, structure): one stream per structure, shared by every plan
    /// of that structure reaching the chunk, exactly as
    /// `StructureStart.placeInChunk` shares one `WorldgenRandom` across the
    /// starts it places.
    ///
    /// `touched` records the columns that changed (for the caller's heightmap
    /// refresh), `surface` is the terrain's own surface (the
    /// `WORLD_SURFACE_WG` heightmap the `terrain_matching` projection's gravity
    /// processor reads), `air` is what a position outside the chunk reads as,
    /// and `loot` rolls the piece's chest contents.
    ///
    /// # Errors
    ///
    /// Returns [`PieceError`] when a processor list or a template state cannot
    /// be resolved, and propagates the executor's [`PlaceError`] for a decor
    /// feature. Both are registry/closure mismatches: the closure is loaded and
    /// validated against `blocks` at startup, so a correctly configured world
    /// does not reach them.
    pub fn write_plans(
        &self,
        chunk: &mut Chunk,
        plans: &VillagePlanSet,
        touched: &mut [bool; 256],
        surface: &dyn Fn(i32, i32) -> i32,
        air: BlockStateId,
        loot: &StructureLoot<'_>,
    ) -> Result<usize, PieceError> {
        let semantics = BlockSemantics::new(self.blocks.as_ref(), self.block_tags.as_ref());
        let geometry = chunk.geometry();
        let min_x = chunk.pos.x * 16;
        let min_z = chunk.pos.z * 16;
        // `ChunkGenerator.getWritableArea`: the chunk's columns, from one above
        // the world's bottom to its top. It both clips a piece's writes and
        // decides which pieces the chunk places at all
        // (`StructureStart.placeInChunk` skips a piece whose box does not
        // intersect it), and that second job is load-bearing for the decor lane:
        // a skipped feature must not draw from the structure's random.
        let writable = (
            BlockPos {
                x: min_x,
                y: geometry.min_y() + 1,
                z: min_z,
            },
            BlockPos {
                x: min_x + 15,
                y: geometry.max_y() - 1,
                z: min_z + 15,
            },
        );
        let clip = BlockClip::new(
            [writable.0.x, writable.0.y, writable.0.z],
            [writable.1.x, writable.1.y, writable.1.z],
        );
        let mut placed = 0;
        // The villagers the pieces put in this chunk. One list for the whole
        // chunk, written after every plan: `set_settlement_inhabitants` replaces
        // the chunk's list, so a per-piece call would drop the previous plans'.
        let mut inhabitants = Vec::new();
        // One random per structure, seeded the way `applyBiomeDecoration` seeds
        // the structure it is about to place: the stream rolls the `LootTableSeed`
        // of every container the structure's pieces write *and* runs the
        // structure's `feature_pool_element` features, in placement order.
        let mut structure_random: BTreeMap<Identifier, WorldgenRandom> = BTreeMap::new();
        let (chunk_x, chunk_z) = (chunk.pos.x, chunk.pos.z);
        for plan in plans.plans() {
            let structure = &plan.assembly().structure;
            // A plan that can draw nothing never touches the stream, so a source
            // whose decor carries no stream for this structure (a closure built
            // without the worldgen cache) can still place it.
            let mut random = plan_draws(self, plan).then(|| {
                structure_random
                    .entry(structure.clone())
                    .or_insert_with(|| {
                        self.decor
                            .random_for(self.seed, chunk_x, chunk_z, structure)
                    })
            });
            let mut writer = ChunkPieceWriter {
                chunk,
                touched,
                surface,
                loot,
                air,
                registry: self.blocks.as_ref(),
                inhabitants: &mut inhabitants,
            };
            for piece in plan.pieces() {
                if !intersects(piece.bounds_min, piece.bounds_max, writable) {
                    continue;
                }
                match &piece.element {
                    PlanElement::Single {
                        template,
                        kind,
                        processors,
                        owner,
                        projection,
                    } => {
                        let Some(template) = self.closure.piece(template) else {
                            continue;
                        };
                        let settings = PieceSettings {
                            position: piece.position,
                            reference_pos: piece.reference_pos,
                            rotation: piece.rotation,
                            // Village pieces are never mirrored: vanilla's
                            // jigsaw placement builds every
                            // `StructurePlaceSettings` with the default
                            // `Mirror.NONE`.
                            mirror: Mirror::None,
                            projection: *projection,
                            element: *kind,
                            processors,
                            owner,
                            clip: Some(clip),
                        };
                        placed += place_piece(
                            &semantics,
                            template,
                            &settings,
                            &mut writer,
                            random.as_deref_mut().map(|random| {
                                random as &mut dyn crate::vanilla_features::RandomSource
                            }),
                        )?;
                    }
                    PlanElement::Feature {
                        placed_feature,
                        owner,
                    } => {
                        let random = random
                            .as_deref_mut()
                            .expect("a feature piece draws from its structure's placement random");
                        let ran = self.decor.place(
                            owner,
                            placed_feature,
                            &mut writer,
                            &semantics,
                            random,
                            piece.position,
                        )?;
                        if ran {
                            placed += 1;
                        }
                    }
                }
            }
        }
        if !inhabitants.is_empty() {
            chunk.set_settlement_inhabitants(&inhabitants);
        }
        Ok(placed)
    }
}

/// Whether a plan's placement draws from its structure's placement random: a
/// `feature_pool_element` always does, and so does any template with a chest,
/// whose `LootTableSeed` [`crate::village::piece::place_piece`] draws.
fn plan_draws(source: &VillagePlanSource, plan: &VillagePlan) -> bool {
    plan.pieces().iter().any(|piece| match &piece.element {
        PlanElement::Feature { .. } => true,
        PlanElement::Single { template, .. } => source
            .closure
            .piece(template)
            .is_some_and(|template| !template.chests().is_empty()),
    })
}

/// `BoundingBox.intersects`: both boxes inclusive on every axis.
fn intersects(min: BlockPos, max: BlockPos, other: (BlockPos, BlockPos)) -> bool {
    max.x >= other.0.x
        && min.x <= other.1.x
        && max.z >= other.0.z
        && min.z <= other.1.z
        && max.y >= other.0.y
        && min.y <= other.1.y
}

/// `legacy_single_pool_element` rather than `single_pool_element`: the two place
/// settings differ (legacy also installs `BlockIgnoreProcessor.STRUCTURE_AND_AIR`
/// last), so the kind comes from the element and is never assumed.
const fn element_kind(legacy: bool) -> PieceElement {
    if legacy {
        PieceElement::LegacySingle
    } else {
        PieceElement::Single
    }
}

/// The writer [`VillagePlanSource::validate`] places through: it keeps nothing,
/// so validation cannot touch a world, and it answers every block read with the
/// pass's world state.
struct ProbePieceWriter {
    /// The `locState` every position reads as: air, or water.
    level_state: BlockStateId,
}

impl ProcessLevel for ProbePieceWriter {
    fn block_state(&self, _pos: BlockPos) -> BlockStateId {
        self.level_state
    }

    fn height(&self, _heightmap: HeightmapType, _x: i32, _z: i32) -> i32 {
        64
    }
}

impl PieceWriter for ProbePieceWriter {
    fn set_block(&mut self, _pos: BlockPos, _state: BlockStateId) {}

    fn set_chest(&mut self, _pos: BlockPos, _chest: &TemplateChest, _loot_seed: u64) {}
}

/// The chunk the plan's blocks are written into, as the level view
/// [`crate::village::piece`] places through.
///
/// Vanilla places structures through a `WorldGenRegion` holding the chunk being
/// filled and its already-generated neighbours. This generator fills one chunk
/// at a time and has no neighbour access, so the level view is that chunk:
/// block reads inside it answer from the chunk, reads outside answer `air`.
/// That is exactly the view `StructurePlaceSettings.getBoundingBox()` clips
/// writes to, so no write leaves the chunk.
struct ChunkPieceWriter<'a, 'b> {
    chunk: &'a mut Chunk,
    touched: &'a mut [bool; 256],
    surface: &'a dyn Fn(i32, i32) -> i32,
    loot: &'a StructureLoot<'b>,
    air: BlockStateId,
    /// The registry the decor lane resolves a block state against when it asks
    /// whether a position is a water source.
    registry: &'a BlockRegistry,
    /// The villagers the placed pieces put in this chunk, in placement order.
    ///
    /// They leave as the chunk's inhabitant markers: the core's one
    /// generation-to-runtime entity handoff, which `mc-net`'s chunk stream turns
    /// into spawned villagers.
    inhabitants: &'a mut Vec<SettlementInhabitantMarker>,
}

impl ChunkPieceWriter<'_, '_> {
    fn local(&self, pos: BlockPos) -> Option<(u8, u8)> {
        let lx = pos.x - self.chunk.pos.x * 16;
        let lz = pos.z - self.chunk.pos.z * 16;
        let lx = u8::try_from(lx).ok().filter(|lx| *lx < 16)?;
        let lz = u8::try_from(lz).ok().filter(|lz| *lz < 16)?;
        Some((lx, lz))
    }
}

impl ProcessLevel for ChunkPieceWriter<'_, '_> {
    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        self.local(pos)
            .and_then(|(lx, lz)| self.chunk.get_block(lx, pos.y, lz))
            .unwrap_or(self.air)
    }

    fn height(&self, _heightmap: HeightmapType, x: i32, z: i32) -> i32 {
        // Only `WORLD_SURFACE_WG` is reachable from the village closure (the
        // `terrain_matching` projection's gravity processor); every other
        // heightmap type reports the same noise surface rather than a heightmap
        // this generator has not built. Vanilla's `getHeight` is the first free
        // block, i.e. one above the surface block.
        (self.surface)(x, z).saturating_add(1)
    }
}

impl PieceWriter for ChunkPieceWriter<'_, '_> {
    fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        let Some((lx, lz)) = self.local(pos) else {
            return;
        };
        if self.chunk.set_block(lx, pos.y, lz, state).is_some() {
            self.touched[lz as usize * 16 + lx as usize] = true;
        }
    }

    fn set_chest(&mut self, pos: BlockPos, chest: &TemplateChest, loot_seed: u64) {
        let contents = chest.resolve_contents(self.loot, loot_seed);
        self.chunk.chests.insert(pos, contents);
    }

    /// The village lane's entity step, as the chunk's inhabitant markers.
    ///
    /// `VillagerData` and `Age` come from the template entity. The templates
    /// author no home, job site or meeting point, and this engine does not model
    /// vanilla's own POI acquisition (a villager claiming a bed, a workstation or
    /// the bell from the blocks around it), so the marker carries the core's
    /// existing answer for a villager with no authored POI: the entity's own
    /// placed position stands in for all three — it is the home it was placed
    /// into, a working profession gets its job site there, and every villager has
    /// somewhere to meet. That is the shape
    /// [`default_villager_pois`](mc_entity::villager_26_1_2::default_villager_pois)
    /// defines and the shape the settlement lane's markers also carry, filled
    /// from its plan.
    fn set_entity(&mut self, placed: &PlacedEntity<'_>) {
        let Some(villager) = placed.entity.villager.as_ref() else {
            return;
        };
        // Only the villager is spawnable here. A zombie villager or one of the
        // mobs a village authors (cats, the animal pens' livestock, the iron
        // golem, a camel, an armour stand) would have to become a different
        // entity with its own retained state; the closure reports what a village
        // authors so the gap is visible instead of silently missing.
        if placed.entity.entity_type != crate::village::closure::SPAWNED_PIECE_MOB {
            return;
        }
        self.inhabitants.push(SettlementInhabitantMarker {
            claim: placed.claim.clone(),
            entity_type: placed.entity.entity_type.clone(),
            position: placed.position,
            villager_kind: villager.kind.clone(),
            profession: villager.profession.clone(),
            level: villager.level,
            home: Some(placed.position),
            // `default_villager_pois` gives a job site to every profession but
            // `none`; the village templates' other profession is `nitwit`, which
            // works nowhere, so the working set is named rather than inferred
            // from "not none".
            job_site: is_working_profession(&villager.profession).then_some(placed.position),
            meeting_point: Some(placed.position),
            age: villager.age,
            yaw: placed.yaw,
            pitch: placed.pitch,
        });
    }
}

/// Whether a template entity's `VillagerData.profession` path is one that works:
/// `none` and `nitwit` have no job site, every other profession does.
fn is_working_profession(profession: &str) -> bool {
    !matches!(profession, "none" | "nitwit")
}

/// The same chunk view serves the decor lane: `minecraft`'s `WorldGenLevel`
/// subset a placed feature writes through.
///
/// One difference from the piece lane is structural, not chosen: vanilla places
/// a structure into the `WorldGenRegion` that holds the chunk being decorated
/// and its already-generated neighbours, so a decor feature can write into a
/// neighbouring chunk, while this generator fills one chunk at a time and has
/// no neighbour access. A feature write that lands outside the chunk is
/// therefore dropped ([`FeatureLevel::set_block`] answers `false` to the
/// caller's existence checks exactly as a write into unloaded ground would),
/// and the neighbour chunk does not place it later: the piece that carries the
/// feature belongs to this chunk alone.
impl FeatureLevel for ChunkPieceWriter<'_, '_> {
    fn min_y(&self) -> i32 {
        self.chunk.geometry().min_y()
    }

    fn max_y(&self) -> i32 {
        self.chunk.geometry().max_y()
    }

    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        ProcessLevel::block_state(self, pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        PieceWriter::set_block(self, pos, state);
    }

    fn is_water_source_at(&self, pos: BlockPos) -> bool {
        // `LevelReader.isFluidAtPosition(pos, water source)`: the block at `pos`
        // is water and its `level` property is 0.
        let state = ProcessLevel::block_state(self, pos);
        let Some(resolved) = self.registry.by_id(state) else {
            return false;
        };
        resolved.block.id.path() == "water"
            && !resolved
                .properties
                .iter()
                .any(|(name, value)| name == "level" && value != "0")
    }
}
