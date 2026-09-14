//! Voxel-shape algebra, the free-space test the jigsaw placer is built on.
//!
//! `JigsawPlacement$Placer` keeps one `childrenFree` shape per parent and lets a
//! candidate child through when the child's box deflated by a quarter is
//! entirely inside that free shape, then removes the child's box from it:
//!
//! ```text
//! childrenFree = Shapes.create(AABB.of(sourceBB))                                    // first child
//! if !Shapes.joinIsNotEmpty(childrenFree, Shapes.create(AABB.of(targetBB).deflate(0.25)), BooleanOp.ONLY_SECOND) {
//!     childrenFree = Shapes.joinUnoptimized(childrenFree, Shapes.create(AABB.of(targetBB)), BooleanOp.ONLY_FIRST);
//! }
//! ```
//!
//! so `join_is_not_empty(free, deflated_target, OnlySecond)` answers "is any part
//! of the deflated target box *not* already free", and the `OnlyFirst` join is
//! "free space minus the child box". This module reproduces the vanilla algebra
//! behind those calls, transcribed from the 26.1.2 bodies of
//! `net.minecraft.world.phys.shapes.Shapes`, `VoxelShape`, `CubeVoxelShape`,
//! `BitSetDiscreteVoxelShape`, `DiscreteVoxelShape` and
//! `net.minecraft.world.phys.AABB`.
//!
//! ## How a box becomes a grid
//!
//! `Shapes.create` splits each axis into `1 << findBits(min, max)` cells, with
//! `findBits` trying 1, 2, 4 and 8 subdivisions and accepting the first where
//! both endpoints are within `1e-7 * intervals` of a grid line. A box outside
//! `[-1e-7, 1.0000001]`, or one whose endpoints never land on a line (every
//! multiple of a sixteenth, for example), falls back to an `ArrayVoxelShape`:
//! the exact endpoints become the axis' two coordinates while the discrete
//! shape stays a single occupied `1x1x1` cell — the finest grid is a **1/8**,
//! not a 1/16, and non-representable boxes are *not* snapped to it.
//!
//! ## How two grids combine
//!
//! `join_unoptimized` runs the three axes through `createIndexMerger`, which
//! picks vanilla's cheapest `IndexMerger` — `DiscreteCubeMerger` (both axes are
//! cube ranges and `cost * lcm <= 256`), `NonOverlappingMerger` (a `1e-7`-clean
//! gap on that axis), `IdenticalMerger` (equal coordinate lists) or
//! `IndirectMerger` (a tolerance-merged walk of both lists). The merged index
//! triple then feeds `BooleanOp.apply` per cell, with indices outside a shape
//! evaluating to `false`. Two consequences matter to a caller:
//!
//! - `join_is_not_empty(free, box, OnlySecond)` is a *containment* test on the
//!   discretised grids, not an AABB overlap test: a target strictly inside a
//!   free cell reports `false` even though the boxes overlap.
//! - `IndirectMerger` merges coordinates closer than `1e-7` onto one grid line,
//!   so an overlap thinner than the tolerance disappears (`Both` reports
//!   `false` for boxes that do overlap by `5e-8`) and `canSkipFirst` /
//!   `canSkipSecond` drop the boundary lines that cannot contribute.
//!
//! `join` additionally runs `VoxelShape.optimize`, which decomposes the joined
//! grid into maximal boxes and rebuilds it — vanilla's canonical form.
//!
//! Numbers in this module were pinned against the real classes: see
//! `village_shapes_tests.rs`.

/// `Shapes.EPSILON`: the tolerance every coordinate comparison uses.
pub const EPSILON: f64 = 1.0e-7;

/// `Direction.Axis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// `AxisCycle.AXIS_VALUES`: the order `Shapes.joinIsNotEmpty` tests axes in.
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    const fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// `net.minecraft.world.phys.AABB`.
///
/// The constructor clamps with `min`/`max`, so an inverted box (a `deflate`
/// wider than the box) is normalised to a swapped, still non-empty box rather
/// than becoming empty; only a zero-width box discretises to the empty shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl Aabb {
    /// `new AABB(double, double, double, double, double, double)`.
    #[must_use]
    pub fn new(min_x: f64, min_y: f64, min_z: f64, max_x: f64, max_y: f64, max_z: f64) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            min_z: min_z.min(max_z),
            max_x: min_x.max(max_x),
            max_y: min_y.max(max_y),
            max_z: min_z.max(max_z),
        }
    }

    /// `AABB.of(BoundingBox)`: the integer bounds are inclusive, so the double
    /// box runs from `min` to `max + 1`.
    #[must_use]
    pub fn of(min_x: i32, min_y: i32, min_z: i32, max_x: i32, max_y: i32, max_z: i32) -> Self {
        Self::new(
            f64::from(min_x),
            f64::from(min_y),
            f64::from(min_z),
            f64::from(max_x) + 1.0,
            f64::from(max_y) + 1.0,
            f64::from(max_z) + 1.0,
        )
    }

    /// `AABB.deflate(double)`.
    #[must_use]
    pub fn deflate(self, amount: f64) -> Self {
        self.inflate(-amount)
    }

    /// `AABB.inflate(double)`.
    #[must_use]
    pub fn inflate(self, amount: f64) -> Self {
        Self::new(
            self.min_x - amount,
            self.min_y - amount,
            self.min_z - amount,
            self.max_x + amount,
            self.max_y + amount,
            self.max_z + amount,
        )
    }

    /// `AABB.min(Axis)`.
    #[must_use]
    pub fn min(self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.min_x,
            Axis::Y => self.min_y,
            Axis::Z => self.min_z,
        }
    }

    /// `AABB.max(Axis)`.
    #[must_use]
    pub fn max(self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.max_x,
            Axis::Y => self.max_y,
            Axis::Z => self.max_z,
        }
    }
}

/// The `BooleanOp` modes the jigsaw free-space test needs.
///
/// `Both` is vanilla's `AND`. Vanilla's `OR` is only used internally by
/// `optimize`, and `FALSE`/`TRUE`/the negations are never reached from
/// `joinUnoptimized`/`joinIsNotEmpty` with a valid `(false, false)` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    /// `BooleanOp.ONLY_FIRST`: in the first shape and not the second.
    OnlyFirst,
    /// `BooleanOp.ONLY_SECOND`: in the second shape and not the first.
    OnlySecond,
    /// `BooleanOp.AND`: in both.
    Both,
}

impl BooleanOp {
    /// `BooleanOp.apply`.
    #[must_use]
    pub fn apply(self, first: bool, second: bool) -> bool {
        Combine::from(self).apply(first, second)
    }
}

/// `BooleanOp` plus the `OR` that `VoxelShape.optimize` folds with.
#[derive(Debug, Clone, Copy)]
enum Combine {
    OnlyFirst,
    OnlySecond,
    Both,
    Or,
}

impl Combine {
    fn apply(self, first: bool, second: bool) -> bool {
        match self {
            Combine::OnlyFirst => first && !second,
            Combine::OnlySecond => second && !first,
            Combine::Both => first && second,
            Combine::Or => first || second,
        }
    }

    /// `BooleanOp.apply(true, false)`.
    fn first_only_matters(self) -> bool {
        self.apply(true, false)
    }

    /// `BooleanOp.apply(false, true)`.
    fn second_only_matters(self) -> bool {
        self.apply(false, true)
    }
}

impl From<BooleanOp> for Combine {
    fn from(op: BooleanOp) -> Self {
        match op {
            BooleanOp::OnlyFirst => Combine::OnlyFirst,
            BooleanOp::OnlySecond => Combine::OnlySecond,
            BooleanOp::Both => Combine::Both,
        }
    }
}

/// One axis of a shape's coordinate list: either vanilla's `CubePointRange`
/// (`i / parts`) or a `DoubleArrayList`. `Shapes.createIndexMerger` branches on
/// the concrete `CubePointRange` type, so the distinction survives joins.
#[derive(Debug, Clone)]
enum Coords {
    Cube { parts: usize },
    Array(Vec<f64>),
}

impl Coords {
    fn size(&self) -> usize {
        match self {
            Coords::Cube { parts } => parts + 1,
            Coords::Array(values) => values.len(),
        }
    }

    fn get(&self, index: usize) -> f64 {
        match self {
            Coords::Cube { parts } => index as f64 / *parts as f64,
            Coords::Array(values) => values[index],
        }
    }

    fn is_cube(&self) -> bool {
        matches!(self, Coords::Cube { .. })
    }

    /// `Objects.equals(DoubleList, DoubleList)`: element-wise, ignoring which
    /// implementation produced the list.
    fn equals(&self, other: &Coords) -> bool {
        self.size() == other.size() && (0..self.size()).all(|i| self.get(i) == other.get(i))
    }

    fn to_values(&self) -> Vec<f64> {
        (0..self.size()).map(|i| self.get(i)).collect()
    }
}

/// `BitSetDiscreteVoxelShape`: occupied cells plus the cached `firstFull` /
/// `lastFull` bounds every `min`/`max` and `isEmpty` answer reads.
#[derive(Debug, Clone)]
struct DiscreteShape {
    sizes: [usize; 3],
    storage: Vec<bool>,
    min: [i32; 3],
    max: [i32; 3],
}

impl DiscreteShape {
    /// `new BitSetDiscreteVoxelShape(xSize, ySize, zSize)`: empty storage, and
    /// `min` starts at the size (so `min(axis)` is `+inf`) while `max` is 0
    /// (so `max(axis)` is `-inf`).
    fn new(sizes: [usize; 3]) -> Self {
        Self {
            sizes,
            storage: vec![false; sizes[0] * sizes[1] * sizes[2]],
            min: [sizes[0] as i32, sizes[1] as i32, sizes[2] as i32],
            max: [0, 0, 0],
        }
    }

    /// The `Shapes.BLOCK` / `ArrayVoxelShape` fallback shape: one occupied cell.
    fn block() -> Self {
        Self::with_filled_bounds([1, 1, 1], [0, 0, 0], [1, 1, 1])
    }

    /// `BitSetDiscreteVoxelShape.withFilledBounds`.
    fn with_filled_bounds(sizes: [usize; 3], min: [i32; 3], max: [i32; 3]) -> Self {
        let mut shape = Self::new(sizes);
        shape.min = min;
        shape.max = max;
        for x in min[0]..max[0] {
            for y in min[1]..max[1] {
                for z in min[2]..max[2] {
                    shape.fill_update_bounds([x, y, z], false);
                }
            }
        }
        shape
    }

    /// The `BitSetDiscreteVoxelShape(DiscreteVoxelShape)` copy constructor.
    fn copy_from(other: &DiscreteShape) -> Self {
        let mut shape = Self {
            sizes: other.sizes,
            storage: other.storage.clone(),
            min: [0; 3],
            max: [0; 3],
        };
        for axis in Axis::ALL {
            shape.min[axis.index()] = other.first_full(axis);
            shape.max[axis.index()] = other.last_full(axis);
        }
        shape
    }

    fn index(&self, cell: [usize; 3]) -> usize {
        (cell[0] * self.sizes[1] + cell[1]) * self.sizes[2] + cell[2]
    }

    /// `isFull` (no bounds check, callers must stay in range).
    fn is_full(&self, cell: [usize; 3]) -> bool {
        self.storage.get(self.index(cell)).copied().unwrap_or(false)
    }

    /// `DiscreteVoxelShape.isFullWide`: `isFull` with out-of-range cells false.
    fn is_full_wide(&self, cell: [i32; 3]) -> bool {
        if cell.iter().any(|&c| c < 0) {
            return false;
        }
        let cell = [cell[0] as usize, cell[1] as usize, cell[2] as usize];
        (0..3).all(|axis| cell[axis] < self.sizes[axis]) && self.is_full(cell)
    }

    fn fill_update_bounds(&mut self, cell: [i32; 3], update_bounds: bool) {
        let index = self.index([cell[0] as usize, cell[1] as usize, cell[2] as usize]);
        self.storage[index] = true;
        if update_bounds {
            for (axis, &coordinate) in cell.iter().enumerate() {
                self.min[axis] = self.min[axis].min(coordinate);
                self.max[axis] = self.max[axis].max(coordinate + 1);
            }
        }
    }

    /// `isEmpty`: the storage, not the bounds.
    fn is_empty(&self) -> bool {
        !self.storage.iter().any(|&full| full)
    }

    fn first_full(&self, axis: Axis) -> i32 {
        self.min[axis.index()]
    }

    fn last_full(&self, axis: Axis) -> i32 {
        self.max[axis.index()]
    }

    /// `BitSetDiscreteVoxelShape.clearZStrip`.
    fn clear_z_strip(&mut self, strip: [usize; 4]) {
        let start = self.index([strip[2], strip[3], strip[0]]);
        let end = self.index([strip[2], strip[3], strip[1]]);
        for bit in &mut self.storage[start..end] {
            *bit = false;
        }
    }

    /// `BitSetDiscreteVoxelShape.isZStripFull`.
    fn is_z_strip_full(&self, strip: [usize; 4]) -> bool {
        if strip[2] >= self.sizes[0] || strip[3] >= self.sizes[1] {
            return false;
        }
        let from = self.index([strip[2], strip[3], strip[0]]);
        let end = self.index([strip[2], strip[3], strip[1]]);
        // `BitSet.nextClearBit(from) >= end`: no cell of the strip is free.
        (from..end).all(|bit| self.storage.get(bit).copied().unwrap_or(false))
    }

    /// `BitSetDiscreteVoxelShape.isXZRectangleFull`.
    fn is_xz_rectangle_full(&self, rect: [usize; 5]) -> bool {
        (rect[0]..rect[1]).all(|x| self.is_z_strip_full([rect[2], rect[3], x, rect[4]]))
    }

    /// `BitSetDiscreteVoxelShape.forAllBoxes`, emitting integer cells.
    fn for_all_boxes(&self, merge_neighbors: bool, consumer: &mut impl FnMut([i32; 3], [i32; 3])) {
        let mut shape = DiscreteShape::copy_from(self);
        for y in 0..shape.sizes[1] {
            for x in 0..shape.sizes[0] {
                let mut last_start_z: i32 = -1;
                for z in 0..=shape.sizes[2] {
                    if shape.is_full_wide([x as i32, y as i32, z as i32]) {
                        if merge_neighbors {
                            if last_start_z == -1 {
                                last_start_z = z as i32;
                            }
                        } else {
                            consumer(
                                [x as i32, y as i32, z as i32],
                                [x as i32 + 1, y as i32 + 1, z as i32 + 1],
                            );
                        }
                    } else if last_start_z != -1 {
                        let (mut end_x, mut end_y) = (x, y);
                        let start_z = last_start_z as usize;
                        shape.clear_z_strip([start_z, z, x, y]);
                        while shape.is_z_strip_full([start_z, z, end_x + 1, y]) {
                            shape.clear_z_strip([start_z, z, end_x + 1, y]);
                            end_x += 1;
                        }
                        while shape.is_xz_rectangle_full([x, end_x + 1, start_z, z, end_y + 1]) {
                            for cx in x..=end_x {
                                shape.clear_z_strip([start_z, z, cx, end_y + 1]);
                            }
                            end_y += 1;
                        }
                        consumer(
                            [x as i32, y as i32, last_start_z],
                            [end_x as i32 + 1, end_y as i32 + 1, z as i32],
                        );
                        last_start_z = -1;
                    }
                }
            }
        }
    }
}

/// `IndexMerger`: how two coordinate lists on one axis merge into a third.
#[derive(Debug, Clone)]
enum IndexMerger {
    /// `DiscreteCubeMerger`: both sides are cube ranges, so cells map by index
    /// division.
    DiscreteCube {
        result_parts: usize,
        first_div: usize,
        second_div: usize,
    },
    /// `IdenticalMerger`: equal coordinate lists.
    Identical { coords: Coords },
    /// `NonOverlappingMerger`: one list ends before the other starts, within
    /// `EPSILON`.
    NonOverlapping {
        lower: Coords,
        upper: Coords,
        swap: bool,
    },
    /// `IndirectMerger`: the tolerance-merged walk of both lists.
    Indirect {
        result: Vec<f64>,
        first_indices: Vec<i32>,
        second_indices: Vec<i32>,
        result_length: usize,
    },
}

impl IndexMerger {
    fn size(&self) -> usize {
        match self {
            IndexMerger::DiscreteCube { result_parts, .. } => result_parts + 1,
            IndexMerger::Identical { coords } => coords.size(),
            IndexMerger::NonOverlapping { lower, upper, .. } => lower.size() + upper.size(),
            IndexMerger::Indirect { result_length, .. } => *result_length,
        }
    }

    fn is_discrete_cube(&self) -> bool {
        matches!(self, IndexMerger::DiscreteCube { .. })
    }

    /// `IndexMerger.getList`.
    fn list(&self) -> Coords {
        match self {
            IndexMerger::DiscreteCube { result_parts, .. } => Coords::Cube {
                parts: *result_parts,
            },
            IndexMerger::Identical { coords } => coords.clone(),
            IndexMerger::NonOverlapping { lower, upper, swap } => {
                let (lower, upper) = if *swap {
                    (upper, lower)
                } else {
                    (lower, upper)
                };
                let mut values = lower.to_values();
                values.extend(upper.to_values());
                Coords::Array(values)
            }
            IndexMerger::Indirect {
                result,
                result_length,
                ..
            } => {
                if *result_length <= 1 {
                    Coords::Array(vec![0.0])
                } else {
                    Coords::Array(result[..*result_length].to_vec())
                }
            }
        }
    }

    /// `IndexMerger.forMergedIndexes`: `false` as soon as the consumer does.
    fn for_merged_indexes(&self, consumer: &mut impl FnMut(i32, i32, usize) -> bool) -> bool {
        match self {
            IndexMerger::DiscreteCube {
                result_parts,
                first_div,
                second_div,
            } => (0..*result_parts)
                .all(|i| consumer((i / second_div) as i32, (i / first_div) as i32, i)),
            IndexMerger::Identical { coords } => {
                (0..coords.size() - 1).all(|i| consumer(i as i32, i as i32, i))
            }
            IndexMerger::NonOverlapping { lower, upper, swap } => {
                let lower_size = lower.size();
                let first = |i: usize| -> i32 { if *swap { -1 } else { i as i32 } };
                let second = |i: usize| -> i32 { if *swap { i as i32 } else { -1 } };
                if !(0..lower_size).all(|i| consumer(first(i), second(i), i)) {
                    return false;
                }
                (0..upper.size() - 1).all(|i| {
                    let (a, b) = ((lower_size - 1) as i32, i as i32);
                    if *swap {
                        consumer(b, a, lower_size + i)
                    } else {
                        consumer(a, b, lower_size + i)
                    }
                })
            }
            IndexMerger::Indirect {
                first_indices,
                second_indices,
                result_length,
                ..
            } => (0..result_length - 1).all(|i| consumer(first_indices[i], second_indices[i], i)),
        }
    }

    /// `IndirectMerger.<init>`.
    fn indirect(
        first: &Coords,
        second: &Coords,
        first_only_matters: bool,
        second_only_matters: bool,
    ) -> Self {
        let (first_size, second_size) = (first.size(), second.size());
        let mut result = Vec::with_capacity(first_size + second_size);
        let mut first_indices = Vec::with_capacity(first_size + second_size);
        let mut second_indices = Vec::with_capacity(first_size + second_size);
        let can_skip_first = !first_only_matters;
        let can_skip_second = !second_only_matters;
        let mut last_value = f64::NAN;
        let mut first_index = 0;
        let mut second_index = 0;
        loop {
            let ran_out_of_first = first_index >= first_size;
            let ran_out_of_second = second_index >= second_size;
            if ran_out_of_first && ran_out_of_second {
                return IndexMerger::Indirect {
                    result_length: result.len().max(1),
                    result,
                    first_indices,
                    second_indices,
                };
            }
            let chose_first = !ran_out_of_first
                && (ran_out_of_second
                    || first.get(first_index) < second.get(second_index) + EPSILON);
            if chose_first {
                first_index += 1;
                if can_skip_first && (second_index == 0 || ran_out_of_second) {
                    continue;
                }
            } else {
                second_index += 1;
                if can_skip_second && (first_index == 0 || ran_out_of_first) {
                    continue;
                }
            }
            let current_first = first_index as i32 - 1;
            let current_second = second_index as i32 - 1;
            let next_value = if chose_first {
                first.get(first_index - 1)
            } else {
                second.get(second_index - 1)
            };
            // `!(lastValue >= nextValue - EPSILON)`, with `lastValue` `NaN` on
            // the first step so the first coordinate always lands in the list.
            let is_new_line = last_value.is_nan() || last_value < next_value - EPSILON;
            if is_new_line {
                first_indices.push(current_first);
                second_indices.push(current_second);
                result.push(next_value);
                last_value = next_value;
            } else {
                let last = first_indices.len() - 1;
                first_indices[last] = current_first;
                second_indices[last] = current_second;
            }
        }
    }
}

/// A discretised shape: `VoxelShape` (`CubeVoxelShape` or `ArrayVoxelShape`) and
/// the coordinate list of each axis.
#[derive(Debug, Clone)]
pub struct VoxelShape {
    shape: DiscreteShape,
    coords: [Coords; 3],
}

impl VoxelShape {
    /// `Shapes.create(AABB)`.
    #[must_use]
    pub fn create(aabb: Aabb) -> Self {
        Self::create_box(
            aabb.min_x, aabb.min_y, aabb.min_z, aabb.max_x, aabb.max_y, aabb.max_z,
        )
    }

    /// `Shapes.create(double, double, double, double, double, double)`.
    ///
    /// A box thinner than `EPSILON` on any axis is `empty()`. A box outside the
    /// unit range, or with endpoints that miss every 1/8 grid line, becomes the
    /// exact-coordinate `ArrayVoxelShape` with a single occupied cell (not a
    /// snap to the grid). Otherwise each axis gets `1 << findBits` cells and the
    /// box is rounded onto that grid.
    #[must_use]
    pub fn create_box(
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> Self {
        if max_x - min_x < EPSILON || max_y - min_y < EPSILON || max_z - min_z < EPSILON {
            return Self::empty();
        }
        let min = [min_x, min_y, min_z];
        let max = [max_x, max_y, max_z];
        let bits = [
            find_bits(min_x, max_x),
            find_bits(min_y, max_y),
            find_bits(min_z, max_z),
        ];
        if bits.iter().any(|&bits| bits < 0) {
            return Self::array(
                DiscreteShape::block(),
                [
                    Coords::Array(vec![min_x, max_x]),
                    Coords::Array(vec![min_y, max_y]),
                    Coords::Array(vec![min_z, max_z]),
                ],
            );
        }
        if bits.iter().all(|&bits| bits == 0) {
            return Self::cube(DiscreteShape::block());
        }
        let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
        let mut low = [0_i32; 3];
        let mut high = [0_i32; 3];
        for axis in Axis::ALL {
            let scale = sizes[axis.index()] as f64;
            low[axis.index()] = java_round(min[axis.index()] * scale) as i32;
            high[axis.index()] = java_round(max[axis.index()] * scale) as i32;
        }
        Self::cube(DiscreteShape::with_filled_bounds(sizes, low, high))
    }

    /// `Shapes.empty()`.
    #[must_use]
    pub fn empty() -> Self {
        Self::array(
            DiscreteShape::new([0, 0, 0]),
            [
                Coords::Array(vec![0.0]),
                Coords::Array(vec![0.0]),
                Coords::Array(vec![0.0]),
            ],
        )
    }

    fn cube(shape: DiscreteShape) -> Self {
        let parts = shape.sizes;
        Self::array(
            shape,
            [
                Coords::Cube { parts: parts[0] },
                Coords::Cube { parts: parts[1] },
                Coords::Cube { parts: parts[2] },
            ],
        )
    }

    fn array(shape: DiscreteShape, coords: [Coords; 3]) -> Self {
        Self { shape, coords }
    }

    fn coords(&self, axis: Axis) -> &Coords {
        &self.coords[axis.index()]
    }

    /// `VoxelShape.isEmpty`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shape.is_empty()
    }

    /// `DiscreteVoxelShape.getSize`: how many cells the shape holds on `axis`.
    #[must_use]
    pub fn grid_size(&self, axis: Axis) -> usize {
        self.shape.sizes[axis.index()]
    }

    /// `VoxelShape.min(Axis)`: `+inf` for an empty shape.
    #[must_use]
    pub fn min(&self, axis: Axis) -> f64 {
        let index = self.shape.first_full(axis);
        if index >= self.shape.sizes[axis.index()] as i32 {
            f64::INFINITY
        } else {
            self.coords(axis).get(index as usize)
        }
    }

    /// `VoxelShape.max(Axis)`: `-inf` for an empty shape.
    #[must_use]
    pub fn max(&self, axis: Axis) -> f64 {
        let index = self.shape.last_full(axis);
        if index <= 0 {
            f64::NEG_INFINITY
        } else {
            self.coords(axis).get(index as usize)
        }
    }

    /// `Shapes.join`: `joinUnoptimized` followed by `VoxelShape.optimize`.
    #[must_use]
    pub fn join(&self, other: &VoxelShape, op: BooleanOp) -> VoxelShape {
        join_unoptimized(self, other, op.into()).optimize()
    }

    /// `Shapes.joinUnoptimized`: the merged-grid combination, without the
    /// `optimize` re-discretisation. This is what the jigsaw placer stores as
    /// the running free space, and its grid is the merged coordinate lists of
    /// both operands.
    #[must_use]
    pub fn join_unoptimized(&self, other: &VoxelShape, op: BooleanOp) -> VoxelShape {
        join_unoptimized(self, other, op.into())
    }

    /// `Shapes.joinIsNotEmpty(first, second, op)`: whether any merged cell
    /// satisfies `op`. For `OnlySecond` on a target box this is "part of the
    /// second shape is not in the first", i.e. not covered by the free space.
    #[must_use]
    pub fn join_is_not_empty(first: &VoxelShape, second: &VoxelShape, op: BooleanOp) -> bool {
        join_is_not_empty(first, second, op.into())
    }

    /// `VoxelShape.toAabbs`: the maximal boxes the shape decomposes into.
    #[must_use]
    pub fn to_aabbs(&self) -> Vec<Aabb> {
        let mut boxes = Vec::new();
        self.for_all_boxes(&mut |low, high| {
            boxes.push(Aabb::new(low[0], low[1], low[2], high[0], high[1], high[2]));
        });
        boxes
    }

    fn for_all_boxes(&self, consumer: &mut impl FnMut([f64; 3], [f64; 3])) {
        self.shape.for_all_boxes(true, &mut |low, high| {
            consumer(
                [
                    self.coords(Axis::X).get(low[0] as usize),
                    self.coords(Axis::Y).get(low[1] as usize),
                    self.coords(Axis::Z).get(low[2] as usize),
                ],
                [
                    self.coords(Axis::X).get(high[0] as usize),
                    self.coords(Axis::Y).get(high[1] as usize),
                    self.coords(Axis::Z).get(high[2] as usize),
                ],
            );
        });
    }

    /// `VoxelShape.optimize`: decompose into maximal boxes and fold them back
    /// with `OR`.
    fn optimize(&self) -> VoxelShape {
        let mut boxes = Vec::new();
        self.for_all_boxes(&mut |low, high| boxes.push([low, high]));
        let mut result = VoxelShape::empty();
        for [low, high] in boxes {
            let cell = VoxelShape::create_box(low[0], low[1], low[2], high[0], high[1], high[2]);
            result = join_unoptimized(&result, &cell, Combine::Or);
        }
        result
    }
}

/// `Shapes.joinUnoptimized`.
fn join_unoptimized(first: &VoxelShape, second: &VoxelShape, op: Combine) -> VoxelShape {
    assert!(
        !op.apply(false, false),
        "boolean op is true on two empty shapes"
    );
    if std::ptr::eq(first, second) {
        return if op.apply(true, true) {
            first.clone()
        } else {
            VoxelShape::empty()
        };
    }
    let first_only_matters = op.first_only_matters();
    let second_only_matters = op.second_only_matters();
    if first.is_empty() {
        return if second_only_matters {
            second.clone()
        } else {
            VoxelShape::empty()
        };
    }
    if second.is_empty() {
        return if first_only_matters {
            first.clone()
        } else {
            VoxelShape::empty()
        };
    }
    let x_merger = create_index_merger(
        1,
        first.coords(Axis::X),
        second.coords(Axis::X),
        first_only_matters,
        second_only_matters,
    );
    let y_merger = create_index_merger(
        x_merger.size() - 1,
        first.coords(Axis::Y),
        second.coords(Axis::Y),
        first_only_matters,
        second_only_matters,
    );
    let z_merger = create_index_merger(
        (x_merger.size() - 1) * (y_merger.size() - 1),
        first.coords(Axis::Z),
        second.coords(Axis::Z),
        first_only_matters,
        second_only_matters,
    );
    let shape = join_shape(
        &first.shape,
        &second.shape,
        &x_merger,
        &y_merger,
        &z_merger,
        op,
    );
    if x_merger.is_discrete_cube() && y_merger.is_discrete_cube() && z_merger.is_discrete_cube() {
        VoxelShape::cube(shape)
    } else {
        VoxelShape::array(shape, [x_merger.list(), y_merger.list(), z_merger.list()])
    }
}

/// `Shapes.joinIsNotEmpty`.
fn join_is_not_empty(first: &VoxelShape, second: &VoxelShape, op: Combine) -> bool {
    assert!(
        !op.apply(false, false),
        "boolean op is true on two empty shapes"
    );
    let first_empty = first.is_empty();
    let second_empty = second.is_empty();
    if first_empty || second_empty {
        return op.apply(!first_empty, !second_empty);
    }
    if std::ptr::eq(first, second) {
        return op.apply(true, true);
    }
    let first_only_matters = op.first_only_matters();
    let second_only_matters = op.second_only_matters();
    for axis in Axis::ALL {
        if first.max(axis) < second.min(axis) - EPSILON
            || second.max(axis) < first.min(axis) - EPSILON
        {
            return first_only_matters || second_only_matters;
        }
    }
    let x_merger = create_index_merger(
        1,
        first.coords(Axis::X),
        second.coords(Axis::X),
        first_only_matters,
        second_only_matters,
    );
    let y_merger = create_index_merger(
        x_merger.size() - 1,
        first.coords(Axis::Y),
        second.coords(Axis::Y),
        first_only_matters,
        second_only_matters,
    );
    let z_merger = create_index_merger(
        (x_merger.size() - 1) * (y_merger.size() - 1),
        first.coords(Axis::Z),
        second.coords(Axis::Z),
        first_only_matters,
        second_only_matters,
    );
    !x_merger.for_merged_indexes(&mut |x1, x2, _| {
        y_merger.for_merged_indexes(&mut |y1, y2, _| {
            z_merger.for_merged_indexes(&mut |z1, z2, _| {
                !op.apply(
                    first.shape.is_full_wide([x1, y1, z1]),
                    second.shape.is_full_wide([x2, y2, z2]),
                )
            })
        })
    })
}

/// `Shapes.createIndexMerger`.
fn create_index_merger(
    cost: usize,
    first: &Coords,
    second: &Coords,
    first_only_matters: bool,
    second_only_matters: bool,
) -> IndexMerger {
    let first_cells = first.size() - 1;
    let second_cells = second.size() - 1;
    if first.is_cube() && second.is_cube() {
        let cells = lcm(first_cells, second_cells);
        if cost as u64 * cells as u64 <= 256 {
            let divisor = gcd(first_cells, second_cells);
            return IndexMerger::DiscreteCube {
                result_parts: cells,
                first_div: first_cells / divisor,
                second_div: second_cells / divisor,
            };
        }
    }
    if first.get(first_cells) < second.get(0) - EPSILON {
        return IndexMerger::NonOverlapping {
            lower: first.clone(),
            upper: second.clone(),
            swap: false,
        };
    }
    if second.get(second_cells) < first.get(0) - EPSILON {
        return IndexMerger::NonOverlapping {
            lower: second.clone(),
            upper: first.clone(),
            swap: true,
        };
    }
    if first_cells == second_cells && first.equals(second) {
        return IndexMerger::Identical {
            coords: first.clone(),
        };
    }
    IndexMerger::indirect(first, second, first_only_matters, second_only_matters)
}

/// `BitSetDiscreteVoxelShape.join`.
fn join_shape(
    first: &DiscreteShape,
    second: &DiscreteShape,
    x_merger: &IndexMerger,
    y_merger: &IndexMerger,
    z_merger: &IndexMerger,
    op: Combine,
) -> DiscreteShape {
    let mut shape = DiscreteShape::new([
        x_merger.size() - 1,
        y_merger.size() - 1,
        z_merger.size() - 1,
    ]);
    let mut bounds = [i32::MAX, i32::MAX, i32::MAX, i32::MIN, i32::MIN, i32::MIN];
    x_merger.for_merged_indexes(&mut |x1, x2, xr| {
        let mut updated_slice = false;
        y_merger.for_merged_indexes(&mut |y1, y2, yr| {
            let mut updated_column = false;
            z_merger.for_merged_indexes(&mut |z1, z2, zr| {
                if op.apply(
                    first.is_full_wide([x1, y1, z1]),
                    second.is_full_wide([x2, y2, z2]),
                ) {
                    let cell = shape.index([xr, yr, zr]);
                    shape.storage[cell] = true;
                    bounds[2] = bounds[2].min(zr as i32);
                    bounds[5] = bounds[5].max(zr as i32);
                    updated_column = true;
                }
                true
            });
            if updated_column {
                bounds[1] = bounds[1].min(yr as i32);
                bounds[4] = bounds[4].max(yr as i32);
                updated_slice = true;
            }
            true
        });
        if updated_slice {
            bounds[0] = bounds[0].min(xr as i32);
            bounds[3] = bounds[3].max(xr as i32);
        }
        true
    });
    shape.min = [bounds[0], bounds[1], bounds[2]];
    shape.max = [
        bounds[3].wrapping_add(1),
        bounds[4].wrapping_add(1),
        bounds[5].wrapping_add(1),
    ];
    shape
}

/// `Shapes.findBits`: the 1, 2, 4 or 8 subdivision the endpoints land on, or
/// `-1` when the box leaves `[-1e-7, 1.0000001]` or misses every grid line.
fn find_bits(min: f64, max: f64) -> i32 {
    if min < -EPSILON || max > 1.0000001 {
        return -1;
    }
    for bits in 0..=3_i32 {
        let intervals = f64::from(1_i32 << bits);
        let scaled_min = min * intervals;
        let scaled_max = max * intervals;
        let found_min = (scaled_min - java_round(scaled_min) as f64).abs() < EPSILON * intervals;
        let found_max = (scaled_max - java_round(scaled_max) as f64).abs() < EPSILON * intervals;
        if found_min && found_max {
            return bits;
        }
    }
    -1
}

/// `Math.round(double)`: `floor(value + 0.5)`, which is what vanilla casts to
/// `int` when it rounds a box onto the grid.
fn java_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// `Shapes.lcm`.
fn lcm(first: usize, second: usize) -> usize {
    first * (second / gcd(first, second))
}

/// `IntMath.gcd`.
fn gcd(first: usize, second: usize) -> usize {
    let (mut a, mut b) = (first, second);
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}
