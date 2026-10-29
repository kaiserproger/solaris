use super::{Binary, DensityRuntime, Id, Mapped, Marker, Node, Shift, Spline};
use std::collections::HashMap;

#[path = "batch.rs"]
mod batch;
#[path = "chunk_fill.rs"]
pub(super) mod chunk_fill;
#[path = "column_sampler.rs"]
pub(super) mod column;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    Raw,
    Terrain,
    Lattice,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Position {
    x: i32,
    y: i32,
    z: i32,
    mode: Mode,
}
#[derive(Clone, Copy)]
struct Cached {
    position: Position,
    value: f32,
}
#[derive(Clone, Copy)]
struct Cell {
    x: i32,
    y: i32,
    z: i32,
    corners: [f32; 8],
}

/// Bounded, node-indexed memoization in lattice coordinates. Only the selected
/// marker nodes own a grid; a miss outside the chunk falls back to evaluation.
struct GridMemo {
    min: [i32; 3],
    size: [usize; 3],
    slots: Vec<usize>,
    values: Vec<Option<f32>>,
}

impl GridMemo {
    fn new(runtime: &DensityRuntime, min: [i32; 3], max: [i32; 3], flat: bool) -> Self {
        let size = std::array::from_fn(|axis| (max[axis] - min[axis] + 1) as usize);
        let volume = size.iter().product::<usize>();
        let mut slots = vec![usize::MAX; runtime.nodes.len()];
        let mut count = 0;
        for (id, node) in runtime.nodes.iter().enumerate() {
            if matches!(
                (flat, node),
                (true, Node::Marker(Marker::FlatCache, _))
                    | (false, Node::Marker(Marker::Interpolated, _))
            ) {
                slots[id] = count * volume;
                count += 1;
            }
        }
        Self {
            min,
            size,
            slots,
            values: vec![None; count * volume],
        }
    }

    fn index(&self, id: Id, point: [i32; 3]) -> Option<usize> {
        let slot = self.slots[id];
        if slot == usize::MAX {
            return None;
        }
        let offset: [usize; 3] = std::array::from_fn(|axis| {
            (i64::from(point[axis]) - i64::from(self.min[axis])) as usize
        });
        if (0..3).any(|axis| offset[axis] >= self.size[axis]) {
            return None;
        }
        Some(slot + (offset[1] * self.size[2] + offset[2]) * self.size[0] + offset[0])
    }

    fn get(&self, id: Id, point: [i32; 3]) -> Option<f32> {
        self.index(id, point).and_then(|index| self.values[index])
    }

    fn put(&mut self, id: Id, point: [i32; 3], value: f32) {
        if let Some(index) = self.index(id, point) {
            self.values[index] = Some(value);
        }
    }
}

struct ChunkMemo {
    corners: GridMemo,
    flat: GridMemo,
}

/// Single-entry last-(x, z) memo for a `cache_2d` wrapper, mirroring
/// `NoiseChunk$Cache2D` (`lastPos2D` + `lastValue`). The key packs only the
/// block X/Z pair; Y is dropped at lookup exactly like `ChunkPos::pack` on
/// `(blockX, blockZ)`. Keyed by wrapper node, so wrapped `NoiseChunk` graphs
/// are never keyed by bare coordinates.
#[derive(Clone, Copy, Default)]
struct PairMemo {
    key: u64,
    value: f32,
    live: bool,
}

/// Zero-cost-off diagnostic counters for density-graph execution. Each call
/// site checks one predictable branch; when `enabled` is false nothing is
/// recorded (no allocation, no atomics). Enable on one sampler to count
/// requested vs actual node evaluations, cache hit/miss, and Min/Max
/// short-circuits.
#[derive(Clone, Default, Debug)]
pub(crate) struct DensityCounters {
    /// `eval` entries before any cache lookup.
    pub requested: u64,
    /// Cache misses that ran the node body (per node, post-wrapper checks).
    pub evaluated: u64,
    /// `eval` entries skipped by a cache hit.
    pub cache_hits: u64,
    /// `cache_2d` last-pair hits.
    pub pair_hits: u64,
    /// `cache_2d` misses that evaluated the wrapped input.
    pub pair_misses: u64,
    /// `min` nodes that skipped `b` via a proven lower bound on `b`.
    pub min_short_circuits: u64,
    /// `max` nodes that skipped `b` via a proven upper bound on `b`.
    pub max_short_circuits: u64,
    /// `mul` nodes that skipped `b` on an exact `0.0` factor.
    pub mul_zero_skips: u64,
    /// Scalar `lerp` evaluations executed inside `interpolate` (7 per
    /// voxel). Measured per chunk to evidence interpolation work; the
    /// 224/cell sharing claim is documented at `interpolate`.
    pub lerp_calls: u64,
    enabled: bool,
}

/// Reusable scratch storage. A caller can retain this for a whole chunk;
/// graph-node values and interpolation corners never allocate per voxel.
pub(crate) struct DensitySampler<'a> {
    runtime: &'a DensityRuntime,
    values: Vec<Option<Cached>>,
    batch_values: Vec<batch::BatchMemo>,
    batch_epoch: u64,
    cells: Vec<Option<Cell>>,
    chunk: Option<ChunkMemo>,
    pair_memos: Vec<PairMemo>,
    counters: DensityCounters,
    /// Lazily computed, coordinate-independent node metadata: proven
    /// `[min, max]` per node (`None` = unknown, short-circuit never fires)
    /// and Y-freedom per node for `cache_2d`. Computed once per sampler on
    /// first use, then read-only — never per-voxel allocation or DAG walk.
    node_bounds: Vec<Option<Option<[f32; 2]>>>,
    node_y_free: Vec<Option<bool>>,
}

impl<'a> DensitySampler<'a> {
    pub(super) fn new(runtime: &'a DensityRuntime) -> Self {
        let len = runtime.nodes.len();
        Self {
            runtime,
            values: vec![None; len],
            batch_values: Vec::new(),
            batch_epoch: 0,
            cells: vec![None; len],
            chunk: None,
            pair_memos: vec![PairMemo::default(); len],
            counters: DensityCounters::default(),
            node_bounds: vec![None; len],
            node_y_free: vec![None; len],
        }
    }

    pub(super) fn for_chunk(runtime: &'a DensityRuntime, min_x: i32, min_z: i32) -> Self {
        Self::for_region(runtime, min_x, min_z, min_x + 15, min_z + 15)
    }

    pub(super) fn for_region(
        runtime: &'a DensityRuntime,
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
    ) -> Self {
        let width = runtime.cell_width;
        let height = runtime.cell_height;
        let min = [
            min_x.div_euclid(width),
            runtime.min_y.div_euclid(height),
            min_z.div_euclid(width),
        ];
        let max = [
            max_x.div_euclid(width) + 1,
            (runtime.min_y + runtime.height - 1).div_euclid(height) + 1,
            max_z.div_euclid(width) + 1,
        ];
        let mut sampler = Self::new(runtime);
        sampler.chunk = Some(ChunkMemo {
            corners: GridMemo::new(runtime, min, max, false),
            flat: GridMemo::new(
                runtime,
                [
                    (min[0] * width).div_euclid(4),
                    0,
                    (min[2] * width).div_euclid(4),
                ],
                [
                    (max[0] * width).div_euclid(4),
                    0,
                    (max[2] * width).div_euclid(4),
                ],
                true,
            ),
        });
        sampler
    }
    pub(crate) fn sample(&mut self, field: Id, x: i32, y: i32, z: i32) -> f32 {
        eval(
            self.runtime,
            self,
            field,
            Position {
                x,
                y,
                z,
                mode: Mode::Raw,
            },
        )
    }
    pub(crate) fn sample_terrain(&mut self, field: Id, x: i32, y: i32, z: i32) -> f32 {
        eval(
            self.runtime,
            self,
            field,
            Position {
                x,
                y,
                z,
                mode: Mode::Terrain,
            },
        )
    }
    /// Additive-only diagnostics: enable counting, read a snapshot, disable.
    /// Off by default; enabling never changes sampled values.
    #[cfg(test)]
    pub(crate) fn enable_counters(&mut self) {
        self.counters.enabled = true;
    }
    #[cfg(test)]
    pub(crate) fn counter_snapshot(&self) -> &DensityCounters {
        &self.counters
    }
}

trait Cache {
    fn get(&self, id: Id, position: Position) -> Option<f32>;
    fn put(&mut self, id: Id, position: Position, value: f32);
    fn cell(&self, id: Id, x: i32, y: i32, z: i32) -> Option<[f32; 8]>;
    fn put_cell(&mut self, id: Id, cell: Cell);
    fn corner(&self, _id: Id, _position: Position) -> Option<f32> {
        None
    }
    fn put_corner(&mut self, _id: Id, _position: Position, _value: f32) {}
    /// `cache_2d` last-(x, z) read, keyed by wrapper node. Default: miss.
    fn pair(&self, _id: Id, _key: u64) -> Option<f32> {
        None
    }
    fn put_pair(&mut self, _id: Id, _key: u64, _value: f32) {}
    /// Lazily computed, coordinate-independent node bound (`None` entry =
    /// unknown; short-circuit never fires). Default recomputes per call —
    /// stateful samplers override with a once-per-sampler cache.
    fn node_bound(&mut self, _id: Id) -> Option<[f32; 2]> {
        None
    }
    /// Lazily computed Y-freedom for `cache_2d`. Default: not free (miss).
    fn node_y_free(&mut self, _id: Id) -> bool {
        false
    }
    fn counters_mut(&mut self) -> Option<&mut DensityCounters> {
        None
    }
}
impl Cache for DensitySampler<'_> {
    #[inline]
    fn get(&self, id: Id, position: Position) -> Option<f32> {
        if position.mode != Mode::Raw
            && let Some(chunk) = &self.chunk
            && chunk.flat.slots[id] != usize::MAX
            && let Some(value) = chunk
                .flat
                .get(id, [position.x.div_euclid(4), 0, position.z.div_euclid(4)])
        {
            return Some(value);
        }
        self.values[id]
            .filter(|value| value.position == position)
            .map(|value| value.value)
    }
    #[inline]
    fn put(&mut self, id: Id, position: Position, value: f32) {
        self.values[id] = Some(Cached { position, value });
        if position.mode != Mode::Raw
            && let Some(chunk) = &mut self.chunk
            && chunk.flat.slots[id] != usize::MAX
        {
            chunk.flat.put(
                id,
                [position.x.div_euclid(4), 0, position.z.div_euclid(4)],
                value,
            );
        }
    }
    #[inline]
    fn cell(&self, id: Id, x: i32, y: i32, z: i32) -> Option<[f32; 8]> {
        self.cells[id]
            .filter(|cell| (cell.x, cell.y, cell.z) == (x, y, z))
            .map(|cell| cell.corners)
    }
    fn put_cell(&mut self, id: Id, cell: Cell) {
        self.cells[id] = Some(cell);
    }
    #[inline]
    fn corner(&self, id: Id, position: Position) -> Option<f32> {
        self.chunk.as_ref()?.corners.get(
            id,
            [
                position.x.div_euclid(self.runtime.cell_width),
                position.y.div_euclid(self.runtime.cell_height),
                position.z.div_euclid(self.runtime.cell_width),
            ],
        )
    }
    #[inline]
    fn put_corner(&mut self, id: Id, position: Position, value: f32) {
        if let Some(chunk) = &mut self.chunk {
            chunk.corners.put(
                id,
                [
                    position.x.div_euclid(self.runtime.cell_width),
                    position.y.div_euclid(self.runtime.cell_height),
                    position.z.div_euclid(self.runtime.cell_width),
                ],
                value,
            );
        }
    }
    fn pair(&self, id: Id, key: u64) -> Option<f32> {
        let memo = self.pair_memos.get(id)?;
        (memo.live && memo.key == key).then_some(memo.value)
    }
    fn put_pair(&mut self, id: Id, key: u64, value: f32) {
        if let Some(memo) = self.pair_memos.get_mut(id) {
            *memo = PairMemo {
                key,
                value,
                live: true,
            };
        }
    }
    fn node_bound(&mut self, id: Id) -> Option<[f32; 2]> {
        if id >= self.node_bounds.len() {
            return None;
        }
        if self.node_bounds[id].is_none() {
            let mut memo = HashMap::new();
            let bound = bounds(self.runtime, id, &mut memo, &mut Vec::new());
            self.node_bounds[id] = Some(bound);
        }
        self.node_bounds[id].flatten()
    }
    fn node_y_free(&mut self, id: Id) -> bool {
        if id >= self.node_y_free.len() {
            return false;
        }
        if self.node_y_free[id].is_none() {
            let mut memo = HashMap::new();
            let free = y_free_inner(self.runtime, id, &mut memo, &mut Vec::new());
            self.node_y_free[id] = Some(free);
        }
        self.node_y_free[id].unwrap_or(false)
    }
    fn counters_mut(&mut self) -> Option<&mut DensityCounters> {
        self.counters.enabled.then_some(&mut self.counters)
    }
}
struct NoCache;
impl Cache for NoCache {
    fn get(&self, _: Id, _: Position) -> Option<f32> {
        None
    }
    fn put(&mut self, _: Id, _: Position, _: f32) {}
    fn cell(&self, _: Id, _: i32, _: i32, _: i32) -> Option<[f32; 8]> {
        None
    }
    fn put_cell(&mut self, _: Id, _: Cell) {}
}

/// Shared raw-field scratch fits on the stack. Collisions only evict entries;
/// both node identity and the complete evaluation position are checked.
struct FieldMemo {
    entries: [Option<(Id, Cached)>; 64],
}
impl Cache for FieldMemo {
    fn get(&self, id: Id, position: Position) -> Option<f32> {
        self.entries[id % self.entries.len()]
            .filter(|(key, value)| *key == id && value.position == position)
            .map(|(_, value)| value.value)
    }
    fn put(&mut self, id: Id, position: Position, value: f32) {
        self.entries[id % self.entries.len()] = Some((id, Cached { position, value }));
    }
    fn cell(&self, _: Id, _: i32, _: i32, _: i32) -> Option<[f32; 8]> {
        None
    }
    fn put_cell(&mut self, _: Id, _: Cell) {}
}

pub(super) fn direct_fields<const N: usize>(
    runtime: &DensityRuntime,
    fields: [Id; N],
    x: i32,
    y: i32,
    z: i32,
) -> [f32; N] {
    let mut cache = FieldMemo {
        entries: [None; 64],
    };
    let position = Position {
        x,
        y,
        z,
        mode: Mode::Raw,
    };
    fields.map(|field| eval(runtime, &mut cache, field, position))
}

pub(super) fn direct_terrain(runtime: &DensityRuntime, field: Id, x: i32, y: i32, z: i32) -> f32 {
    eval(
        runtime,
        &mut NoCache,
        field,
        Position {
            x,
            y,
            z,
            mode: Mode::Terrain,
        },
    )
}
/// Proven `[min, max]` interval for a node body over every input position.
/// `None` on any component means "unknown": the Min/Max short-circuit treats
/// that side as unbounded and simply never skips (correctness first; missing
/// bounds only lose speed). Crucially this walks the DAG lazily per query and
/// memoizes per call — it never eager-walks the whole graph, so
/// `RangeChoice`/zero-`Mul`/lazy branches stay lazy: only the nodes on the
/// queried Min/Max path are bounded.
fn bounds(
    runtime: &DensityRuntime,
    id: Id,
    memo: &mut HashMap<Id, Option<[f32; 2]>>,
    stack: &mut Vec<Id>,
) -> Option<[f32; 2]> {
    if let Some(cached) = memo.get(&id) {
        return *cached;
    }
    // Cycle guard: recursive graphs cannot exist (compile rejects them), but
    // a re-entrant query must not recurse forever — treat as unknown.
    if stack.contains(&id) {
        return None;
    }
    stack.push(id);
    let bound = exact_bound(runtime, id, memo, stack);
    stack.pop();
    memo.insert(id, bound);
    bound
}

fn exact_bound(
    runtime: &DensityRuntime,
    id: Id,
    memo: &mut HashMap<Id, Option<[f32; 2]>>,
    stack: &mut Vec<Id>,
) -> Option<[f32; 2]> {
    let bound = |input: Id, memo: &mut HashMap<Id, Option<[f32; 2]>>, stack: &mut Vec<Id>| {
        bounds(runtime, input, memo, stack)
    };
    match &runtime.nodes[id] {
        Node::Constant(value) => Some([*value as f32, *value as f32]),
        Node::Binary(kind, a, b) => {
            let a = bound(*a, memo, stack)?;
            let b = bound(*b, memo, stack)?;
            Some(match kind {
                Binary::Add => [a[0] + b[0], a[1] + b[1]],
                Binary::Mul => {
                    let products = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
                    // `0 * inf` is NaN: the product is reachable in eval
                    // (`Mul` short-circuits exact 0.0), so an indeterminate
                    // bound is unknown rather than a folded extreme.
                    if products.iter().any(|v| v.is_nan()) {
                        return None;
                    }
                    [
                        products.iter().fold(f32::INFINITY, |m, v| m.min(*v)),
                        products.iter().fold(f32::NEG_INFINITY, |m, v| m.max(*v)),
                    ]
                }
                // `min(a, b)` lies within `[min(a0, b0), min(a1, b1)]` and
                // dually for max; widened by one ulp each side for safety.
                Binary::Min => [widen_lo(a[0].min(b[0])), widen_hi(a[1].min(b[1]))],
                Binary::Max => [widen_lo(a[0].max(b[0])), widen_hi(a[1].max(b[1]))],
            })
        }
        Node::Mapped(kind, input) => {
            let v = bound(*input, memo, stack)?;
            Some(match kind {
                Mapped::Abs => {
                    if v[0] >= 0.0 {
                        [v[0], v[1]]
                    } else if v[1] <= 0.0 {
                        [-v[1], -v[0]]
                    } else {
                        [0.0, v[0].abs().max(v[1].abs())]
                    }
                }
                Mapped::Square => {
                    if v[0] >= 0.0 {
                        [v[0] * v[0], v[1] * v[1]]
                    } else if v[1] <= 0.0 {
                        [v[1] * v[1], v[0] * v[0]]
                    } else {
                        [0.0, (v[0] * v[0]).max(v[1] * v[1])]
                    }
                }
                Mapped::Cube => [v[0] * v[0] * v[0], v[1] * v[1] * v[1]],
                Mapped::HalfNegative | Mapped::QuarterNegative => {
                    let factor = if matches!(kind, Mapped::HalfNegative) {
                        0.5
                    } else {
                        0.25
                    };
                    if v[0] >= 0.0 {
                        [v[0], v[1]]
                    } else if v[1] <= 0.0 {
                        [v[0] * factor, v[1] * factor]
                    } else {
                        [v[0] * factor, v[1]]
                    }
                }
                // `1/x` is unbounded across 0; elsewhere monotone on each
                // side — unknown if the interval straddles 0.
                Mapped::Invert => {
                    if v[0] > 0.0 || v[1] < 0.0 {
                        [1.0 / v[1], 1.0 / v[0]]
                    } else {
                        return None;
                    }
                }
                // `clamp(x,-1,1)/2 - x^3/24` is monotone non-decreasing
                // (derivative `1/2 - x^2/8 >= 3/8` on `[-1,1]`).
                Mapped::Squeeze => {
                    let f = |x: f32| {
                        let x = x.clamp(-1.0, 1.0);
                        x / 2.0 - x * x * x / 24.0
                    };
                    [widen_lo(f(v[0])), widen_hi(f(v[1]))]
                }
            })
        }
        Node::Clamp(input, min, max) => {
            let v = bound(*input, memo, stack)?;
            Some([
                v[0].clamp(*min as f32, *max as f32),
                v[1].clamp(*min as f32, *max as f32),
            ])
        }
        Node::Gradient(from_y, to_y, from, to) => {
            let (from, to) = (*from as f32, *to as f32);
            // `((y - from_y) / (to_y - from_y)).clamp(0, 1)` feeds
            // `from + t * (to - from)`. Equal cell heights divide 0/0
            // (NaN survives clamp), non-finite endpoints can overflow the
            // difference to NaN/inf, and cancellation can land outside
            // `[from, to]` — none of which a finite interval can represent,
            // since NaN poisons `min`/`max` comparisons asymmetrically.
            if from_y == to_y || !from.is_finite() || !to.is_finite() {
                return None;
            }
            // Sound outward bound: extend past each endpoint by the
            // difference magnitude plus rounding of the two operations.
            let diff = (to - from).abs();
            let mag = from.abs().max(to.abs()).max(1.0);
            let pad = diff + diff * 2.0 * f32::EPSILON + f32::EPSILON * mag;
            let (lo, hi) = (widen_lo(from.min(to) - pad), widen_hi(from.max(to) + pad));
            if !lo.is_finite() || !hi.is_finite() {
                return None;
            }
            Some([lo, hi])
        }
        Node::Noise(noise, ..) => {
            let b = runtime.noises[*noise].uniform_bound() as f32;
            Some([-b, b])
        }
        Node::Shift(..) => {
            // No uniform bound is threaded through `Shift` here: treat as
            // unknown so the short-circuit never fires — correctness first.
            None
        }
        Node::Shifted(noise, ..) => {
            let b = runtime.noises[*noise].uniform_bound() as f32;
            Some([-b, b])
        }
        // `rarity * |noise|` with `rarity in {0.5..3}`: non-negative, at most
        // `3 * noise_bound`.
        Node::Weird(noise, input, two_dimensional) => {
            let b = runtime.noises[*noise].uniform_bound() as f32;
            let peak = if *two_dimensional { 3.0 } else { 2.0 };
            let _ = bound(*input, memo, stack)?;
            Some([0.0, peak * b])
        }
        // Either branch may run depending on `input`: union of both arms.
        Node::Range(input, _, _, inside, outside) => {
            let _ = bound(*input, memo, stack)?;
            let a = bound(*inside, memo, stack)?;
            let b = bound(*outside, memo, stack)?;
            Some([widen_lo(a[0].min(b[0])), widen_hi(a[1].max(b[1]))])
        }
        // Splines interpolate/extrapolate between bounded constants: sample
        // the constant pool conservatively — walk values only, never
        // coordinates (coordinates stay lazy at eval).
        Node::Spline(value) => spline_bound(value),
        // Wrappers never change the value: the bound of the wrapped input.
        Node::Marker(_, input) => bound(*input, memo, stack),
        Node::Blended(..) => None,
        Node::FindTop(..) => None,
    }
}

/// Widen one ulp outward so float rounding in the bound derivation itself can
/// never flip a short-circuit decision.
fn widen_lo(v: f32) -> f32 {
    if v.is_finite() {
        v - f32::EPSILON * v.abs().max(1.0)
    } else {
        v
    }
}
fn widen_hi(v: f32) -> f32 {
    if v.is_finite() {
        v + f32::EPSILON * v.abs().max(1.0)
    } else {
        v
    }
}

fn spline_bound(value: &Spline) -> Option<[f32; 2]> {
    match value {
        Spline::Constant(v) => Some([*v, *v]),
        Spline::Multipoint { points, .. } => {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for point in points {
                let [a, b] = spline_bound(&point.value)?;
                // Linear extrapolation past an edge point can leave
                // `[a, b]` when the edge derivative is nonzero.
                if point.derivative != 0.0 {
                    return None;
                }
                if !a.is_finite() || !b.is_finite() {
                    return None;
                }
                lo = lo.min(a);
                hi = hi.max(b);
            }
            // f32 intermediates (`y2 - y1`, derivative terms, location
            // `width`, `t`) can overflow to inf/NaN from finite endpoints:
            // reject unless the COMBINED value span and every adjacent
            // location width stay finite in f32 and well inside f32 range.
            if !lo.is_finite() || !hi.is_finite() {
                return None;
            }
            if (hi - lo).abs() > 1.0e30 {
                return None;
            }
            for pair in points.windows(2) {
                let width = pair[1].location - pair[0].location;
                // Evaluated as f32 `width` then `t = (input - a)/width`:
                // non-finite or zero f32 width breaks the bound.
                if !width.is_finite() || width == 0.0 {
                    return None;
                }
            }
            // Widen past rounding of the interior cubic operations.
            let span = (hi - lo).abs().max(1.0);
            let pad = f32::EPSILON * span + 1e-6 * span.max(1.0);
            let (lo, hi) = (lo - pad, hi + pad);
            if !lo.is_finite() || !hi.is_finite() {
                return None;
            }
            Some([lo, hi])
        }
    }
}

fn eval<C: Cache>(runtime: &DensityRuntime, cache: &mut C, id: Id, p: Position) -> f32 {
    let counters = cache.counters_mut().is_some();
    if counters {
        count(cache, |c| c.requested += 1);
    }
    if let Some(value) = cache.get(id, p) {
        if counters {
            count(cache, |c| {
                c.cache_hits += 1;
            });
        }
        return value;
    }
    if counters {
        count(cache, |c| c.evaluated += 1);
    }
    let value = match &runtime.nodes[id] {
        Node::Constant(value) => *value as f32,
        Node::Binary(kind, a, b) => {
            let a = eval(runtime, cache, *a, p);
            match kind {
                Binary::Add => a + eval(runtime, cache, *b, p),
                Binary::Mul => {
                    if a == 0.0 {
                        if counters {
                            count(cache, |c| c.mul_zero_skips += 1);
                        }
                        0.0
                    } else {
                        a * eval(runtime, cache, *b, p)
                    }
                }
                // Proven short-circuit mirroring vanilla `Ap2`: `min(a, b)`
                // is `a` when `a` is strictly below every possible `b`
                // (dually `max(a, b)` is `a` when strictly above every
                // possible `b`). Strictness preserves `+0.0`/`-0.0` and NaN
                // edge semantics of `f32::min`/`max`: at
                // equality both arms are evaluated exactly as before.
                // Bounds are proven intervals, never samples; on unknown
                // bounds both arms run exactly as before.
                Binary::Min => {
                    if cache.node_bound(*b).is_some_and(|bound| a < bound[0]) {
                        if counters {
                            count(cache, |c| c.min_short_circuits += 1);
                        }
                        a
                    } else {
                        a.min(eval(runtime, cache, *b, p))
                    }
                }
                Binary::Max => {
                    if cache.node_bound(*b).is_some_and(|bound| a > bound[1]) {
                        if counters {
                            count(cache, |c| c.max_short_circuits += 1);
                        }
                        a
                    } else {
                        a.max(eval(runtime, cache, *b, p))
                    }
                }
            }
        }
        Node::Mapped(kind, input) => {
            let value = eval(runtime, cache, *input, p);
            match kind {
                Mapped::Abs => value.abs(),
                Mapped::Square => value * value,
                Mapped::Cube => value * value * value,
                Mapped::HalfNegative => {
                    if value > 0.0 {
                        value
                    } else {
                        value * 0.5
                    }
                }
                Mapped::QuarterNegative => {
                    if value > 0.0 {
                        value
                    } else {
                        value * 0.25
                    }
                }
                Mapped::Invert => 1.0 / value,
                Mapped::Squeeze => {
                    let value = value.clamp(-1.0, 1.0);
                    value / 2.0 - value * value * value / 24.0
                }
            }
        }
        Node::Clamp(input, min, max) => {
            eval(runtime, cache, *input, p).clamp(*min as f32, *max as f32)
        }
        Node::Gradient(from_y, to_y, from, to) => lerp(
            ((p.y as f32 - *from_y as f32) / (*to_y - *from_y) as f32).clamp(0.0, 1.0),
            *from as f32,
            *to as f32,
        ),
        Node::Noise(noise, xz, y) => runtime.noises[*noise].sample(
            f64::from(p.x) * xz,
            f64::from(p.y) * y,
            f64::from(p.z) * xz,
        ),
        Node::Shift(noise, shift) => {
            let (x, y, z) = match shift {
                Shift::All => (p.x, p.y, p.z),
                Shift::A => (p.x, 0, p.z),
                Shift::B => (p.z, p.x, 0),
            };
            runtime.noises[*noise].sample(
                f64::from(x) * 0.25,
                f64::from(y) * 0.25,
                f64::from(z) * 0.25,
            ) * 4.0
        }
        Node::Shifted(noise, sx, sy, sz, xz, y) => {
            let x = f64::from(p.x) * xz + f64::from(eval(runtime, cache, *sx, p));
            let y = f64::from(p.y) * y + f64::from(eval(runtime, cache, *sy, p));
            let z = f64::from(p.z) * xz + f64::from(eval(runtime, cache, *sz, p));
            runtime.noises[*noise].sample(x, y, z)
        }
        Node::Weird(noise, input, two_dimensional) => {
            let value = eval(runtime, cache, *input, p);
            let rarity = if *two_dimensional {
                if value < -0.75 {
                    0.5
                } else if value < -0.5 {
                    0.75
                } else if value < 0.5 {
                    1.0
                } else if value < 0.75 {
                    2.0
                } else {
                    3.0
                }
            } else if value < -0.5 {
                0.75
            } else if value < 0.0 {
                1.0
            } else if value < 0.5 {
                1.5
            } else {
                2.0
            };
            rarity
                * runtime.noises[*noise]
                    .sample(
                        f64::from(p.x) / f64::from(rarity),
                        f64::from(p.y) / f64::from(rarity),
                        f64::from(p.z) / f64::from(rarity),
                    )
                    .abs()
        }
        Node::Range(input, min, max, inside, outside) => {
            let input = eval(runtime, cache, *input, p);
            eval(
                runtime,
                cache,
                if input >= *min as f32 && input < *max as f32 {
                    *inside
                } else {
                    *outside
                },
                p,
            )
        }
        Node::Spline(value) => spline(runtime, cache, value, p),
        Node::Marker(Marker::Interpolated, input) if p.mode == Mode::Terrain => {
            interpolate(runtime, cache, id, *input, p)
        }
        Node::Marker(Marker::FlatCache, input) if p.mode != Mode::Raw => eval(
            runtime,
            cache,
            *input,
            Position {
                x: p.x.div_euclid(4) * 4,
                y: 0,
                z: p.z.div_euclid(4) * 4,
                mode: Mode::Lattice,
            },
        ),
        // `cache_2d`: single-entry last-(x, z) memo, Y dropped — but only
        // where the wrapped input is provably Y-free (see `y_free`).
        // Otherwise delegate positionally, exactly like a cache miss. Keyed
        // by wrapper node.
        Node::Marker(Marker::Cache2D, input) if p.mode != Mode::Raw => {
            let input = *input;
            if cache.node_y_free(input) {
                let key = pair_key(p.x, p.z);
                if let Some(hit) = cache.pair(id, key) {
                    if counters {
                        count(cache, |c| {
                            c.pair_hits += 1;
                        });
                    }
                    cache.put(id, p, hit);
                    return hit;
                }
                if counters {
                    count(cache, |c| c.pair_misses += 1);
                }
                let value = eval(
                    runtime,
                    cache,
                    input,
                    Position {
                        x: p.x,
                        y: 0,
                        z: p.z,
                        mode: Mode::Lattice,
                    },
                );
                cache.put_pair(id, key, value);
                cache.put(id, p, value);
                return value;
            }
            eval(runtime, cache, input, p)
        }
        // `cache_once` and `cache_all_in_cell` stay transparent here: the
        // scalar sampler has no batch/cell-fill scope (the bytecode's
        // `arrayInterpolationCounter` / `cellCaches` lifetimes do not exist
        // on this path), so keying by bare coordinates would alias distinct
        // wrapper contexts. Plain delegate preserves exact semantics.
        Node::Marker(_, input) => eval(runtime, cache, *input, p),
        Node::Blended(index) => runtime.blended[*index].sample(p.x, p.y, p.z),
        Node::FindTop(density, bound, lower, step) => {
            let mut y = (eval(runtime, cache, *bound, p) / *step as f32).floor() as i32 * step;
            let mut result = *lower;
            while y >= *lower {
                if eval(
                    runtime,
                    cache,
                    *density,
                    Position {
                        y,
                        mode: if p.mode == Mode::Raw {
                            Mode::Raw
                        } else {
                            Mode::Lattice
                        },
                        ..p
                    },
                ) > 0.0
                {
                    result = y;
                    break;
                }
                y -= step;
            }
            result as f32
        }
    };
    cache.put(id, p, value);
    value
}

/// Bump one counter when (and only when) counting is enabled. The
/// `counters_mut().is_some()` check at each call site keeps the disabled
/// path to one predictable branch with no allocation or atomics.
fn count<C: Cache>(cache: &mut C, bump: impl FnOnce(&mut DensityCounters)) {
    if let Some(counters) = cache.counters_mut() {
        bump(counters);
    }
}

/// `ChunkPos::pack(blockX, blockZ)` analogue: 32 bits each, Y dropped.
fn pair_key(x: i32, z: i32) -> u64 {
    (x as u64 & 0xffff_ffff) << 32 | (z as u64 & 0xffff_ffff)
}

fn y_free_inner(
    runtime: &DensityRuntime,
    id: Id,
    memo: &mut HashMap<Id, bool>,
    stack: &mut Vec<Id>,
) -> bool {
    if let Some(cached) = memo.get(&id) {
        return *cached;
    }
    if stack.contains(&id) {
        return false;
    }
    stack.push(id);
    let free = match &runtime.nodes[id] {
        Node::Constant(_) => true,
        Node::Binary(_, a, b) => {
            y_free_inner(runtime, *a, memo, stack) && y_free_inner(runtime, *b, memo, stack)
        }
        Node::Mapped(_, input) => y_free_inner(runtime, *input, memo, stack),
        // Gradient reads `p.y` directly.
        Node::Gradient(..) => false,
        // `Noise(x*xz, y*y_scale, z*xz)`: Y-free iff `y_scale == 0`.
        Node::Noise(_, _, y_scale) => *y_scale == 0.0,
        // `Shift::A` samples at `(x, 0, z)`, `Shift::B` at `(z, x, 0)`:
        // neither observes `p.y`. Only `Shift::All` reads `p.y`.
        Node::Shift(_, shift) => !matches!(shift, Shift::All),
        // three shifts are Y-free AND `y_scale == 0` (else `p.y * y` leaks).
        Node::Shifted(_, sx, sy, sz, _, y_scale) => {
            *y_scale == 0.0
                && y_free_inner(runtime, *sx, memo, stack)
                && y_free_inner(runtime, *sy, memo, stack)
                && y_free_inner(runtime, *sz, memo, stack)
        }
        // The mapper input is evaluated; `|noise|` itself is sampled at
        // `(x/r, y/r, z/r)` — always Y-observing.
        Node::Weird(..) => false,
        // Either branch may run: Y-free iff input and both arms are.
        Node::Range(input, _, _, inside, outside) => {
            y_free_inner(runtime, *input, memo, stack)
                && y_free_inner(runtime, *inside, memo, stack)
                && y_free_inner(runtime, *outside, memo, stack)
        }
        Node::Spline(..) => false,
        // `Interpolated` evaluates its input at lattice corners and lerps by
        // in-cell Y: Y-observing even when the wrapped subgraph is Y-free.
        // All other markers are transparent for Y-freedom.
        Node::Marker(Marker::Interpolated, _) => false,
        Node::Marker(_, input) => y_free_inner(runtime, *input, memo, stack),
        Node::Blended(..) => false,
        Node::FindTop(..) => false,
        // `Clamp` preserves Y-freedom of its input.
        Node::Clamp(input, _, _) => y_free_inner(runtime, *input, memo, stack),
    };
    stack.pop();
    memo.insert(id, free);
    free
}

fn interpolate<C: Cache>(
    runtime: &DensityRuntime,
    cache: &mut C,
    id: Id,
    input: Id,
    p: Position,
) -> f32 {
    let width = runtime.cell_width;
    let height = runtime.cell_height;
    let x = p.x.div_euclid(width) * width;
    let y = p.y.div_euclid(height) * height;
    let z = p.z.div_euclid(width) * width;
    let corners = if let Some(corners) = cache.cell(id, x, y, z) {
        corners
    } else {
        let corners = std::array::from_fn(|index| {
            let position = Position {
                x: x + (index as i32 & 1) * width,
                y: y + ((index as i32 >> 1) & 1) * height,
                z: z + ((index as i32 >> 2) & 1) * width,
                mode: Mode::Lattice,
            };
            if let Some(value) = cache.corner(id, position) {
                value
            } else {
                let value = eval(runtime, cache, input, position);
                cache.put_corner(id, position, value);
                value
            }
        });
        cache.put_cell(id, Cell { x, y, z, corners });
        corners
    };
    // Y-then-X-then-Z trilerp with shared intermediates: 7 scalar lerps per
    // voxel (4 Y + 2 X + 1 Z), each the IDENTICAL `from + t * (to - from)`
    // with the same operands in the same order as the naive 7-lerp nest
    // (no previous+delta accumulation), so bits are unchanged. The 224/cell
    // figure (32 Y + 64 X + 128 Z) counts DISTINCT (t, from, to) triples
    // across a 4x8x4 cell — evidenced by `lerp_calls` in tests — not cached
    // intermediates (cells store corners only, like the baseline).
    if cache.counters_mut().is_some() {
        count(cache, |c| c.lerp_calls += 7);
    }
    interpolate_corners(corners, p.x - x, p.y - y, p.z - z, width, height)
}

/// Trilerp of one cell's 8 corners at integer in-cell offsets, Y first, then
/// X, then Z — the `NoiseChunk` update order (not `Mth.lerp3`'s). Corner
/// layout is index bit 0 = +x, bit 1 = +y, bit 2 = +z.
fn interpolate_corners(
    corners: [f32; 8],
    dx: i32,
    dy: i32,
    dz: i32,
    width: i32,
    height: i32,
) -> f32 {
    let tx = dx as f32 / width as f32;
    let ty = dy as f32 / height as f32;
    let tz = dz as f32 / width as f32;
    // Y stage: 4 intermediates (one per (x, z) edge).
    let y00 = lerp(ty, corners[0], corners[2]);
    let y10 = lerp(ty, corners[1], corners[3]);
    let y01 = lerp(ty, corners[4], corners[6]);
    let y11 = lerp(ty, corners[5], corners[7]);
    // X stage: 2 intermediates (one per z slice).
    let x0 = lerp(tx, y00, y10);
    let x1 = lerp(tx, y01, y11);
    // Z stage: final value.
    lerp(tz, x0, x1)
}

fn spline<C: Cache>(runtime: &DensityRuntime, cache: &mut C, value: &Spline, p: Position) -> f32 {
    match value {
        Spline::Constant(value) => *value,
        Spline::Multipoint { coordinate, points } => {
            let input = eval(runtime, cache, *coordinate, p);
            let next = points.partition_point(|point| point.location <= input);
            if next == 0 || next == points.len() {
                let edge = &points[if next == 0 { 0 } else { points.len() - 1 }];
                let value = spline(runtime, cache, &edge.value, p);
                if edge.derivative == 0.0 {
                    value
                } else {
                    value + edge.derivative * (input - edge.location)
                }
            } else {
                let a = &points[next - 1];
                let b = &points[next];
                let width = b.location - a.location;
                let t = (input - a.location) / width;
                let y1 = spline(runtime, cache, &a.value, p);
                let y2 = spline(runtime, cache, &b.value, p);
                let a = a.derivative * width - (y2 - y1);
                let b = -b.derivative * width + (y2 - y1);
                y1 + t * (y2 - y1) + t * (1.0 - t) * (a + t * (b - a))
            }
        }
    }
}
fn lerp(t: f32, from: f32, to: f32) -> f32 {
    from + t * (to - from)
}

#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;
