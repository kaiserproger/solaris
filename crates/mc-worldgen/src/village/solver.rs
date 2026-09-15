//! The jigsaw solver: `ChunkGenerator.createStructures`,
//! `JigsawStructure.findGenerationPoint` and `JigsawPlacement`.
//!
//! Transcribed from the 26.1.2 bodies. For one candidate chunk the engine, like
//! vanilla, does three things in order:
//!
//! 1. **Selection.** `ChunkGenerator.createStructures` seeds one
//!    `WorldgenRandom(LegacyRandomSource(0))` with `setLargeFeatureSeed(seed,
//!    chunkX, chunkZ)` and repeatedly draws a weighted entry from the set's
//!    remaining entries, until one of them generates. An entry that does not
//!    generate (its biome gate rejects the start position) is removed and the
//!    next draw picks among what is left, from the *same* stream, so a chunk
//!    where any set entry is eligible holds a structure;
//! 2. **Assembly.** `JigsawStructure.findGenerationPoint` anchors the start
//!    piece on the named start jigsaw (`JigsawPlacement.getRandomNamedJigsaw`)
//!    and sets its Y from the `WORLD_SURFACE_WG` first-free height at the centre
//!    of its bounding box, then `JigsawPlacement$Placer.tryPlacingChildren`
//!    grows the structure: every source jigsaw (shuffled, then sorted by
//!    `selection_priority` descending) walks the target pool's shuffled
//!    templates and their shuffled rotations, accepts the first jigsaw pair that
//!    `canAttach` and whose target box fits the free space
//!    (`join_is_not_empty(free, create(target.deflate(0.25)), ONLY_SECOND)`),
//!    raises the target box by `use_expansion_hack`'s `expandTo`, computes the
//!    target's Y per the rigid/non-rigid rules, writes both junctions and queues
//!    the child at the source jigsaw's `placement_priority`;
//! 3. **Biome gate.** `Structure.findValidGenerationPoint` resolves the biome at
//!    the *stub position* (the structure's computed centre) and tests it against
//!    the selected structure's biome tag. Only then is the assembly kept.
//!
//! Growth runs on `GenerationContext.random`, a second
//! `WorldgenRandom(LegacyRandomSource(0))` seeded with the same
//! `setLargeFeatureSeed(seed, chunkX, chunkZ)` the selection used — a fresh
//! object, so the two streams are independent.
//!
//! A pool element is either a piece (`single_pool_element` /
//! `legacy_single_pool_element`, grown from its template) or a placed feature
//! (`feature_pool_element`, a **terminal leaf**: its synthetic jigsaw points at
//! the empty pool at `position` and its bounding box is the degenerate
//! `position..position`, so it attaches, terminates and contributes no further
//! growth — but it consumes the same draws every other candidate does, which is
//! why skipping it would move the whole village).
//!
//! Piece *placement* (writing blocks, rotation of states, processors) lives in
//! `super::piece`; this module only produces the plan. Feature execution lives
//! in `super::decor`, driven from the plan.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use mc_data::Identifier;
use mc_data::village_data::{ProcessorRef, Projection, StructureSpec};
use mc_world::BlockPos;

use super::beard::BeardContribution;
use super::closure::{ClosureElement, ClosurePool, ClosureStructure, VillageClosure};
use super::placement::{
    RandomSpreadPlacement, set_large_feature_seed, set_large_feature_with_salt, start_origin,
};
use super::shapes::{Aabb, BooleanOp, VoxelShape};
use crate::structures::{BlockFace, Joint, StructureTemplate};
use crate::vanilla_features::{LegacyRandom, RandomSource};

/// `Rotation`, in vanilla's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

impl Rotation {
    /// The rotation's quarter turns, i.e. `Rotation.getIndex()`.
    const fn turns(self) -> usize {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 1,
            Self::Clockwise180 => 2,
            Self::CounterClockwise90 => 3,
        }
    }

    /// The rotation's quarter turns, i.e. `Rotation.getIndex()`: `0` for
    /// [`Rotation::None`], `1` for a clockwise quarter turn, and so on.
    ///
    /// The site lane reports a generated piece's rotation with this, so a
    /// consumer that only needs the quarter turn does not re-derive the mapping
    /// from the placement lane.
    #[must_use]
    pub const fn quarter_turns(self) -> u16 {
        self.turns() as u16
    }

    /// `Rotation.getRandom(random)`.
    #[must_use]
    pub fn get_random(random: &mut impl RandomSource) -> Self {
        match random.next_int_bounded(4) {
            0 => Self::None,
            1 => Self::Clockwise90,
            2 => Self::Clockwise180,
            _ => Self::CounterClockwise90,
        }
    }

    /// `Rotation.getShuffled(random)`, i.e. `Util.shuffle` over the values.
    #[must_use]
    pub fn get_shuffled(random: &mut impl RandomSource) -> Vec<Self> {
        let mut values = vec![
            Self::None,
            Self::Clockwise90,
            Self::Clockwise180,
            Self::CounterClockwise90,
        ];
        shuffle(&mut values, random);
        values
    }

    /// `StructureTemplate.transform(pos, Mirror.NONE, rotation, BlockPos.ZERO)`.
    #[must_use]
    pub fn transform(self, pos: [i32; 3]) -> [i32; 3] {
        let [x, y, z] = pos;
        match self {
            Self::None => [x, y, z],
            Self::Clockwise90 => [-z, y, x],
            Self::Clockwise180 => [-x, y, -z],
            Self::CounterClockwise90 => [z, y, -x],
        }
    }

    /// `Rotation.rotate(Direction)`, i.e. `OctahedralGroup.rotate` on one face:
    /// `up`/`down` are fixed by a rotation about Y, and the four horizontals
    /// step clockwise.
    #[must_use]
    pub fn rotate_face(self, face: BlockFace) -> BlockFace {
        const CARDINALS: [BlockFace; 4] = [
            BlockFace::North,
            BlockFace::East,
            BlockFace::South,
            BlockFace::West,
        ];
        let index = match face {
            BlockFace::Up | BlockFace::Down => return face,
            BlockFace::North => 0,
            BlockFace::East => 1,
            BlockFace::South => 2,
            BlockFace::West => 3,
        };
        CARDINALS[(index + self.turns()) % 4]
    }

    /// `JigsawBlock.rotate` (`OctahedralGroup.rotate(FrontAndTop)`): both faces
    /// rotate independently, so a jigsaw's `front` and `top` are each turned.
    #[must_use]
    pub fn rotate_front_and_top(self, front: BlockFace, top: BlockFace) -> (BlockFace, BlockFace) {
        (self.rotate_face(front), self.rotate_face(top))
    }
}

/// `Util.shuffle`.
pub fn shuffle<T>(values: &mut [T], random: &mut impl RandomSource) {
    for index in (2..=values.len()).rev() {
        let swap = random.next_int_bounded(index as i32) as usize;
        values.swap(index - 1, swap);
    }
}

/// One pool element, with what placement needs from it.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacedElement {
    /// `minecraft:empty_pool_element`. A drawn one ends the target search for
    /// its source jigsaw.
    Empty,
    /// `single_pool_element` / `legacy_single_pool_element`, placed from its
    /// template.
    Single {
        template: Identifier,
        /// The pool this element belongs to: the entry a processor failure is
        /// reported against and the piece's processor list source.
        owner: Identifier,
        projection: Projection,
        processors: ProcessorRef,
        /// `legacy_single_pool_element` rather than `single_pool_element`. The
        /// two install different block-ignore processors, so the kind travels
        /// with the element instead of being re-derived from the template id.
        legacy: bool,
    },
    /// `feature_pool_element`: a placed feature, placed where the piece lands.
    Feature {
        placed_feature: Identifier,
        owner: Identifier,
        projection: Projection,
    },
}

impl PlacedElement {
    /// The element's `StructurePoolElement.getProjection`; `None` for
    /// `empty_pool_element`, which is never placed.
    #[must_use]
    pub fn projection(&self) -> Option<Projection> {
        match self {
            Self::Empty => None,
            Self::Single { projection, .. } | Self::Feature { projection, .. } => Some(*projection),
        }
    }
}

/// One piece the solver placed, without its blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPiece {
    pub element: PlacedElement,
    pub projection: Projection,
    /// World position of the element's origin (a template's origin, or a
    /// feature element's single block).
    pub position: BlockPos,
    pub rotation: Rotation,
    pub ground_level_delta: i32,
    pub bounds_min: BlockPos,
    pub bounds_max: BlockPos,
    pub depth: i32,
}

/// `JigsawJunction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Junction {
    pub source: BlockPos,
    pub ground_y: i32,
    pub delta_y: i32,
    pub projection: Projection,
}

/// Everything the solver decided.
#[derive(Debug, Clone, PartialEq)]
pub struct VillageAssembly {
    pub structure: Identifier,
    pub biome_tag: Identifier,
    pub placement: RandomSpreadPlacement,
    pub chunk: (i32, i32),
    /// The `GenerationStub` position: the first piece's box centre at its own
    /// ground level. The biome gate resolves the biome here.
    pub origin: BlockPos,
    pub pieces: Vec<PlacedPiece>,
    pub junctions: Vec<Junction>,
    pub beard_pieces: Vec<BeardContribution>,
}

impl VillageAssembly {
    /// The distinct placed elements, in placement order.
    #[must_use]
    pub fn elements(&self) -> Vec<&PlacedElement> {
        let mut out: Vec<&PlacedElement> = Vec::new();
        for piece in &self.pieces {
            if !out.contains(&&piece.element) {
                out.push(&piece.element);
            }
        }
        out
    }
}

/// The gate a candidate structure's biome is tested against: the world's own
/// biome at a column, and the tag contents that answer whether a biome
/// satisfies a structure's `biome` tag. Every gate question is "is this
/// column's biome in this tag", so the solver carries the pair as one.
struct BiomeGate<'a> {
    biome_at: &'a dyn Fn(i32, i32) -> Identifier,
    matches: &'a dyn Fn(&Identifier, &Identifier) -> bool,
}

/// `ChunkGenerator.createStructures`'s selection plus
/// `JigsawStructure.findGenerationPoint`: the structure a candidate chunk
/// generates, or `None` when the chunk is not a candidate or no entry of the
/// set passes its biome gate.
///
/// `free_height(x, z)` is the `WORLD_SURFACE_WG` first-free-height query,
/// `biome_at(x, z)` the terrain's plan-free biome at a column (vanilla resolves
/// `getNoiseBiome` at the stub position; the terrain's own biome is the same
/// answer for that column), and `biome_matches(tag, biome)` the tag membership
/// of the worldgen biome tag the structure names.
pub fn assemble_village(
    closure: &VillageClosure,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    biome_at: impl Fn(i32, i32) -> Identifier,
    biome_matches: impl Fn(&Identifier, &Identifier) -> bool,
    free_height: impl Fn(i32, i32) -> i32,
) -> Option<VillageAssembly> {
    if !closure.placement.is_placement_chunk(seed, chunk_x, chunk_z) {
        return None;
    }
    // The selection random: a fresh legacy source, seeded the way
    // `ChunkGenerator.createStructures` seeds its own.
    let gate = BiomeGate {
        biome_at: &biome_at,
        matches: &biome_matches,
    };
    let mut random = LegacyRandom::new(set_large_feature_seed(seed, chunk_x, chunk_z));
    let mut remaining: Vec<&ClosureStructure> = closure.structures.iter().collect();
    let mut total: i64 = remaining
        .iter()
        .map(|structure| i64::from(structure.weight.max(1)))
        .sum();
    while !remaining.is_empty() {
        let choice = i64::from(random.next_int_bounded(i32::try_from(total).ok()?));
        let mut index = 0;
        let mut walk = choice;
        for (position, structure) in remaining.iter().enumerate() {
            walk -= i64::from(structure.weight.max(1));
            if walk < 0 {
                index = position;
                break;
            }
        }
        let selected = remaining[index];
        if let Some(assembly) = assemble_structure(
            closure,
            selected,
            seed,
            chunk_x,
            chunk_z,
            &gate,
            &free_height,
        ) {
            return Some(assembly);
        }
        total -= i64::from(selected.weight.max(1));
        remaining.remove(index);
    }
    None
}

/// `Structure.generate` + `findValidGenerationPoint` for one set entry: the
/// assembly, kept only when the selected structure's biome tag contains the
/// biome at the stub position.
///
/// Vanilla grows the whole village and then judges the gate. The engine decides
/// the gate from the *start piece* first, because the answer cannot depend on
/// the growth and the growth's draws belong to this attempt's own random, which
/// is discarded when the attempt fails — so the village that is kept is
/// identical, while a candidate chunk whose biome hosts no entry of the set
/// costs a start piece instead of five grown villages. The per-column plan
/// lookup makes that the difference between usable and pathological chunk
/// generation.
fn assemble_structure(
    closure: &VillageClosure,
    structure: &ClosureStructure,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    gate: &BiomeGate<'_>,
    free_height: &impl Fn(i32, i32) -> i32,
) -> Option<VillageAssembly> {
    let (mut assembly, mut random) =
        plan_start(closure, structure, seed, chunk_x, chunk_z, free_height)?;
    let biome = (gate.biome_at)(assembly.origin.x, assembly.origin.z);
    if !(gate.matches)(&assembly.biome_tag, &biome) {
        return None;
    }
    grow_start(closure, structure, &mut assembly, &mut random, free_height);
    Some(assembly)
}

/// `JigsawPlacement.addPieces` up to the growth: the start piece is drawn and
/// anchored, its Y projected to the heightmap, and the assembly it starts is
/// returned with the growth random positioned where vanilla begins to grow.
fn plan_start(
    closure: &VillageClosure,
    structure: &ClosureStructure,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    free_height: &impl Fn(i32, i32) -> i32,
) -> Option<(VillageAssembly, LegacyRandom)> {
    let spec = &structure.spec;
    let mut random = LegacyRandom::new(set_large_feature_seed(seed, chunk_x, chunk_z));
    let rotation = Rotation::get_random(&mut random);
    let center_pool = closure.pool(&spec.start_pool)?;
    let center_element = pick_random_template(center_pool, &mut random)?;
    if matches!(center_element, PlacedElement::Empty) {
        return None;
    }
    let origin = start_origin(chunk_x, chunk_z, structure.start_height, 0);
    let anchored = anchor_origin(
        closure,
        &center_element,
        &spec.start_jigsaw_name,
        origin,
        rotation,
        &mut random,
    );

    let (mut center_min, mut center_max) =
        element_bounds(&center_element, closure, anchored, rotation)?;
    // `StructurePoolElement.getGroundLevelDelta` is 1 for every element kind the
    // village closure reaches.
    let ground_level_delta = 1;
    let bottom_y = if spec.project_start_to_heightmap.is_some() {
        let center_x = (center_max.x + center_min.x) / 2;
        let center_z = (center_max.z + center_min.z) / 2;
        free_height(center_x, center_z)
    } else {
        anchored.y
    };
    let shift = bottom_y - (center_min.y + ground_level_delta);
    center_min.y += shift;
    center_max.y += shift;
    let center_position = BlockPos {
        x: anchored.x,
        y: anchored.y + shift,
        z: anchored.z,
    };
    let projection = center_element
        .projection()
        .expect("the empty element returned above");
    let center_piece = PlacedPiece {
        element: center_element,
        projection,
        position: center_position,
        rotation,
        ground_level_delta,
        bounds_min: center_min,
        bounds_max: center_max,
        depth: 0,
    };

    let center_x = (center_max.x + center_min.x) / 2;
    let center_z = (center_max.z + center_min.z) / 2;
    let center_y = bottom_y + (anchored.y - origin.y);

    let mut assembly = VillageAssembly {
        structure: structure.id.clone(),
        biome_tag: biomes_tag(spec),
        placement: closure.placement,
        chunk: (chunk_x, chunk_z),
        origin: BlockPos {
            x: center_x,
            y: center_y,
            z: center_z,
        },
        pieces: vec![center_piece.clone()],
        junctions: Vec::new(),
        beard_pieces: Vec::new(),
    };
    // Vanilla's `Beardifier` collects every RIGID piece of the structure start,
    // and the start piece is one of them: the town centre pulls the terrain
    // around it exactly like the pieces grown from it.
    if center_piece.projection == Projection::Rigid {
        assembly.beard_pieces.push(BeardContribution {
            min: center_piece.bounds_min,
            max: center_piece.bounds_max,
            ground_level_delta: center_piece.ground_level_delta,
        });
    }
    Some((assembly, random))
}

/// `JigsawPlacement.addPieces`' growth: the free-space limit, the start piece's
/// own shape and the placer.
fn grow_start(
    closure: &VillageClosure,
    structure: &ClosureStructure,
    assembly: &mut VillageAssembly,
    random: &mut LegacyRandom,
    free_height: &impl Fn(i32, i32) -> i32,
) {
    let spec = &structure.spec;
    if spec.size <= 0 {
        return;
    }
    let center_piece = &assembly.pieces[0];
    let center_x = (center_piece.bounds_max.x + center_piece.bounds_min.x) / 2;
    let center_z = (center_piece.bounds_max.z + center_piece.bounds_min.z) / 2;
    let horizontal = spec.max_distance_from_center.horizontal;
    let vertical = spec.max_distance_from_center.vertical;
    let limit = Aabb::new(
        f64::from(center_x - horizontal),
        f64::from(assembly.origin.y - vertical),
        f64::from(center_z - horizontal),
        f64::from(center_x + horizontal + 1),
        f64::from(assembly.origin.y + vertical + 1),
        f64::from(center_z + horizontal + 1),
    );
    let root = VoxelShape::create(limit).join(
        &VoxelShape::create(Aabb::of(
            center_piece.bounds_min.x,
            center_piece.bounds_min.y,
            center_piece.bounds_min.z,
            center_piece.bounds_max.x,
            center_piece.bounds_max.y,
            center_piece.bounds_max.z,
        )),
        BooleanOp::OnlyFirst,
    );
    let center_piece = center_piece.clone();

    let mut placer = Placer {
        closure,
        spec,
        random,
        free_height,
        pieces: &mut assembly.pieces,
        junctions: &mut assembly.junctions,
        beard_pieces: &mut assembly.beard_pieces,
        placing: PriorityQueue::default(),
    };
    let context_free: FreeShape = Rc::new(RefCell::new(root));
    placer.grow(&center_piece, &context_free, 0);
    while let Some(state) = placer.placing.pop() {
        placer.grow(&state.piece, &state.free, state.depth);
    }
}

/// `JigsawBlockInfo` reduced to what placement asks of it: the world position of
/// the jigsaw, its rotated faces, and the connection data.
#[derive(Debug, Clone, PartialEq)]
pub struct Jigsaw {
    pub pos: BlockPos,
    pub front: BlockFace,
    pub top: BlockFace,
    pub joint: Joint,
    pub name: Identifier,
    pub target: Identifier,
    pub pool: Identifier,
    pub placement_priority: i32,
}

/// One free-space handle: vanilla's `MutableObject<VoxelShape>`, which several
/// queued pieces share and grow in place.
type FreeShape = Rc<RefCell<VoxelShape>>;

/// One queued piece: the piece, the free shape it grows into and its depth.
struct PieceState {
    piece: PlacedPiece,
    free: FreeShape,
    depth: i32,
}

/// `SequencedPriorityIterator`: one FIFO queue per priority, drained highest
/// priority first.
///
/// Vanilla keeps a cached pointer to the highest non-empty queue and re-scans
/// the map only when that queue empties, which is the same order as scanning for
/// the maximum on every pop: every `add` of a priority appends to that
/// priority's single deque.
#[derive(Default)]
struct PriorityQueue {
    buckets: BTreeMap<i32, VecDeque<PieceState>>,
}

impl PriorityQueue {
    fn add(&mut self, state: PieceState, priority: i32) {
        self.buckets.entry(priority).or_default().push_back(state);
    }

    fn pop(&mut self) -> Option<PieceState> {
        let priority = *self.buckets.keys().next_back()?;
        let bucket = self.buckets.get_mut(&priority)?;
        let state = bucket.pop_front();
        if bucket.is_empty() {
            self.buckets.remove(&priority);
        }
        state
    }
}

/// `JigsawPlacement$Placer`.
struct Placer<'a> {
    closure: &'a VillageClosure,
    spec: &'a StructureSpec,
    random: &'a mut LegacyRandom,
    free_height: &'a dyn Fn(i32, i32) -> i32,
    pieces: &'a mut Vec<PlacedPiece>,
    junctions: &'a mut Vec<Junction>,
    beard_pieces: &'a mut Vec<BeardContribution>,
    placing: PriorityQueue,
}

impl Placer<'_> {
    /// `JigsawPlacement$Placer.tryPlacingChildren`.
    fn grow(&mut self, source: &PlacedPiece, context_free: &FreeShape, depth: i32) {
        let source_rigid = source.projection == Projection::Rigid;
        let mut source_free: Option<FreeShape> = None;
        let source_box_y = source.bounds_min.y;
        let source_ground_level_delta = source.ground_level_delta;

        let source_jigsaws = jigsaws_for(
            &source.element,
            self.closure,
            source.position,
            source.rotation,
            self.random,
        );
        'source_jigsaw: for source_jigsaw in source_jigsaws {
            let source_jigsaw_pos = source_jigsaw.pos;
            let front = source_jigsaw.front.step();
            let target_jigsaw_pos = BlockPos {
                x: source_jigsaw_pos.x + front[0],
                y: source_jigsaw_pos.y + front[1],
                z: source_jigsaw_pos.z + front[2],
            };
            let source_jigsaw_local_y = source_jigsaw_pos.y - source_box_y;
            let mut source_jigsaw_base_height = i32::MIN;
            let Some(target_pool) = self.closure.pool(&source_jigsaw.pool) else {
                continue;
            };
            let fallback = self.closure.pool(&target_pool.fallback);
            let attach_inside = inside(source, target_jigsaw_pos);
            let children_free: FreeShape = if attach_inside {
                source_free
                    .get_or_insert_with(|| {
                        Rc::new(RefCell::new(VoxelShape::create(Aabb::of(
                            source.bounds_min.x,
                            source.bounds_min.y,
                            source.bounds_min.z,
                            source.bounds_max.x,
                            source.bounds_max.y,
                            source.bounds_max.z,
                        ))))
                    })
                    .clone()
            } else {
                context_free.clone()
            };

            let mut target_elements: Vec<PlacedElement> = Vec::new();
            if depth != self.spec.size {
                target_elements.extend(shuffled_templates(target_pool, self.random));
            }
            if let Some(fallback) = fallback {
                target_elements.extend(shuffled_templates(fallback, self.random));
            }

            for target_element in target_elements {
                if matches!(target_element, PlacedElement::Empty) {
                    break;
                }
                for target_rotation in Rotation::get_shuffled(self.random) {
                    let target_jigsaws = jigsaws_for(
                        &target_element,
                        self.closure,
                        BlockPos { x: 0, y: 0, z: 0 },
                        target_rotation,
                        self.random,
                    );
                    let Some((hack_min, hack_max)) = element_bounds(
                        &target_element,
                        self.closure,
                        BlockPos { x: 0, y: 0, z: 0 },
                        target_rotation,
                    ) else {
                        continue;
                    };
                    let expand_to = self.expand_to(hack_min, hack_max, &target_jigsaws);
                    for target_jigsaw in &target_jigsaws {
                        if !can_attach(&source_jigsaw, target_jigsaw) {
                            continue;
                        }
                        let target_local = target_jigsaw.pos;
                        let raw_position = BlockPos {
                            x: target_jigsaw_pos.x - target_local.x,
                            y: target_jigsaw_pos.y - target_local.y,
                            z: target_jigsaw_pos.z - target_local.z,
                        };
                        let Some((raw_min, _)) = element_bounds(
                            &target_element,
                            self.closure,
                            raw_position,
                            target_rotation,
                        ) else {
                            continue;
                        };
                        let Some(target_projection) = target_element.projection() else {
                            continue;
                        };
                        let target_rigid = target_projection == Projection::Rigid;
                        let delta_y = source_jigsaw_local_y - target_jigsaw.pos.y + front[1];
                        let target_box_y = if source_rigid && target_rigid {
                            source_box_y + delta_y
                        } else {
                            if source_jigsaw_base_height == i32::MIN {
                                source_jigsaw_base_height =
                                    (self.free_height)(source_jigsaw_pos.x, source_jigsaw_pos.z);
                            }
                            source_jigsaw_base_height - target_jigsaw.pos.y
                        };
                        let y_offset = target_box_y - raw_min.y;
                        let target_position = BlockPos {
                            x: raw_position.x,
                            y: raw_position.y + y_offset,
                            z: raw_position.z,
                        };
                        let Some((target_min, mut target_max)) = element_bounds(
                            &target_element,
                            self.closure,
                            target_position,
                            target_rotation,
                        ) else {
                            continue;
                        };
                        if expand_to > 0 {
                            // `use_expansion_hack`: the target's own box grows
                            // upward to `newSize` before it is tested, so the
                            // piece that carries it (and its beard contribution)
                            // is that taller box. Vanilla measures the box with
                            // `maxY() - minY()`, not `getYSpan()`.
                            let new_size = (expand_to + 1).max(target_max.y - target_min.y);
                            target_max.y = target_max.y.max(target_min.y + new_size);
                        }
                        let target_box = Aabb::of(
                            target_min.x,
                            target_min.y,
                            target_min.z,
                            target_max.x,
                            target_max.y,
                            target_max.z,
                        );
                        if VoxelShape::join_is_not_empty(
                            &children_free.borrow(),
                            &VoxelShape::create(target_box.deflate(0.25)),
                            BooleanOp::OnlySecond,
                        ) {
                            continue;
                        }
                        let grown = children_free.borrow().join_unoptimized(
                            &VoxelShape::create(target_box),
                            BooleanOp::OnlyFirst,
                        );
                        *children_free.borrow_mut() = grown.clone();
                        let target_ground_level_delta = if target_rigid {
                            source_ground_level_delta - delta_y
                        } else {
                            // `StructurePoolElement.getGroundLevelDelta`.
                            1
                        };
                        let junction_y = if source_rigid {
                            source_box_y + source_jigsaw_local_y
                        } else if target_rigid {
                            target_box_y + target_jigsaw.pos.y
                        } else {
                            if source_jigsaw_base_height == i32::MIN {
                                source_jigsaw_base_height =
                                    (self.free_height)(source_jigsaw_pos.x, source_jigsaw_pos.z);
                            }
                            source_jigsaw_base_height + delta_y / 2
                        };
                        self.junctions.push(Junction {
                            source: target_jigsaw_pos,
                            ground_y: junction_y - source_jigsaw_local_y
                                + source_ground_level_delta,
                            delta_y,
                            projection: target_projection,
                        });
                        self.junctions.push(Junction {
                            source: source_jigsaw_pos,
                            ground_y: junction_y - target_jigsaw.pos.y + target_ground_level_delta,
                            delta_y: -delta_y,
                            projection: source.projection,
                        });
                        if target_rigid {
                            self.beard_pieces.push(BeardContribution {
                                min: target_min,
                                max: target_max,
                                ground_level_delta: target_ground_level_delta,
                            });
                        }
                        let target_piece = PlacedPiece {
                            element: target_element.clone(),
                            projection: target_projection,
                            position: target_position,
                            rotation: target_rotation,
                            ground_level_delta: target_ground_level_delta,
                            bounds_min: target_min,
                            bounds_max: target_max,
                            depth: depth + 1,
                        };
                        self.pieces.push(target_piece.clone());
                        if depth < self.spec.size {
                            self.placing.add(
                                PieceState {
                                    piece: target_piece,
                                    free: children_free.clone(),
                                    depth: depth + 1,
                                },
                                source_jigsaw.placement_priority,
                            );
                        }
                        // `continue label129`: the remaining candidates for this
                        // source jigsaw are abandoned once one attaches.
                        continue 'source_jigsaw;
                    }
                }
            }
        }
    }

    /// `use_expansion_hack`'s `expandTo`: the tallest pool size among the target
    /// jigsaws whose front position lands inside the candidate's own box.
    fn expand_to(&self, min: BlockPos, max: BlockPos, jigsaws: &[Jigsaw]) -> i32 {
        if !self.spec.use_expansion_hack || max.y - min.y + 1 > 16 {
            return 0;
        }
        let mut expand_to = 0;
        for jigsaw in jigsaws {
            let front = jigsaw.front.step();
            let front_pos = BlockPos {
                x: jigsaw.pos.x + front[0],
                y: jigsaw.pos.y + front[1],
                z: jigsaw.pos.z + front[2],
            };
            if front_pos.x < min.x
                || front_pos.x > max.x
                || front_pos.y < min.y
                || front_pos.y > max.y
                || front_pos.z < min.z
                || front_pos.z > max.z
            {
                continue;
            }
            let pool_size = self
                .closure
                .pool(&jigsaw.pool)
                .map_or(0, |pool| pool.max_size);
            let fallback_size = self
                .closure
                .pool(&jigsaw.pool)
                .and_then(|pool| self.closure.pool(&pool.fallback))
                .map_or(0, |pool| pool.max_size);
            expand_to = expand_to.max(pool_size.max(fallback_size));
        }
        expand_to
    }
}

/// Whether `pos` is inside the piece's bounding box (`BoundingBox.isInside`).
fn inside(piece: &PlacedPiece, pos: BlockPos) -> bool {
    pos.x >= piece.bounds_min.x
        && pos.x <= piece.bounds_max.x
        && pos.y >= piece.bounds_min.y
        && pos.y <= piece.bounds_max.y
        && pos.z >= piece.bounds_min.z
        && pos.z <= piece.bounds_max.z
}

/// `StructureTemplatePool.getRandomTemplate`: a uniform draw from the
/// weight-expanded element list. `None` for an empty pool, which vanilla answers
/// with `EmptyPoolElement` (and draws nothing).
fn pick_random_template(
    pool: &ClosurePool,
    random: &mut impl RandomSource,
) -> Option<PlacedElement> {
    let flat = pool_elements(pool);
    if flat.is_empty() {
        return None;
    }
    let index = random.next_int_bounded(i32::try_from(flat.len()).ok()?);
    Some(flat[index as usize].clone())
}

/// `StructureTemplatePool.getShuffledTemplates`: a shuffled copy of the
/// weight-expanded element list.
fn shuffled_templates(pool: &ClosurePool, random: &mut impl RandomSource) -> Vec<PlacedElement> {
    let mut flat = pool_elements(pool);
    shuffle(&mut flat, random);
    flat
}

/// The pool's elements, each repeated `weight` times, in vanilla's order.
fn pool_elements(pool: &ClosurePool) -> Vec<PlacedElement> {
    let mut flat = Vec::new();
    for (weight, element) in &pool.elements {
        let placed = match element {
            ClosureElement::Empty => PlacedElement::Empty,
            ClosureElement::Single {
                piece,
                projection,
                processors,
                legacy,
            } => PlacedElement::Single {
                template: piece.clone(),
                owner: pool.id.clone(),
                projection: *projection,
                processors: processors.clone(),
                legacy: *legacy,
            },
            ClosureElement::Feature {
                placed_feature,
                projection,
            } => PlacedElement::Feature {
                placed_feature: placed_feature.clone(),
                owner: pool.id.clone(),
                projection: *projection,
            },
        };
        for _ in 0..(*weight).max(1) {
            flat.push(placed.clone());
        }
    }
    flat
}

/// `JigsawPlacement.getRandomNamedJigsaw`: the anchored origin of the start
/// piece, `position - localAnchor`.
fn anchor_origin(
    closure: &VillageClosure,
    element: &PlacedElement,
    start_jigsaw: &Option<Identifier>,
    position: BlockPos,
    rotation: Rotation,
    random: &mut impl RandomSource,
) -> BlockPos {
    let Some(target) = start_jigsaw else {
        return position;
    };
    for jigsaw in jigsaws_for(element, closure, position, rotation, random) {
        if jigsaw.name == *target {
            // `jigsaw.pos` is the connector's world position when the element is
            // placed *at* `position`; vanilla anchors the element so that
            // connector lands exactly on `position`:
            // `localAnchor = anchoredPosition - position`, then
            // `adjustedPosition = position - localAnchor`.
            let local = BlockPos {
                x: jigsaw.pos.x - position.x,
                y: jigsaw.pos.y - position.y,
                z: jigsaw.pos.z - position.z,
            };
            return BlockPos {
                x: position.x - local.x,
                y: position.y - local.y,
                z: position.z - local.z,
            };
        }
    }
    position
}

/// `StructurePoolElement.getShuffledJigsawBlocks`: the element's jigsaw blocks
/// in world coordinates, shuffled, highest `selection_priority` first.
///
/// A feature element ignores the rotation and reports one synthetic jigsaw at
/// `position` (`FeaturePoolElement.getShuffledJigsawBlocks`), whose empty pool
/// and `minecraft:empty` target make the branch terminate.
pub(crate) fn jigsaws_for(
    element: &PlacedElement,
    closure: &VillageClosure,
    position: BlockPos,
    rotation: Rotation,
    random: &mut impl RandomSource,
) -> Vec<Jigsaw> {
    match element {
        PlacedElement::Single { template, .. } => {
            let Some(template) = closure.piece(template) else {
                return Vec::new();
            };
            let mut ranked: Vec<(i32, Jigsaw)> = template
                .jigsaws()
                .iter()
                .map(|jigsaw| {
                    let local = rotation.transform(jigsaw.pos);
                    let (front, top) = rotation.rotate_front_and_top(jigsaw.front, jigsaw.top);
                    (
                        jigsaw.selection_priority,
                        Jigsaw {
                            pos: BlockPos {
                                x: position.x + local[0],
                                y: position.y + local[1],
                                z: position.z + local[2],
                            },
                            front,
                            top,
                            joint: jigsaw.joint,
                            name: jigsaw.name.clone(),
                            target: jigsaw.target.clone(),
                            pool: jigsaw.pool.clone(),
                            placement_priority: jigsaw.placement_priority,
                        },
                    )
                })
                .collect();
            shuffle(&mut ranked, random);
            ranked.sort_by(|first, second| second.0.cmp(&first.0));
            ranked.into_iter().map(|(_, jigsaw)| jigsaw).collect()
        }
        PlacedElement::Feature { .. } => vec![Jigsaw {
            pos: position,
            front: BlockFace::Down,
            top: BlockFace::South,
            joint: Joint::Rollable,
            name: static_id("minecraft:bottom"),
            target: static_id("minecraft:empty"),
            pool: static_id("minecraft:empty"),
            placement_priority: 0,
        }],
        PlacedElement::Empty => Vec::new(),
    }
}

fn static_id(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).expect("vanilla identifiers are valid")
}

/// `StructurePoolElement.getBoundingBox`: the element's box at `position`.
///
/// A feature element's size is `Vec3i.ZERO`, so its box is the degenerate
/// `position..position` — one block tall, which is what keeps it out of the
/// free-space test's way and lets it attach anywhere.
fn element_bounds(
    element: &PlacedElement,
    closure: &VillageClosure,
    position: BlockPos,
    rotation: Rotation,
) -> Option<(BlockPos, BlockPos)> {
    match element {
        PlacedElement::Empty => None,
        PlacedElement::Feature { .. } => Some((position, position)),
        PlacedElement::Single { template, .. } => {
            piece_bounds(closure.piece(template)?, position, rotation)
        }
    }
}

/// `StructureTemplate.getBoundingBox(settings, position)`: the box of the
/// template's two transformed extreme corners, moved to `position`.
fn piece_bounds(
    template: &StructureTemplate,
    position: BlockPos,
    rotation: Rotation,
) -> Option<(BlockPos, BlockPos)> {
    let [size_x, size_y, size_z] = template.size();
    if size_x <= 0 || size_y <= 0 || size_z <= 0 {
        return None;
    }
    let corner = rotation.transform([0, 0, 0]);
    let opposite = rotation.transform([size_x - 1, size_y - 1, size_z - 1]);
    let mut min = [0; 3];
    let mut max = [0; 3];
    for axis in 0..3 {
        min[axis] = corner[axis].min(opposite[axis]);
        max[axis] = corner[axis].max(opposite[axis]);
    }
    Some((
        BlockPos {
            x: position.x + min[0],
            y: position.y + min[1],
            z: position.z + min[2],
        },
        BlockPos {
            x: position.x + max[0],
            y: position.y + max[1],
            z: position.z + max[2],
        },
    ))
}

/// `JigsawBlock.canAttach`.
#[must_use]
pub fn can_attach(source: &Jigsaw, target: &Jigsaw) -> bool {
    let source_front = source.front.step();
    let target_front = target.front.step();
    let opposed = source_front == [-target_front[0], -target_front[1], -target_front[2]];
    let joint_ok = source.joint == Joint::Rollable || source.top == target.top;
    opposed && joint_ok && source.target == target.name
}

fn biomes_tag(structure: &StructureSpec) -> Identifier {
    match &structure.biomes {
        mc_data::village_data::BiomeSet::Tag(tag) => tag.clone(),
        mc_data::village_data::BiomeSet::Biomes(biomes) => biomes
            .first()
            .cloned()
            .unwrap_or_else(|| static_id("minecraft:plains")),
    }
}

/// The random source the placement formula itself runs on
/// (`WorldgenRandom.setLargeFeatureWithSalt`).
#[must_use]
pub fn placement_seed(seed: i64, chunk_x: i32, chunk_z: i32, salt: i64) -> i64 {
    set_large_feature_with_salt(seed, chunk_x, chunk_z, salt)
}
