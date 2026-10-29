//! Whole-chunk `final_density` in two phases, bit-identical to sampling every
//! block through [`DensitySampler::sample_terrain`]:
//!
//! 1. every lattice corner of every `interpolated` input over the chunk, in
//!    full eight-lane batches of the lazy evaluator (its branches, bounds and
//!    flat caches unchanged);
//! 2. the graph outside `interpolated` (vanilla: 17 nodes, no noise) compiled
//!    once into straight-line ops and run over whole 256-column layers. Its
//!    nodes are pure, so evaluating both arms of a branch and selecting per
//!    lane gives the lazy evaluator's values.
//!
//! Interpolation keeps `interpolate_corners`' operand order: Y lerps per
//! corner column, then X per corner row, then Z, each `from + t * (to - from)`.
use super::*;

/// The block-level graph of a field; `None` when it has a node this path
/// does not evaluate (noise, splines, column caches), so callers keep the
/// general sampler.
pub(crate) struct BlockProgram {
    ops: Vec<Op>,
    root: usize,
    /// Input node of each `interpolated` slot.
    slots: Vec<Id>,
}

#[derive(Clone, Copy)]
enum Op {
    Constant(f32),
    Add(usize, usize),
    Mul(usize, usize),
    /// Lazy `min(a, b)` keeps `a` when it is below `b`'s proven lower bound.
    Min(usize, usize, Option<f32>),
    Max(usize, usize, Option<f32>),
    Mapped(Mapped, usize),
    Clamp(usize, f32, f32),
    Gradient(i32, i32, f32, f32),
    Range(usize, f32, f32, usize, usize),
    Slot(usize),
}

impl BlockProgram {
    pub(crate) fn compile(runtime: &DensityRuntime, field: Id) -> Option<Self> {
        let mut program = Self {
            ops: Vec::new(),
            root: 0,
            slots: Vec::new(),
        };
        let mut memo = HashMap::new();
        program.root = program.emit(runtime, field, &mut memo)?;
        Some(program)
    }

    fn emit(
        &mut self,
        runtime: &DensityRuntime,
        id: Id,
        memo: &mut HashMap<Id, usize>,
    ) -> Option<usize> {
        if let Some(&reg) = memo.get(&id) {
            return Some(reg);
        }
        let bound = |side: usize, b: Id| {
            bounds(runtime, b, &mut HashMap::new(), &mut Vec::new()).map(|bound| bound[side])
        };
        let op = match &runtime.nodes[id] {
            Node::Constant(value) => Op::Constant(*value as f32),
            Node::Binary(kind, a, b) => {
                let (ra, rb) = (self.emit(runtime, *a, memo)?, self.emit(runtime, *b, memo)?);
                match kind {
                    Binary::Add => Op::Add(ra, rb),
                    Binary::Mul => Op::Mul(ra, rb),
                    Binary::Min => Op::Min(ra, rb, bound(0, *b)),
                    Binary::Max => Op::Max(ra, rb, bound(1, *b)),
                }
            }
            Node::Mapped(kind, input) => Op::Mapped(*kind, self.emit(runtime, *input, memo)?),
            Node::Clamp(input, min, max) => {
                Op::Clamp(self.emit(runtime, *input, memo)?, *min as f32, *max as f32)
            }
            Node::Gradient(from_y, to_y, from, to) => {
                Op::Gradient(*from_y, *to_y, *from as f32, *to as f32)
            }
            Node::Range(input, min, max, inside, outside) => Op::Range(
                self.emit(runtime, *input, memo)?,
                *min as f32,
                *max as f32,
                self.emit(runtime, *inside, memo)?,
                self.emit(runtime, *outside, memo)?,
            ),
            Node::Marker(Marker::Interpolated, input) => {
                let slot = self.slots.iter().position(|slot| slot == input);
                Op::Slot(slot.unwrap_or_else(|| {
                    self.slots.push(*input);
                    self.slots.len() - 1
                }))
            }
            // Terrain-mode evaluation treats these as transparent.
            Node::Marker(Marker::CacheOnce | Marker::CacheAllInCell | Marker::Pass, input) => {
                let reg = self.emit(runtime, *input, memo)?;
                memo.insert(id, reg);
                return Some(reg);
            }
            _ => return None,
        };
        self.ops.push(op);
        let reg = self.ops.len() - 1;
        memo.insert(id, reg);
        Some(reg)
    }
}

const LANES: usize = 256;

/// `output[(y - min_y) * 256 + z * 16 + x]`, as `fill_density_chunk`.
pub(crate) fn fill(
    runtime: &DensityRuntime,
    program: &BlockProgram,
    min_x: i32,
    min_z: i32,
    output: &mut [f32],
) {
    let (width, height) = (runtime.cell_width, runtime.cell_height);
    let (min_y, max_y) = (runtime.min_y, runtime.min_y + runtime.height);
    let corner_x = min_x.div_euclid(width);
    let corner_z = min_z.div_euclid(width);
    let level_0 = min_y.div_euclid(height);
    let nx = ((min_x + 15).div_euclid(width) - corner_x + 2) as usize;
    let nz = ((min_z + 15).div_euclid(width) - corner_z + 2) as usize;
    let levels = ((max_y - 1).div_euclid(height) - level_0 + 2) as usize;
    let per_level = nx * nz;
    let per_slot = levels * per_level;

    // Phase 1: corners, `[slot][level][z][x]`.
    let mut sampler = DensitySampler::for_chunk(runtime, min_x, min_z);
    if sampler.batch_values.is_empty() {
        sampler
            .batch_values
            .resize(runtime.nodes.len(), batch::BatchMemo::default());
    }
    let mut corners = vec![0.0f32; program.slots.len() * per_slot];
    let point = |index: usize| {
        let level = index / per_level;
        let rest = index % per_level;
        Position {
            x: (corner_x + (rest % nx) as i32) * width,
            y: (level_0 + level as i32) * height,
            z: (corner_z + (rest / nx) as i32) * width,
            mode: Mode::Lattice,
        }
    };
    for (slot, &input) in program.slots.iter().enumerate() {
        let values = &mut corners[slot * per_slot..][..per_slot];
        for start in (0..per_slot).step_by(8) {
            let count = (per_slot - start).min(8);
            let points = std::array::from_fn(|lane| point(start + lane.min(count - 1)));
            let active = u8::MAX >> (8 - count);
            let batch = sampler.eval_new_batch(input, &points, active);
            values[start..start + count].copy_from_slice(&batch[..count]);
        }
    }

    // Phase 2: one 256-column layer at a time.
    let mut regs = vec![[0.0f32; LANES]; program.ops.len()];
    let mut slot_layers = vec![[0.0f32; LANES]; program.slots.len()];
    let mut y_lerped = vec![0.0f32; per_level];
    let mut x_lerped = vec![0.0f32; nz * 16];
    let fraction = |v: i32, size: i32| (v - v.div_euclid(size) * size) as f32 / size as f32;
    let column_x: [(usize, f32); 16] = std::array::from_fn(|x| {
        let x = min_x + x as i32;
        (
            (x.div_euclid(width) - corner_x) as usize,
            fraction(x, width),
        )
    });
    let column_z: [(usize, f32); 16] = std::array::from_fn(|z| {
        let z = min_z + z as i32;
        (
            (z.div_euclid(width) - corner_z) as usize,
            fraction(z, width),
        )
    });
    for (offset, layer) in output.as_chunks_mut::<LANES>().0.iter_mut().enumerate() {
        let y = min_y + offset as i32;
        let level = (y.div_euclid(height) - level_0) as usize;
        let ty = fraction(y, height);
        for (slot, out) in slot_layers.iter_mut().enumerate() {
            let below = &corners[slot * per_slot + level * per_level..][..per_level];
            let above = &corners[slot * per_slot + (level + 1) * per_level..][..per_level];
            for ((lerped, from), to) in y_lerped.iter_mut().zip(below).zip(above) {
                *lerped = lerp(ty, *from, *to);
            }
            for row in 0..nz {
                let lerped = &y_lerped[row * nx..][..nx];
                for (x, &(cell, tx)) in column_x.iter().enumerate() {
                    x_lerped[row * 16 + x] = lerp(tx, lerped[cell], lerped[cell + 1]);
                }
            }
            for (z, &(cell, tz)) in column_z.iter().enumerate() {
                for x in 0..16 {
                    out[z * 16 + x] =
                        lerp(tz, x_lerped[cell * 16 + x], x_lerped[(cell + 1) * 16 + x]);
                }
            }
        }
        run(&program.ops, &mut regs, &slot_layers, y);
        *layer = regs[program.root];
    }
}

fn run(ops: &[Op], regs: &mut [[f32; LANES]], slots: &[[f32; LANES]], y: i32) {
    for (index, op) in ops.iter().enumerate() {
        let (done, rest) = regs.split_at_mut(index);
        let out = &mut rest[0];
        match *op {
            Op::Constant(value) => out.fill(value),
            Op::Add(a, b) => lanes2(out, &done[a], &done[b], |a, b| a + b),
            Op::Mul(a, b) => lanes2(
                out,
                &done[a],
                &done[b],
                |a, b| {
                    if a == 0.0 { 0.0 } else { a * b }
                },
            ),
            Op::Min(a, b, low) => match low {
                Some(low) => lanes2(
                    out,
                    &done[a],
                    &done[b],
                    |a, b| if a < low { a } else { a.min(b) },
                ),
                None => lanes2(out, &done[a], &done[b], f32::min),
            },
            Op::Max(a, b, high) => match high {
                Some(high) => lanes2(
                    out,
                    &done[a],
                    &done[b],
                    |a, b| if a > high { a } else { a.max(b) },
                ),
                None => lanes2(out, &done[a], &done[b], f32::max),
            },
            Op::Mapped(kind, a) => {
                let a = &done[a];
                match kind {
                    Mapped::Abs => lanes1(out, a, f32::abs),
                    Mapped::Square => lanes1(out, a, |v| v * v),
                    Mapped::Cube => lanes1(out, a, |v| v * v * v),
                    Mapped::HalfNegative => lanes1(out, a, |v| if v > 0.0 { v } else { v * 0.5 }),
                    Mapped::QuarterNegative => {
                        lanes1(out, a, |v| if v > 0.0 { v } else { v * 0.25 })
                    }
                    Mapped::Invert => lanes1(out, a, |v| 1.0 / v),
                    Mapped::Squeeze => lanes1(out, a, |v| {
                        let v = v.clamp(-1.0, 1.0);
                        v / 2.0 - v * v * v / 24.0
                    }),
                }
            }
            Op::Clamp(a, min, max) => lanes1(out, &done[a], |v| v.clamp(min, max)),
            Op::Gradient(from_y, to_y, from, to) => out.fill(lerp(
                ((y as f32 - from_y as f32) / (to_y - from_y) as f32).clamp(0.0, 1.0),
                from,
                to,
            )),
            Op::Range(input, min, max, inside, outside) => {
                let (input, inside, outside) = (&done[input], &done[inside], &done[outside]);
                for lane in 0..LANES {
                    out[lane] = if input[lane] >= min && input[lane] < max {
                        inside[lane]
                    } else {
                        outside[lane]
                    };
                }
            }
            Op::Slot(slot) => *out = slots[slot],
        }
    }
}

#[inline(always)]
fn lanes1(out: &mut [f32; LANES], a: &[f32; LANES], f: impl Fn(f32) -> f32) {
    for (out, a) in out.iter_mut().zip(a) {
        *out = f(*a);
    }
}

#[inline(always)]
fn lanes2(out: &mut [f32; LANES], a: &[f32; LANES], b: &[f32; LANES], f: impl Fn(f32, f32) -> f32) {
    for ((out, a), b) in out.iter_mut().zip(a).zip(b) {
        *out = f(*a, *b);
    }
}
