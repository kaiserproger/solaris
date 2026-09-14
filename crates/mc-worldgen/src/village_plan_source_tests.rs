//! Plan-source tests: the one village plan lookup, the two terrain-pipeline
//! consumers it feeds, and the byte-identical unplanned path.
//!
//! Two layers. The synthetic layer builds a one-piece village closure by hand,
//! so the adapter's own behaviour — the plans it returns for a chunk, the piece
//! blocks it writes, the region it reports, the analogue it applies, and the
//! generator paths that must not change when no plan is configured — is covered
//! without any content cache. The live layer runs the real `minecraft:villages`
//! closure from a content cache (`/tmp/jdk-cold2` when present, else
//! `SOLARIS_CONTENT_CACHE` or `<workspace>/data/vanilla`) and skips loudly,
//! never silently, when none is available.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::village_data::{
    BiomeSet, DimensionPadding, HeightProviderSpec, HeightmapType, LiquidSettings, MaxDistance,
    ProcessorRef, Projection, StructureSpec, StructureStep, TerrainAdaptation, VerticalAnchor,
};
use mc_world::chunk::OVERWORLD_GEOMETRY;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkPos};

use crate::structures::{StructureTemplate, TemplateBlock};
use crate::terrain::TerrainGenerator;
use crate::vanilla_features::{BiomeTagIndex, BlockTagIndex};
use crate::village::beard::{BEARD_KERNEL_RADIUS, apply_columns_over};
use crate::village::closure::{ClosureElement, ClosurePool, ClosureStructure, VillageClosure};
use crate::village::load_village_closure;
use crate::village::placement::RandomSpreadPlacement;
use crate::village::plan_source::{NEIGHBOURHOOD_CHUNK_RADIUS, VillagePlanSet, VillagePlanSource};

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

/// The block tags the synthetic village's processors test against. `false` for
/// everything: the synthetic closure reaches no tag-based rule test, and the
/// real tag index belongs to the activation change.
struct NoTags;

impl BlockTagIndex for NoTags {
    fn block_in_tag(&self, _tag: &Identifier, _block: &Identifier) -> bool {
        false
    }
}

/// The biome tag contents the synthetic closure's structures gate on: every
/// biome is a member of every tag, so the gate accepts the structure and the
/// tests below are about the plan, not about biome data.
struct AllBiomes;

impl BiomeTagIndex for AllBiomes {
    fn biome_in_tag(&self, _tag: &Identifier, _biome: &Identifier) -> bool {
        true
    }
}

fn required_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block report"),
    )
}

/// A block the terrain, caves and decoration never produce, so a chunk holding
/// it can only have got it from a village piece.
fn marker_state(blocks: &BlockRegistry) -> BlockStateId {
    blocks
        .block(&identifier("minecraft:diamond_block"))
        .expect("the required registry carries the marker block")
        .default
}

/// A one-piece village: one structure whose start pool holds a single rigid
/// piece and whose size is zero, so the solver places the start piece and stops.
/// Everything the adapter does with a plan is reachable from it.
fn synthetic_source(seed: i64) -> (Arc<VillagePlanSource>, Arc<BlockRegistry>) {
    synthetic_source_with_center(seed, [3, 4, 3])
}

/// The same synthetic village with a centre template of `size`: a template wide
/// enough reaches into the next `random_spread` candidate's chunks, which is how
/// the multi-plan test below produces two villages over one chunk.
fn synthetic_source_with_center(
    seed: i64,
    size: [i32; 3],
) -> (Arc<VillagePlanSource>, Arc<BlockRegistry>) {
    // The real set's spacing (34/8) can put two candidates `separation + 1`
    // chunks apart; the multi-plan test compresses the grid so that case is
    // reachable with a small template.
    synthetic_source_with_grid(
        seed,
        size,
        RandomSpreadPlacement {
            spacing: 34,
            separation: 8,
            salt: 10_387_312,
            triangular: false,
        },
    )
}

/// The synthetic village over a chosen placement grid.
fn synthetic_source_with_grid(
    seed: i64,
    size: [i32; 3],
    placement: RandomSpreadPlacement,
) -> (Arc<VillagePlanSource>, Arc<BlockRegistry>) {
    let blocks = required_registry();
    let marker = marker_state(&blocks);
    let template_id = identifier("test:village/center");
    let template = StructureTemplate::new(
        size,
        vec![
            TemplateBlock {
                pos: [0, 0, size[2] - 1],
                state: marker,
            },
            TemplateBlock {
                pos: [1, 1, size[2] - 1],
                state: marker,
            },
            TemplateBlock {
                pos: [size[0] - 1, 2, size[2] - 1],
                state: marker,
            },
        ],
    );
    let pool_id = identifier("test:village/centers");
    let processor_id = identifier("test:village/processors");
    let structure_id = identifier("test:village_plains");
    let spec = StructureSpec {
        id: structure_id.clone(),
        structure_type: identifier("minecraft:jigsaw"),
        biomes: BiomeSet::Biomes(vec![identifier("minecraft:plains")]),
        spawn_overrides: Vec::new(),
        step: StructureStep::SurfaceStructures,
        terrain_adaptation: TerrainAdaptation::BeardThin,
        start_pool: pool_id.clone(),
        start_jigsaw_name: None,
        size: 0,
        start_height: HeightProviderSpec::Constant(VerticalAnchor::Absolute(0)),
        use_expansion_hack: true,
        project_start_to_heightmap: Some(HeightmapType::WorldSurfaceWg),
        max_distance_from_center: MaxDistance {
            horizontal: 80,
            vertical: 80,
        },
        dimension_padding: DimensionPadding { bottom: 0, top: 0 },
        liquid_settings: LiquidSettings::ApplyWaterlogging,
    };
    let pool = ClosurePool {
        id: pool_id.clone(),
        fallback: identifier("minecraft:empty"),
        elements: vec![(
            1,
            ClosureElement::Single {
                piece: template_id.clone(),
                projection: Projection::Rigid,
                processors: ProcessorRef::List(processor_id.clone()),
                legacy: true,
            },
        )],
    };
    let closure = VillageClosure {
        structure_set: identifier("test:villages"),
        placement,
        structures: vec![ClosureStructure {
            id: structure_id,
            weight: 1,
            spec,
        }],
        pools: BTreeMap::from([(pool_id, pool)]),
        processor_lists: BTreeMap::from([(processor_id, Vec::new())]),
        pieces: BTreeMap::from([(template_id, template)]),
        placed_features: Vec::new(),
    };
    let source = VillagePlanSource::new(
        Arc::new(closure),
        seed,
        Arc::clone(&blocks),
        Arc::new(NoTags),
        Arc::new(AllBiomes),
    )
    .expect("the synthetic closure places against the required registry");
    (Arc::new(source), blocks)
}

/// The first chunk the placement formula starts a structure in, scanning a
/// whole `spacing` grid from the origin.
fn placement_chunk(source: &VillagePlanSource) -> (i32, i32) {
    placement_chunks(source, 1)
        .into_iter()
        .next()
        .expect("the placement formula starts a structure somewhere in a full grid")
}

/// The placement chunks of the first `cells` grid cells, in a fixed order.
fn placement_chunks(source: &VillagePlanSource, cells: i32) -> Vec<(i32, i32)> {
    let placement = source.closure().placement;
    let mut chunks = Vec::new();
    for grid_z in 0..cells {
        for grid_x in 0..cells {
            let candidate = placement.potential_chunk(
                source.seed(),
                grid_x * placement.spacing,
                grid_z * placement.spacing,
            );
            if placement.is_placement_chunk(source.seed(), candidate.0, candidate.1) {
                chunks.push(candidate);
            }
        }
    }
    chunks
}

fn synthetic_generator(
    seed: i64,
    blocks: &Arc<BlockRegistry>,
    source: Option<&Arc<VillagePlanSource>>,
) -> TerrainGenerator {
    let mut generator = TerrainGenerator::new(seed, Arc::clone(blocks));
    if let Some(source) = source {
        generator = generator.with_village_plans(Arc::clone(source));
    }
    generator
}

/// The lookup the generator performs for a chunk, from an unplanned generator's
/// own answers: the router's surface as `WORLD_SURFACE_WG` first-free height and
/// the router's biome as the gate's biome.
fn plan_set(
    source: &VillagePlanSource,
    plain: &TerrainGenerator,
    chunk: (i32, i32),
) -> Option<VillagePlanSet> {
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);
    source.plans_for_chunk(chunk.0, chunk.1, &free_height, &biome_at)
}

/// Every block of a chunk, in `(y, lz, lx)` order: `Chunk` carries no
/// `PartialEq`, and this is what "byte-identical" means here.
fn chunk_blocks(chunk: &Chunk) -> Vec<BlockStateId> {
    let geometry = chunk.geometry();
    let mut blocks = Vec::new();
    for y in geometry.min_y()..geometry.max_y() {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                blocks.push(chunk.get_block(lx, y, lz).expect("inside the chunk"));
            }
        }
    }
    blocks
}

fn chunk_heightmaps(chunk: &Chunk) -> (Vec<u32>, Vec<u32>) {
    let blocking = chunk
        .heightmaps
        .get("MOTION_BLOCKING")
        .expect("the generator writes MOTION_BLOCKING");
    let surface = chunk
        .heightmaps
        .get("WORLD_SURFACE")
        .expect("the generator writes WORLD_SURFACE");
    let mut blocking_values = Vec::new();
    let mut surface_values = Vec::new();
    for lz in 0..16u8 {
        for lx in 0..16u8 {
            blocking_values.push(blocking.get(lx, lz));
            surface_values.push(surface.get(lx, lz));
        }
    }
    (blocking_values, surface_values)
}

/// The blocks that differ between two chunks, with each world position, inside
/// the plan's region check applied by the caller.
fn chunk_differences(
    planned: &Chunk,
    plain: &Chunk,
) -> Vec<(BlockPos, BlockStateId, BlockStateId)> {
    let geometry = planned.geometry();
    let mut differences = Vec::new();
    for y in geometry.min_y()..geometry.max_y() {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let before = plain.get_block(lx, y, lz).expect("inside the chunk");
                let after = planned.get_block(lx, y, lz).expect("inside the chunk");
                if before != after {
                    differences.push((
                        BlockPos {
                            x: planned.pos.x * 16 + i32::from(lx),
                            y,
                            z: planned.pos.z * 16 + i32::from(lz),
                        },
                        before,
                        after,
                    ));
                }
            }
        }
    }
    differences
}

fn inside_region(min: BlockPos, max: BlockPos, pos: BlockPos) -> bool {
    (min.x..=max.x).contains(&pos.x)
        && (min.y..=max.y).contains(&pos.y)
        && (min.z..=max.z).contains(&pos.z)
}

/// The plan lookup answers for the chunk the placement formula started a
/// village in, reports that start chunk, and reaches exactly the region its
/// piece occupies, inflated by the beard kernel the analogue reaches through.
#[test]
fn synthetic_plan_is_found_for_its_placement_chunk() {
    let seed = 7;
    let (source, blocks) = synthetic_source(seed);
    let (chunk_x, chunk_z) = placement_chunk(&source);
    let plain = synthetic_generator(seed, &blocks, None);
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);

    let set = plan_set(&source, &plain, (chunk_x, chunk_z))
        .expect("the placement chunk starts the synthetic village");
    assert_eq!(set.plans().len(), 1);
    let plan = &set.plans()[0];
    assert_eq!(plan.start_chunk(), (chunk_x, chunk_z));
    assert_eq!(plan.assembly().chunk, (chunk_x, chunk_z));
    assert_eq!(plan.pieces().len(), 1);
    assert_eq!(plan.pieces()[0].template, identifier("test:village/center"));

    // The region is the rigid start piece's own box, inflated by the kernel
    // radius the analogue reaches through: the start piece is a RIGID piece, so
    // vanilla's Beardifier collects it like any other.
    let piece = &plan.assembly().pieces[0];
    let expected_min = BlockPos {
        x: piece.bounds_min.x - BEARD_KERNEL_RADIUS,
        y: piece.bounds_min.y - BEARD_KERNEL_RADIUS,
        z: piece.bounds_min.z - BEARD_KERNEL_RADIUS,
    };
    let expected_max = BlockPos {
        x: piece.bounds_max.x + BEARD_KERNEL_RADIUS,
        y: piece.bounds_max.y + BEARD_KERNEL_RADIUS,
        z: piece.bounds_max.z + BEARD_KERNEL_RADIUS,
    };
    assert_eq!(set.affected_box(), (expected_min, expected_max));
    assert_eq!(plan.affected_box(), (expected_min, expected_max));
    let (min, max) = set.affected_box();
    let (centre_x, centre_z) = ((min.x + max.x) / 2, (min.z + max.z) / 2);
    assert!(plan.affects_column(centre_x, centre_z));
    assert!(plan.affects_chunk(chunk_x, chunk_z));
    assert_eq!(
        source
            .plans_for_column(centre_x, centre_z, &free_height, &biome_at)
            .map(|set| set.plans()[0].start_chunk()),
        Some((chunk_x, chunk_z)),
    );
    assert!(
        source
            .plans_for_column(min.x - 1, centre_z, &free_height, &biome_at)
            .is_none()
    );

    // A chunk past the neighbourhood has no plan, and the column surfaces there
    // are the router's: the plan source changes nothing it does not reach.
    let far = (chunk_x + NEIGHBOURHOOD_CHUNK_RADIUS + 4, chunk_z);
    assert!(
        source
            .plans_for_chunk(far.0, far.1, &free_height, &biome_at)
            .is_none()
    );

    let planned = synthetic_generator(seed, &blocks, Some(&source));
    let far_pos = ChunkPos { x: far.0, z: far.1 };
    assert_eq!(
        chunk_blocks(&planned.generate(far_pos)),
        chunk_blocks(&plain.generate(far_pos)),
    );
    for lx in 0..16i32 {
        for lz in 0..16i32 {
            let (world_x, world_z) = (far.0 * 16 + lx, far.1 * 16 + lz);
            assert_eq!(
                planned.surface_height(world_x, world_z),
                plain.surface_height(world_x, world_z),
            );
        }
    }
}

/// Two villages can reach one chunk: `random_spread` candidates in neighbouring
/// grid cells can be as little as `separation + 1` chunks apart while a plan
/// reaches up to `NEIGHBOURHOOD_CHUNK_RADIUS` chunks past its start, so a chunk
/// in that gap holds part of both. The regression is that *no* reaching plan is
/// dropped: the lookup returns every one of them, in ascending start-chunk
/// order, each is placed, and the analogue reads all of them.
#[test]
fn every_plan_reaching_a_chunk_is_returned_and_placed() {
    let seed = 7;
    // A grid with no spread, so every candidate sits exactly `spacing` chunks
    // from the next and the set of villages over one chunk is a property of the
    // seed rather than of the enumeration.
    let (source, blocks) = synthetic_source_with_grid(
        seed,
        [40, 4, 40],
        RandomSpreadPlacement {
            spacing: 3,
            separation: 2,
            salt: 10_387_312,
            triangular: false,
        },
    );
    let placement = source.closure().placement;
    let plain = synthetic_generator(seed, &blocks, None);
    // A chunk that is no village's start chunk, in the gap between candidates.
    let overlap = (5, 0);
    let set = plan_set(&source, &plain, overlap).expect("the overlap chunk is reached");
    let starts: Vec<(i32, i32)> = set.plans().iter().map(|plan| plan.start_chunk()).collect();

    // The expectation is derived, not tuned: every candidate start chunk in the
    // neighbourhood whose own plan's region reaches the overlap chunk must be in
    // the set, and nothing else.
    let mut expected = Vec::new();
    for offset_z in -NEIGHBOURHOOD_CHUNK_RADIUS..=NEIGHBOURHOOD_CHUNK_RADIUS {
        for offset_x in -NEIGHBOURHOOD_CHUNK_RADIUS..=NEIGHBOURHOOD_CHUNK_RADIUS {
            let candidate = (overlap.0 + offset_x, overlap.1 + offset_z);
            if !placement.is_placement_chunk(seed, candidate.0, candidate.1) {
                continue;
            }
            let own = plan_set(&source, &plain, candidate).expect("its own start chunk is reached");
            let plan = own
                .plans()
                .iter()
                .find(|plan| plan.start_chunk() == candidate)
                .expect("a placement chunk's own plan is in its own set");
            if plan.affects_chunk(overlap.0, overlap.1) {
                expected.push(candidate);
            }
        }
    }
    expected.sort_unstable();
    assert_eq!(
        starts, expected,
        "every plan whose region reaches the chunk must be returned",
    );
    assert!(
        starts.len() > 1,
        "the fixture must exercise several villages over one chunk, got {starts:?}",
    );
    assert_eq!(
        set.plans()
            .iter()
            .map(|plan| plan.start_chunk())
            .collect::<Vec<_>>(),
        {
            let mut sorted = starts.clone();
            sorted.sort_unstable();
            sorted
        },
        "the set is in ascending start-chunk order",
    );

    // Every plan in the set is placed: each one's own template block lands in the
    // chunk that holds it, whatever chunk that is.
    let generator = synthetic_generator(seed, &blocks, Some(&source));
    let marker = marker_state(&blocks);
    for plan in set.plans() {
        let template = source
            .closure()
            .piece(&plan.pieces()[0].template)
            .expect("the plan's template is in the closure");
        let piece = &plan.pieces()[0];
        let world = template.blocks().iter().map(|block| {
            let local = piece.rotation.transform(block.pos);
            BlockPos {
                x: piece.position.x + local[0],
                y: piece.position.y + local[1],
                z: piece.position.z + local[2],
            }
        });
        let mut witnessed = 0usize;
        for position in world {
            let chunk = generator.generate(ChunkPos {
                x: position.x.div_euclid(16),
                z: position.z.div_euclid(16),
            });
            witnessed += usize::from(
                chunk.get_block(
                    position.x.rem_euclid(16) as u8,
                    position.y,
                    position.z.rem_euclid(16) as u8,
                ) == Some(marker),
            );
        }
        assert_eq!(
            witnessed,
            3,
            "the village started at {:?} is in the set and every one of its template \
             blocks must be placed",
            plan.start_chunk(),
        );
    }

    // The analogue reads every plan: at least one column of the overlap chunk is
    // moved by a village other than the first, so the set's surface is not the
    // first plan's surface.
    let geometry = OVERWORLD_GEOMETRY;
    let (min_y, max_y) = (geometry.min_y(), geometry.max_y());
    let mut changed_by_another = 0usize;
    let mut columns = Vec::new();
    for world_z in overlap.1 * 16..overlap.1 * 16 + 16 {
        for world_x in overlap.0 * 16..overlap.0 * 16 + 16 {
            let base = plain.surface_height(world_x, world_z);
            let set_y = set.adjusted_surface_y(world_x, world_z, base, min_y, max_y);
            let first_y = apply_columns_over(
                &[set.plans()[0].beard_source()],
                &[(world_x, world_z, base)],
                min_y,
                max_y,
            )
            .first()
            .map_or(base, |column| column.adjusted_y);
            if set_y != first_y {
                changed_by_another += 1;
            }
            columns.push((world_x, world_z, base));
        }
    }
    assert!(
        changed_by_another > 0,
        "another village must change columns of the chunk they share",
    );

    // The many-column path is the same function as the single-column path.
    let moved = set.adjusted_columns(&columns, min_y, max_y);
    let moved_by_column: BTreeMap<(i32, i32), i32> = moved
        .iter()
        .map(|column| ((column.x, column.z), column.adjusted_y))
        .collect();
    for (world_x, world_z, base) in &columns {
        assert_eq!(
            set.adjusted_surface_y(*world_x, *world_z, *base, min_y, max_y),
            moved_by_column
                .get(&(*world_x, *world_z))
                .copied()
                .unwrap_or(*base),
        );
    }
}

/// A planned chunk writes the plan's piece blocks at the plan's own transform,
/// including a piece that spans a chunk boundary; a chunk with no plan is
/// byte-identical — blocks, heightmaps and every column's surface — to the same
/// generator built without a plan source.
#[test]
fn planned_chunk_writes_plan_blocks_and_unplanned_chunk_is_byte_identical() {
    let seed = 7;
    let (source, blocks) = synthetic_source(seed);
    let (chunk_x, chunk_z) = placement_chunk(&source);
    let plain = synthetic_generator(seed, &blocks, None);
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);
    let set = source
        .plans_for_chunk(chunk_x, chunk_z, &free_height, &biome_at)
        .expect("the placement chunk starts the synthetic village");

    let planned = synthetic_generator(seed, &blocks, Some(&source));
    assert!(planned.village_plan_source().is_some());
    assert!(plain.village_plan_source().is_none());

    // The marker block is the plan's own: the terrain, caves and decoration
    // never produce it, so every marker in the chunks the piece reaches sits at
    // the plan's transform of a template block (`PlacedPiece.position` plus the
    // rotation), and the unplanned world has none of them.
    let marker = marker_state(&blocks);
    let plan = &set.plans()[0];
    let piece = &plan.pieces()[0];
    let template = source
        .closure()
        .piece(&piece.template)
        .expect("the plan's template is in the closure");
    let mut expected: Vec<BlockPos> = template
        .blocks()
        .iter()
        .map(|block| {
            let local = piece.rotation.transform(block.pos);
            BlockPos {
                x: piece.position.x + local[0],
                y: piece.position.y + local[1],
                z: piece.position.z + local[2],
            }
        })
        .collect();
    expected.sort_by_key(|world| (world.y, world.z, world.x));
    assert_eq!(
        expected.len(),
        3,
        "every template block has a world position"
    );

    // The piece is anchored on the chunk corner, so its box and its blocks can
    // straddle the chunk boundary: generate every chunk the blocks land in, as
    // a world would, and collect what each of them wrote.
    let piece_chunks: BTreeSet<(i32, i32)> = expected
        .iter()
        .map(|world| (world.x.div_euclid(16), world.z.div_euclid(16)))
        .collect();
    let mut witnessed = Vec::new();
    let mut generated = BTreeMap::new();
    for (piece_chunk_x, piece_chunk_z) in &piece_chunks {
        let pos = ChunkPos {
            x: *piece_chunk_x,
            z: *piece_chunk_z,
        };
        let planned_chunk = planned.generate(pos);
        let plain_chunk = plain.generate(pos);
        assert!(
            !chunk_differences(&planned_chunk, &plain_chunk).is_empty(),
            "chunk {pos:?} holds part of the piece and must carry its blocks",
        );
        for y in planned_chunk.geometry().min_y()..planned_chunk.geometry().max_y() {
            for lz in 0..16u8 {
                for lx in 0..16u8 {
                    if planned_chunk.get_block(lx, y, lz) == Some(marker) {
                        let world = BlockPos {
                            x: piece_chunk_x * 16 + i32::from(lx),
                            y,
                            z: piece_chunk_z * 16 + i32::from(lz),
                        };
                        assert!(
                            expected.contains(&world),
                            "marker at {world:?} is not one of the plan's template positions",
                        );
                        witnessed.push(world);
                    }
                    assert_ne!(
                        plain_chunk.get_block(lx, y, lz),
                        Some(marker),
                        "the unplanned world carries no marker block",
                    );
                }
            }
        }
        generated.insert(
            (*piece_chunk_x, *piece_chunk_z),
            chunk_blocks(&planned_chunk),
        );
    }
    witnessed.sort_by_key(|world| (world.y, world.z, world.x));
    assert_eq!(
        witnessed, expected,
        "every template block is written into the chunks that hold it",
    );

    // The same generator re-run produces the same chunks: the plan lookup is a
    // pure function of the chunk, so a replay cannot drift.
    for ((chunk_x, chunk_z), blocks_written) in &generated {
        assert_eq!(
            chunk_blocks(&planned.generate(ChunkPos {
                x: *chunk_x,
                z: *chunk_z
            })),
            *blocks_written,
        );
    }

    // A chunk with no plan: byte-identical blocks and heightmaps, and every
    // column's surface equal to the unplanned generator's.
    let far = ChunkPos {
        x: chunk_x + NEIGHBOURHOOD_CHUNK_RADIUS + 4,
        z: chunk_z,
    };
    assert!(
        source
            .plans_for_chunk(far.x, far.z, &free_height, &biome_at)
            .is_none()
    );
    let planned_far = planned.generate(far);
    let plain_far = plain.generate(far);
    assert_eq!(chunk_blocks(&planned_far), chunk_blocks(&plain_far));
    assert_eq!(chunk_heightmaps(&planned_far), chunk_heightmaps(&plain_far));
    for lx in 0..16i32 {
        for lz in 0..16i32 {
            let (world_x, world_z) = (far.x * 16 + lx, far.z * 16 + lz);
            assert_eq!(
                planned.surface_height(world_x, world_z),
                plain.surface_height(world_x, world_z),
                "column ({world_x}, {world_z}) outside every plan must be untouched",
            );
        }
    }
}

/// The lookup is an on-demand derivation, not a stored value: the same key
/// yields an equal set every time, and the generator's own chunk lookup reads
/// that same set.
#[test]
fn plan_lookup_is_deterministic_for_a_key() {
    let seed = 7;
    let (source, blocks) = synthetic_source(seed);
    let (chunk_x, chunk_z) = placement_chunk(&source);
    let plain = synthetic_generator(seed, &blocks, None);
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);

    let first = source
        .plans_for_chunk(chunk_x, chunk_z, &free_height, &biome_at)
        .expect("the placement chunk starts the synthetic village");
    let second = source
        .plans_for_chunk(chunk_x, chunk_z, &free_height, &biome_at)
        .expect("the same key yields the same set");
    assert_eq!(first.plans().len(), second.plans().len());
    for (left, right) in first.plans().iter().zip(second.plans()) {
        assert_eq!(left.assembly(), right.assembly());
        assert_eq!(left.pieces(), right.pieces());
        assert_eq!(left.affected_box(), right.affected_box());
        assert_eq!(left.junctions(), right.junctions());
    }
    assert_eq!(first.affected_box(), second.affected_box());

    // The generator configured with this source plans its chunk against the
    // same set: the column it reports inside the region is the set's own
    // adjusted surface, and outside the region it is the router's.
    let generator = synthetic_generator(seed, &blocks, Some(&source));
    let (min, max) = first.affected_box();
    let geometry = OVERWORLD_GEOMETRY;
    let mut columns = Vec::new();
    for world_z in min.z..=max.z {
        for world_x in min.x..=max.x {
            columns.push((world_x, world_z, plain.surface_height(world_x, world_z)));
        }
    }
    let moved = first.adjusted_columns(&columns, geometry.min_y(), geometry.max_y());
    let moved_by_column: BTreeMap<(i32, i32), i32> = moved
        .iter()
        .map(|column| ((column.x, column.z), column.adjusted_y))
        .collect();
    for (world_x, world_z, base) in &columns {
        let from_set = first.adjusted_surface_y(
            *world_x,
            *world_z,
            *base,
            geometry.min_y(),
            geometry.max_y(),
        );
        assert_eq!(
            from_set,
            moved_by_column
                .get(&(*world_x, *world_z))
                .copied()
                .unwrap_or(*base),
        );
        assert_eq!(generator.surface_height(*world_x, *world_z), from_set);
    }
}

/// `<SOLARIS_CONTENT_CACHE>`, then `/tmp/jdk-cold2`, then
/// `<workspace>/data/vanilla`.
fn content_cache() -> Option<PathBuf> {
    content_cache_among([
        std::env::var("SOLARIS_CONTENT_CACHE")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_default(),
        PathBuf::from("/tmp/jdk-cold2"),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(|workspace| workspace.join("data").join("vanilla"))
            .unwrap_or_default(),
    ])
}

/// The first candidate that actually holds a vanilla content cache. A directory
/// without one selects nothing, which is what makes the live tests skip loudly
/// instead of running against an empty cache.
fn content_cache_among(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|dir| dir.join("data").join("minecraft").join("worldgen").is_dir())
}

/// The live tests' skip condition: a directory without a vanilla content cache
/// selects nothing, so the absence is reported rather than silently passing.
#[test]
fn content_cache_selection_ignores_directories_without_a_cache() {
    let empty = tempfile::tempdir().expect("a temporary directory");
    assert!(
        content_cache_among([empty.path().to_path_buf()]).is_none(),
        "a directory with no data/minecraft/worldgen is not a content cache",
    );

    let root = empty.path().join("data").join("minecraft").join("worldgen");
    std::fs::create_dir_all(&root).expect("creating the cache layout");
    assert_eq!(
        content_cache_among([empty.path().to_path_buf()]),
        Some(empty.path().to_path_buf()),
    );
    // Precedence is the candidate order, so the env var wins over the
    // machine-wide cache path when both hold one.
    assert_eq!(
        content_cache_among([
            std::path::PathBuf::from("/definitely/not/a/cache"),
            empty.path().to_path_buf(),
        ]),
        Some(empty.path().to_path_buf()),
    );
}

/// The real `minecraft:villages` closure and a source that gates only on the
/// placement formula (every structure's biome tag is accepted), so the search
/// finds the first placement chunk that actually places a village.
fn live_source(cache: &Path, seed: i64) -> Option<(Arc<VillagePlanSource>, Arc<BlockRegistry>)> {
    let blocks = required_registry();
    let closure = load_village_closure(cache, &identifier("minecraft:villages"), &blocks).ok()?;
    let source = VillagePlanSource::new(
        Arc::new(closure),
        seed,
        Arc::clone(&blocks),
        Arc::new(NoTags),
        Arc::new(AllBiomes),
    )
    .ok()?;
    Some((Arc::new(source), blocks))
}

/// The plan the generator would read for the chunk at `(chunk_x, chunk_z)`,
/// resolved from the same lookup the generator uses: `plain.surface_height` is
/// the router's own surface, so this is the generator's `WORLD_SURFACE_WG`
/// first-free height.
fn live_plan(
    source: &VillagePlanSource,
    plain: &TerrainGenerator,
    chunk: (i32, i32),
) -> Option<VillagePlanSet> {
    plan_set(source, plain, chunk)
}

/// Live proof over the real cache: a placement chunk yields a plan, the plan's
/// own beard columns move, the generator's surfaces inside the region are those
/// moved values, the chunk carries the village's blocks and nothing outside the
/// region, and the region stays inside the hole in the placement grid.
///
/// Loud skip, never a silent pass: without a cache the test prints why it did
/// not run.
#[test]
fn live_village_plan_moves_columns_and_writes_blocks() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP live_village_plan_moves_columns_and_writes_blocks: no vanilla content cache \
             found (set SOLARIS_CONTENT_CACHE, or run with /tmp/jdk-cold2 present)"
        );
        return;
    };
    let seed = 4242;
    let Some((source, blocks)) = live_source(&cache, seed) else {
        println!(
            "SKIP live_village_plan_moves_columns_and_writes_blocks: the village closure did not \
             load from {}",
            cache.display()
        );
        return;
    };
    let plain = synthetic_generator(seed, &blocks, None);
    let generator = synthetic_generator(seed, &blocks, Some(&source));
    let geometry = OVERWORLD_GEOMETRY;

    let mut found = None;
    for chunk in placement_chunks(&source, 8) {
        if let Some(set) = live_plan(&source, &plain, chunk) {
            found = Some((chunk, set));
            break;
        }
    }
    let (chunk, set) = found.expect("a placement chunk places a village in the first 8x8 cells");
    let pos = ChunkPos {
        x: chunk.0,
        z: chunk.1,
    };
    assert_eq!(
        set.plans().len(),
        1,
        "the first placement chunk holds one village"
    );
    let plan = &set.plans()[0];

    let (min, max) = set.affected_box();
    println!(
        "village plan at chunk ({}, {}): {} pieces, {} beard pieces, region {min:?}..{max:?}",
        chunk.0,
        chunk.1,
        plan.pieces().len(),
        plan.assembly().beard_pieces.len(),
    );
    assert_eq!(plan.start_chunk(), chunk);
    assert_eq!(plan.pieces().len(), plan.assembly().pieces.len());

    // The bound the neighbourhood lookup relies on: the region reaches at most
    // NEIGHBOURHOOD_CHUNK_RADIUS chunks past the start chunk it was assembled
    // for. A real plan overrun means the lookup can miss a chunk it should
    // return the plan for.
    let reach = NEIGHBOURHOOD_CHUNK_RADIUS * 16;
    let start_min_x = chunk.0 * 16;
    let start_min_z = chunk.1 * 16;
    assert!(
        start_min_x - min.x <= reach
            && max.x - (start_min_x + 15) <= reach
            && start_min_z - min.z <= reach
            && max.z - (start_min_z + 15) <= reach,
        "plan region {min:?}..{max:?} reaches past the {NEIGHBOURHOOD_CHUNK_RADIUS}-chunk \
         neighbourhood of its start chunk {chunk:?}",
    );

    // Both consumers read the same set: the columns the set's own analogue
    // moves are the surfaces the generator reports, and they differ from the
    // unplanned terrain.
    let mut columns = Vec::new();
    for world_z in start_min_z..start_min_z + 16 {
        for world_x in start_min_x..start_min_x + 16 {
            columns.push((world_x, world_z, plain.surface_height(world_x, world_z)));
        }
    }
    let moved = set.adjusted_columns(&columns, geometry.min_y(), geometry.max_y());
    assert!(
        !moved.is_empty(),
        "the set's beard contributions must move at least one column of the start chunk; \
         {} beard pieces, {} junctions",
        plan.assembly().beard_pieces.len(),
        plan.junctions().len(),
    );
    for column in &moved {
        assert_ne!(column.adjusted_y, column.surface_y);
        assert_eq!(
            generator.surface_height(column.x, column.z),
            column.adjusted_y,
            "the generator's surface query must read the plan the chunk is filled from",
        );
        assert_eq!(
            generator.diagnostic_sample(column.x, column.z).surface_y,
            column.adjusted_y,
            "the diagnostic sample reports the terrain the generator writes",
        );
        assert_ne!(
            generator.surface_height(column.x, column.z),
            plain.surface_height(column.x, column.z),
            "a moved column must differ from the unplanned terrain",
        );
    }

    // The chunk carries the village's blocks, and every changed block is inside
    // the plan's region.
    let planned_chunk = generator.generate(pos);
    let plain_chunk = plain.generate(pos);
    let differences = chunk_differences(&planned_chunk, &plain_chunk);
    assert!(
        !differences.is_empty(),
        "the planned chunk must carry the village's blocks",
    );
    for (position, _, _) in &differences {
        assert!(
            inside_region(min, max, *position),
            "block {position:?} changed outside the plan region {min:?}..{max:?}",
        );
    }
    assert_eq!(
        chunk_blocks(&generator.generate(pos)),
        chunk_blocks(&planned_chunk),
        "the same seed must replay the same chunk",
    );
    assert_eq!(
        chunk_heightmaps(&generator.generate(pos)),
        chunk_heightmaps(&planned_chunk),
    );

    // A real chunk the placement grid leaves without a village is byte-identical
    // to the unplanned world: blocks, heightmaps and every column's surface.
    let far = (12..=40)
        .map(|offset| (chunk.0 + offset, chunk.1))
        .find(|(x, z)| live_plan(&source, &plain, (*x, *z)).is_none())
        .expect("the placement grid leaves a chunk without a village within 40 chunks");
    println!("unplanned chunk {far:?} checked against the unplanned world");
    let far_pos = ChunkPos { x: far.0, z: far.1 };
    let planned_far = generator.generate(far_pos);
    let plain_far = plain.generate(far_pos);
    assert_eq!(chunk_blocks(&planned_far), chunk_blocks(&plain_far));
    assert_eq!(chunk_heightmaps(&planned_far), chunk_heightmaps(&plain_far));
    for lx in 0..16i32 {
        for lz in 0..16i32 {
            let (world_x, world_z) = (far.0 * 16 + lx, far.1 * 16 + lz);
            assert_eq!(
                generator.surface_height(world_x, world_z),
                plain.surface_height(world_x, world_z),
                "column ({world_x}, {world_z}) outside every plan must be untouched",
            );
        }
    }
}

/// The column lookup and the chunk lookup are one lookup: a column inside the
/// plan's region resolves to the same start chunk and the same assembly.
#[test]
fn live_village_plan_lookup_agrees_for_chunk_and_column() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP live_village_plan_lookup_agrees_for_chunk_and_column: no vanilla content cache \
             found (set SOLARIS_CONTENT_CACHE, or run with /tmp/jdk-cold2 present)"
        );
        return;
    };
    let seed = 4242;
    let Some((source, blocks)) = live_source(&cache, seed) else {
        println!(
            "SKIP live_village_plan_lookup_agrees_for_chunk_and_column: the village closure did \
             not load from {}",
            cache.display()
        );
        return;
    };
    let plain = synthetic_generator(seed, &blocks, None);

    let mut found = None;
    for chunk in placement_chunks(&source, 8) {
        if let Some(set) = live_plan(&source, &plain, chunk) {
            found = Some((chunk, set));
            break;
        }
    }
    let (chunk, set) = found.expect("a placement chunk places a village in the first 8x8 cells");
    let plan = &set.plans()[0];
    let (min, max) = set.affected_box();
    let (centre_x, centre_z) = ((min.x + max.x) / 2, (min.z + max.z) / 2);
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);

    let from_column = source
        .plans_for_column(centre_x, centre_z, &free_height, &biome_at)
        .expect("the region's centre resolves to the plan that reaches it");
    assert_eq!(from_column.plans()[0].start_chunk(), chunk);
    assert_eq!(from_column.plans()[0].assembly(), plan.assembly());
    assert_eq!(from_column.plans()[0].pieces(), plan.pieces());

    // Re-deriving the plan for the same chunk yields the same village: the
    // lookup has no state to drift.
    let again = live_plan(&source, &plain, chunk).expect("the lookup is deterministic");
    assert_eq!(again.plans()[0].assembly(), plan.assembly());
    assert_eq!(again.plans()[0].pieces(), plan.pieces());
}
