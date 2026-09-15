//! Vanilla piece placement: `StructureTemplate.placeInWorld` for a jigsaw piece.
//!
//! The solver ([`super::solver`]) decides *where* a piece goes and with which
//! rotation; this module turns that decision into world blocks the way
//! `StructureTemplate.placeInWorld` does when
//! `SinglePoolElement.place`/`LegacySinglePoolElement.place` call it:
//!
//! 1. every template block is transformed by the piece's rotation (and mirror)
//!    around the rotation pivot and offset by the piece's world position
//!    (`StructureTemplate.processBlockInfos`'s
//!    `calculateRelativePosition(settings, blockInfo.pos).offset(position)`);
//! 2. the piece's processor list runs over every block in list order
//!    ([`crate::village::processors::apply_processors`]), with the *unrotated*
//!    template state as the input — rotation is applied to the processor's
//!    result, not before it;
//! 3. a block whose processed position is outside
//!    `settings.getBoundingBox()` is dropped
//!    (`placeInWorld`'s `boundingBox.isInside(blockPos)` guard);
//! 4. the surviving state is `state.mirror(settings.getMirror()).rotate(
//!    settings.getRotation())` and is written through [`PieceWriter`].
//!
//! ## What the jigsaw path passes in
//!
//! `SinglePoolElement.getSettings` builds the settings this module consumes:
//! `setBoundingBox(chunkBB)`, `setRotation(rotation)`, `setKnownShape(true)`,
//! `setIgnoreEntities(false)`, `BlockIgnoreProcessor.STRUCTURE_BLOCK`,
//! `setFinalizeEntities(true)`, the element's `LiquidSettings` (the village
//! closure never overrides it, and no village structure sets `liquid_settings`,
//! so it is `JigsawStructure.DEFAULT_LIQUID_SETTINGS` =
//! `APPLY_WATERLOGGING`), then `JigsawReplacementProcessor` when the piece does
//! not keep its jigsaws, the element's processor list, and the projection's
//! processors. `LegacySinglePoolElement.getSettings` pops the `STRUCTURE_BLOCK`
//! ignore and appends `STRUCTURE_AND_AIR` last; every single element the village
//! pools reach is legacy (592 legacy / 0 single), so [`PieceSettings::element`]
//! is data-driven and passed through to
//! [`crate::village::processors::piece_processors`] — it is never hardcoded
//! here.
//!
//! Worldgen placement passes `keepJigsaws = false` and `Mirror.NONE`, and never
//! sets a rotation pivot, so the pivot is `BlockPos.ZERO`; [`transform`] carries
//! the pivot and mirror parameters the general method has, and the piece path
//! calls it with zeros.
//!
//! ## Flags that are deliberately not modelled
//!
//! - **Entities.** `setIgnoreEntities(false)` and `setFinalizeEntities(true)`
//!   make `placeInWorld` call `placeEntities` over the template's `entities`
//!   list. The engine's template loader ([`crate::structures`]) reads `size`,
//!   `palette` and `blocks` only, so a piece carries no entity list and there is
//!   nothing to place or finalize. Villagers come from the village's own
//!   population path, not from piece NBT.
//! - **`finalizeProcessing`.** `StructureTemplate.processBlockInfos` runs a
//!   `finalizeProcessing` pass per processor after the block loop. The default
//!   returns the list unchanged and the only override in 26.1.2 is
//!   `CappedProcessor`; none of the 16 processor lists the village closure
//!   reaches is anything but `minecraft:rule`, so there is no pass to run.
//!   ([`crate::village::processors::apply_processors`] is the block loop.)
//! - **The water flow-fill loop.** `placeInWorld`'s `toFill`/`lockedFluids`
//!   sweep and the fluid tick `SimpleWaterloggedBlock.placeLiquid` schedules
//!   need a fluid-tick model Solaris does not have at paste time; the
//!   waterlogging *decision* is modelled (see [`waterlogged_state`]).
//! - **`updateShapeAtEdge`/`updateFromNeighbourShapes`.** Both sit behind
//!   `!settings.getKnownShape()`, and the jigsaw path sets `knownShape = true`.
//!
//! ## Fidelity
//!
//! The transform, the state rotation and the state mirroring were pinned
//! against the real 26.1.2 classes: a scratch runner outside the repository
//! drove the real `StructureTemplate.placeInWorld` over a real village template
//! with the real piece settings, and compared `BlockState.rotate`/`mirror`
//! against this module's rule over every distinct block state of every village
//! template in the content cache (483 templates, 617 states) — zero
//! disagreements for all four rotations and all three mirrors.
//! [`crate::village_piece_tests`] asserts the outcomes it printed.

use mc_data::Identifier;
use mc_data::village_data::{Projection, StructureProcessorSpec};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};
use thiserror::Error;

use crate::structures::{StructureTemplate, TemplateChest, TemplateEntity};
use crate::vanilla_features::{BlockSemantics, CompileError, RandomSource};
use crate::village::processors::{
    PieceBlock, PieceElement, ProcessLevel, apply_processors, piece_processors,
};
use crate::village::solver::Rotation;

/// `Mirror`, in vanilla's order.
///
/// `BlockState.mirror` is `getBlock().mirror(state, mirror)`, and in 26.1.2 each
/// block overrides `BlockBehaviour.mirror` itself: the property rule in
/// [`mirror_state`] is the union of those overrides for the blocks the village
/// templates carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirror {
    /// `none`.
    None,
    /// `left_right`: mirrors the Z axis.
    LeftRight,
    /// `front_back`: mirrors the X axis.
    FrontBack,
}

/// `Direction.Plane.HORIZONTAL`, in vanilla's order — the order every rotation
/// of a horizontal direction indexes into.
const HORIZONTAL: [&str; 4] = ["north", "east", "south", "west"];

/// The rotation's quarter turns, i.e. `Rotation.getIndex()`.
fn turns(rotation: Rotation) -> usize {
    match rotation {
        Rotation::None => 0,
        Rotation::Clockwise90 => 1,
        Rotation::Clockwise180 => 2,
        Rotation::CounterClockwise90 => 3,
    }
}

/// `Direction.Plane.HORIZONTAL` index of a direction value, `None` for
/// `up`/`down`.
fn horizontal_index(value: &str) -> Option<usize> {
    HORIZONTAL.iter().position(|direction| *direction == value)
}

/// A property's value, or `false` when the block does not carry it — the
/// four-way `north`/`east`/`south`/`west` properties are always declared
/// together, so the default is only a guard.
fn property(properties: &[(String, String)], name: &str) -> String {
    properties
        .iter()
        .find(|(key, _)| key == name)
        .map_or_else(|| "false".to_owned(), |(_, value)| value.clone())
}

/// Rewrite a state's properties through `map` and resolve the result.
///
/// `map` sees the property name, its current value and the state's *original*
/// property list: the group rewrites (`north`/`east`/`south`/`west`, the
/// `orientation` tokens) read their source from there, exactly as vanilla's
/// chained `setValue` calls do. `None` keeps the property; `None` from the
/// whole function means the rewritten combination is not a registered state.
fn rewrite_state(
    registry: &BlockRegistry,
    state: BlockStateId,
    mut map: impl FnMut(&str, &str, &[(String, String)]) -> Option<String>,
) -> Option<BlockStateId> {
    let resolved = registry.by_id(state)?;
    let block = resolved.block.id.clone();
    let original = resolved.properties.clone();
    let mut properties = original.clone();
    for (name, value) in &mut properties {
        if let Some(replacement) = map(name, value, &original) {
            *value = replacement;
        }
    }
    registry.by_name_and_props(&block, &properties)
}

/// Vanilla `StructureTemplate.transform(BlockPos, Mirror, Rotation, pivot)`.
///
/// The mirror is applied first and only on its own axis (`LEFT_RIGHT` negates
/// `z`, `FRONT_BACK` negates `x`); the rotation then composes with the pivot,
/// and `NONE` returns the mirrored position — or the original when nothing was
/// mirrored, which is the same value.
///
/// The jigsaw path calls this with `Mirror::None` and `pivot = BlockPos.ZERO`,
/// where it is exactly [`Rotation::transform`] in the solver.
#[must_use]
pub fn transform(pos: [i32; 3], mirror: Mirror, rotation: Rotation, pivot: [i32; 3]) -> [i32; 3] {
    let [mut x, y, mut z] = pos;
    let mut mirrored = true;
    match mirror {
        Mirror::LeftRight => z = -z,
        Mirror::FrontBack => x = -x,
        Mirror::None => mirrored = false,
    }
    let [pivot_x, _, pivot_z] = pivot;
    match rotation {
        Rotation::CounterClockwise90 => [pivot_x - pivot_z + z, y, pivot_x + pivot_z - x],
        Rotation::Clockwise90 => [pivot_x + pivot_z - z, y, pivot_z - pivot_x + x],
        Rotation::Clockwise180 => [pivot_x + pivot_x - x, y, pivot_z + pivot_z - z],
        Rotation::None if mirrored => [x, y, z],
        Rotation::None => pos,
    }
}

/// Rotate the cardinal tokens of a `_`-joined value, leaving everything else
/// (`up`, `down`, ...) alone.
fn rotate_tokens(value: &str, turns: usize) -> String {
    value
        .split('_')
        .map(|token| {
            horizontal_index(token).map_or_else(
                || token.to_owned(),
                |index| HORIZONTAL[(index + turns) % 4].to_owned(),
            )
        })
        .collect::<Vec<_>>()
        .join("_")
}

/// Mirror the cardinal tokens of a `_`-joined value.
fn mirror_tokens(value: &str, mirror: Mirror) -> String {
    value
        .split('_')
        .map(|token| mirror_direction(token, mirror).to_owned())
        .collect::<Vec<_>>()
        .join("_")
}

/// `Mirror.mirror(Direction)`: the opposite direction on the mirrored axis,
/// everything else unchanged.
fn mirror_direction(value: &str, mirror: Mirror) -> &str {
    match (mirror, value) {
        (Mirror::LeftRight, "north") => "south",
        (Mirror::LeftRight, "south") => "north",
        (Mirror::FrontBack, "east") => "west",
        (Mirror::FrontBack, "west") => "east",
        _ => value,
    }
}

/// The axis a state's `facing` lies on: `Some(Axis::Z)` for `north`/`south`,
/// `Some(Axis::X)` for `east`/`west`, `None` for a block without a horizontal
/// `facing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FacingAxis {
    X,
    Z,
}

fn facing_axis(properties: &[(String, String)]) -> Option<FacingAxis> {
    match properties
        .iter()
        .find(|(name, _)| name == "facing")
        .map(|(_, value)| value.as_str())
    {
        Some("north" | "south") => Some(FacingAxis::Z),
        Some("east" | "west") => Some(FacingAxis::X),
        _ => None,
    }
}

/// Vanilla `BlockState.rotate(Rotation)`, i.e. `getBlock().rotate(state,
/// rotation)`.
///
/// 26.1.2 rotates per block — `BlockBehaviour.rotate` returns the state
/// unchanged and only the blocks that need it override — and every overriding
/// block moves one of five property families, so the transcription is
/// property-driven:
///
/// - `facing` (`HorizontalDirectionalBlock.rotate`, `StairBlock.rotate`,
///   `LadderBlock`, `DoorBlock`, `ChestBlock`, ...) rotates the cardinal value;
///   `up`/`down` stay;
/// - `north`/`east`/`south`/`west` (`CrossCollisionBlock.rotate`, which fences,
///   walls and panes extend, `VineBlock.rotate`, `MultifaceBlock.rotate`)
///   rotate the four as a group;
/// - `axis` (`RotatedPillarBlock.rotatePillar`) swaps `x`/`z` on a quarter turn;
/// - `rotation` 0..15 (`StandingSignBlock`, `BannerBlock`) advances by four per
///   turn;
/// - `orientation` (`JigsawBlock.rotate` through `FrontAndTop.rotate`) rotates
///   its cardinal token.
///
/// Everything else is preserved: rotation is orientation-preserving, so a stair
/// keeps its `shape` and a door its `hinge`.
///
/// `None` when the rotated property combination is not a registered state.
/// Vanilla cannot fail here — every combination it can produce exists — so this
/// is a guard for a registry that does not match the template, and the caller
/// fails closed rather than writing a state it invented.
#[must_use]
pub fn rotate_state(
    registry: &BlockRegistry,
    state: BlockStateId,
    rotation: Rotation,
) -> Option<BlockStateId> {
    let turns = turns(rotation);
    if turns == 0 {
        return Some(state);
    }
    rewrite_state(registry, state, |name, value, original| match name {
        "facing" => horizontal_index(value).map(|index| HORIZONTAL[(index + turns) % 4].to_owned()),
        "north" | "east" | "south" | "west" => {
            let index = horizontal_index(name)?;
            Some(property(original, HORIZONTAL[(index + 4 - turns) % 4]))
        }
        "axis" if turns % 2 == 1 => match value {
            "x" => Some("z".to_owned()),
            "z" => Some("x".to_owned()),
            _ => None,
        },
        "rotation" => value
            .parse::<u32>()
            .ok()
            .map(|value| ((value + 4 * turns as u32) % 16).to_string()),
        "orientation" => Some(rotate_tokens(value, turns)),
        _ => None,
    })
}

/// `Mirror.mirror(int rotation, int steps)`.
fn mirror_rotation(rotation: i32, steps: i32, mirror: Mirror) -> i32 {
    let half = steps / 2;
    let corrected = if rotation > half {
        rotation - steps
    } else {
        rotation
    };
    match mirror {
        Mirror::LeftRight => (half - corrected + steps).rem_euclid(steps),
        Mirror::FrontBack => (steps - corrected).rem_euclid(steps),
        Mirror::None => rotation,
    }
}

/// `StairBlock.mirror`'s `shape` flip, which only happens when the mirror's
/// axis is the axis the stair faces along, and which leaves the inner shapes
/// alone under `FRONT_BACK`:
///
/// ```text
/// LEFT_RIGHT, facing on Z:  outer_left <-> outer_right, inner_left <-> inner_right
/// FRONT_BACK, facing on X:  outer_left <-> outer_right, inner shapes unchanged
/// anything else:            unchanged
/// ```
fn stair_shape_mirror(shape: &str, axis: Option<FacingAxis>, mirror: Mirror) -> Option<String> {
    let aligned = match mirror {
        Mirror::LeftRight => axis == Some(FacingAxis::Z),
        Mirror::FrontBack => axis == Some(FacingAxis::X),
        Mirror::None => false,
    };
    if !aligned {
        return None;
    }
    let inner_flips = mirror == Mirror::LeftRight;
    match shape {
        "outer_left" => Some("outer_right".to_owned()),
        "outer_right" => Some("outer_left".to_owned()),
        "inner_left" if inner_flips => Some("inner_right".to_owned()),
        "inner_right" if inner_flips => Some("inner_left".to_owned()),
        _ => None,
    }
}

/// Vanilla `BlockState.mirror(Mirror)`, i.e. `getBlock().mirror(state, mirror)`.
///
/// The overrides the village blocks reach are `HorizontalDirectionalBlock.mirror`
/// (the `facing` value), `CrossCollisionBlock.mirror` (the four-way booleans),
/// `DoorBlock.mirror` (`state.rotate(mirror.getRotation(FACING)).cycle(HINGE)` —
/// the hinge always flips, the facing rotates only when the mirror's axis is the
/// facing axis), `StairBlock.mirror` (the `shape` table in
/// [`stair_shape_mirror`]), the sign and banner blocks' `rotation`, and
/// `JigsawBlock.mirror` (the `orientation`). `RotatedPillarBlock` does **not**
/// override `mirror`, so an `axis` is left alone — mirroring a log does not turn
/// it.
///
/// `None` when the mirrored property combination is not a registered state; the
/// caller fails closed, as in [`rotate_state`].
#[must_use]
pub fn mirror_state(
    registry: &BlockRegistry,
    state: BlockStateId,
    mirror: Mirror,
) -> Option<BlockStateId> {
    if mirror == Mirror::None {
        return Some(state);
    }
    rewrite_state(registry, state, |name, value, original| match name {
        "facing" => Some(mirror_direction(value, mirror).to_owned()),
        "north" | "east" | "south" | "west" => {
            let source = match mirror {
                Mirror::LeftRight => match name {
                    "north" => "south",
                    "south" => "north",
                    _ => name,
                },
                Mirror::FrontBack => match name {
                    "east" => "west",
                    "west" => "east",
                    _ => name,
                },
                Mirror::None => name,
            };
            Some(property(original, source))
        }
        "rotation" => value
            .parse::<i32>()
            .ok()
            .map(|value| mirror_rotation(value, 16, mirror).to_string()),
        "orientation" => Some(mirror_tokens(value, mirror)),
        "hinge" => match value {
            "left" => Some("right".to_owned()),
            "right" => Some("left".to_owned()),
            _ => None,
        },
        "shape" => stair_shape_mirror(value, facing_axis(original), mirror),
        _ => None,
    })
}

/// A world-space clip: vanilla `BoundingBox`, whose `isInside(BlockPos)` is an
/// inclusive comparison on all three axes.
///
/// `StructureTemplate.placeInWorld` clips to `settings.getBoundingBox()`, which
/// `SinglePoolElement.place` sets to the chunk box the piece is being placed
/// into (`chunkBB`); a `None` clip is vanilla's `BoundingBox.infinite()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockClip {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl BlockClip {
    #[must_use]
    pub fn new(min: [i32; 3], max: [i32; 3]) -> Self {
        Self { min, max }
    }

    /// Vanilla `BoundingBox.isInside(BlockPos)`.
    #[must_use]
    pub fn contains(&self, pos: BlockPos) -> bool {
        pos.x >= self.min[0]
            && pos.x <= self.max[0]
            && pos.y >= self.min[1]
            && pos.y <= self.max[1]
            && pos.z >= self.min[2]
            && pos.z <= self.max[2]
    }
}

/// Vanilla `StructurePlaceSettings`, the fields the jigsaw path sets.
///
/// See the module documentation for what `SinglePoolElement.getSettings` passes
/// and which settings this module does not need.
#[derive(Debug, Clone)]
pub struct PieceSettings<'a> {
    /// `StructurePoolElement.place`'s `position`: the world position of the
    /// template's origin (`PlacedPiece.position`).
    pub position: BlockPos,
    /// `StructurePoolElement.place`'s `referencePos`: the position the structure
    /// started at. Only the position predicates read it.
    pub reference_pos: BlockPos,
    pub rotation: Rotation,
    /// `settings.getMirror()`. `SinglePoolElement.place` never sets one, so the
    /// jigsaw path always passes [`Mirror::None`].
    pub mirror: Mirror,
    /// The element's projection. `TerrainMatching` appends the projection's
    /// `GravityProcessor` to the processor list.
    pub projection: Projection,
    /// The element kind, from the pool element the plan chose
    /// (`single_pool_element` or `legacy_single_pool_element`). It decides which
    /// block-ignore processors the piece gets, so it is caller data and never
    /// assumed here.
    pub element: PieceElement,
    /// The element's processor list (`ProcessorRef`), in list order.
    pub processors: &'a [StructureProcessorSpec],
    /// The pool element id, used to name the referring entry when a processor
    /// list fails to compile.
    pub owner: &'a Identifier,
    /// `settings.getBoundingBox()`. `None` is vanilla's infinite box.
    pub clip: Option<BlockClip>,
}

/// The world a piece is written into.
///
/// It is [`ProcessLevel`] plus the write, so one object serves both the
/// processor chain's reads (`locState`, heightmaps) and the placement's writes.
pub trait PieceWriter: ProcessLevel {
    /// Vanilla `LevelWriter.setBlock` for a piece block, with the final state.
    fn set_block(&mut self, pos: BlockPos, state: BlockStateId);

    /// Vanilla's block-entity step for a chest the template carries.
    ///
    /// `chest` is the template's own chest and `loot_seed` is the
    /// `LootTableSeed` vanilla drew for this container from the same random that
    /// places the structure's decor, so the writer can roll the real table with
    /// its own [`crate::structures::StructureLoot`].
    fn set_chest(&mut self, pos: BlockPos, chest: &TemplateChest, loot_seed: u64);

    /// Vanilla's entity step for one entity the template places.
    ///
    /// A writer that keeps nothing — the startup validation probe — implements
    /// nothing here: placing an entity is a world write like placing a block.
    fn set_entity(&mut self, _placed: &PlacedEntity<'_>) {}
}

/// One template entity [`place_piece`] places, resolved into the world.
///
/// Vanilla's `StructureEntityInfo` after `StructureTemplate.placeEntities` has
/// transformed it: the position the entity is moved to, the block position the
/// piece's bounding box was tested with, and the yaw it is snapped to. The
/// `UUID` vanilla strips from the template's NBT is replaced by [`Self::claim`],
/// a deterministic identity for the placement.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEntity<'a> {
    /// The template entity this placement came from.
    pub entity: &'a TemplateEntity,
    /// `transform(blockPos, mirror, rotation, pivot).offset(position)` and the
    /// block the bounding box dropped it by.
    pub block: BlockPos,
    /// `transform(pos, mirror, rotation, pivot).add(position)`.
    pub position: [f64; 3],
    /// `entity.rotate(rotation) + entity.mirror(mirror) - entity.getYRot()`.
    pub yaw: f32,
    /// The pitch vanilla snaps the entity to: `entity.getXRot()`, the template's
    /// authored `Rotation[1]`. It is *not* rotated or mirrored — vanilla passes
    /// the entity's own pitch straight to `snapTo`.
    pub pitch: f32,
    /// The source pool element's id, the piece's world origin and the entity's
    /// index within its template.
    pub claim: String,
}

#[derive(Debug, Error)]
pub enum PieceError {
    #[error(transparent)]
    Processors(#[from] CompileError),
    #[error(transparent)]
    Decor(#[from] crate::vanilla_features::PlaceError),
    #[error(
        "piece block at ({pos:?}) cannot be written: the state after rotation/mirroring is not a \
         registered state ({state:?})"
    )]
    UnresolvableState { pos: BlockPos, state: BlockStateId },
}

/// Vanilla `StructurePoolElement.place` for one piece.
///
/// Runs `SinglePoolElement.place`/`LegacySinglePoolElement.place`: the template
/// transform, the processor chain (with `keepJigsaws = false`, worldgen's
/// value), the bounding-box clip and the mirror/rotate of each surviving state,
/// then writes the result through `writer`. Returns the number of blocks
/// written.
///
/// The jigsaw `final_state` substitution is the processor chain's
/// (`Processor::JigsawReplacement`), which is why this module only has to hand
/// each jigsaw block its resolved `final_state`; a `structure_void` final state
/// is dropped there. A template with an empty palette, or one whose size has a
/// non-positive axis, writes nothing and returns `Ok(0)`, matching
/// `placeInWorld`'s early `false`.
///
/// `random` is the structure's placement random, which vanilla draws a chest's
/// `LootTableSeed` from. It is `None` for a piece that draws nothing — no chest
/// of this template survives its clip — and drawing is the only thing that
/// needs it.
pub fn place_piece(
    semantics: &BlockSemantics<'_>,
    template: &StructureTemplate,
    settings: &PieceSettings<'_>,
    writer: &mut dyn PieceWriter,
    mut random: Option<&mut dyn RandomSource>,
) -> Result<usize, PieceError> {
    let processors = piece_processors(
        semantics,
        settings.owner,
        settings.processors,
        settings.projection,
        false,
        settings.element,
    )?;

    // `placeInWorld` walks `settings.getRandomPalette(...).blocks()`, which is
    // every palette entry; the loader keeps the jigsaw entries in their own list,
    // so they are appended. Order is immaterial to the outcome: every processor
    // that draws seeds from the block's own position.
    let mut blocks = Vec::with_capacity(template.blocks().len() + template.jigsaws().len());
    for block in template.blocks() {
        blocks.push(PieceBlock {
            pos: world_position(block.pos, settings),
            template_y: block.pos[1],
            state: block.state,
            jigsaw_final_state: None,
        });
    }
    for jigsaw in template.jigsaws() {
        blocks.push(PieceBlock {
            pos: world_position(jigsaw.pos, settings),
            template_y: jigsaw.pos[1],
            state: jigsaw.state,
            jigsaw_final_state: Some(jigsaw.final_state),
        });
    }

    let processed = apply_processors(
        semantics,
        writer,
        &blocks,
        &processors,
        settings.reference_pos,
    );

    let registry = semantics.blocks();
    let chests: Vec<(BlockPos, usize)> = template
        .chests()
        .iter()
        .enumerate()
        .map(|(index, chest)| (world_position(chest.pos, settings), index))
        .collect();

    let mut written = 0;
    for block in &processed {
        if let Some(clip) = settings.clip
            && !clip.contains(block.pos)
        {
            continue;
        }
        let state = mirror_state(registry, block.state, settings.mirror)
            .and_then(|state| rotate_state(registry, state, settings.rotation))
            .ok_or(PieceError::UnresolvableState {
                pos: block.pos,
                state: block.state,
            })?;
        let state = waterlogged_state(semantics, writer, block.pos, state)?;
        writer.set_block(block.pos, state);
        written += 1;
        // Vanilla loads the chest's block entity after `setBlock` succeeded, at
        // the chest block's own processed position. Matching on the transformed
        // template position is the same thing for every chest the village
        // closure reaches: gravity only moves blocks of `terrain_matching`
        // pieces, and no piece with a chest is one.
        if let Some((_, index)) = chests.iter().find(|(pos, _)| *pos == block.pos) {
            // `StructureTemplate.placeInWorld` draws `LootTableSeed` from the
            // random that places the structure, once per container the piece
            // actually writes and never for a block the clip dropped.
            let loot_seed = random
                .as_deref_mut()
                .expect("a chest-bearing piece draws its loot seed from the structure's random")
                .next_long() as u64;
            writer.set_chest(block.pos, &template.chests()[*index], loot_seed);
        }
    }
    // `placeInWorld`'s entity step runs after the whole block and block-entity
    // loop (`settings.isIgnoreEntities()` is false for every jigsaw piece), and
    // draws nothing: the placement order of the two steps is observable only
    // through the blocks above.
    for placed in placed_entities(template, settings) {
        writer.set_entity(&placed);
    }
    Ok(written)
}

/// The entities [`place_piece`] places, in world coordinates.
///
/// Vanilla `StructureTemplate.placeEntities`: the template's entity list is
/// transformed by the piece's mirror, rotation and pivot, offset by the piece's
/// own world position, and an entity whose *block* position falls outside the
/// piece's bounding box is dropped — the same per-chunk clip the blocks go
/// through.
fn placed_entities<'a>(
    template: &'a StructureTemplate,
    settings: &PieceSettings<'_>,
) -> Vec<PlacedEntity<'a>> {
    template
        .entities()
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| {
            let block = world_position(entity.block_position, settings);
            if let Some(clip) = settings.clip
                && !clip.contains(block)
            {
                return None;
            }
            let [x, y, z] = transform_position(entity.position, settings.mirror, settings.rotation);
            Some(PlacedEntity {
                entity,
                block,
                position: [
                    x + f64::from(settings.position.x),
                    y + f64::from(settings.position.y),
                    z + f64::from(settings.position.z),
                ],
                yaw: entity_yaw(entity.yaw, settings.mirror, settings.rotation),
                pitch: entity.pitch,
                claim: format!(
                    "{}@{}:{}:{}#{}",
                    settings.owner,
                    settings.position.x,
                    settings.position.y,
                    settings.position.z,
                    index
                ),
            })
        })
        .collect()
}

/// `StructureTemplate.transform(Vec3, Mirror, Rotation, BlockPos)`: the
/// double-precision sibling of [`transform`], with the `1.0 -` mirroring and the
/// `+ 1` term vanilla's `Vec3` overload carries. Every jigsaw piece passes a
/// zero rotation pivot, so the pivot is not a parameter here.
pub(crate) fn transform_position(pos: [f64; 3], mirror: Mirror, rotation: Rotation) -> [f64; 3] {
    let [mut x, y, mut z] = pos;
    let mut mirrored = true;
    match mirror {
        Mirror::LeftRight => z = 1.0 - z,
        Mirror::FrontBack => x = 1.0 - x,
        Mirror::None => mirrored = false,
    }
    match rotation {
        Rotation::CounterClockwise90 => [z, y, 1.0 - x],
        Rotation::Clockwise90 => [1.0 - z, y, x],
        Rotation::Clockwise180 => [1.0 - x, y, 1.0 - z],
        Rotation::None if mirrored => [x, y, z],
        Rotation::None => pos,
    }
}

/// `Mth.wrapDegrees`: Java's truncating `%` into `-180..180`.
pub(crate) fn wrap_degrees(value: f32) -> f32 {
    let mut value = value % 360.0;
    if value >= 180.0 {
        value -= 360.0;
    }
    if value < -180.0 {
        value += 360.0;
    }
    value
}

/// The yaw vanilla snaps a placed entity to:
/// `entity.rotate(rotation) + entity.mirror(mirror) - entity.getYRot()`, where
/// `getYRot` is the yaw the entity's own NBT carried. `rotate` and `mirror` wrap
/// their input; `getYRot` does not, which is why the raw yaw appears twice.
pub(crate) fn entity_yaw(yaw: f32, mirror: Mirror, rotation: Rotation) -> f32 {
    let angle = wrap_degrees(yaw);
    let rotated = match rotation {
        Rotation::None => angle,
        Rotation::Clockwise90 => angle + 90.0,
        Rotation::Clockwise180 => angle + 180.0,
        Rotation::CounterClockwise90 => angle + 270.0,
    };
    let mirrored = match mirror {
        Mirror::None => angle,
        Mirror::LeftRight => -angle,
        Mirror::FrontBack => 180.0 - angle,
    };
    rotated + (mirrored - yaw)
}

/// `calculateRelativePosition(settings, pos).offset(position)`, with the
/// jigsaw path's zero rotation pivot.
fn world_position(local: [i32; 3], settings: &PieceSettings<'_>) -> BlockPos {
    let [x, y, z] = transform(local, settings.mirror, settings.rotation, [0, 0, 0]);
    BlockPos {
        x: settings.position.x + x,
        y: settings.position.y + y,
        z: settings.position.z + z,
    }
}

/// Whether a world state exposes a water fluid state: a water block, one of the
/// water plants, or a waterlogged block — `SimpleWaterloggedBlock.getFluidState`
/// returns `Fluids.WATER` when `waterlogged` is true, and `Fluids.EMPTY`
/// otherwise.
fn holds_water(registry: &BlockRegistry, state: BlockStateId) -> bool {
    let Some(resolved) = registry.by_id(state) else {
        return false;
    };
    if matches!(
        resolved.block.id.path(),
        "water" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "bubble_column"
    ) {
        return true;
    }
    resolved
        .properties
        .iter()
        .any(|(name, value)| name == "waterlogged" && value == "true")
}

/// Vanilla `placeInWorld`'s `LiquidSettings.APPLY_WATERLOGGING` branch for one
/// block.
///
/// `placeInWorld` reads `level.getFluidState(blockPos)` before writing and, when
/// the written block is a `LiquidBlockContainer` whose state is not already
/// waterlogged, calls `placeLiquid(level, pos, state, previousFluidState)`.
/// `SimpleWaterloggedBlock.placeLiquid` sets `waterlogged = true` exactly when
/// the previous fluid is water, so the decision here is: a state with
/// `waterlogged=false` written into a position that already holds water becomes
/// `waterlogged=true`.
///
/// The village path always applies this: no village pool element carries
/// `override_liquid_settings` and no village structure carries
/// `liquid_settings`, so `JigsawStructure.DEFAULT_LIQUID_SETTINGS` applies.
/// `placeLiquid`'s `scheduleTick` and the flow-fill loop that follows the write
/// are not modelled (see the module documentation).
fn waterlogged_state(
    semantics: &BlockSemantics<'_>,
    level: &dyn ProcessLevel,
    pos: BlockPos,
    state: BlockStateId,
) -> Result<BlockStateId, PieceError> {
    let registry = semantics.blocks();
    let unresolvable = PieceError::UnresolvableState { pos, state };
    let Some(resolved) = registry.by_id(state) else {
        return Err(unresolvable);
    };
    let dry = resolved
        .properties
        .iter()
        .any(|(name, value)| name == "waterlogged" && value == "false");
    if !dry || !holds_water(registry, level.block_state(pos)) {
        return Ok(state);
    }
    rewrite_state(registry, state, |name, value, _| {
        (name == "waterlogged" && value == "false").then(|| "true".to_owned())
    })
    .ok_or(unresolvable)
}
