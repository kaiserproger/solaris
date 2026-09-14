//! Vanilla `minecraft:tree` placement (checkpoint A2).
//!
//! Transcribed from `TreeFeature`, `TrunkPlacer`/`StraightTrunkPlacer`/
//! `ForkingTrunkPlacer`, `FoliagePlacer`/`BlobFoliagePlacer`/
//! `SpruceFoliagePlacer`/`PineFoliagePlacer`/`AcaciaFoliagePlacer`,
//! `TwoLayersFeatureSize` and `LeavesBlock.getOptionalDistanceAt`, all read from
//! the decompiled 26.1.2 bodies.
//!
//! ## Modelled boundary: `java.util.HashMap` treeification
//!
//! `TreeFeature.updateLeaves` writes a leaf's `distance` every time it processes
//! it, and a leaf can sit in several of the seven per-distance sets at once, so
//! the value that survives is the one written last — which follows the
//! *iteration order* of those `HashSet`s. This module emulates that order
//! (`Vec3i.hashCode`, `h ^ (h >>> 16)`, a 16-bucket table doubling at 3/4 load,
//! insertion-ordered chains, and `HashMap.treeifyBin`'s resize when a bin reaches
//! 8 entries below 64 buckets).
//!
//! Past that: once the table has at least 64 buckets and a bin reaches 8
//! entries, real `HashMap` converts the bin to a red-black tree and
//! `moveRootToFront` moves the root to the front of the chain, so iteration
//! order stops being insertion order. That case is **not** modelled. It is
//! unreachable for the four village trees (their per-distance sets stay in the
//! low tens: the largest, oak, has 53 leaves across 7 sets and the fixture
//! comparison against real Java covers all of them), but a large enough tree
//! could reach it, so `add` returns [`PlaceError::TreeOrderBoundary`] instead of
//! guessing — in debug *and* release, with no `debug_assert` in the path.
//! Placement positions are unaffected either way; only leaf `distance` values
//! could differ past the boundary.
//!
//! `TreeFeature.place` ends with `StructureTemplate.updateShapeAtEdge`, which
//! runs `BlockState.updateShape` for every face-adjacent pair in the tree's
//! bounding shape. Every block the reachable trees write (logs, leaves, dirt on
//! `below_trunk_provider`) either keeps the default `updateShape` (returns the
//! same state) or, for leaves, only schedules a tick, so that pass writes
//! nothing and is not reproduced here. A decoration or block whose shape
//! depends on its neighbours would need it.

use std::collections::HashSet;

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::{
    FoliagePlacerSpec, TreeSpec, TrunkPlacerSpec, TwoLayersFeatureSize,
};
use mc_world::BlockPos;

use super::placement::CompiledIntProvider;
use super::provider::CompiledStateProvider;
use super::random::RandomSource;
use super::{BlockSemantics, CompileError, FeatureLevel, PlaceError};

/// `FoliagePlacer.FoliageAttachment`.
#[derive(Debug, Clone, Copy)]
struct FoliageAttachment {
    pos: BlockPos,
    radius_offset: i32,
    double_trunk: bool,
}

/// A `minecraft:tree` configuration compiled against the block registry.
#[derive(Debug, Clone)]
pub(crate) struct CompiledTree {
    trunk_provider: CompiledStateProvider,
    trunk_placer: CompiledTrunkPlacer,
    foliage_provider: CompiledStateProvider,
    foliage_placer: CompiledFoliagePlacer,
    minimum_size: TwoLayersFeatureSize,
    ignore_vines: bool,
    below_trunk_provider: CompiledStateProvider,
}

#[derive(Debug, Clone, Copy)]
struct TrunkHeights {
    base_height: i32,
    height_rand_a: i32,
    height_rand_b: i32,
}

#[derive(Debug, Clone, Copy)]
enum CompiledTrunkPlacer {
    Straight(TrunkHeights),
    Forking(TrunkHeights),
}

#[derive(Debug, Clone)]
struct FoliageParts {
    radius: CompiledIntProvider,
    offset: CompiledIntProvider,
}

/// Which foliage placer a row belongs to; `Copy` so row placement does not hold
/// a borrow on the tree it is placing into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoliageKind {
    Blob,
    Spruce,
    Pine,
    Acacia,
}

#[derive(Debug, Clone)]
enum CompiledFoliagePlacer {
    Blob {
        height: i32,
        parts: FoliageParts,
    },
    Spruce {
        trunk_height: CompiledIntProvider,
        parts: FoliageParts,
    },
    Pine {
        height: CompiledIntProvider,
        parts: FoliageParts,
    },
    Acacia {
        parts: FoliageParts,
    },
}

impl CompiledTree {
    pub(crate) fn compile(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &TreeSpec,
    ) -> Result<Self, CompileError> {
        let foliage_provider =
            CompiledStateProvider::compile(semantics, owner, &spec.foliage_provider)?;
        // `TreeFeature.updateLeaves` calls `setValue(DISTANCE, ...)` on every
        // leaf it reaches, so a foliage state without that property is a data
        // error vanilla would throw on.
        for state in foliage_provider.producible_states() {
            let Some(block) = semantics.blocks().by_id(state) else {
                continue;
            };
            if semantics.property(state, "distance").is_none() {
                return Err(CompileError::MissingStateProperty {
                    owner: owner.clone(),
                    block: block.block.id.clone(),
                    property: "distance",
                });
            }
        }
        Ok(Self {
            trunk_provider: CompiledStateProvider::compile(semantics, owner, &spec.trunk_provider)?,
            trunk_placer: match spec.trunk_placer {
                TrunkPlacerSpec::Straight(heights) => CompiledTrunkPlacer::Straight(heights.into()),
                TrunkPlacerSpec::Forking(heights) => CompiledTrunkPlacer::Forking(heights.into()),
            },
            foliage_provider,
            foliage_placer: match &spec.foliage_placer {
                FoliagePlacerSpec::Blob {
                    radius,
                    offset,
                    height,
                } => CompiledFoliagePlacer::Blob {
                    height: *height,
                    parts: FoliageParts {
                        radius: CompiledIntProvider::compile(owner, radius)?,
                        offset: CompiledIntProvider::compile(owner, offset)?,
                    },
                },
                FoliagePlacerSpec::Spruce {
                    radius,
                    offset,
                    trunk_height,
                } => CompiledFoliagePlacer::Spruce {
                    trunk_height: CompiledIntProvider::compile(owner, trunk_height)?,
                    parts: FoliageParts {
                        radius: CompiledIntProvider::compile(owner, radius)?,
                        offset: CompiledIntProvider::compile(owner, offset)?,
                    },
                },
                FoliagePlacerSpec::Pine {
                    radius,
                    offset,
                    height,
                } => CompiledFoliagePlacer::Pine {
                    height: CompiledIntProvider::compile(owner, height)?,
                    parts: FoliageParts {
                        radius: CompiledIntProvider::compile(owner, radius)?,
                        offset: CompiledIntProvider::compile(owner, offset)?,
                    },
                },
                FoliagePlacerSpec::Acacia { radius, offset } => CompiledFoliagePlacer::Acacia {
                    parts: FoliageParts {
                        radius: CompiledIntProvider::compile(owner, radius)?,
                        offset: CompiledIntProvider::compile(owner, offset)?,
                    },
                },
            },
            minimum_size: spec.minimum_size,
            ignore_vines: spec.ignore_vines,
            below_trunk_provider: CompiledStateProvider::compile(
                semantics,
                owner,
                &spec.below_trunk_provider,
            )?,
        })
    }

    /// Vanilla `TreeFeature.place` over a compiled configuration.
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
    ) -> Result<bool, PlaceError> {
        let mut placement = TreePlacement {
            level,
            semantics,
            random,
            tree: self,
            logs: JavaHashSet::new(),
            foliage: HashSet::new(),
            roots: JavaHashSet::new(),
        };
        placement.run(origin)
    }
}

impl From<mc_data::vanilla_feature_closure::TrunkPlacerHeights> for TrunkHeights {
    fn from(heights: mc_data::vanilla_feature_closure::TrunkPlacerHeights) -> Self {
        Self {
            base_height: heights.base_height,
            height_rand_a: heights.height_rand_a,
            height_rand_b: heights.height_rand_b,
        }
    }
}

impl CompiledTrunkPlacer {
    /// `TrunkPlacer.getTreeHeight`.
    fn tree_height(&self, random: &mut impl RandomSource) -> i32 {
        let TrunkHeights {
            base_height,
            height_rand_a,
            height_rand_b,
        } = match self {
            Self::Straight(heights) | Self::Forking(heights) => *heights,
        };
        base_height
            + random.next_int_bounded(height_rand_a + 1)
            + random.next_int_bounded(height_rand_b + 1)
    }

    /// `TrunkPlacer.isFree`.
    fn is_free<R: RandomSource>(
        &self,
        placement: &TreePlacement<'_, '_, R>,
        pos: BlockPos,
    ) -> bool {
        let state = placement.level.block_state(pos);
        placement.semantics.is_valid_tree_pos(state) || placement.semantics.is_log(state)
    }
}

impl CompiledFoliagePlacer {
    fn kind(&self) -> FoliageKind {
        match self {
            Self::Blob { .. } => FoliageKind::Blob,
            Self::Spruce { .. } => FoliageKind::Spruce,
            Self::Pine { .. } => FoliageKind::Pine,
            Self::Acacia { .. } => FoliageKind::Acacia,
        }
    }

    fn parts(&self) -> &FoliageParts {
        match self {
            Self::Blob { parts, .. }
            | Self::Spruce { parts, .. }
            | Self::Pine { parts, .. }
            | Self::Acacia { parts, .. } => parts,
        }
    }

    /// Vanilla `FoliagePlacer.foliageHeight`.
    fn foliage_height(&self, random: &mut impl RandomSource, tree_height: i32) -> i32 {
        match self {
            Self::Blob { height, .. } => *height,
            Self::Spruce { trunk_height, .. } => {
                std::cmp::max(4, tree_height - trunk_height.sample(random))
            }
            Self::Pine { height, .. } => height.sample(random),
            Self::Acacia { .. } => 0,
        }
    }

    /// Vanilla `FoliagePlacer.foliageRadius` (pine adds a height-dependent
    /// bonus).
    fn foliage_radius(&self, random: &mut impl RandomSource, trunk_height: i32) -> i32 {
        let radius = self.parts().radius.sample(random);
        match self {
            Self::Pine { .. } => {
                radius + random.next_int_bounded(std::cmp::max(trunk_height + 1, 1))
            }
            _ => radius,
        }
    }

    /// Vanilla `FoliagePlacer.createFoliage`; the `offset` draw happens first.
    fn create_foliage(
        &self,
        placement: &mut TreePlacement<'_, '_, impl RandomSource>,
        attachment: FoliageAttachment,
        foliage_height: i32,
        leaf_radius: i32,
    ) {
        let kind = self.kind();
        let offset = self.parts().offset.sample(placement.random);
        match self {
            Self::Blob { .. } => {
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    let current_radius =
                        std::cmp::max(leaf_radius + attachment.radius_offset - 1 - yo / 2, 0);
                    placement.place_leaves_row(
                        attachment.pos,
                        current_radius,
                        yo,
                        attachment.double_trunk,
                        kind,
                    );
                    yo -= 1;
                }
            }
            Self::Spruce { .. } => {
                let mut current_radius = placement.random.next_int_bounded(2);
                let mut max_radius = 1;
                let mut min_radius = 0;
                let mut yo = offset;
                while yo >= -foliage_height {
                    placement.place_leaves_row(
                        attachment.pos,
                        current_radius,
                        yo,
                        attachment.double_trunk,
                        kind,
                    );
                    if current_radius >= max_radius {
                        current_radius = min_radius;
                        min_radius = 1;
                        max_radius =
                            std::cmp::min(max_radius + 1, leaf_radius + attachment.radius_offset);
                    } else {
                        current_radius += 1;
                    }
                    yo -= 1;
                }
            }
            Self::Pine { .. } => {
                let mut current_radius = 0;
                let mut yo = offset;
                while yo >= offset - foliage_height {
                    placement.place_leaves_row(
                        attachment.pos,
                        current_radius,
                        yo,
                        attachment.double_trunk,
                        kind,
                    );
                    if current_radius >= 1 && yo == offset - foliage_height + 1 {
                        current_radius -= 1;
                    } else if current_radius < leaf_radius + attachment.radius_offset {
                        current_radius += 1;
                    }
                    yo -= 1;
                }
            }
            Self::Acacia { .. } => {
                let double_trunk = attachment.double_trunk;
                let foliage_pos = offset_pos(attachment.pos, 0, offset, 0);
                placement.place_leaves_row(
                    foliage_pos,
                    leaf_radius + attachment.radius_offset,
                    -1 - foliage_height,
                    double_trunk,
                    kind,
                );
                placement.place_leaves_row(
                    foliage_pos,
                    leaf_radius - 1,
                    -foliage_height,
                    double_trunk,
                    kind,
                );
                placement.place_leaves_row(
                    foliage_pos,
                    leaf_radius + attachment.radius_offset - 1,
                    0,
                    double_trunk,
                    kind,
                );
            }
        }
    }

    /// Vanilla `FoliagePlacer.shouldSkipLocation`, via
    /// `shouldSkipLocationSigned`.
    fn should_skip_location(
        kind: FoliageKind,
        random: &mut impl RandomSource,
        dx: i32,
        y: i32,
        dz: i32,
        current_radius: i32,
        double_trunk: bool,
    ) -> bool {
        let (min_dx, min_dz) = if double_trunk {
            (
                std::cmp::min(dx.abs(), (dx - 1).abs()),
                std::cmp::min(dz.abs(), (dz - 1).abs()),
            )
        } else {
            (dx.abs(), dz.abs())
        };
        match kind {
            FoliageKind::Blob => {
                min_dx == current_radius
                    && min_dz == current_radius
                    && (random.next_int_bounded(2) == 0 || y == 0)
            }
            FoliageKind::Spruce | FoliageKind::Pine => {
                min_dx == current_radius && min_dz == current_radius && current_radius > 0
            }
            FoliageKind::Acacia => {
                if y == 0 {
                    (min_dx > 1 || min_dz > 1) && min_dx != 0 && min_dz != 0
                } else {
                    min_dx == current_radius && min_dz == current_radius && current_radius > 0
                }
            }
        }
    }
}

/// Mutable state for one tree placement: the world, the RNG and vanilla's
/// `logs`/`foliage`/`roots` position sets.
struct TreePlacement<'a, 'b, R: RandomSource> {
    level: &'a mut dyn FeatureLevel,
    semantics: &'a BlockSemantics<'b>,
    random: &'a mut R,
    tree: &'a CompiledTree,
    logs: JavaHashSet,
    foliage: HashSet<BlockPos>,
    roots: JavaHashSet,
}

impl<R: RandomSource> TreePlacement<'_, '_, R> {
    /// Vanilla `TreeFeature.doPlace` plus `TreeFeature.place`'s post-passes.
    fn run(&mut self, origin: BlockPos) -> Result<bool, PlaceError> {
        let tree = self.tree;
        let tree_height = tree.trunk_placer.tree_height(self.random);
        let foliage_height = tree.foliage_placer.foliage_height(self.random, tree_height);
        let trunk_height = tree_height - foliage_height;
        let leaf_radius = tree
            .foliage_placer
            .foliage_radius(self.random, trunk_height);
        // No `root_placer` reaches the A2 closure, so the trunk origin is the
        // placement origin.
        let trunk_origin = origin;
        let min_y = std::cmp::min(origin.y, trunk_origin.y);
        let max_y = std::cmp::max(origin.y, trunk_origin.y) + tree_height + 1;
        if min_y < self.level.min_y() + 1 || max_y > self.level.max_y() + 1 {
            return Ok(false);
        }
        let clipped = self.max_free_tree_height(tree_height, trunk_origin);
        let min_clipped = tree.minimum_size.min_clipped_height;
        if clipped < tree_height && min_clipped.is_none_or(|height| clipped < height) {
            return Ok(false);
        }

        let placer = &tree.foliage_placer;
        let attachments = self.place_trunk(clipped, trunk_origin)?;
        for attachment in attachments {
            placer.create_foliage(self, attachment, foliage_height, leaf_radius);
        }

        if self.logs.is_empty() && self.foliage.is_empty() {
            return Ok(false);
        }
        let Some(bounds) = self.bounds() else {
            return Ok(false);
        };
        self.update_leaves(bounds)?;
        // `StructureTemplate.updateShapeAtEdge` writes nothing for logs and
        // leaves; see the module docs.
        Ok(true)
    }

    /// Vanilla `TreeFeature.getMaxFreeTreeHeight`.
    fn max_free_tree_height(&self, max_tree_height: i32, tree_pos: BlockPos) -> i32 {
        for y in 0..=max_tree_height + 1 {
            let radius = size_at_height(self.tree.minimum_size, max_tree_height, y);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let pos = offset_pos(tree_pos, x, y, z);
                    if !self.tree.trunk_placer.is_free(self, pos)
                        || (!self.tree.ignore_vines
                            && self.semantics.is_vine(self.level.block_state(pos)))
                    {
                        return y - 2;
                    }
                }
            }
        }
        max_tree_height
    }

    /// Vanilla `TrunkPlacer.placeBelowTrunkBlock`.
    fn place_below_trunk_block(&mut self, pos: BlockPos) -> Result<(), PlaceError> {
        if let Some(state) = self.tree.below_trunk_provider.optional_state(
            self.level,
            self.semantics,
            self.random,
            pos,
        ) {
            self.level.set_block(pos, state);
            self.logs.add(pos)?;
        }
        Ok(())
    }

    /// Vanilla `TrunkPlacer.placeLog`.
    fn place_log(&mut self, pos: BlockPos) -> Result<bool, PlaceError> {
        if !self.tree.trunk_placer.is_free(self, pos) {
            return Ok(false);
        }
        let state = self
            .tree
            .trunk_provider
            .state(self.level, self.semantics, self.random, pos);
        self.level.set_block(pos, state);
        self.logs.add(pos)?;
        Ok(true)
    }

    /// `StraightTrunkPlacer.placeTrunk` / `ForkingTrunkPlacer.placeTrunk`.
    fn place_trunk(
        &mut self,
        tree_height: i32,
        origin: BlockPos,
    ) -> Result<Vec<FoliageAttachment>, PlaceError> {
        Ok(match self.tree.trunk_placer {
            CompiledTrunkPlacer::Straight(_) => {
                self.place_below_trunk_block(offset_pos(origin, 0, -1, 0))?;
                for y in 0..tree_height {
                    self.place_log(offset_pos(origin, 0, y, 0))?;
                }
                vec![FoliageAttachment {
                    pos: offset_pos(origin, 0, tree_height, 0),
                    radius_offset: 0,
                    double_trunk: false,
                }]
            }
            CompiledTrunkPlacer::Forking(_) => {
                self.place_below_trunk_block(offset_pos(origin, 0, -1, 0))?;
                let mut attachments = Vec::new();
                let lean = horizontal_direction(self.random);
                let lean_height = tree_height - self.random.next_int_bounded(4) - 1;
                let mut lean_steps = 3 - self.random.next_int_bounded(3);
                let mut tx = origin.x;
                let mut tz = origin.z;
                let mut tip_y = None;

                for yo in 0..tree_height {
                    let y = origin.y + yo;
                    if yo >= lean_height && lean_steps > 0 {
                        tx += lean.step_x();
                        tz += lean.step_z();
                        lean_steps -= 1;
                    }
                    if self.place_log(BlockPos { x: tx, y, z: tz })? {
                        tip_y = Some(y + 1);
                    }
                }
                if let Some(y) = tip_y {
                    attachments.push(FoliageAttachment {
                        pos: BlockPos { x: tx, y, z: tz },
                        radius_offset: 1,
                        double_trunk: false,
                    });
                }

                tx = origin.x;
                tz = origin.z;
                let branch = horizontal_direction(self.random);
                if branch != lean {
                    let branch_pos = lean_height - self.random.next_int_bounded(2) - 1;
                    let mut branch_steps = 1 + self.random.next_int_bounded(3);
                    let mut branch_tip = None;
                    let mut yo = branch_pos;
                    while yo < tree_height && branch_steps > 0 {
                        if yo >= 1 {
                            let y = origin.y + yo;
                            tx += branch.step_x();
                            tz += branch.step_z();
                            if self.place_log(BlockPos { x: tx, y, z: tz })? {
                                branch_tip = Some(y + 1);
                            }
                        }
                        yo += 1;
                        branch_steps -= 1;
                    }
                    if let Some(y) = branch_tip {
                        attachments.push(FoliageAttachment {
                            pos: BlockPos { x: tx, y, z: tz },
                            radius_offset: 0,
                            double_trunk: false,
                        });
                    }
                }
                attachments
            }
        })
    }

    /// Vanilla `FoliagePlacer.placeLeavesRow`.
    fn place_leaves_row(
        &mut self,
        origin: BlockPos,
        current_radius: i32,
        y: i32,
        double_trunk: bool,
        kind: FoliageKind,
    ) {
        let row_offset = if double_trunk { 1 } else { 0 };
        for dx in -current_radius..=current_radius + row_offset {
            for dz in -current_radius..=current_radius + row_offset {
                if CompiledFoliagePlacer::should_skip_location(
                    kind,
                    self.random,
                    dx,
                    y,
                    dz,
                    current_radius,
                    double_trunk,
                ) {
                    continue;
                }
                let pos = offset_pos(origin, dx, y, dz);
                self.try_place_leaf(pos);
            }
        }
    }

    /// Vanilla `FoliagePlacer.tryPlaceLeaf`.
    fn try_place_leaf(&mut self, pos: BlockPos) -> bool {
        let state = self.level.block_state(pos);
        let persistent = self.semantics.property(state, "persistent") == Some("true");
        if persistent || !self.semantics.is_valid_tree_pos(state) {
            return false;
        }
        let mut leaf =
            self.tree
                .foliage_provider
                .state(self.level, self.semantics, self.random, pos);
        if self.semantics.property(leaf, "waterlogged").is_some() {
            let water = self.level.is_water_source_at(pos);
            leaf = self
                .semantics
                .with_property(leaf, "waterlogged", if water { "true" } else { "false" })
                .unwrap_or(leaf);
        }
        self.level.set_block(pos, leaf);
        self.foliage.insert(pos);
        true
    }

    fn bounds(&self) -> Option<Bounds> {
        let mut bounds: Option<Bounds> = None;
        for pos in self
            .roots
            .iter()
            .chain(self.logs.iter())
            .chain(self.foliage.iter().copied())
        {
            bounds = Some(match bounds {
                None => Bounds::at(pos),
                Some(current) => current.with(pos),
            });
        }
        bounds
    }

    /// Vanilla `TreeFeature.updateLeaves`: a breadth-first walk from the logs
    /// that rewrites each reached leaf's `distance` property to its distance
    /// from the nearest log.
    fn update_leaves(&mut self, bounds: Bounds) -> Result<(), PlaceError> {
        let mut filled: HashSet<(i32, i32, i32)> = HashSet::new();
        for pos in self.roots.iter() {
            if bounds.contains(pos) {
                filled.insert(bounds.relative(pos));
            }
        }
        let mut to_check: Vec<JavaHashSet> = (0..7).map(|_| JavaHashSet::new()).collect();
        for pos in self.logs.iter() {
            to_check[0].add(pos)?;
        }

        let mut smallest_distance = 0usize;
        loop {
            while smallest_distance >= 7 || !to_check[smallest_distance].is_empty() {
                if smallest_distance >= 7 {
                    return Ok(());
                }
                let Some(pos) = to_check[smallest_distance].first() else {
                    break;
                };
                to_check[smallest_distance].remove(pos);
                if !bounds.contains(pos) {
                    continue;
                }
                if smallest_distance != 0 {
                    let state = self.level.block_state(pos);
                    debug_assert!(
                        self.semantics.property(state, "distance").is_some(),
                        "only leaf states enter the distance walk"
                    );
                    if let Some(state) = self.semantics.with_property(
                        state,
                        "distance",
                        &smallest_distance.to_string(),
                    ) {
                        self.level.set_block(pos, state);
                    }
                }
                filled.insert(bounds.relative(pos));

                for (dx, dy, dz) in NEIGHBOURS {
                    let neighbour = offset_pos(pos, dx, dy, dz);
                    if !bounds.contains(neighbour) {
                        continue;
                    }
                    if filled.contains(&bounds.relative(neighbour)) {
                        continue;
                    }
                    let current = self.level.block_state(neighbour);
                    let Some(distance) = self.semantics.leaf_distance(current) else {
                        continue;
                    };
                    let new_distance = std::cmp::min(distance, smallest_distance as i32 + 1);
                    if new_distance < 7 {
                        to_check[new_distance as usize].add(neighbour)?;
                        smallest_distance = std::cmp::min(smallest_distance, new_distance as usize);
                    }
                }
            }
            smallest_distance += 1;
        }
    }
}

/// `HashMap.TREEIFY_THRESHOLD`.
const TREEIFY_THRESHOLD: usize = 8;
/// `HashMap.MIN_TREEIFY_CAPACITY`.
const MIN_TREEIFY_CAPACITY: usize = 64;

/// A faithful `java.util.HashSet` iteration order for `BlockPos`.
///
/// `TreeFeature.updateLeaves` rewrites a leaf's `distance` every time it
/// processes that leaf, and a leaf can sit in several distance sets at once, so
/// *which* write lands last depends on the iteration order of those sets.
/// Vanilla's order is `java.util.HashMap`'s: bucket index from
/// `Vec3i.hashCode` (`(y + z*31)*31 + x`) spread by `h ^ (h >>> 16)`, a table
/// that starts at 16 buckets and doubles when the size passes 3/4 of it, and
/// chains walked in insertion order. Rust's `HashSet` is randomised per
/// process, so it cannot stand in.
#[derive(Debug, Clone, Default)]
pub(crate) struct JavaHashSet {
    table: Vec<Vec<BlockPos>>,
    size: usize,
}

impl JavaHashSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// `HashSet.add`, returning whether the set changed (Java reports no
    /// structural modification for a duplicate, which is why vanilla never sees
    /// a `ConcurrentModificationException` here).
    ///
    /// `HashMap.putVal` grows the table when the size passes 3/4 of it and then
    /// treeifies a bin that reaches 8 entries once the table has at least 64
    /// buckets; below that it resizes instead. The resize is modelled, the
    /// treeify is not: past it the emulation would report an order real
    /// `HashMap` does not have, so it stops with a typed error instead. The
    /// check is unconditional, so debug and release behave identically.
    pub(crate) fn add(&mut self, pos: BlockPos) -> Result<bool, PlaceError> {
        if self.table.is_empty() {
            self.resize();
        }
        let index = self.bucket(pos);
        if self.table[index].contains(&pos) {
            return Ok(false);
        }
        self.table[index].push(pos);
        self.size += 1;
        if self.size > self.table.len() * 3 / 4 {
            self.resize();
        }
        let index = self.bucket(pos);
        let bin_size = self.table[index].len();
        if bin_size >= TREEIFY_THRESHOLD {
            if self.table.len() >= MIN_TREEIFY_CAPACITY {
                return Err(PlaceError::TreeOrderBoundary {
                    bin_size,
                    capacity: self.table.len(),
                    pos,
                });
            }
            self.resize();
        }
        Ok(true)
    }

    fn remove(&mut self, pos: BlockPos) {
        if self.table.is_empty() {
            return;
        }
        let index = self.bucket(pos);
        if let Some(position) = self.table[index].iter().position(|entry| *entry == pos) {
            self.table[index].remove(position);
            self.size -= 1;
        }
    }

    /// `iterator().next()`: the first entry in bucket order.
    fn first(&self) -> Option<BlockPos> {
        self.table.iter().find_map(|bucket| bucket.first().copied())
    }

    /// `for (BlockPos pos : set)`.
    fn iter(&self) -> impl Iterator<Item = BlockPos> + '_ {
        self.table.iter().flat_map(|bucket| bucket.iter().copied())
    }

    fn resize(&mut self) {
        let old_capacity = self.table.len();
        let capacity = if old_capacity == 0 {
            16
        } else {
            old_capacity * 2
        };
        let mut table: Vec<Vec<BlockPos>> = vec![Vec::new(); capacity];
        for (index, bucket) in self.table.iter().enumerate() {
            let (low, high): (Vec<BlockPos>, Vec<BlockPos>) = bucket
                .iter()
                .copied()
                .partition(|pos| hash_high_bit(*pos, old_capacity) == 0);
            table[index] = low;
            table[index + old_capacity] = high;
        }
        self.table = table;
    }

    fn bucket(&self, pos: BlockPos) -> usize {
        (self.table.len() - 1) & java_hash(pos) as usize
    }
}

/// `Vec3i.hashCode()` spread by `HashMap.hash`.
pub(crate) fn java_hash(pos: BlockPos) -> u32 {
    let hash = pos
        .y
        .wrapping_add(pos.z.wrapping_mul(31))
        .wrapping_mul(31)
        .wrapping_add(pos.x);
    (hash ^ ((hash as u32 >> 16) as i32)) as u32
}

/// `(e.hash & oldCap) == 0`, the split `HashMap.resize` performs.
fn hash_high_bit(pos: BlockPos, old_capacity: usize) -> u32 {
    java_hash(pos) & old_capacity as u32
}

fn offset_pos(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

/// `Direction.values()`, the order `updateLeaves` scans neighbours in.
const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

/// `Direction.Plane.HORIZONTAL` in vanilla's order: north, east, south, west.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Horizontal {
    North,
    East,
    South,
    West,
}

impl Horizontal {
    fn step_x(self) -> i32 {
        match self {
            Self::East => 1,
            Self::West => -1,
            Self::North | Self::South => 0,
        }
    }

    fn step_z(self) -> i32 {
        match self {
            Self::South => 1,
            Self::North => -1,
            Self::East | Self::West => 0,
        }
    }
}

/// `Plane.HORIZONTAL.getRandomDirection(random)` =
/// `Util.getRandom(faces, random)`.
fn horizontal_direction(random: &mut impl RandomSource) -> Horizontal {
    match random.next_int_bounded(4) {
        0 => Horizontal::North,
        1 => Horizontal::East,
        2 => Horizontal::South,
        _ => Horizontal::West,
    }
}

/// `BoundingBox.encapsulatingPositions`.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: BlockPos,
    max: BlockPos,
}

impl Bounds {
    fn at(pos: BlockPos) -> Self {
        Self { min: pos, max: pos }
    }

    fn with(self, pos: BlockPos) -> Self {
        Self {
            min: BlockPos {
                x: self.min.x.min(pos.x),
                y: self.min.y.min(pos.y),
                z: self.min.z.min(pos.z),
            },
            max: BlockPos {
                x: self.max.x.max(pos.x),
                y: self.max.y.max(pos.y),
                z: self.max.z.max(pos.z),
            },
        }
    }

    fn contains(self, pos: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&pos.x)
            && (self.min.y..=self.max.y).contains(&pos.y)
            && (self.min.z..=self.max.z).contains(&pos.z)
    }

    fn relative(self, pos: BlockPos) -> (i32, i32, i32) {
        (pos.x - self.min.x, pos.y - self.min.y, pos.z - self.min.z)
    }
}

/// Vanilla `TwoLayersFeatureSize.getSizeAtHeight`.
fn size_at_height(size: TwoLayersFeatureSize, _tree_height: i32, yo: i32) -> i32 {
    if yo < size.limit {
        size.lower_size
    } else {
        size.upper_size
    }
}
