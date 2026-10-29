//! Far-terrain generation program for native clients the operator lets
//! generate never-generated terrain themselves (`[lod] share_terrain_keys`).
//!
//! It carries what [`VanillaDensity::lod_surface_height`] and the climate
//! biome choice need and nothing else: the `lod_surface` field and the six
//! climate fields flattened into straight-line code, the materialized octaves
//! (permutation table and offsets) of only the noises those fields read, and
//! the biome parameter rows. No level seed, positional factory seed or cave,
//! aquifer and ore-vein noise is included, but the octaves still derive from
//! the seed through a non-cryptographic generator: treat it as revealing it.
//!
//! Evaluation (the reference the client must reproduce): a column `(x, z)`
//! is snapped down to the cell grid. Surface code instructions
//! `[0, flat_len)` run once at `(x & !3, 0, z & !3)` (a `flat_cache`), the
//! rest at each lattice corner `(x, y, z)` from the top down, exactly as
//! [`VanillaDensity::lod_surface_height`]. The height is clamped to the world,
//! then the climate code runs at the biome quart of `(x, height, z)` (block
//! coordinates rounded down to multiples of 4, unsnapped `x`/`z`) and picks
//! the first parameter row with the smallest squared distance to the
//! quantized climate (`(value as f32 * 10000) as i64`, offset target 0).
//! Every instruction writes the register of its own index.
//!
//! Layout, little-endian, version 1:
//! - `b"SLOD"`, `u16 version`, `i32 min_y, height, sea_level, cell_width,
//!   cell_height`
//! - `u32 n` octaves: `[u8; 256] permutation, f64 xo, yo, zo`
//! - `u32 n` normal noises: `f64 value_factor`, then two Perlin sets of
//!   `f64 input, f64 value, u32 m` levels of `f64 amplitude, u32 octave`
//!   (`u32::MAX` = absent level)
//! - `u32 n` blended noises: `[u32; 40]` octaves (16 min, 16 max, 8 main),
//!   `f64 xz, y, xz_factor, y_factor, smear` (`xz`, `y` already times 684.412)
//! - surface code: `u32 flat_len, u32 root`, code; climate code:
//!   `[u32; 6] roots` (temperature, vegetation, continents, erosion, depth,
//!   ridges), code. Code: `u32 n` instructions of `u8 op, [u32; 4] args,
//!   [f64; 2] constants`, then `u32 n` spline points of `f32 location,
//!   f32 derivative, u8 kind (0 constant, 1 register), u32 value`
//!   (constant: `f32` bits)
//! - `u16 n` biome names (`u16 length`, UTF-8), `u16 n` parameter values
//!   (`i64`), `u32 n` rows of `[u8; 14]` value indices (min, max per
//!   temperature, humidity, continentalness, erosion, depth, weirdness,
//!   offset) and `u16` biome name index, in search order.
//!
//! Operations and their `args`/`constants`, `r` = register, `i` = integer:
//! 0 constant `c0`; 1 add, 2 mul, 3 min, 4 max `r0 r1`; 5 abs, 6 square,
//! 7 cube, 8 half_negative, 9 quarter_negative, 10 invert, 11 squeeze `r0`;
//! 12 clamp `r0`, `c0..c1`; 13 y gradient `i0 from_y, i1 to_y` (i32 bits),
//! `c0 from, c1 to`; 14 noise `i0 noise`, `c0 xz, c1 y` scale; 15 shift
//! `i0 noise, i1 kind (0 xyz, 1 x0z, 2 zx0)`; 16 shifted noise `i0 noise,
//! r1 r2 r3 shift xyz`, `c0 xz, c1 y`; 17 weird scaled `i0 noise, r1 input,
//! i2 type_2`; 18 range `r0 input, r1 inside, r2 outside`, `c0 min
//! inclusive, c1 max exclusive`; 19 spline `r0 coordinate, i1 first point,
//! i2 point count` (f32 arithmetic, as vanilla); 20 blended `i0 index`.
//! The node semantics are those of `evaluate.rs` in raw mode.

use std::collections::HashMap;

use super::{Binary, DensityRuntime, Id, Mapped, Marker, Node, Shift, Spline, VanillaDensity};
use crate::vanilla_climate::VanillaClimate;
use crate::vanilla_climate::noise::{ImprovedNoise, PerlinNoise};

pub const PROGRAM_VERSION: u16 = 1;

/// Encodes the far-terrain program; `Ok(None)` when the router lacks the
/// LOD surface shape.
pub(crate) fn encode(
    density: &VanillaDensity,
    climate: &VanillaClimate,
) -> Result<Option<Vec<u8>>, String> {
    let Some(surface) = density.lod_surface else {
        return Ok(None);
    };
    let runtime = &*density.runtime;
    if !std::ptr::eq(runtime, &*climate.runtime()) {
        return Err("climate and density must share one router".into());
    }
    let mut exporter = Exporter {
        runtime,
        octaves: Vec::new(),
        octave_ids: HashMap::new(),
        noises: Vec::new(),
        noise_ids: HashMap::new(),
        blended: Vec::new(),
        blended_ids: HashMap::new(),
        corner: None,
    };
    let mut surface_code = Code::default();
    let surface_root = exporter.emit(&mut surface_code, surface, Context::Terrain)?;
    let (fields, rows) = climate.program_parts();
    let mut climate_code = Code::default();
    let mut climate_roots = [Reg::Point(0); 6];
    for (root, field) in climate_roots.iter_mut().zip(fields) {
        *root = exporter.emit(&mut climate_code, field, Context::Raw)?;
    }
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

    let mut out = Writer(Vec::with_capacity(256 * 1024));
    out.0.extend(b"SLOD");
    out.u16(PROGRAM_VERSION);
    for value in [
        runtime.min_y,
        runtime.height,
        runtime.sea_level,
        runtime.cell_width,
        runtime.cell_height,
    ] {
        out.i32(value);
    }
    out.len(exporter.octaves.len())?;
    for octave in &exporter.octaves {
        out.0.extend(octave.p);
        for value in [octave.xo, octave.yo, octave.zo] {
            out.f64(value);
        }
    }
    out.len(exporter.noises.len())?;
    for &noise in &exporter.noises {
        let noise = &runtime.noises[noise];
        out.f64(noise.value_factor);
        for perlin in [&noise.first, &noise.second] {
            exporter.perlin(&mut out, perlin)?;
        }
    }
    out.len(exporter.blended.len())?;
    for &blended in &exporter.blended {
        let noise = &runtime.blended[blended];
        for octave in noise.min.iter().chain(&noise.max).chain(&noise.main) {
            out.u32(exporter.octave_ids[&std::ptr::from_ref(octave)]);
        }
        for value in [
            noise.xz,
            noise.y,
            noise.xz_factor,
            noise.y_factor,
            noise.smear,
        ] {
            out.f64(value);
        }
    }
    let flat_len = surface_code.flat.len();
    out.len(flat_len)?;
    out.u32(surface_code.register(surface_root));
    surface_code.write(&mut out)?;
    for root in climate_roots {
        out.u32(climate_code.register(root));
    }
    climate_code.write(&mut out)?;

    let mut names: Vec<&str> = Vec::new();
    let mut values: Vec<i64> = rows
        .iter()
        .flat_map(|(bounds, _)| bounds.iter().flatten().copied())
        .collect();
    values.sort_unstable();
    values.dedup();
    let value_ids: HashMap<i64, u8> = values
        .iter()
        .enumerate()
        .map(|(index, value)| u8::try_from(index).map(|index| (*value, index)))
        .collect::<Result<_, _>>()
        .map_err(|_| "more than 256 distinct biome parameter values")?;
    let mut name_ids = HashMap::new();
    let rows: Vec<([u8; 14], u16)> = rows
        .iter()
        .map(|(bounds, name)| {
            let next = names.len();
            let name = *name_ids.entry(*name).or_insert_with(|| {
                names.push(name);
                next
            });
            let mut indices = [0u8; 14];
            for (slot, value) in indices.iter_mut().zip(bounds.iter().flatten()) {
                *slot = value_ids[value];
            }
            (indices, name as u16)
        })
        .collect();
    out.u16(u16::try_from(names.len()).map_err(|_| "too many biomes")?);
    for name in names {
        out.u16(u16::try_from(name.len()).map_err(|_| "biome name too long")?);
        out.0.extend(name.as_bytes());
    }
    out.u16(u16::try_from(values.len()).map_err(|_| "too many values")?);
    for value in values {
        out.0.extend(value.to_le_bytes());
    }
    out.len(rows.len())?;
    for (indices, name) in rows {
        out.0.extend(indices);
        out.u16(name);
    }
    Ok(Some(out.0))
}

/// Position a node is evaluated at: the raw block position (climate), a
/// lattice corner (LOD surface), or the corner's `flat_cache` column.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Context {
    Raw,
    Terrain,
    Flat,
}

#[derive(Clone, Copy)]
enum Reg {
    Flat(u32),
    Point(u32),
}

#[derive(Clone, Copy)]
enum Arg {
    Int(u32),
    Reg(Reg),
}

struct Inst {
    op: u8,
    args: [Arg; 4],
    constants: [f64; 2],
}

enum PointValue {
    Constant(f32),
    Reg(Reg),
}

struct Point {
    location: f32,
    derivative: f32,
    value: PointValue,
}

#[derive(Default)]
struct Code {
    flat: Vec<Inst>,
    point: Vec<Inst>,
    points: Vec<Point>,
    memo: HashMap<(Id, Context), Reg>,
}

impl Code {
    fn register(&self, reg: Reg) -> u32 {
        match reg {
            Reg::Flat(index) => index,
            Reg::Point(index) => self.flat.len() as u32 + index,
        }
    }

    fn write(&self, out: &mut Writer) -> Result<(), String> {
        out.len(self.flat.len() + self.point.len())?;
        for inst in self.flat.iter().chain(&self.point) {
            out.0.push(inst.op);
            for arg in inst.args {
                out.u32(match arg {
                    Arg::Int(value) => value,
                    Arg::Reg(reg) => self.register(reg),
                });
            }
            for value in inst.constants {
                out.f64(value);
            }
        }
        out.len(self.points.len())?;
        for point in &self.points {
            out.0.extend(point.location.to_le_bytes());
            out.0.extend(point.derivative.to_le_bytes());
            match point.value {
                PointValue::Constant(value) => {
                    out.0.push(0);
                    out.u32(value.to_bits());
                }
                PointValue::Reg(reg) => {
                    out.0.push(1);
                    out.u32(self.register(reg));
                }
            }
        }
        Ok(())
    }
}

struct Exporter<'a> {
    runtime: &'a DensityRuntime,
    octaves: Vec<&'a ImprovedNoise>,
    octave_ids: HashMap<*const ImprovedNoise, u32>,
    noises: Vec<usize>,
    noise_ids: HashMap<usize, u32>,
    blended: Vec<usize>,
    blended_ids: HashMap<usize, u32>,
    /// Chunk-fill mode only (`program_gpu.rs`): the lattice-corner code and
    /// its roots, one per `interpolated` marker met in block context.
    corner: Option<(Code, Vec<Reg>)>,
}

impl<'a> Exporter<'a> {
    fn octave(&mut self, octave: &'a ImprovedNoise) -> u32 {
        let next = self.octaves.len() as u32;
        *self
            .octave_ids
            .entry(std::ptr::from_ref(octave))
            .or_insert_with(|| {
                self.octaves.push(octave);
                next
            })
    }

    fn noise(&mut self, noise: usize) -> Arg {
        let next = self.noises.len() as u32;
        Arg::Int(*self.noise_ids.entry(noise).or_insert_with(|| {
            self.noises.push(noise);
            next
        }))
    }

    fn perlin(&self, out: &mut Writer, perlin: &PerlinNoise) -> Result<(), String> {
        out.f64(perlin.input);
        out.f64(perlin.value);
        out.len(perlin.levels.len())?;
        for (level, amplitude) in perlin.levels.iter().zip(&perlin.amplitudes) {
            out.f64(*amplitude);
            out.u32(level.as_ref().map_or(u32::MAX, |octave| {
                self.octave_ids[&std::ptr::from_ref(octave)]
            }));
        }
        Ok(())
    }

    fn emit(&mut self, code: &mut Code, id: Id, context: Context) -> Result<Reg, String> {
        if let Some(&reg) = code.memo.get(&(id, context)) {
            return Ok(reg);
        }
        let int = |value: i32| Arg::Int(value as u32);
        let none = Arg::Int(0);
        let runtime = self.runtime;
        let (op, args, constants) = match &runtime.nodes[id] {
            Node::Constant(value) => (0, [none; 4], [*value, 0.0]),
            Node::Binary(kind, a, b) => {
                let a = Arg::Reg(self.emit(code, *a, context)?);
                let b = Arg::Reg(self.emit(code, *b, context)?);
                let op = match kind {
                    Binary::Add => 1,
                    Binary::Mul => 2,
                    Binary::Min => 3,
                    Binary::Max => 4,
                };
                (op, [a, b, none, none], [0.0; 2])
            }
            Node::Mapped(kind, input) => {
                let input = Arg::Reg(self.emit(code, *input, context)?);
                let op = match kind {
                    Mapped::Abs => 5,
                    Mapped::Square => 6,
                    Mapped::Cube => 7,
                    Mapped::HalfNegative => 8,
                    Mapped::QuarterNegative => 9,
                    Mapped::Invert => 10,
                    Mapped::Squeeze => 11,
                };
                (op, [input, none, none, none], [0.0; 2])
            }
            Node::Clamp(input, min, max) => {
                let input = Arg::Reg(self.emit(code, *input, context)?);
                (12, [input, none, none, none], [*min, *max])
            }
            Node::Gradient(from_y, to_y, from, to) => {
                (13, [int(*from_y), int(*to_y), none, none], [*from, *to])
            }
            Node::Noise(noise, xz, y) => (14, [self.noise(*noise), none, none, none], [*xz, *y]),
            Node::Shift(noise, shift) => {
                let kind = match shift {
                    Shift::All => 0,
                    Shift::A => 1,
                    Shift::B => 2,
                };
                (15, [self.noise(*noise), int(kind), none, none], [0.0; 2])
            }
            Node::Shifted(noise, sx, sy, sz, xz, y) => {
                let noise = self.noise(*noise);
                let sx = Arg::Reg(self.emit(code, *sx, context)?);
                let sy = Arg::Reg(self.emit(code, *sy, context)?);
                let sz = Arg::Reg(self.emit(code, *sz, context)?);
                (16, [noise, sx, sy, sz], [*xz, *y])
            }
            Node::Weird(noise, input, two_dimensional) => {
                let noise = self.noise(*noise);
                let input = Arg::Reg(self.emit(code, *input, context)?);
                let kind = Arg::Int(u32::from(*two_dimensional));
                (17, [noise, input, kind, none], [0.0; 2])
            }
            Node::Range(input, min, max, inside, outside) => {
                let input = Arg::Reg(self.emit(code, *input, context)?);
                let inside = Arg::Reg(self.emit(code, *inside, context)?);
                let outside = Arg::Reg(self.emit(code, *outside, context)?);
                (18, [input, inside, outside, none], [*min, *max])
            }
            Node::Spline(spline) => {
                let reg = match self.spline(code, spline, context)? {
                    PointValue::Reg(reg) => reg,
                    PointValue::Constant(value) => {
                        push(code, context, 0, [none; 4], [f64::from(value), 0.0])
                    }
                };
                code.memo.insert((id, context), reg);
                return Ok(reg);
            }
            Node::Marker(Marker::Interpolated, input)
                if context == Context::Raw && self.corner.is_some() =>
            {
                let (mut corner, mut slots) = self.corner.take().expect("guarded");
                let inner = self.emit(&mut corner, *input, Context::Terrain);
                let slot = slots.len() as u32;
                if let Ok(reg) = inner {
                    slots.push(reg);
                }
                self.corner = Some((corner, slots));
                inner?;
                (21, [Arg::Int(slot), none, none, none], [0.0; 2])
            }
            Node::Marker(Marker::FlatCache | Marker::Cache2D, _)
                if context == Context::Raw && self.corner.is_some() =>
            {
                return Err("column caches outside `interpolated` are not supported".into());
            }
            Node::Marker(marker, input) => {
                let inner = match (marker, context) {
                    (Marker::FlatCache, Context::Terrain | Context::Flat) => Context::Flat,
                    _ => context,
                };
                let reg = self.emit(code, *input, inner)?;
                code.memo.insert((id, context), reg);
                return Ok(reg);
            }
            Node::Blended(index) => {
                let next = self.blended.len() as u32;
                let index = *self.blended_ids.entry(*index).or_insert_with(|| {
                    self.blended.push(*index);
                    next
                });
                (20, [Arg::Int(index), none, none, none], [0.0; 2])
            }
            Node::FindTop(..) => {
                return Err("find_top_surface is not supported in the far-terrain program".into());
            }
        };
        let reg = push(code, context, op, args, constants);
        code.memo.insert((id, context), reg);
        Ok(reg)
    }

    fn spline(
        &mut self,
        code: &mut Code,
        spline: &Spline,
        context: Context,
    ) -> Result<PointValue, String> {
        let (coordinate, points) = match spline {
            Spline::Constant(value) => return Ok(PointValue::Constant(*value)),
            Spline::Multipoint { coordinate, points } => (coordinate, points),
        };
        let coordinate = self.emit(code, *coordinate, context)?;
        let mut resolved = Vec::with_capacity(points.len());
        for point in points {
            resolved.push(Point {
                location: point.location,
                derivative: point.derivative,
                value: self.spline(code, &point.value, context)?,
            });
        }
        let first = code.points.len() as u32;
        let count = resolved.len() as u32;
        code.points.extend(resolved);
        Ok(PointValue::Reg(push(
            code,
            context,
            19,
            [
                Arg::Reg(coordinate),
                Arg::Int(first),
                Arg::Int(count),
                Arg::Int(0),
            ],
            [0.0; 2],
        )))
    }
}

fn push(code: &mut Code, context: Context, op: u8, args: [Arg; 4], constants: [f64; 2]) -> Reg {
    let inst = Inst {
        op,
        args,
        constants,
    };
    if context == Context::Flat {
        code.flat.push(inst);
        Reg::Flat(code.flat.len() as u32 - 1)
    } else {
        code.point.push(inst);
        Reg::Point(code.point.len() as u32 - 1)
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, value: u16) {
        self.0.extend(value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend(value.to_le_bytes());
    }
    fn i32(&mut self, value: i32) {
        self.0.extend(value.to_le_bytes());
    }
    fn f64(&mut self, value: f64) {
        self.0.extend(value.to_le_bytes());
    }
    fn len(&mut self, value: usize) -> Result<(), String> {
        self.u32(u32::try_from(value).map_err(|_| "program table too large")?);
        Ok(())
    }
}

#[cfg(feature = "gpu")]
#[path = "program_gpu.rs"]
pub(crate) mod gpu;
