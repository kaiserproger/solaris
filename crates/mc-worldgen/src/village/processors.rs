//! Vanilla structure-processor execution for village piece templates.
//!
//! `StructureTemplate.processBlockInfos` is the pipeline a jigsaw piece goes
//! through: every template block is transformed into a world position, then
//! handed to each processor of the piece's `StructurePlaceSettings` in order
//! (a processor returning `null` removes the block), and finally each processor
//! gets a `finalizeProcessing` pass. This module owns the middle step — the
//! per-processor algebra — for the processor types the village closure reaches,
//! transcribed from the 26.1.2 bodies:
//!
//! - [`Processor::Rule`] — vanilla `RuleProcessor`, the only processor the 16
//!   reachable village processor lists use,
//! - [`Processor::BlockAge`] — `BlockAgeProcessor`,
//! - [`Processor::Gravity`] — `GravityProcessor`, which the `terrain_matching`
//!   projection appends,
//! - [`Processor::JigsawReplacement`] — `JigsawReplacementProcessor`,
//! - [`Processor::ProtectedBlocks`] — `ProtectedBlockProcessor`,
//! - [`Processor::BlockIgnore`] — `BlockIgnoreProcessor`, which the piece
//!   settings builders add themselves rather than a pool naming it.
//!
//! ## Randomness
//!
//! Nothing here takes a seed: every draw comes from a `LegacyRandomSource`
//! seeded per position with `Mth.getSeed(pos)` ([`get_seed`]), exactly as
//! vanilla does. `RuleProcessor` seeds `Mth.getSeed(processedBlockInfo.pos())`
//! itself, and `BlockAgeProcessor` calls `settings.getRandom(pos)`, which falls
//! back to the same per-position seed because the jigsaw path never installs a
//! `settings.random`. All arithmetic is `java.util.Random`'s 48-bit LCG, so a
//! faithful run needs no world seed at all — only the block positions.
//!
//! ## Predicate order and short-circuiting
//!
//! `ProcessorRule.test` is `input && location && position`, and Java's `&&`
//! short-circuits, so a rule consumes randomness only from the predicates it
//! actually reaches: `random_block_match` draws one `nextFloat` (and only when
//! the block already matched), and the two linear position predicates draw one
//! `nextFloat` each. Preserving that order is load-bearing — drawing eagerly,
//! or evaluating the predicates in another order, shifts the stream for every
//! later rule and every later position.
//!
//! ## Fidelity
//!
//! The semantics of every implemented processor are pinned against a reference
//! run of the real classes from the bundled 26.1.2 server jar
//! (`net.minecraft.core.VpRef`, kept outside the repository) over fixed block
//! lists; the module's test file asserts the outcomes it printed.
//! Nothing is approximated: a processor type the `mc-data` loader rejects never
//! reaches [`compile_processors`], and the state resolution it does perform
//! fails closed naming the referring entry.

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::BlockStateSpec;
use mc_data::village_data::{
    Axis, HeightmapType, PosRuleTestSpec, ProcessorRuleSpec, Projection, RuleTestSpec,
    StructureProcessorSpec,
};
use mc_world::{BlockPos, BlockStateId};

use crate::vanilla_features::{BlockSemantics, CompileError, LegacyRandom, RandomSource};

/// `Mth.getSeed(BlockPos)`: the per-position seed every random processor draws
/// from.
///
/// Transcribed literally, including the **32-bit** multiply on the `x` term:
/// vanilla computes `x * 3129871` in `int` and only then widens it into the
/// `long` expression, so the high bits wrap before the `^`. With
/// `x = 1234567` that wrap changes the seed (`-124002568974199` here versus the
/// `97570757070985` a pure `long` multiply gives), which is why the reference
/// run pins these numbers rather than a formula.
#[must_use]
pub fn get_seed(pos: BlockPos) -> i64 {
    let x = i64::from(pos.x.wrapping_mul(3_129_871));
    let z = i64::from(pos.z).wrapping_mul(116_129_781);
    let seed = x ^ z ^ i64::from(pos.y);
    seed.wrapping_mul(seed)
        .wrapping_mul(42_317_861)
        .wrapping_add(seed.wrapping_mul(11))
        >> 16
}

/// The world a processor reads.
///
/// This is the subset of vanilla `LevelReader` the processors use. Rule and
/// protected-block tests read the block already in the world at the processed
/// position (`locState`), and `gravity` reads a heightmap. Village assembly
/// supplies the adapter over the generation world; tests supply an in-memory
/// level.
pub trait ProcessLevel {
    /// Vanilla `LevelReader.getBlockState`.
    fn block_state(&self, pos: BlockPos) -> BlockStateId;

    /// Vanilla `LevelReader.getHeight(Heightmap.Types, int, int)`.
    fn height(&self, heightmap: HeightmapType, x: i32, z: i32) -> i32;

    /// Vanilla `level instanceof ServerLevel`.
    ///
    /// `GravityProcessor` rewrites its `WORLD_SURFACE_WG`/`OCEAN_FLOOR_WG`
    /// heightmaps to the non-`WG` ones only on a `ServerLevel`. Worldgen pieces
    /// place through a `WorldGenRegion`, so the village path leaves this false
    /// and uses the `WG` heightmap as written.
    fn is_server_level(&self) -> bool {
        false
    }
}

/// One block of a piece, at its world position: vanilla's
/// `StructureTemplate.StructureBlockInfo`.
///
/// `pos` is the transformed world position (rotation, mirror and placement
/// origin already applied), which is what `processBlockInfos` builds before it
/// starts the processor chain. `jigsaw_final_state` carries what
/// `StructureBlockInfo.nbt`'s `final_state` resolves to for a `minecraft:jigsaw`
/// block — [`crate::structures::TemplateJigsaw::final_state`] — and is `None`
/// for every other block, matching vanilla's `nbt == null` case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PieceBlock {
    /// The block's world position: `processedBlockInfo.pos()`, what every
    /// position-seeded processor and the bounding-box clip read.
    pub pos: BlockPos,
    /// The block's `y` in the template, before rotation and the piece's
    /// position: vanilla's `originalBlockInfo.pos().getY()`, which
    /// [`Processor::Gravity`] adds to the terrain height. Rotation never moves
    /// a block's `y`, so this is the template position's `y` unchanged.
    pub template_y: i32,
    pub state: BlockStateId,
    pub jigsaw_final_state: Option<BlockStateId>,
}

/// A compiled `RuleTest`: what a rule matches against the block being placed.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleTest {
    /// `always_true`: matches without drawing.
    AlwaysTrue,
    /// `block_match`: `state.is(block)`, i.e. the same block kind whatever its
    /// properties. Matches without drawing.
    BlockMatch(Identifier),
    /// `blockstate_match`: `state == blockState`, i.e. the exact same state
    /// including every property. Matches without drawing.
    BlockStateMatch(BlockStateId),
    /// `tag_match`: `state.is(tag)`. Matches without drawing.
    TagMatch(Identifier),
    /// `random_block_match`: `state.is(block) && random.nextFloat() < probability`,
    /// so the draw happens only when the block already matched.
    RandomBlockMatch { block: Identifier, probability: f32 },
    /// `random_blockstate_match`:
    /// `state == blockState && random.nextFloat() < probability`.
    RandomBlockStateMatch {
        state: BlockStateId,
        probability: f32,
    },
}

impl RuleTest {
    fn test(
        &self,
        semantics: &BlockSemantics<'_>,
        state: BlockStateId,
        random: &mut dyn RandomSource,
    ) -> bool {
        match self {
            Self::AlwaysTrue => true,
            Self::BlockMatch(block) => semantics.is_block(state, block),
            Self::BlockStateMatch(expected) => state == *expected,
            Self::TagMatch(tag) => semantics.in_tag(state, tag),
            Self::RandomBlockMatch { block, probability } => {
                semantics.is_block(state, block) && random.next_float() < *probability
            }
            Self::RandomBlockStateMatch {
                state: expected,
                probability,
            } => state == *expected && random.next_float() < *probability,
        }
    }
}

/// A compiled `PosRuleTest`: what a rule matches against the positions.
///
/// Both linear predicates measure `world_pos.distManhattan(world_reference)` —
/// the *processed* block against the reference position the piece was placed
/// with — not the template-local position, despite vanilla's `inTemplatePos`
/// parameter name. That parameter is passed but never read by any predicate
/// (all three position predicate kinds ignore it), so it is not modelled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PosRuleTest {
    /// `always_true`: matches without drawing. The default when a rule omits
    /// `position_predicate`.
    AlwaysTrue,
    /// `linear_pos`: `random.nextFloat() <= clampedLerp(inverseLerp(dist,
    /// min_dist, max_dist), min_chance, max_chance)`.
    Linear {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
    },
    /// `axis_aligned_linear_pos`: `linear_pos` over the distance along one axis.
    AxisAlignedLinear {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
        axis: Axis,
    },
}

impl PosRuleTest {
    fn test(
        &self,
        world_pos: BlockPos,
        world_reference: BlockPos,
        random: &mut dyn RandomSource,
    ) -> bool {
        let (min_chance, max_chance, min_dist, max_dist, distance) = match *self {
            Self::AlwaysTrue => return true,
            Self::Linear {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
            } => (
                min_chance,
                max_chance,
                min_dist,
                max_dist,
                manhattan(world_pos, world_reference),
            ),
            Self::AxisAlignedLinear {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
                axis,
            } => (
                min_chance,
                max_chance,
                min_dist,
                max_dist,
                axis_distance(world_pos, world_reference, axis),
            ),
        };
        // `Mth.inverseLerp` and `Mth.clampedLerp` are `float` arithmetic here;
        // the `int` distance widens into the `float` parameter as Java does.
        let factor = (distance as f32 - min_dist as f32) / (max_dist as f32 - min_dist as f32);
        let chance = if factor < 0.0 {
            min_chance
        } else if factor > 1.0 {
            max_chance
        } else {
            min_chance + factor * (max_chance - min_chance)
        };
        random.next_float() <= chance
    }
}

/// One compiled `ProcessorRule`. Rules are evaluated in list order and the
/// first one whose three predicates all pass supplies `output_state`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRule {
    pub input: RuleTest,
    pub location: RuleTest,
    /// `position_predicate`, `always_true` when the rule omits it.
    pub position: PosRuleTest,
    pub output_state: BlockStateId,
}

impl ProcessorRule {
    /// Vanilla `ProcessorRule.test`: input, then location, then position, each
    /// short-circuiting.
    fn test(
        &self,
        semantics: &BlockSemantics<'_>,
        input_state: BlockStateId,
        loc_state: BlockStateId,
        world_pos: BlockPos,
        reference: BlockPos,
        random: &mut dyn RandomSource,
    ) -> bool {
        self.input.test(semantics, input_state, random)
            && self.location.test(semantics, loc_state, random)
            && self.position.test(world_pos, reference, random)
    }

    fn from_spec(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &ProcessorRuleSpec,
    ) -> Result<Self, CompileError> {
        Ok(Self {
            input: rule_test(semantics, owner, &spec.input_predicate)?,
            location: rule_test(semantics, owner, &spec.location_predicate)?,
            position: pos_rule_test(&spec.position_predicate),
            output_state: semantics.resolve_state(owner, &spec.output_state)?,
        })
    }
}

/// The states `BlockAgeProcessor` can emit, resolved once at compile time so
/// execution allocates nothing and a missing block fails closed with the
/// referring entry named.
///
/// `stone_brick_stairs` and `mossy_stone_brick_stairs` are the eight
/// `getRandomFacingStairs` combinations: index by `Direction.Plane.HORIZONTAL`
/// order (`north`, `east`, `south`, `west`) and then by `Half.values()` order
/// (`top`, `bottom`), which is the order the two `nextInt` draws select from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAgeStates {
    pub cracked_stone_bricks: BlockStateId,
    pub mossy_stone_bricks: BlockStateId,
    pub stone_slab: BlockStateId,
    pub stone_brick_slab: BlockStateId,
    pub mossy_stone_brick_slab: BlockStateId,
    pub mossy_stone_brick_wall: BlockStateId,
    pub crying_obsidian: BlockStateId,
    pub stone_brick_stairs: [[BlockStateId; 2]; 4],
    pub mossy_stone_brick_stairs: [[BlockStateId; 2]; 4],
    /// `minecraft:mossy_stone_brick_stairs`'s default state: the base
    /// `withPropertiesOf` copies onto in the stair branch.
    pub mossy_stone_brick_stairs_default: BlockStateId,
    /// `minecraft:stairs`, kept here so the per-block tag test allocates
    /// nothing.
    pub stairs: Identifier,
    /// `minecraft:slabs`.
    pub slabs: Identifier,
    /// `minecraft:walls`.
    pub walls: Identifier,
}

impl BlockAgeStates {
    /// Resolve every state the age pass can emit against the block registry.
    pub fn resolve(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
    ) -> Result<Self, CompileError> {
        Ok(Self {
            cracked_stone_bricks: default_state(semantics, owner, "cracked_stone_bricks")?,
            mossy_stone_bricks: default_state(semantics, owner, "mossy_stone_bricks")?,
            stone_slab: default_state(semantics, owner, "stone_slab")?,
            stone_brick_slab: default_state(semantics, owner, "stone_brick_slab")?,
            mossy_stone_brick_slab: default_state(semantics, owner, "mossy_stone_brick_slab")?,
            mossy_stone_brick_wall: default_state(semantics, owner, "mossy_stone_brick_wall")?,
            crying_obsidian: default_state(semantics, owner, "crying_obsidian")?,
            stone_brick_stairs: facing_stairs(semantics, owner, "stone_brick_stairs")?,
            mossy_stone_brick_stairs: facing_stairs(semantics, owner, "mossy_stone_brick_stairs")?,
            mossy_stone_brick_stairs_default: default_state(
                semantics,
                owner,
                "mossy_stone_brick_stairs",
            )?,
            stairs: identifier("minecraft:stairs"),
            slabs: identifier("minecraft:slabs"),
            walls: identifier("minecraft:walls"),
        })
    }
}

/// One processor of a piece's settings, in application order.
#[derive(Debug, Clone, PartialEq)]
pub enum Processor {
    /// `minecraft:rule`.
    Rule { rules: Vec<ProcessorRule> },
    /// `minecraft:block_age`. `mossiness` is the only field the codec carries.
    BlockAge {
        mossiness: f32,
        states: BlockAgeStates,
    },
    /// `minecraft:gravity`.
    Gravity {
        heightmap: HeightmapType,
        offset: i32,
    },
    /// `minecraft:jigsaw_replacement`: a `minecraft:jigsaw` block with a
    /// resolved `final_state` becomes that state, and a `structure_void` result
    /// removes the block.
    ///
    /// Vanilla's early return for
    /// `SharedConstants.DEBUG_KEEP_JIGSAW_BLOCKS_DURING_STRUCTURE_GEN` is not
    /// modelled: it is a `debugFlag` system property, false for every
    /// production run (and for the reference run, which replaced its jigsaws).
    JigsawReplacement,
    /// `minecraft:protected_blocks`: keeps the block only when the world state
    /// at its position is *not* in the tag.
    ProtectedBlocks { cannot_replace: Identifier },
    /// `minecraft:block_ignore`: drops the block when its kind is listed.
    BlockIgnore { blocks: Vec<Identifier> },
}

impl Processor {
    /// Compile one `mc-data` processor spec.
    pub fn from_spec(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &StructureProcessorSpec,
    ) -> Result<Self, CompileError> {
        Ok(match spec {
            StructureProcessorSpec::Rule { rules } => Self::Rule {
                rules: rules
                    .iter()
                    .map(|rule| ProcessorRule::from_spec(semantics, owner, rule))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            StructureProcessorSpec::BlockAge { mossiness } => Self::BlockAge {
                mossiness: *mossiness,
                states: BlockAgeStates::resolve(semantics, owner)?,
            },
            StructureProcessorSpec::Gravity { heightmap, offset } => Self::Gravity {
                heightmap: *heightmap,
                offset: *offset,
            },
            StructureProcessorSpec::JigsawReplacement => Self::JigsawReplacement,
            StructureProcessorSpec::ProtectedBlocks { cannot_replace } => Self::ProtectedBlocks {
                cannot_replace: cannot_replace.clone(),
            },
        })
    }
}

/// Which pool element produced the piece. Vanilla's two single-element settings
/// builders differ only in the block-ignore processors they install, which is
/// why the piece kind is part of [`piece_processors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceElement {
    /// `single_pool_element`: `BlockIgnoreProcessor.STRUCTURE_BLOCK` first.
    Single,
    /// `legacy_single_pool_element`: that processor is popped again and
    /// `BlockIgnoreProcessor.STRUCTURE_AND_AIR` is appended last. Every village
    /// pool element is a legacy single element.
    LegacySingle,
}

/// Compile the processor list a piece is placed with, in vanilla's application
/// order: `SinglePoolElement.getSettings` adds
/// `BlockIgnoreProcessor.STRUCTURE_BLOCK`, then `JigsawReplacementProcessor`
/// when the piece does not keep jigsaws, then the element's own processor list,
/// then the projection's processors.
///
/// `LegacySinglePoolElement.getSettings` then pops that first `STRUCTURE_BLOCK`
/// ignore and appends `STRUCTURE_AND_AIR`, which is why a legacy piece ignores
/// air as well. Worldgen piece placement passes `keep_jigsaws = false`, so
/// every jigsaw block is replaced.
pub fn piece_processors(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    specs: &[StructureProcessorSpec],
    projection: Projection,
    keep_jigsaws: bool,
    element: PieceElement,
) -> Result<Vec<Processor>, CompileError> {
    let mut processors = Vec::with_capacity(specs.len() + 3);
    if element == PieceElement::Single {
        processors.push(Processor::BlockIgnore {
            blocks: vec![identifier("minecraft:structure_block")],
        });
    }
    if !keep_jigsaws {
        processors.push(Processor::JigsawReplacement);
    }
    processors.extend(compile_processors(semantics, owner, specs)?);
    if projection == Projection::TerrainMatching {
        // `Projection.TERRAIN_MATCHING` carries
        // `GravityProcessor(WORLD_SURFACE_WG, -1)`.
        processors.push(Processor::Gravity {
            heightmap: HeightmapType::WorldSurfaceWg,
            offset: -1,
        });
    }
    if element == PieceElement::LegacySingle {
        processors.push(Processor::BlockIgnore {
            blocks: vec![
                identifier("minecraft:air"),
                identifier("minecraft:structure_block"),
            ],
        });
    }
    Ok(processors)
}

/// Compile a `mc-data` processor list. Unknown processor types never reach here
/// — `mc-data`'s loader rejects them — so the only failure is a state the block
/// registry cannot resolve, reported against `owner`.
pub fn compile_processors(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    specs: &[StructureProcessorSpec],
) -> Result<Vec<Processor>, CompileError> {
    specs
        .iter()
        .map(|spec| Processor::from_spec(semantics, owner, spec))
        .collect()
}

/// Vanilla `StructureProcessor.processBlock`: one processor over one block.
///
/// `original` is the block as it entered the chain — `GravityProcessor`'s
/// vertical delta comes from *its* `y` — and `processed` is the current result
/// of the processors before this one. `None` means the block is removed;
/// vanilla's `BlockIgnoreProcessor` and friends are `None`-returning
/// processors, not a separate filtering step.
#[must_use]
pub fn process_block(
    semantics: &BlockSemantics<'_>,
    level: &dyn ProcessLevel,
    processor: &Processor,
    original: &PieceBlock,
    processed: PieceBlock,
    reference_pos: BlockPos,
) -> Option<PieceBlock> {
    match processor {
        Processor::Rule { rules } => {
            let mut random = LegacyRandom::new(get_seed(processed.pos));
            let loc_state = level.block_state(processed.pos);
            for rule in rules {
                if rule.test(
                    semantics,
                    processed.state,
                    loc_state,
                    processed.pos,
                    reference_pos,
                    &mut random,
                ) {
                    // `getOutputTag` is a passthrough block-entity modifier
                    // (the loader accepts nothing else), so the tag — and with
                    // it the jigsaw's `final_state` — survives unchanged.
                    return Some(PieceBlock {
                        state: rule.output_state,
                        ..processed
                    });
                }
            }
            Some(processed)
        }
        Processor::BlockAge { mossiness, states } => {
            // `settings.getRandom(pos)`: the jigsaw path leaves
            // `StructurePlaceSettings.random` unset, so this is `Mth.getSeed(pos)`.
            let mut random = LegacyRandom::new(get_seed(processed.pos));
            match age_replacement(semantics, states, processed.state, *mossiness, &mut random) {
                Some(state) => Some(PieceBlock { state, ..processed }),
                None => Some(processed),
            }
        }
        Processor::Gravity { heightmap, offset } => {
            let heightmap = if level.is_server_level() {
                server_level_heightmap(*heightmap)
            } else {
                *heightmap
            };
            let height = level.height(heightmap, processed.pos.x, processed.pos.z) + offset;
            Some(PieceBlock {
                pos: BlockPos {
                    x: processed.pos.x,
                    y: height + original.template_y,
                    z: processed.pos.z,
                },
                ..processed
            })
        }
        Processor::JigsawReplacement => {
            if !is_named_block(semantics, processed.state, "jigsaw") {
                return Some(processed);
            }
            // Vanilla warns and keeps the block when the NBT is missing; a tag
            // without `final_state` would read as `minecraft:air`, but the
            // template loader resolves the field or refuses the template.
            let Some(final_state) = processed.jigsaw_final_state else {
                return Some(processed);
            };
            if is_named_block(semantics, final_state, "structure_void") {
                return None;
            }
            Some(PieceBlock {
                state: final_state,
                jigsaw_final_state: None,
                ..processed
            })
        }
        Processor::ProtectedBlocks { cannot_replace } => {
            if semantics.in_tag(level.block_state(processed.pos), cannot_replace) {
                None
            } else {
                Some(processed)
            }
        }
        Processor::BlockIgnore { blocks } => {
            if blocks
                .iter()
                .any(|block| semantics.is_block(processed.state, block))
            {
                None
            } else {
                Some(processed)
            }
        }
    }
}

/// Vanilla `StructureTemplate.processBlockInfos`'s inner loop: run every
/// processor over every block, in list order, dropping blocks a processor
/// removes. The surviving blocks come back in input order — with `gravity` the
/// only processor that moves them.
///
/// The template transform (`calculateRelativePosition(settings, pos).offset(position)`,
/// rotation, mirror) is the caller's; `reference_pos` is the reference the
/// piece was placed with, which only the position predicates read.
///
/// Vanilla's trailing `finalizeProcessing` pass is a no-op here: every
/// processor type that can reach this module keeps `StructureProcessor`'s
/// default implementation, and the one override — `CappedProcessor` — fails
/// closed in `mc-data`'s loader rather than arriving as a spec.
#[must_use]
pub fn apply_processors(
    semantics: &BlockSemantics<'_>,
    level: &dyn ProcessLevel,
    blocks: &[PieceBlock],
    processors: &[Processor],
    reference_pos: BlockPos,
) -> Vec<PieceBlock> {
    let mut processed_blocks = Vec::with_capacity(blocks.len());
    for original in blocks {
        let mut current = Some(*original);
        for processor in processors {
            let Some(processed) = current else {
                break;
            };
            current = process_block(
                semantics,
                level,
                processor,
                original,
                processed,
                reference_pos,
            );
        }
        if let Some(block) = current {
            processed_blocks.push(block);
        }
    }
    processed_blocks
}

/// Vanilla `BlockAgeProcessor.processBlock`'s state test and replacement.
fn age_replacement(
    semantics: &BlockSemantics<'_>,
    states: &BlockAgeStates,
    state: BlockStateId,
    mossiness: f32,
    random: &mut dyn RandomSource,
) -> Option<BlockStateId> {
    if is_named_block(semantics, state, "stone_bricks")
        || is_named_block(semantics, state, "stone")
        || is_named_block(semantics, state, "chiseled_stone_bricks")
    {
        return replace_full_stone_block(states, mossiness, random);
    }
    if semantics.in_tag(state, &states.stairs) {
        return replace_stairs(semantics, states, state, mossiness, random);
    }
    if semantics.in_tag(state, &states.slabs) {
        return if random.next_float() < mossiness {
            with_properties_of(semantics, states.mossy_stone_brick_slab, state)
        } else {
            None
        };
    }
    if semantics.in_tag(state, &states.walls) {
        return if random.next_float() < mossiness {
            with_properties_of(semantics, states.mossy_stone_brick_wall, state)
        } else {
            None
        };
    }
    if is_named_block(semantics, state, "obsidian") && random.next_float() < 0.15 {
        return Some(states.crying_obsidian);
    }
    None
}

/// `BlockAgeProcessor.maybeReplaceFullStoneBlock`.
///
/// Both replacement arrays are built before the mossiness draw picks between
/// them, so **both** random stair orientations are drawn even when the
/// non-mossy array is then discarded.
fn replace_full_stone_block(
    states: &BlockAgeStates,
    mossiness: f32,
    random: &mut dyn RandomSource,
) -> Option<BlockStateId> {
    if random.next_float() >= 0.5 {
        return None;
    }
    let non_mossy = [
        states.cracked_stone_bricks,
        random_facing_stairs(&states.stone_brick_stairs, random),
    ];
    let mossy = [
        states.mossy_stone_bricks,
        random_facing_stairs(&states.mossy_stone_brick_stairs, random),
    ];
    let replacements = if random.next_float() < mossiness {
        &mossy
    } else {
        &non_mossy
    };
    Some(replacements[random.next_int_bounded(2) as usize])
}

/// `BlockAgeProcessor.maybeReplaceStairs`. `NON_MOSSY_REPLACEMENTS` is a static
/// array, so nothing is drawn for it, and the mossy array is built — including
/// the `withPropertiesOf` copy — before the mossiness draw.
fn replace_stairs(
    semantics: &BlockSemantics<'_>,
    states: &BlockAgeStates,
    state: BlockStateId,
    mossiness: f32,
    random: &mut dyn RandomSource,
) -> Option<BlockStateId> {
    if random.next_float() >= 0.5 {
        return None;
    }
    // Unreachable fallback: the compile step resolved the target block, and
    // every `minecraft:stairs` state shares its whole property schema with
    // `mossy_stone_brick_stairs`, so the copy resolves.
    let mossy_stairs =
        with_properties_of(semantics, states.mossy_stone_brick_stairs_default, state)
            .unwrap_or(states.mossy_stone_brick_stairs[0][0]);
    let mossy = random.next_float() < mossiness;
    let index = random.next_int_bounded(2) as usize;
    Some(match (mossy, index) {
        (true, 0) => mossy_stairs,
        (true, _) => states.mossy_stone_brick_slab,
        (false, 0) => states.stone_slab,
        (false, _) => states.stone_brick_slab,
    })
}

fn random_facing_stairs(
    states: &[[BlockStateId; 2]; 4],
    random: &mut dyn RandomSource,
) -> BlockStateId {
    let facing = random.next_int_bounded(4) as usize;
    let half = random.next_int_bounded(2) as usize;
    states[facing][half]
}

/// `LevelReader`'s heightmap rewriting for a `ServerLevel`: the worldgen
/// heightmaps are not available there, so the two `WG` types fall back to their
/// generated counterparts and every other type passes through.
fn server_level_heightmap(heightmap: HeightmapType) -> HeightmapType {
    match heightmap {
        HeightmapType::WorldSurfaceWg => HeightmapType::WorldSurface,
        HeightmapType::OceanFloorWg => HeightmapType::OceanFloor,
        other => other,
    }
}

/// `BlockStateBase.withPropertiesOf`: start from `target_default` and copy
/// every property `source` also has, leaving the target's own extra properties
/// at their defaults.
fn with_properties_of(
    semantics: &BlockSemantics<'_>,
    target_default: BlockStateId,
    source: BlockStateId,
) -> Option<BlockStateId> {
    let target = semantics.blocks().by_id(target_default)?;
    let source = semantics.blocks().by_id(source)?;
    let properties: Vec<(String, String)> = target
        .properties
        .iter()
        .map(|(key, default_value)| {
            let value = source
                .properties
                .iter()
                .find(|(source_key, _)| source_key == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| default_value.clone());
            (key.clone(), value)
        })
        .collect();
    semantics
        .blocks()
        .by_name_and_props(&target.block.id, &properties)
}

fn rule_test(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    spec: &RuleTestSpec,
) -> Result<RuleTest, CompileError> {
    Ok(match spec {
        RuleTestSpec::AlwaysTrue => RuleTest::AlwaysTrue,
        RuleTestSpec::BlockMatch { block } => RuleTest::BlockMatch(block.clone()),
        RuleTestSpec::BlockStateMatch { state } => {
            RuleTest::BlockStateMatch(semantics.resolve_state(owner, state)?)
        }
        RuleTestSpec::TagMatch { tag } => RuleTest::TagMatch(tag.clone()),
        RuleTestSpec::RandomBlockMatch { block, probability } => RuleTest::RandomBlockMatch {
            block: block.clone(),
            probability: *probability,
        },
        RuleTestSpec::RandomBlockStateMatch { state, probability } => {
            RuleTest::RandomBlockStateMatch {
                state: semantics.resolve_state(owner, state)?,
                probability: *probability,
            }
        }
    })
}

fn pos_rule_test(spec: &PosRuleTestSpec) -> PosRuleTest {
    match *spec {
        PosRuleTestSpec::AlwaysTrue => PosRuleTest::AlwaysTrue,
        PosRuleTestSpec::LinearPos {
            min_chance,
            max_chance,
            min_dist,
            max_dist,
        } => PosRuleTest::Linear {
            min_chance,
            max_chance,
            min_dist,
            max_dist,
        },
        PosRuleTestSpec::AxisAlignedLinearPos {
            min_chance,
            max_chance,
            min_dist,
            max_dist,
            axis,
        } => PosRuleTest::AxisAlignedLinear {
            min_chance,
            max_chance,
            min_dist,
            max_dist,
            axis,
        },
    }
}

/// `BlockPos.distManhattan`.
fn manhattan(first: BlockPos, second: BlockPos) -> i32 {
    first
        .x
        .wrapping_sub(second.x)
        .wrapping_abs()
        .wrapping_add(first.y.wrapping_sub(second.y).wrapping_abs())
        .wrapping_add(first.z.wrapping_sub(second.z).wrapping_abs())
}

/// `AxisAlignedLinearPosTest`'s distance: `Direction.get(POSITIVE, axis)` has a
/// single `+1` step component, so this is the absolute difference along `axis`,
/// truncated through `float` as vanilla's `(int)(xd + yd + zd)` does.
fn axis_distance(world_pos: BlockPos, world_reference: BlockPos, axis: Axis) -> i32 {
    let delta = match axis {
        Axis::X => world_pos.x.wrapping_sub(world_reference.x),
        Axis::Y => world_pos.y.wrapping_sub(world_reference.y),
        Axis::Z => world_pos.z.wrapping_sub(world_reference.z),
    };
    delta.wrapping_abs() as f32 as i32
}

/// `BlockState.is(Blocks.<name>)` for the `minecraft`-namespaced constants this
/// module compares against, without allocating an `Identifier` per block.
fn is_named_block(semantics: &BlockSemantics<'_>, state: BlockStateId, path: &str) -> bool {
    semantics.blocks().by_id(state).is_some_and(|state| {
        state.block.id.namespace() == "minecraft" && state.block.id.path() == path
    })
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).expect("vanilla block and tag identifiers are valid")
}

/// A block's default state, resolved by name.
fn default_state(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    path: &str,
) -> Result<BlockStateId, CompileError> {
    let block = identifier(&format!("minecraft:{path}"));
    let default = semantics.blocks().block(&block).map(|block| block.default);
    let Some(default) = default.and_then(|state| semantics.blocks().by_id(state)) else {
        return Err(CompileError::UnknownBlockState {
            owner: owner.clone(),
            block,
        });
    };
    let properties: Vec<(String, String)> = default
        .properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    semantics
        .blocks()
        .by_name_and_props(&block, &properties)
        .ok_or(CompileError::UnknownBlockState {
            owner: owner.clone(),
            block,
        })
}

/// The eight `Direction.Plane.HORIZONTAL` x `Half.values()` states of a stair
/// block, as `getRandomFacingStairs` builds them from the default state.
fn facing_stairs(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    path: &str,
) -> Result<[[BlockStateId; 2]; 4], CompileError> {
    const FACINGS: [&str; 4] = ["north", "east", "south", "west"];
    // `Half.values()` is `TOP, BOTTOM`, in that order.
    const HALVES: [&str; 2] = ["top", "bottom"];
    let block = identifier(&format!("minecraft:{path}"));
    let mut states = [[BlockStateId(0); 2]; 4];
    for (facing_index, facing) in FACINGS.into_iter().enumerate() {
        for (half_index, half) in HALVES.into_iter().enumerate() {
            states[facing_index][half_index] = resolve_defaulted(
                semantics,
                owner,
                &BlockStateSpec {
                    block: block.clone(),
                    properties: vec![
                        ("facing".to_owned(), facing.to_owned()),
                        ("half".to_owned(), half.to_owned()),
                    ],
                },
            )?;
        }
    }
    Ok(states)
}

/// Resolve a state spec whose omitted properties take the block's default
/// values, as `BlockState.CODEC` reads them. The village closure always writes
/// complete property lists, so this agrees with [`BlockSemantics::resolve_state`]
/// on every state the closure reaches; being default-aware keeps a partial spec
/// usable instead of refusing a state vanilla would happily parse.
fn resolve_defaulted(
    semantics: &BlockSemantics<'_>,
    owner: &Identifier,
    spec: &BlockStateSpec,
) -> Result<BlockStateId, CompileError> {
    let Some(block) = semantics.blocks().block(&spec.block) else {
        return Err(CompileError::UnknownBlockState {
            owner: owner.clone(),
            block: spec.block.clone(),
        });
    };
    let Some(default_state) = semantics.blocks().by_id(block.default) else {
        return Err(CompileError::UnknownBlockState {
            owner: owner.clone(),
            block: spec.block.clone(),
        });
    };
    let properties: Vec<(String, String)> = default_state
        .properties
        .iter()
        .map(|(key, default_value)| {
            let value = spec
                .properties
                .iter()
                .find(|(spec_key, _)| spec_key == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| default_value.clone());
            (key.clone(), value)
        })
        .collect();
    semantics
        .blocks()
        .by_name_and_props(&spec.block, &properties)
        .ok_or_else(|| CompileError::UnknownBlockState {
            owner: owner.clone(),
            block: spec.block.clone(),
        })
}
