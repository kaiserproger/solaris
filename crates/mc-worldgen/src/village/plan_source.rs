//! The one village-plan lookup the terrain pipeline consults.
//!
//! [`assemble_village`] (in [`super::solver`]) decides a village; this module is
//! the adapter that turns one plan into the only two things the terrain
//! pipeline needs from it:
//!
//! - the columns [`crate::village::beard::apply_columns`] moves, which the
//!   generator reads at its surface decision
//!   ([`crate::terrain::TerrainGenerator::surface_height`]), and
//! - the piece blocks [`crate::village::piece::place_piece`] writes, which the
//!   generator writes at its structure step.
//!
//! ## One lookup for both consumers
//!
//! There is no cache and no second source of truth. For the chunk being filled
//! the generator asks [`VillagePlanSource::plans_for_chunk`] once; the
//! [`VillagePlanSet`] it returns holds the plans the chunk's piece blocks are
//! placed from and the plans its beard columns are read from — the same objects,
//! not re-derived copies. The lookup assembles on demand by enumerating the
//! candidate structure-start chunks within [`NEIGHBOURHOOD_CHUNK_RADIUS`] of the
//! chunk and keeping every one whose affected box overlaps it.
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
//! piece: the element kind (`single_pool_element` /
//! `legacy_single_pool_element`), the element's processor list, and the
//! projection, rotation, position and reference position `place_piece` needs.
//! Nothing here re-derives placement — the solver owns that — and nothing here
//! applies terrain: [`VillagePlan::adjusted_surface_y`] delegates to
//! [`apply_columns`].
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

use std::sync::Arc;

use mc_data::Identifier;
use mc_data::village_data::{HeightmapType, ProcessorRef, Projection, StructureProcessorSpec};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk};

use crate::structures::{StructureLoot, TemplateChest};
use crate::vanilla_features::{BiomeTagIndex, BlockSemantics, BlockTagIndex};
use crate::village::beard::{
    BeardColumn, BeardJunction, BeardSource, affected_box, apply_columns_over,
};
use crate::village::closure::{ClosureElement, VillageClosure};
use crate::village::piece::{
    BlockClip, Mirror, PieceError, PieceSettings, PieceWriter, place_piece,
};
use crate::village::processors::{PieceElement, ProcessLevel};
use crate::village::solver::{Rotation, VillageAssembly, assemble_village};

/// How far, in chunks, a plan's affected box can reach from its start chunk.
///
/// The bound is `ceil((max_distance_from_center + piece width + kernel radius) /
/// 16)` over the village structures (`(80 + 16 + 12) / 16 = 6.75`), rounded up
/// and given a chunk of margin. A column outside this neighbourhood of a
/// structure start cannot be inside that plan's affected box.
pub const NEIGHBOURHOOD_CHUNK_RADIUS: i32 = 8;

/// The column a candidate structure's biome gate is evaluated at: the start
/// chunk's minimum block corner, which is the position
/// [`crate::village::placement::start_origin`] anchors the start piece on.
#[must_use]
pub const fn start_column(chunk_x: i32, chunk_z: i32) -> (i32, i32) {
    (chunk_x * 16, chunk_z * 16)
}

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

/// One piece of a plan, with everything `place_piece` needs for it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanPiece {
    /// The template's id in the closure.
    pub template: Identifier,
    /// `single_pool_element` or `legacy_single_pool_element`; the two install
    /// different block-ignore processors, so the kind is carried, never assumed.
    pub element: PieceElement,
    /// The element's processor list, named lists resolved against the closure.
    pub processors: Vec<StructureProcessorSpec>,
    /// The template pool that referred this element: the entry a processor
    /// compile failure is reported against.
    pub owner: Identifier,
    pub projection: Projection,
    /// World position of the template's origin.
    pub position: BlockPos,
    /// The reference position the element's position predicates read.
    pub reference_pos: BlockPos,
    pub rotation: Rotation,
    pub depth: i32,
}

/// One assembled village, with the placement data its pieces are written from.
#[derive(Debug, Clone, PartialEq)]
pub struct VillagePlan {
    assembly: VillageAssembly,
    junctions: Vec<BeardJunction>,
    affected_min: BlockPos,
    affected_max: BlockPos,
    pieces: Vec<PlanPiece>,
}

impl VillagePlan {
    /// The solver's assembly: pieces, junctions and RIGID beard contributions.
    #[must_use]
    pub const fn assembly(&self) -> &VillageAssembly {
        &self.assembly
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
    pub const fn start_chunk(&self) -> (i32, i32) {
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
    seed: i64,
    blocks: Arc<BlockRegistry>,
    block_tags: Arc<dyn BlockTagIndex + Send + Sync>,
    biome_tags: Arc<dyn BiomeTagIndex + Send + Sync>,
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
    /// gate reads.
    ///
    /// # Errors
    ///
    /// The first [`PieceError`] the validation pass produces.
    pub fn new(
        closure: Arc<VillageClosure>,
        seed: i64,
        blocks: Arc<BlockRegistry>,
        block_tags: Arc<dyn BlockTagIndex + Send + Sync>,
        biome_tags: Arc<dyn BiomeTagIndex + Send + Sync>,
    ) -> Result<Self, PieceError> {
        let source = Self {
            closure,
            seed,
            blocks,
            block_tags,
            biome_tags,
        };
        source.validate()?;
        Ok(source)
    }

    /// The closure every plan is assembled from.
    #[must_use]
    pub fn closure(&self) -> &VillageClosure {
        &self.closure
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

    /// `JigsawStructure.findGenerationPoint`'s gate for one candidate start
    /// chunk: the placement formula decides whether a structure can start
    /// there, the biome gate decides whether it actually does.
    fn plan_for_start_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        free_height: &dyn Fn(i32, i32) -> i32,
        biome_at: &dyn Fn(i32, i32) -> Identifier,
    ) -> Option<VillagePlan> {
        if !self
            .closure
            .placement
            .is_placement_chunk(self.seed, chunk_x, chunk_z)
        {
            return None;
        }
        let (gate_x, gate_z) = start_column(chunk_x, chunk_z);
        let biome = biome_at(gate_x, gate_z);
        let biome_matches = |tag: &Identifier| self.biome_tags.biome_in_tag(tag, &biome);
        let assembly = assemble_village(
            &self.closure,
            self.seed,
            chunk_x,
            chunk_z,
            biome_matches,
            free_height,
        )?;
        self.plan_from(assembly)
    }

    /// Turn the solver's assembly into the plan, resolving per piece the
    /// element data placement needs. `None` when the closure cannot resolve a
    /// placed piece: the closure is loaded by walking exactly the pools the
    /// solver draws from, so this is a defect rather than a runtime case.
    fn plan_from(&self, assembly: VillageAssembly) -> Option<VillagePlan> {
        let junctions: Vec<BeardJunction> = assembly
            .junctions
            .iter()
            .map(|junction| BeardJunction {
                x: junction.source.x,
                ground_y: junction.ground_y,
                z: junction.source.z,
            })
            .collect();
        let mut pieces = Vec::with_capacity(assembly.pieces.len());
        let mut bounds: Option<(BlockPos, BlockPos)> = None;
        for piece in &assembly.pieces {
            bounds = Some(match bounds {
                None => (piece.bounds_min, piece.bounds_max),
                Some((min, max)) => (min_of(min, piece.bounds_min), max_of(max, piece.bounds_max)),
            });
            pieces.push(PlanPiece {
                template: piece.element.clone(),
                element: element_kind(piece.legacy),
                processors: self.element_processors(&piece.processors),
                owner: piece.owner.clone(),
                projection: piece.projection,
                position: piece.position,
                reference_pos: assembly.origin,
                rotation: piece.rotation,
                depth: piece.depth,
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
    /// names for the same template.
    fn element_processors(&self, processors: &ProcessorRef) -> Vec<StructureProcessorSpec> {
        match processors {
            ProcessorRef::List(list) => self
                .closure
                .processor_lists
                .get(list)
                .cloned()
                .unwrap_or_default(),
            ProcessorRef::Inline(specs) => specs.clone(),
        }
    }

    /// Place every piece the closure reaches, under every rotation the solver
    /// can pick and under both world states the reachable processors branch on,
    /// to prove the registry can carry the village before a world uses it.
    ///
    /// Placement rewrites block states (`rotate_state`, `mirror_state`) and the
    /// waterlogging decision reads the world, so a registry missing a rotated
    /// combination fails here, loudly, instead of silently dropping piece blocks
    /// inside chunk generation where nothing can report it. Nothing is written:
    /// the probe writer discards every block.
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
                            processors: &self.element_processors(processors),
                            owner: &pool.id,
                            clip: None,
                        };
                        place_piece(&semantics, template, &settings, &mut writer)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Write `plan`'s piece blocks into `chunk`, clipped to the chunk's box:
    /// vanilla places a structure into the generation region box it was
    /// referenced by, so a piece spanning several chunks writes its own part
    /// into each chunk that asks for it.
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
    /// be resolved. Both are registry/closure mismatches: the closure is loaded
    /// against `blocks`, so a correctly configured world does not reach them.
    pub fn write_pieces(
        &self,
        chunk: &mut Chunk,
        plan: &VillagePlan,
        touched: &mut [bool; 256],
        surface: &dyn Fn(i32, i32) -> i32,
        air: BlockStateId,
        loot: &StructureLoot<'_>,
    ) -> Result<usize, PieceError> {
        let semantics = BlockSemantics::new(self.blocks.as_ref(), self.block_tags.as_ref());
        let geometry = chunk.geometry();
        let min_x = chunk.pos.x * 16;
        let min_z = chunk.pos.z * 16;
        let clip = BlockClip::new(
            [min_x, geometry.min_y(), min_z],
            [min_x + 15, geometry.max_y() - 1, min_z + 15],
        );
        let mut placed = 0;
        for piece in &plan.pieces {
            let Some(template) = self.closure.piece(&piece.template) else {
                continue;
            };
            let settings = PieceSettings {
                position: piece.position,
                reference_pos: piece.reference_pos,
                rotation: piece.rotation,
                // Village pieces are never mirrored: vanilla's jigsaw placement
                // builds every `StructurePlaceSettings` with the default
                // `Mirror.NONE`.
                mirror: Mirror::None,
                projection: piece.projection,
                element: piece.element,
                processors: &piece.processors,
                owner: &piece.owner,
                clip: Some(clip),
            };
            let mut writer = ChunkPieceWriter {
                chunk,
                touched,
                surface,
                loot,
                air,
            };
            placed += place_piece(&semantics, template, &settings, &mut writer)?;
        }
        Ok(placed)
    }
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

    fn set_chest(&mut self, _pos: BlockPos, _chest: &TemplateChest, _index: usize) {}
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

    fn set_chest(&mut self, pos: BlockPos, chest: &TemplateChest, index: usize) {
        let contents = chest.resolve_contents(self.loot, [pos.x, pos.y, pos.z], index);
        self.chunk.chests.insert(pos, contents);
    }
}
