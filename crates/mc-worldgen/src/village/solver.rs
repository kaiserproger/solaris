//! The jigsaw solver: `JigsawPlacement.addPieces` and `tryPlacingChildren`.
//!
//! Transcribed from the 26.1.2 bodies. Placement starts at the structure set's
//! candidate chunk (see [`super::placement`]), rolls the weighted structure,
//! gates it on the structure's biome tag, and then:
//!
//! 1. picks the start pool's weighted element and the start piece's rotation
//!    (`Rotation.getRandom`), anchoring the piece on the named start jigsaw when
//!    the structure names one (`position - localAnchor`, with the jigsaw list
//!    shuffled first);
//! 2. sets the start piece's Y from the `WORLD_SURFACE_WG` height at the centre
//!    of its bounding box, moving it by `bottomY - (bbox.minY() +
//!    groundLevelDelta)`;
//! 3. grows the village: for each source jigsaw (shuffled, then sorted by
//!    `selection_priority` descending) it walks the target pool's templates and
//!    their `Rotation.getShuffled` rotations, accepts the first jigsaw pair that
//!    `canAttach` and whose target bounding box fits the free space
//!    (`VoxelShape::join_is_not_empty(free, create(target.deflate(0.25)),
//!    ONLY_SECOND)`), computes the target's Y per the rigid/non-rigid rules,
//!    writes both junctions, and enqueues the child for `depth + 1 <= size`;
//! 4. collects the RIGID pieces as beard contributions.
//!
//! Piece *placement* (writing blocks, rotation of states, processors) lives in
//! `super::piece`; this module only produces the plan.

use std::collections::VecDeque;

use mc_data::Identifier;
use mc_data::village_data::{PoolElementSpec, Projection, StructureSpec};
use mc_world::BlockPos;

use super::beard::BeardContribution;
use super::closure::{ClosureElement, ClosurePool, VillageClosure};
use super::placement::{RandomSpreadPlacement, WeightedStructure, start_origin};
use super::shapes::{Aabb, BooleanOp, VoxelShape};
use crate::structures::StructureTemplate;
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
}

/// `Util.shuffle`.
pub fn shuffle<T>(values: &mut [T], random: &mut impl RandomSource) {
    for index in (2..=values.len()).rev() {
        let swap = random.next_int_bounded(index as i32) as usize;
        values.swap(index - 1, swap);
    }
}

/// One piece the solver placed, without its blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPiece {
    pub element: Identifier,
    /// The pool element that placed this piece: its pool is the entry a
    /// processor compile failure is reported against.
    pub owner: Identifier,
    /// The element's processor list, exactly as the pool declared it.
    pub processors: mc_data::village_data::ProcessorRef,
    /// `legacy_single_pool_element` rather than `single_pool_element`. The two
    /// install different block-ignore processors, so the kind travels with the
    /// piece instead of being re-derived from the template id later.
    pub legacy: bool,
    pub projection: Projection,
    /// World position of the template's origin.
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
    pub origin: BlockPos,
    pub pieces: Vec<PlacedPiece>,
    pub junctions: Vec<Junction>,
    pub beard_pieces: Vec<BeardContribution>,
}

impl VillageAssembly {
    /// The distinct placeable elements, in placement order.
    #[must_use]
    pub fn elements(&self) -> Vec<Identifier> {
        let mut out: Vec<Identifier> = Vec::new();
        for piece in &self.pieces {
            if !out.contains(&piece.element) {
                out.push(piece.element.clone());
            }
        }
        out
    }
}

/// `JigsawStructure.findGenerationPoint` + `JigsawPlacement.addPieces`.
///
/// `free_height(x, z)` is the `WORLD_SURFACE_WG` first-free-height query, and
/// `biome_matches` the structure's biome tag gate.
pub fn assemble_village(
    closure: &VillageClosure,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    biome_matches: impl Fn(&Identifier) -> bool,
    free_height: impl Fn(i32, i32) -> i32,
) -> Option<VillageAssembly> {
    if !closure.placement.is_placement_chunk(seed, chunk_x, chunk_z) {
        return None;
    }
    let weighted: Vec<WeightedStructure> = closure
        .structures
        .iter()
        .map(|structure| WeightedStructure {
            id: structure.id.clone(),
            weight: structure.weight,
            biomes: biomes_tag(&structure.spec),
            start_height: start_height(&structure.spec),
        })
        .collect();
    let index = pick_structure(closure.placement, seed, chunk_x, chunk_z, &weighted)?;
    let structure = &closure.structures.get(index)?.spec;
    if !biome_matches(&biomes_tag(structure)) {
        return None;
    }

    let mut random = LegacyRandom::new(placement_seed(
        seed,
        chunk_x,
        chunk_z,
        closure.placement.salt,
    ));
    let center_pool = closure.pool(&structure.start_pool)?;
    let center_choice = pick_element(center_pool, &mut random)?;
    let (center_element, center_owner, center_processors, center_legacy, center_projection) =
        match center_choice {
            PlacedKind::Single {
                piece,
                owner,
                projection,
                processors,
                legacy,
            } => (
                piece.clone(),
                owner.clone(),
                processors.clone(),
                legacy,
                projection,
            ),
            _ => return None,
        };
    let center_template = closure.piece(&center_element)?;
    let rotation = Rotation::get_random(&mut random);

    let origin = start_origin(chunk_x, chunk_z, start_height(structure), 0);
    let anchored = anchor_origin(
        closure,
        center_element.clone(),
        &structure.start_jigsaw_name,
        origin,
        rotation,
        &mut random,
    );

    let center_bounds = piece_bounds(center_template, anchored, rotation)?;
    let ground_level_delta = 1;
    let bottom_y = if structure.project_start_to_heightmap.is_some() {
        let center_x = (center_bounds.0.x + center_bounds.1.x) / 2;
        let center_z = (center_bounds.0.z + center_bounds.1.z) / 2;
        free_height(center_x, center_z)
    } else {
        anchored.y
    };
    let old_absolute_ground_y = center_bounds.0.y + ground_level_delta;
    let shift = bottom_y - old_absolute_ground_y;
    let center_piece = PlacedPiece {
        element: center_element.clone(),
        owner: center_owner,
        processors: center_processors,
        legacy: center_legacy,
        projection: center_projection,
        position: BlockPos {
            x: anchored.x,
            y: anchored.y + shift,
            z: anchored.z,
        },
        rotation,
        ground_level_delta,
        bounds_min: BlockPos {
            x: center_bounds.0.x,
            y: center_bounds.0.y + shift,
            z: center_bounds.0.z,
        },
        bounds_max: BlockPos {
            x: center_bounds.1.x,
            y: center_bounds.1.y + shift,
            z: center_bounds.1.z,
        },
        depth: 0,
    };

    let center_x = (center_piece.bounds_min.x + center_piece.bounds_max.x) / 2;
    let center_z = (center_piece.bounds_min.z + center_piece.bounds_max.z) / 2;
    let center_y = bottom_y + (anchored.y - origin.y);

    let mut assembly = VillageAssembly {
        structure: structure.id.clone(),
        biome_tag: biomes_tag(structure),
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
    if structure.size <= 0 {
        return Some(assembly);
    }

    let horizontal = structure.max_distance_from_center.horizontal;
    let vertical = structure.max_distance_from_center.vertical;
    let limit = Aabb::of(
        center_x - horizontal,
        center_y - vertical,
        center_z - horizontal,
        center_x + horizontal + 1,
        center_y + vertical + 1,
        center_z + horizontal + 1,
    );
    let root = VoxelShape::create(limit).join(
        &VoxelShape::create(Aabb::of(
            center_piece.bounds_min.x,
            center_piece.bounds_min.y,
            center_piece.bounds_min.z,
            center_piece.bounds_max.x + 1,
            center_piece.bounds_max.y + 1,
            center_piece.bounds_max.z + 1,
        )),
        BooleanOp::OnlyFirst,
    );

    let mut queue: VecDeque<(PlacedPiece, VoxelShape, i32, i32)> = VecDeque::new();
    let mut free = root.clone();
    grow(
        closure,
        structure,
        &center_piece,
        &mut free,
        &mut queue,
        &mut assembly,
        &mut random,
        &free_height,
    );
    while let Some((piece, mut children_free, _depth, _priority)) = queue.pop_front() {
        grow(
            closure,
            structure,
            &piece,
            &mut children_free,
            &mut queue,
            &mut assembly,
            &mut random,
            &free_height,
        );
    }
    Some(assembly)
}

/// One candidate element of a pool, with what placement needs from it.
enum PlacedKind {
    Single {
        piece: Identifier,
        /// The pool this element belongs to, so the placed piece knows the entry
        /// its processor list is reported against.
        owner: Identifier,
        projection: Projection,
        processors: mc_data::village_data::ProcessorRef,
        /// `legacy_single_pool_element` or `single_pool_element`; the piece
        /// placement step passes this to the processors module.
        legacy: bool,
    },
    Feature {
        feature: Identifier,
        projection: Projection,
    },
    Empty,
}

/// `StructureTemplatePool.getRandomTemplate`: the flat expanded weight list.
fn pick_element(pool: &ClosurePool, random: &mut impl RandomSource) -> Option<PlacedKind> {
    let flat = flat_elements(pool);
    if flat.is_empty() {
        return None;
    }
    let index = random.next_int_bounded(i32::try_from(flat.len()).ok()?) as usize;
    Some(flat[index].clone())
}

/// The pool's elements, each repeated `weight` times, in vanilla's order.
fn flat_elements(pool: &ClosurePool) -> Vec<PlacedKind> {
    let mut flat = Vec::new();
    for (weight, element) in &pool.elements {
        let kind = match element {
            ClosureElement::Empty => PlacedKind::Empty,
            ClosureElement::Single {
                piece,
                projection,
                processors,
                legacy,
            } => PlacedKind::Single {
                piece: piece.clone(),
                owner: pool.id.clone(),
                projection: *projection,
                processors: processors.clone(),
                legacy: *legacy,
            },
            ClosureElement::Feature {
                placed_feature,
                projection,
            } => PlacedKind::Feature {
                feature: placed_feature.clone(),
                projection: *projection,
            },
        };
        for _ in 0..(*weight).max(1) {
            flat.push(kind.clone());
        }
    }
    flat
}

impl Clone for PlacedKind {
    fn clone(&self) -> Self {
        match self {
            Self::Single {
                piece,
                owner,
                projection,
                processors,
                legacy,
            } => Self::Single {
                piece: piece.clone(),
                owner: owner.clone(),
                projection: *projection,
                processors: processors.clone(),
                legacy: *legacy,
            },
            Self::Feature {
                feature,
                projection,
            } => Self::Feature {
                feature: feature.clone(),
                projection: *projection,
            },
            Self::Empty => Self::Empty,
        }
    }
}

/// `JigsawPlacement.getRandomNamedJigsaw`.
fn anchor_origin(
    closure: &VillageClosure,
    piece: Identifier,
    start_jigsaw: &Option<Identifier>,
    position: BlockPos,
    rotation: Rotation,
    random: &mut impl RandomSource,
) -> BlockPos {
    let Some(target) = start_jigsaw else {
        return position;
    };
    let Some(template) = closure.piece(&piece) else {
        return position;
    };
    let mut jigsaws = shuffled_jigsaws(template, random);
    for jigsaw in jigsaws.drain(..) {
        if jigsaw.name == *target {
            let local = rotation.transform(jigsaw.pos);
            return BlockPos {
                x: position.x - local[0],
                y: position.y - local[1],
                z: position.z - local[2],
            };
        }
    }
    position
}

/// `SinglePoolElement.getShuffledJigsawBlocks`: shuffle, then a stable sort by
/// `selection_priority` descending.
fn shuffled_jigsaws(
    template: &StructureTemplate,
    random: &mut impl RandomSource,
) -> Vec<crate::structures::TemplateJigsaw> {
    let mut jigsaws = template.jigsaws().to_vec();
    shuffle(&mut jigsaws, random);
    jigsaws.sort_by(|first, second| second.selection_priority.cmp(&first.selection_priority));
    jigsaws
}

/// `StructurePoolElement.getBoundingBox` for the template at `position`.
fn piece_bounds(
    template: &StructureTemplate,
    position: BlockPos,
    rotation: Rotation,
) -> Option<(BlockPos, BlockPos)> {
    let [size_x, size_y, size_z] = template.size();
    if size_x <= 0 || size_y <= 0 || size_z <= 0 {
        return None;
    }
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    for corner in [
        [0, 0, 0],
        [size_x - 1, 0, 0],
        [0, 0, size_z - 1],
        [size_x - 1, 0, size_z - 1],
        [0, size_y - 1, 0],
        [size_x - 1, size_y - 1, 0],
        [0, size_y - 1, size_z - 1],
        [size_x - 1, size_y - 1, size_z - 1],
    ] {
        let rotated = rotation.transform(corner);
        for axis in 0..3 {
            min[axis] = min[axis].min(rotated[axis]);
            max[axis] = max[axis].max(rotated[axis]);
        }
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

/// `JigsawPlacement.Placer.tryPlacingChildren`.
#[allow(clippy::too_many_arguments)]
fn grow(
    closure: &VillageClosure,
    structure: &StructureSpec,
    source: &PlacedPiece,
    free: &mut VoxelShape,
    queue: &mut VecDeque<(PlacedPiece, VoxelShape, i32, i32)>,
    assembly: &mut VillageAssembly,
    random: &mut LegacyRandom,
    free_height: &impl Fn(i32, i32) -> i32,
) {
    let Some(source_template) = closure.piece(&source.element) else {
        return;
    };
    let source_rigid = source.projection == Projection::Rigid;
    let mut source_free = None;
    let mut source_jigsaw_base_height = i32::MIN;

    for source_jigsaw in shuffled_jigsaws(source_template, random) {
        let source_jigsaw_pos = BlockPos {
            x: source.position.x + source_jigsaw.pos[0],
            y: source.position.y + source_jigsaw.pos[1],
            z: source.position.z + source_jigsaw.pos[2],
        };
        let front = source_jigsaw.front.step();
        let target_jigsaw_pos = BlockPos {
            x: source_jigsaw_pos.x + front[0],
            y: source_jigsaw_pos.y + front[1],
            z: source_jigsaw_pos.z + front[2],
        };
        let source_local_y = source_jigsaw_pos.y - source.bounds_min.y;
        let Some(target_pool) = closure.pool(&source_jigsaw.pool) else {
            continue;
        };
        let attach_inside = source.bounds_min.x <= target_jigsaw_pos.x
            && target_jigsaw_pos.x <= source.bounds_max.x
            && source.bounds_min.y <= target_jigsaw_pos.y
            && target_jigsaw_pos.y <= source.bounds_max.y
            && source.bounds_min.z <= target_jigsaw_pos.z
            && target_jigsaw_pos.z <= source.bounds_max.z;

        let mut candidates = if source.depth != structure.size {
            flat_elements(target_pool)
        } else {
            Vec::new()
        };
        if let Some(fallback) = closure.pool(&target_pool.fallback) {
            candidates.extend(flat_elements(fallback));
        }

        'candidates: for candidate in candidates {
            let PlacedKind::Single {
                piece,
                owner,
                projection: target_projection,
                processors: target_processors,
                legacy: target_legacy,
            } = &candidate
            else {
                if matches!(candidate, PlacedKind::Empty) {
                    break;
                }
                continue;
            };
            let Some(target_template) = closure.piece(piece) else {
                continue;
            };
            for target_rotation in Rotation::get_shuffled(random) {
                let target_jigsaws = shuffled_jigsaws(target_template, random);
                let Some(_hack_bounds) = piece_bounds(
                    target_template,
                    BlockPos { x: 0, y: 0, z: 0 },
                    target_rotation,
                ) else {
                    continue;
                };
                for target_jigsaw in &target_jigsaws {
                    if !can_attach(&source_jigsaw, target_jigsaw) {
                        continue;
                    }
                    let target_local = target_rotation.transform(target_jigsaw.pos);
                    let raw_position = BlockPos {
                        x: target_jigsaw_pos.x - target_local[0],
                        y: target_jigsaw_pos.y - target_local[1],
                        z: target_jigsaw_pos.z - target_local[2],
                    };
                    let Some((raw_min, _)) =
                        piece_bounds(target_template, raw_position, target_rotation)
                    else {
                        continue;
                    };
                    let target_rigid = *target_projection == Projection::Rigid;
                    let delta_y = source_local_y - target_jigsaw.pos[1] + front[1];
                    let target_box_y = if source_rigid && target_rigid {
                        source.bounds_min.y + delta_y
                    } else {
                        if source_jigsaw_base_height == i32::MIN {
                            source_jigsaw_base_height =
                                free_height(source_jigsaw_pos.x, source_jigsaw_pos.z);
                        }
                        source_jigsaw_base_height - target_jigsaw.pos[1]
                    };
                    let y_offset = target_box_y - raw_min.y;
                    let target_position = BlockPos {
                        x: raw_position.x,
                        y: raw_position.y + y_offset,
                        z: raw_position.z,
                    };
                    let Some((target_min, target_max)) =
                        piece_bounds(target_template, target_position, target_rotation)
                    else {
                        continue;
                    };
                    let target_box = Aabb::of(
                        target_min.x,
                        target_min.y,
                        target_min.z,
                        target_max.x + 1,
                        target_max.y + 1,
                        target_max.z + 1,
                    );
                    let children_free: VoxelShape = if attach_inside {
                        source_free
                            .get_or_insert_with(|| {
                                VoxelShape::create(Aabb::of(
                                    source.bounds_min.x,
                                    source.bounds_min.y,
                                    source.bounds_min.z,
                                    source.bounds_max.x + 1,
                                    source.bounds_max.y + 1,
                                    source.bounds_max.z + 1,
                                ))
                            })
                            .clone()
                    } else {
                        free.clone()
                    };
                    let is_shared_free = !attach_inside;
                    if VoxelShape::join_is_not_empty(
                        &children_free,
                        &VoxelShape::create(target_box.deflate(0.25)),
                        BooleanOp::OnlySecond,
                    ) {
                        continue;
                    }
                    let grown = children_free
                        .join_unoptimized(&VoxelShape::create(target_box), BooleanOp::OnlyFirst);
                    let target_ground_level_delta = if target_rigid {
                        source.ground_level_delta - delta_y
                    } else {
                        0
                    };
                    let target_piece = PlacedPiece {
                        element: piece.clone(),
                        owner: owner.clone(),
                        processors: target_processors.clone(),
                        legacy: *target_legacy,
                        projection: *target_projection,
                        position: target_position,
                        rotation: target_rotation,
                        ground_level_delta: target_ground_level_delta,
                        bounds_min: target_min,
                        bounds_max: target_max,
                        depth: source.depth + 1,
                    };
                    let junction_y = if source_rigid {
                        source.bounds_min.y + source_local_y
                    } else if target_rigid {
                        target_box_y + target_jigsaw.pos[1]
                    } else {
                        if source_jigsaw_base_height == i32::MIN {
                            source_jigsaw_base_height =
                                free_height(source_jigsaw_pos.x, source_jigsaw_pos.z);
                        }
                        source_jigsaw_base_height + delta_y / 2
                    };
                    assembly.junctions.push(Junction {
                        source: target_jigsaw_pos,
                        ground_y: junction_y - source_local_y + source.ground_level_delta,
                        delta_y,
                        projection: *target_projection,
                    });
                    assembly.junctions.push(Junction {
                        source: source_jigsaw_pos,
                        ground_y: junction_y - target_jigsaw.pos[1] + target_ground_level_delta,
                        delta_y: -delta_y,
                        projection: source.projection,
                    });
                    if target_rigid {
                        assembly.beard_pieces.push(BeardContribution {
                            min: target_min,
                            max: target_max,
                            ground_level_delta: target_ground_level_delta,
                        });
                    }
                    if is_shared_free {
                        *free = grown.clone();
                    }
                    assembly.pieces.push(target_piece.clone());
                    if source.depth < structure.size {
                        queue.push_back((target_piece, grown, source.depth + 1, 0));
                    }
                    continue 'candidates;
                }
            }
        }
    }
}

/// `JigsawBlock.canAttach`.
#[must_use]
pub fn can_attach(
    source: &crate::structures::TemplateJigsaw,
    target: &crate::structures::TemplateJigsaw,
) -> bool {
    let source_front = source.front.step();
    let target_front = target.front.step();
    let opposed = source_front == [-target_front[0], -target_front[1], -target_front[2]];
    let joint_ok = source.joint == crate::structures::Joint::Rollable || source.top == target.top;
    opposed && joint_ok && source.target == target.name
}

fn biomes_tag(structure: &StructureSpec) -> Identifier {
    match &structure.biomes {
        mc_data::village_data::BiomeSet::Tag(tag) => tag.clone(),
        mc_data::village_data::BiomeSet::Biomes(biomes) => biomes
            .first()
            .cloned()
            .unwrap_or_else(|| Identifier::parse("minecraft:plains".to_owned()).expect("valid")),
    }
}

fn start_height(structure: &StructureSpec) -> i32 {
    match &structure.start_height {
        mc_data::village_data::HeightProviderSpec::Constant(
            mc_data::village_data::VerticalAnchor::Absolute(value),
        ) => *value,
        _ => 0,
    }
}

/// The weighted structure roll: vanilla expands the set's weights into a flat
/// list and draws once with the placement's per-chunk random, then the
/// structure's biome gate decides whether it actually starts.
#[must_use]
pub fn pick_structure(
    placement: RandomSpreadPlacement,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    structures: &[WeightedStructure],
) -> Option<usize> {
    let mut flat: Vec<usize> = Vec::new();
    for (index, structure) in structures.iter().enumerate() {
        for _ in 0..structure.weight.max(1) {
            flat.push(index);
        }
    }
    if flat.is_empty() {
        return None;
    }
    let mut random = LegacyRandom::new(placement_seed(seed, chunk_x, chunk_z, placement.salt));
    let draw = random.next_int_bounded(i32::try_from(flat.len()).ok()?);
    Some(flat[draw as usize])
}

/// The structure a chunk's weighted roll picks, before the biome gate.
#[must_use]
pub fn rolled_structure(
    closure: &VillageClosure,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
) -> Option<Identifier> {
    let weighted: Vec<WeightedStructure> = closure
        .structures
        .iter()
        .map(|structure| WeightedStructure {
            id: structure.id.clone(),
            weight: structure.weight,
            biomes: biomes_tag(&structure.spec),
            start_height: start_height(&structure.spec),
        })
        .collect();
    let index = pick_structure(closure.placement, seed, chunk_x, chunk_z, &weighted)?;
    Some(closure.structures.get(index)?.id.clone())
}

/// The random source the placement itself runs on
/// (`WorldgenRandom.setLargeFeatureWithSalt`).
#[must_use]
pub fn placement_seed(seed: i64, chunk_x: i32, chunk_z: i32, salt: i64) -> i64 {
    super::placement::set_large_feature_with_salt(seed, chunk_x, chunk_z, salt)
}

/// The elements a `feature_pool_element` contributes, for the caller to run
/// through the A1/A2 executor.
#[must_use]
pub fn feature_elements(pool: &ClosurePool) -> Vec<(u32, Identifier)> {
    pool.elements
        .iter()
        .filter_map(|(weight, element)| match element {
            ClosureElement::Feature { placed_feature, .. } => {
                Some((*weight, placed_feature.clone()))
            }
            _ => None,
        })
        .collect()
}

/// A pool element type that placement does not produce on its own.
#[must_use]
pub fn element_projection(pool: &ClosurePool, index: usize) -> Option<Projection> {
    pool.elements
        .get(index)
        .and_then(|(_, element)| match element {
            ClosureElement::Single { projection, .. }
            | ClosureElement::Feature { projection, .. } => Some(*projection),
            ClosureElement::Empty => None,
        })
}

/// `PoolElementSpec` is what the data side parses; the engine works on
/// [`ClosureElement`], so this is the one place the two meet.
#[must_use]
pub fn closure_element_kind(element: &PoolElementSpec) -> &'static str {
    match element {
        PoolElementSpec::Empty => "empty",
        PoolElementSpec::Single(_) => "single",
        PoolElementSpec::LegacySingle(_) => "legacy_single",
        PoolElementSpec::List { .. } => "list",
        PoolElementSpec::Feature { .. } => "feature",
    }
}
