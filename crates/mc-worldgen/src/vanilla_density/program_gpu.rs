//! Word image of `final_density` for the GPU worldgen experiment
//! (`vanilla_density/gpu.rs`, `density.wgsl`).
//!
//! Chunk fill evaluates `final_density` as `fill_density_chunk` does: each
//! `interpolated` input at the lattice corners (its `flat_cache` parts once
//! per corner column at y 0), the rest per block from the corners trilerped
//! Y, X, Z. The corner code is the far-terrain program's straight-line form
//! (`Context::Terrain`); the block code adds op 21 `interpolated slot`.
//!
//! Noise coordinates never pass through f32. Every sampling instruction has
//! a table of per-level multipliers `k mod 256` in unsigned 8.56 fixed point,
//! so the GPU computes `x * k + offset` exactly modulo the permutation's
//! 256-cell period from the integer block coordinate (the CPU's f64 `wrap`
//! only removes multiples of 2^25). Only the in-cell fraction becomes f32,
//! as on the CPU.
//!
//! Words (u32; f32 as bits; fixed = lo, hi):
//! - octave: 64 packed permutation words, xo, yo, zo fixed (70 words)
//! - noise record: value factor, (weight, level count) per Perlin set, then
//!   (amplitude, octave or `u32::MAX`) per level, first set then second
//! - level table of a sampling instruction: per variant (weird rarity), set
//!   and level: k_xz fixed, k_y fixed, f32 `input * set factor` (5 words)
//! - blended record: 40 octave ids (16 min, 16 max, 8 main), then 16
//!   min/max and 8 main levels of k_xz, k_y, f32 y smear, f32 y-max scale,
//!   then per octave and world y in `[min_y, min_y + height]` the f32 smear
//!   step `delta` exactly as the CPU computes it. It depends on y alone,
//!   and at lattice y the quotient is an exact integer whose floor GPU
//!   division rounding would flip for whole layers.
//! - instruction: op, 4 args, 2 f32 constants, level table offset
//! - spline point: f32 location, f32 derivative, register flag, value
use super::*;
use crate::vanilla_climate::noise::NormalNoise;

pub(crate) struct ChunkWords {
    pub(crate) words: Vec<u32>,
    pub(crate) min_y: i32,
    pub(crate) height: i32,
    pub(crate) cell_width: i32,
    pub(crate) cell_height: i32,
    /// Instructions, instruction count, points, flat_len.
    pub(crate) corner: [u32; 4],
    /// Instructions, instruction count, points, root register.
    pub(crate) block: [u32; 4],
    /// Octaves, noise record offsets, blended record offsets, slot registers.
    pub(crate) tables: [u32; 4],
    pub(crate) slots: u32,
    pub(crate) registers: u32,
}

/// Rarity of the weird-scaled sampler variants, in threshold order.
const RARITY_2D: [f64; 5] = [0.5, 0.75, 1.0, 2.0, 3.0];
const RARITY_3D: [f64; 4] = [0.75, 1.0, 1.5, 2.0];
const SECOND_SET_FACTOR: f64 = 1.0181268882175227;

pub(crate) fn chunk_words(density: &VanillaDensity) -> Result<ChunkWords, String> {
    let runtime = &*density.runtime;
    let mut exporter = Exporter {
        runtime,
        octaves: Vec::new(),
        octave_ids: HashMap::new(),
        noises: Vec::new(),
        noise_ids: HashMap::new(),
        blended: Vec::new(),
        blended_ids: HashMap::new(),
        corner: Some((Code::default(), Vec::new())),
    };
    let mut block = Code::default();
    let root = exporter.emit(&mut block, density.final_density, Context::Raw)?;
    let (corner, slots) = exporter.corner.take().expect("chunk mode");
    for blended in exporter.blended.clone() {
        let noise = &runtime.blended[blended];
        for octave in noise.min.iter().chain(&noise.max).chain(&noise.main) {
            exporter.octave(octave);
        }
    }
    for noise in exporter.noises.clone() {
        let noise = &runtime.noises[noise];
        for perlin in [&noise.first, &noise.second] {
            for octave in perlin.levels.iter().flatten() {
                exporter.octave(octave);
            }
        }
    }

    let mut w = Vec::with_capacity(64 * 1024);
    let octaves = w.len() as u32;
    for octave in &exporter.octaves {
        for bytes in octave.p.as_chunks::<4>().0 {
            w.push(u32::from_le_bytes(*bytes));
        }
        for value in [octave.xo, octave.yo, octave.zo] {
            w.extend(fixed(value)?);
        }
    }

    let noises = w.len() as u32;
    w.resize(w.len() + exporter.noises.len(), 0);
    for (index, &noise) in exporter.noises.iter().enumerate() {
        w[noises as usize + index] = w.len() as u32;
        let noise = &runtime.noises[noise];
        w.push((noise.value_factor as f32).to_bits());
        for perlin in [&noise.first, &noise.second] {
            w.push((perlin.value as f32).to_bits());
            w.push(perlin.levels.len() as u32);
        }
        for perlin in [&noise.first, &noise.second] {
            for (level, amplitude) in perlin.levels.iter().zip(&perlin.amplitudes) {
                w.push((*amplitude as f32).to_bits());
                w.push(level.as_ref().map_or(u32::MAX, |octave| {
                    exporter.octave_ids[&std::ptr::from_ref(octave)]
                }));
            }
        }
    }

    let blended = w.len() as u32;
    w.resize(w.len() + exporter.blended.len(), 0);
    for (index, &noise) in exporter.blended.iter().enumerate() {
        w[blended as usize + index] = w.len() as u32;
        let noise = &runtime.blended[noise];
        for octave in noise.min.iter().chain(&noise.max).chain(&noise.main) {
            w.push(exporter.octave_ids[&std::ptr::from_ref(octave)]);
        }
        let smear = noise.y * noise.smear;
        let mut scale = 1.0;
        for _ in 0..16 {
            w.extend(fixed(noise.xz * scale)?);
            w.extend(fixed(noise.y * scale)?);
            w.push(((smear * scale) as f32).to_bits());
            w.push(((noise.y * scale) as f32).to_bits());
            scale /= 2.0;
        }
        let main_smear = smear / noise.y_factor;
        let mut scale = 1.0;
        for _ in 0..8 {
            w.extend(fixed(noise.xz / noise.xz_factor * scale)?);
            w.extend(fixed(noise.y / noise.y_factor * scale)?);
            w.push(((main_smear * scale) as f32).to_bits());
            w.push(((noise.y / noise.y_factor * scale) as f32).to_bits());
            scale /= 2.0;
        }
        let ys = runtime.min_y..=runtime.min_y + runtime.height;
        for (index, octave) in noise.min.iter().chain(&noise.max).enumerate() {
            let scale = 0.5_f64.powi(index as i32 % 16);
            for py in ys.clone() {
                let y = f64::from(py) * noise.y;
                w.push(smear_delta(octave, y * scale, smear * scale, y * scale).to_bits());
            }
        }
        for (index, octave) in noise.main.iter().enumerate() {
            let scale = 0.5_f64.powi(index as i32);
            for py in ys.clone() {
                let y = f64::from(py) * noise.y / noise.y_factor;
                w.push(smear_delta(octave, y * scale, main_smear * scale, y * scale).to_bits());
            }
        }
    }

    let slot_table = w.len() as u32;
    w.extend(slots.iter().map(|reg| corner.register(*reg)));
    let corner_code = write_code(&mut w, &exporter, &corner)?;
    let block_code = write_code(&mut w, &exporter, &block)?;
    let corner_len = (corner.flat.len() + corner.point.len()) as u32;
    let block_len = block.point.len() as u32;
    Ok(ChunkWords {
        words: w,
        min_y: runtime.min_y,
        height: runtime.height,
        cell_width: runtime.cell_width,
        cell_height: runtime.cell_height,
        corner: [
            corner_code[0],
            corner_len,
            corner_code[1],
            corner.flat.len() as u32,
        ],
        block: [
            block_code[0],
            block_len,
            block_code[1],
            block.register(root),
        ],
        tables: [octaves, noises, blended, slot_table],
        slots: slots.len() as u32,
        registers: corner_len.max(block_len),
    })
}

/// Level tables, then instructions and spline points; returns their offsets.
fn write_code(w: &mut Vec<u32>, exporter: &Exporter, code: &Code) -> Result<[u32; 2], String> {
    let runtime = exporter.runtime;
    let noise = |arg: Arg| -> Result<&NormalNoise, String> {
        match arg {
            Arg::Int(index) => Ok(&runtime.noises[exporter.noises[index as usize]]),
            Arg::Reg(_) => Err("noise argument is a register".into()),
        }
    };
    let insts: Vec<&Inst> = code.flat.iter().chain(&code.point).collect();
    let mut tables = Vec::with_capacity(insts.len());
    for inst in &insts {
        let [xz, y] = inst.constants;
        tables.push(match inst.op {
            14 | 16 => level_table(w, noise(inst.args[0])?, &[1.0], xz, y)?,
            15 => level_table(w, noise(inst.args[0])?, &[1.0], 0.25, 0.25)?,
            17 => {
                let rarities: &[f64] = match inst.args[2] {
                    Arg::Int(0) => &RARITY_3D,
                    _ => &RARITY_2D,
                };
                level_table(w, noise(inst.args[0])?, rarities, 1.0, 1.0)?
            }
            _ => 0,
        });
    }
    let at = w.len() as u32;
    for (inst, table) in insts.iter().zip(tables) {
        w.push(u32::from(inst.op));
        for arg in inst.args {
            w.push(match arg {
                Arg::Int(value) => value,
                Arg::Reg(reg) => code.register(reg),
            });
        }
        for value in inst.constants {
            w.push((value as f32).to_bits());
        }
        w.push(table);
    }
    let points = w.len() as u32;
    for point in &code.points {
        w.push(point.location.to_bits());
        w.push(point.derivative.to_bits());
        match point.value {
            PointValue::Constant(value) => w.extend([0, value.to_bits()]),
            PointValue::Reg(reg) => w.extend([1, code.register(reg)]),
        }
    }
    Ok([at, points])
}

/// Per rarity divisor, Perlin set and level: the coordinate multipliers the
/// CPU applies (`x * xz / rarity`, times the set factor and level input).
fn level_table(
    w: &mut Vec<u32>,
    noise: &NormalNoise,
    divisors: &[f64],
    xz: f64,
    y: f64,
) -> Result<u32, String> {
    let at = w.len() as u32;
    for divisor in divisors {
        for (perlin, factor) in [(&noise.first, 1.0), (&noise.second, SECOND_SET_FACTOR)] {
            let mut input = perlin.input;
            for _ in &perlin.levels {
                let scale = factor * input;
                w.extend(fixed(xz / divisor * scale)?);
                w.extend(fixed(y / divisor * scale)?);
                w.push((scale as f32).to_bits());
                input *= 2.0;
            }
        }
    }
    Ok(at)
}

/// `k mod 256` as unsigned 8.56 fixed point: (lo, hi) words.
fn fixed(k: f64) -> Result<[u32; 2], String> {
    if !k.is_finite() {
        return Err(format!("non-finite noise coordinate scale {k}"));
    }
    let value = (k.rem_euclid(256.0) * 72_057_594_037_927_936.0) as u64;
    Ok([value as u32, (value >> 32) as u32])
}

/// The Y smear step of `ImprovedNoise::sample_with_y_scale` at unwrapped
/// octave y `y` (blended noise stays far inside one wrap period).
fn smear_delta(octave: &ImprovedNoise, y: f64, y_scale: f64, y_max: f64) -> f32 {
    let y = y + octave.yo;
    let yr = (y - y.floor()) as f32;
    let (y_scale, y_max) = (y_scale as f32, y_max as f32);
    if y_scale == 0.0 {
        return 0.0;
    }
    let limited = if y_max >= 0.0 && y_max < yr {
        y_max
    } else {
        yr
    };
    (limited / y_scale + 1.0e-7_f32).floor() * y_scale
}
