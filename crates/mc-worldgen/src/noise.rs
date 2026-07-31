//! Small hash-based value noise used by deterministic world generation.
//!
//! Not a translation of vanilla's algorithm — Solaris uses its own
//! deterministic noise per ADR 0001 / PROJECT_SPEC §8.1. The shape is
//! a Perlin-style fade curve over lattice values produced by a
//! `xxhash`-flavoured scrambler. Outputs are continuous,
//! deterministic in `(seed, x, z)`, and bounded to `[-1.0, 1.0]`.
//!
//! The implementation is intentionally small and dep-free; we trade
//! the spectral quality of a full simplex / OpenSimplex setup for
//! readability and zero extra dependencies.

const HASH_PRIME_1: u64 = 0x9E37_79B9_7F4A_7C15;
const HASH_PRIME_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const HASH_PRIME_3: u64 = 0x165667B19E3779F9;

/// Scramble three signed integers into a 32-bit value. Bit-mixed in a
/// way that makes adjacent inputs produce uncorrelated outputs — the
/// pre-condition for a usable hash-noise.
#[inline]
fn hash3(x: i32, z: i32, seed: i64) -> u32 {
    let mut h = seed as u64;
    h = h.wrapping_add(x as i64 as u64).wrapping_mul(HASH_PRIME_1);
    h ^= h >> 27;
    h = h.wrapping_add(z as i64 as u64).wrapping_mul(HASH_PRIME_2);
    h ^= h >> 23;
    h = h.wrapping_mul(HASH_PRIME_3);
    h ^= h >> 32;
    (h & 0xFFFF_FFFF) as u32
}

#[inline]
fn hash4(x: i32, y: i32, z: i32, seed: i64) -> u32 {
    let mut h = (seed as u64 ^ HASH_PRIME_2).wrapping_mul(HASH_PRIME_3);
    h ^= h >> 31;
    h ^= (x as i64 as u64).wrapping_mul(HASH_PRIME_1);
    h = h.wrapping_mul(HASH_PRIME_2);
    h ^= h >> 27;
    h ^= (y as i64 as u64).wrapping_mul(HASH_PRIME_2);
    h = h.wrapping_mul(HASH_PRIME_3);
    h ^= h >> 23;
    h ^= (z as i64 as u64).wrapping_mul(HASH_PRIME_3);
    h ^= h >> 29;
    h = h.wrapping_mul(HASH_PRIME_1 ^ HASH_PRIME_3);
    h ^= h >> 32;
    (h & 0xFFFF_FFFF) as u32
}

/// Lattice value at integer `(x, z)`. `[-1.0, 1.0]`.
#[inline]
fn lattice(x: i32, z: i32, seed: i64) -> f64 {
    let h = hash3(x, z, seed);
    // Map `0..=u32::MAX` to `-1..=1` linearly.
    (h as f64 / u32::MAX as f64) * 2.0 - 1.0
}

#[inline]
fn lattice_3d(x: i32, y: i32, z: i32, seed: i64) -> f64 {
    let h = hash4(x, y, z, seed);
    (h as f64 / u32::MAX as f64) * 2.0 - 1.0
}

/// Perlin's quintic fade: `6t^5 - 15t^4 + 10t^3`. C2-smooth, removes
/// the visible grid that a plain bilinear blend leaves behind.
#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Smooth 2D value noise. Returns a value in `[-1.0, 1.0]`.
///
/// `seed` contributes to every lattice sample so different seeds produce
/// deterministic field variants. The input period is `2^31` cells, far beyond
/// any reasonable world size.
#[must_use]
#[inline(always)]
pub fn value_noise_2d(x: f64, z: f64, seed: i64) -> f64 {
    let xi = x.floor();
    let zi = z.floor();
    let xf = x - xi;
    let zf = z - zi;
    let xi = xi as i32;
    let zi = zi as i32;

    let v00 = lattice(xi, zi, seed);
    let v10 = lattice(xi.wrapping_add(1), zi, seed);
    let v01 = lattice(xi, zi.wrapping_add(1), seed);
    let v11 = lattice(xi.wrapping_add(1), zi.wrapping_add(1), seed);

    let u = fade(xf);
    let v = fade(zf);

    let a = lerp(v00, v10, u);
    let b = lerp(v01, v11, u);
    lerp(a, b, v)
}

/// Multi-octave value noise. Each octave doubles the input frequency
/// and halves the amplitude. Useful when a single octave looks too
/// blobby. `octaves=1` is equivalent to [`value_noise_2d`].
#[must_use]
#[inline(always)]
pub fn fbm_2d(x: f64, z: f64, seed: i64, octaves: u32, persistence: f64) -> f64 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut max = 0.0;
    for o in 0..octaves {
        total += value_noise_2d(x * frequency, z * frequency, seed ^ (o as i64 + 1)) * amplitude;
        max += amplitude;
        amplitude *= persistence;
        frequency *= 2.0;
    }
    if max > 0.0 { total / max } else { 0.0 }
}

/// Smooth 3D value noise. Returns a value in `[-1.0, 1.0]`.
#[must_use]
pub fn value_noise_3d(x: f64, y: f64, z: f64, seed: i64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let zi = z.floor();
    let xf = x - xi;
    let yf = y - yi;
    let zf = z - zi;
    let xi = xi as i32;
    let yi = yi as i32;
    let zi = zi as i32;

    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);
    let x00 = lerp(
        lattice_3d(xi, yi, zi, seed),
        lattice_3d(xi.wrapping_add(1), yi, zi, seed),
        u,
    );
    let x10 = lerp(
        lattice_3d(xi, yi.wrapping_add(1), zi, seed),
        lattice_3d(xi.wrapping_add(1), yi.wrapping_add(1), zi, seed),
        u,
    );
    let x01 = lerp(
        lattice_3d(xi, yi, zi.wrapping_add(1), seed),
        lattice_3d(xi.wrapping_add(1), yi, zi.wrapping_add(1), seed),
        u,
    );
    let x11 = lerp(
        lattice_3d(xi, yi.wrapping_add(1), zi.wrapping_add(1), seed),
        lattice_3d(
            xi.wrapping_add(1),
            yi.wrapping_add(1),
            zi.wrapping_add(1),
            seed,
        ),
        u,
    );
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

#[derive(Clone, Copy)]
struct GridAxisPoint {
    lattice_offset: usize,
    fade: f64,
}

struct ValueNoise3dGrid {
    x: Vec<GridAxisPoint>,
    z: Vec<GridAxisPoint>,
    lattice_width: usize,
    lower_y: Vec<f64>,
    upper_y: Vec<f64>,
    y_fade: f64,
}

impl ValueNoise3dGrid {
    fn new(
        min_world_x: i32,
        min_world_z: i32,
        side: usize,
        world_y: i32,
        divisors: [f64; 3],
        frequency: f64,
        seed: i64,
    ) -> Self {
        let (x, min_lattice_x, max_lattice_x) =
            grid_axis(min_world_x, side, divisors[0], frequency);
        let (z, min_lattice_z, max_lattice_z) =
            grid_axis(min_world_z, side, divisors[2], frequency);
        let lattice_width = usize::try_from(max_lattice_x - min_lattice_x + 2)
            .expect("bounded noise lattice width fits usize");
        let lattice_height = usize::try_from(max_lattice_z - min_lattice_z + 2)
            .expect("bounded noise lattice height fits usize");
        let y = (f64::from(world_y) / divisors[1]) * frequency;
        let lattice_y = y.floor();
        let y_fade = fade(y - lattice_y);
        let lattice_y = lattice_y as i32;
        let mut lower_y = Vec::with_capacity(lattice_width * lattice_height);
        let mut upper_y = Vec::with_capacity(lattice_width * lattice_height);
        for z_offset in 0..lattice_height {
            let lattice_z = min_lattice_z.wrapping_add(z_offset as i32);
            for x_offset in 0..lattice_width {
                let lattice_x = min_lattice_x.wrapping_add(x_offset as i32);
                lower_y.push(lattice_3d(lattice_x, lattice_y, lattice_z, seed));
                upper_y.push(lattice_3d(
                    lattice_x,
                    lattice_y.wrapping_add(1),
                    lattice_z,
                    seed,
                ));
            }
        }
        Self {
            x,
            z,
            lattice_width,
            lower_y,
            upper_y,
            y_fade,
        }
    }

    #[inline]
    fn sample(&self, x: usize, z: usize) -> f64 {
        let x = self.x[x];
        let z = self.z[z];
        let lower_left = z.lattice_offset * self.lattice_width + x.lattice_offset;
        let upper_left = lower_left + self.lattice_width;
        let x00 = lerp(
            self.lower_y[lower_left],
            self.lower_y[lower_left + 1],
            x.fade,
        );
        let x10 = lerp(
            self.upper_y[lower_left],
            self.upper_y[lower_left + 1],
            x.fade,
        );
        let x01 = lerp(
            self.lower_y[upper_left],
            self.lower_y[upper_left + 1],
            x.fade,
        );
        let x11 = lerp(
            self.upper_y[upper_left],
            self.upper_y[upper_left + 1],
            x.fade,
        );
        lerp(
            lerp(x00, x10, self.y_fade),
            lerp(x01, x11, self.y_fade),
            z.fade,
        )
    }
}

fn grid_axis(
    min_world: i32,
    side: usize,
    divisor: f64,
    frequency: f64,
) -> (Vec<GridAxisPoint>, i32, i32) {
    let mut points = Vec::with_capacity(side);
    let mut min_lattice = i32::MAX;
    let mut max_lattice = i32::MIN;
    for offset in 0..side {
        let world = i64::from(min_world) + offset as i64;
        let world = i32::try_from(world).expect("noise grid stays inside i32 block coordinates");
        let value = (f64::from(world) / divisor) * frequency;
        let lattice = value.floor();
        let lattice_i32 = lattice as i32;
        min_lattice = min_lattice.min(lattice_i32);
        max_lattice = max_lattice.max(lattice_i32);
        points.push((lattice_i32, fade(value - lattice)));
    }
    let points = points
        .into_iter()
        .map(|(lattice, fade)| GridAxisPoint {
            lattice_offset: usize::try_from(lattice - min_lattice)
                .expect("noise lattice offset fits usize"),
            fade,
        })
        .collect();
    (points, min_lattice, max_lattice)
}

pub(crate) struct Fbm3dTwoOctaveGrid {
    first: ValueNoise3dGrid,
    second: ValueNoise3dGrid,
}

impl Fbm3dTwoOctaveGrid {
    pub(crate) fn new(
        min_world_x: i32,
        min_world_z: i32,
        side: usize,
        world_y: i32,
        divisors: [f64; 3],
        seed: i64,
    ) -> Self {
        Self {
            first: ValueNoise3dGrid::new(
                min_world_x,
                min_world_z,
                side,
                world_y,
                divisors,
                1.0,
                seed ^ 1,
            ),
            second: ValueNoise3dGrid::new(
                min_world_x,
                min_world_z,
                side,
                world_y,
                divisors,
                2.0,
                seed ^ 2,
            ),
        }
    }

    #[inline]
    pub(crate) fn sample(&self, x: usize, z: usize) -> f64 {
        let mut total = 0.0;
        total += self.first.sample(x, z);
        total += self.second.sample(x, z) * 0.5;
        total / 1.5
    }
}

/// Multi-octave 3D value noise with normalized amplitude.
#[must_use]
pub fn fbm_3d(x: f64, y: f64, z: f64, seed: i64, octaves: u32, persistence: f64) -> f64 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut max = 0.0;
    for octave in 0..octaves {
        total += value_noise_3d(
            x * frequency,
            y * frequency,
            z * frequency,
            seed ^ (octave as i64 + 1),
        ) * amplitude;
        max += amplitude;
        amplitude *= persistence;
        frequency *= 2.0;
    }
    if max > 0.0 { total / max } else { 0.0 }
}

#[cfg(test)]
#[path = "noise_tests.rs"]
mod tests;
