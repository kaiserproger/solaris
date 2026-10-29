//! Keyed xoroshiro noise initialization audited against the local 26.1.2 jar.
use md5::{Digest, Md5};

#[derive(Debug, Clone)]
pub(crate) struct Xoroshiro {
    lo: u64,
    hi: u64,
}
impl Xoroshiro {
    fn halves(mut lo: u64, mut hi: u64) -> Self {
        if lo | hi == 0 {
            lo = 0x9e3779b97f4a7c15;
            hi = 0x6a09e667f3bcc909;
        }
        Self { lo, hi }
    }
    fn seeded(seed: u64) -> Self {
        let lo = seed ^ 0x6a09e667f3bcc909;
        Self::halves(mix(lo), mix(lo.wrapping_add(0x9e3779b97f4a7c15)))
    }
    pub(crate) fn next_long(&mut self) -> u64 {
        let lo = self.lo;
        let hi = self.hi;
        let result = lo.wrapping_add(hi).rotate_left(17).wrapping_add(lo);
        let hi = hi ^ lo;
        self.lo = lo.rotate_left(49) ^ hi ^ (hi << 21);
        self.hi = hi.rotate_left(28);
        result
    }
    pub(crate) fn next_double(&mut self) -> f64 {
        (self.next_long() >> 11) as f64 * (1.0 / 9007199254740992.0)
    }
    pub(crate) fn next_int_bounded(&mut self, bound: i32) -> i32 {
        let bound = bound as u32;
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let product = u64::from(self.next_long() as u32) * u64::from(bound);
            if product as u32 >= threshold {
                return (product >> 32) as i32;
            }
        }
    }
    pub(crate) fn positional(&mut self) -> NoiseFactory {
        NoiseFactory {
            lo: self.next_long(),
            hi: self.next_long(),
        }
    }
}
fn mix(mut n: u64) -> u64 {
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
#[derive(Debug, Clone)]
pub(crate) struct NoiseFactory {
    lo: u64,
    hi: u64,
}
impl NoiseFactory {
    pub(crate) fn new(seed: u64) -> Self {
        Xoroshiro::seeded(seed).positional()
    }
    pub(crate) fn random(&self, identifier: &str) -> Xoroshiro {
        let digest = Md5::digest(identifier.as_bytes());
        let lo = u64::from_be_bytes(digest[..8].try_into().expect("MD5 half"));
        let hi = u64::from_be_bytes(digest[8..].try_into().expect("MD5 half"));
        Xoroshiro::halves(lo ^ self.lo, hi ^ self.hi)
    }
    pub(crate) fn at(&self, x: i32, y: i32, z: i32) -> Xoroshiro {
        let seed = i64::from(x.wrapping_mul(3_129_871))
            ^ i64::from(z).wrapping_mul(116_129_781)
            ^ i64::from(y);
        let seed = seed
            .wrapping_mul(seed)
            .wrapping_mul(42_317_861)
            .wrapping_add(seed.wrapping_mul(11))
            >> 16;
        Xoroshiro::halves(self.lo ^ seed as u64, self.hi)
    }
    pub(crate) fn normal(
        &self,
        identifier: &str,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> NormalNoise {
        let mut random = self.random(identifier);
        let first = PerlinNoise::new(&mut random, first_octave, amplitudes);
        let second = PerlinNoise::new(&mut random, first_octave, amplitudes);
        let min = amplitudes.iter().position(|a| *a != 0.0).unwrap_or(0);
        let max = amplitudes.iter().rposition(|a| *a != 0.0).unwrap_or(min);
        let value_factor = (1.0 / 6.0) / (0.1 * (1.0 + 1.0 / ((max - min) as f64 + 1.0)));
        NormalNoise {
            first,
            second,
            value_factor,
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct NormalNoise {
    pub(crate) first: PerlinNoise,
    pub(crate) second: PerlinNoise,
    pub(crate) value_factor: f64,
}
impl NormalNoise {
    pub(crate) fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        // Scale the precise coordinates BEFORE splitting each octave's lattice.
        // Casting x/y/z here would erase adjacent blocks near the world border.
        let factor = 1.0181268882175227;
        (self.first.sample(x, y, z) + self.second.sample(x * factor, y * factor, z * factor))
            * self.value_factor as f32
    }
    /// Proven uniform bound: `|sample(x, y, z)| <= returned` for every input.
    /// Each `ImprovedNoise` octave is a convex combination (fade weights in
    /// `[0, 1]`) of eight gradient dots, and every gradient-table row has
    /// L1 norm at most 2 with offsets in `[0, 1]` (`y_scale` is 0 on this
    /// path, so `gy = yr`), hence `|octave| <= 2` up to float rounding.
    /// Both octave sets share amplitudes and weights, so
    /// `|sample| <= 4 * |value_factor| * sum(|amplitude| * weight)`; the
    /// `1.001` factor plus `1e-9` widen past any rounding accumulation.
    /// `wrap` only moves the sampling lattice, so the bound is input
    /// independent. Used for proven Min/Max short-circuits, never sampled.
    pub(crate) fn uniform_bound(&self) -> f64 {
        let mut weighted = 0.0;
        let mut weight = self.first.value;
        for amplitude in &self.first.amplitudes {
            weighted += amplitude.abs() * weight;
            weight /= 2.0;
        }
        4.0 * self.value_factor.abs() * weighted * 1.001 + 1e-9
    }
}
#[derive(Debug, Clone)]
pub(crate) struct PerlinNoise {
    pub(crate) levels: Vec<Option<ImprovedNoise>>,
    pub(crate) amplitudes: Vec<f64>,
    pub(crate) input: f64,
    pub(crate) value: f64,
}
impl PerlinNoise {
    fn new(random: &mut Xoroshiro, first: i32, amplitudes: &[f64]) -> Self {
        let factory = random.positional();
        let levels = amplitudes
            .iter()
            .enumerate()
            .map(|(i, a)| {
                if *a == 0.0 {
                    None
                } else {
                    Some(ImprovedNoise::new(
                        &mut factory.random(&format!("octave_{}", first + i as i32)),
                    ))
                }
            })
            .collect();
        let n = amplitudes.len() as i32;
        Self {
            levels,
            amplitudes: amplitudes.to_vec(),
            input: 2f64.powi(first),
            value: 2f64.powi(n - 1) / (2f64.powi(n) - 1.0),
        }
    }
    fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut input = self.input;
        let mut weight = self.value as f32;
        let mut sum = 0.0;
        for (level, amplitude) in self.levels.iter().zip(&self.amplitudes) {
            if let Some(level) = level {
                sum += *amplitude as f32
                    * level.sample_with_y_scale(
                        wrap(x * input),
                        wrap(y * input),
                        wrap(z * input),
                        0.0,
                        0.0,
                    )
                    * weight;
            }
            input *= 2.0;
            weight /= 2.0;
        }
        sum
    }
}
pub(crate) fn wrap(v: f64) -> f64 {
    let rounded = v / 33554432.0 + 0.5;
    // Test after the addition: rounding can reach 1.0 just below the half-period.
    // A zero wrap count leaves v (including -0.0) unchanged.
    if (0.0..1.0).contains(&rounded) {
        v
    } else {
        v - rounded.floor() * 33554432.0
    }
}
#[derive(Debug, Clone)]
pub(crate) struct ImprovedNoise {
    pub(crate) p: [u8; 256],
    pub(crate) xo: f64,
    pub(crate) yo: f64,
    pub(crate) zo: f64,
}
impl ImprovedNoise {
    pub(crate) fn new(random: &mut Xoroshiro) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256 {
            let j = random.next_int_bounded((256 - i) as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }
    fn p(&self, n: i32) -> i32 {
        i32::from(self.p[(n & 255) as usize])
    }
    pub(crate) fn sample_with_y_scale(
        &self,
        x: f64,
        y: f64,
        z: f64,
        y_scale: f64,
        y_max: f64,
    ) -> f32 {
        let x = x + self.xo;
        let y = y + self.yo;
        let z = z + self.zo;
        let ix = floor_i32(x);
        let iy = floor_i32(y);
        let iz = floor_i32(z);
        // Only the small within-cell offsets enter fp32. Offsets and the
        // permutation's integer cell retain their fp64/integer precision.
        let xr = (x - f64::from(ix)) as f32;
        let yr = (y - f64::from(iy)) as f32;
        let zr = (z - f64::from(iz)) as f32;
        let y_scale = y_scale as f32;
        let y_max = y_max as f32;
        let delta = if y_scale != 0.0 {
            let limited = if y_max >= 0.0 && y_max < yr {
                y_max
            } else {
                yr
            };
            (limited / y_scale + 1.0e-7_f32).floor() * y_scale
        } else {
            0.0
        };
        let gy = yr - delta;
        let x0 = self.p(ix);
        let x1 = self.p(ix + 1);
        let a = self.p(x0 + iy);
        let b = self.p(x0 + iy + 1);
        let c = self.p(x1 + iy);
        let d = self.p(x1 + iy + 1);
        let fx = fade(xr);
        let fy = fade(yr);
        let fz = fade(zr);
        let lower = lerp(
            fy,
            lerp(
                fx,
                grad(self.p(a + iz), xr, gy, zr),
                grad(self.p(c + iz), xr - 1.0, gy, zr),
            ),
            lerp(
                fx,
                grad(self.p(b + iz), xr, gy - 1.0, zr),
                grad(self.p(d + iz), xr - 1.0, gy - 1.0, zr),
            ),
        );
        let upper = lerp(
            fy,
            lerp(
                fx,
                grad(self.p(a + iz + 1), xr, gy, zr - 1.0),
                grad(self.p(c + iz + 1), xr - 1.0, gy, zr - 1.0),
            ),
            lerp(
                fx,
                grad(self.p(b + iz + 1), xr, gy - 1.0, zr - 1.0),
                grad(self.p(d + iz + 1), xr - 1.0, gy - 1.0, zr - 1.0),
            ),
        );
        lerp(fz, lower, upper)
    }
}
/// Exactly `v.floor() as i32`, including saturating casts and NaN -> 0.
#[inline]
fn floor_i32(v: f64) -> i32 {
    let truncated = v as i32;
    // Conversion truncates toward zero. Only a negative fractional remainder
    // needs correction; saturation also preserves values at/below i32::MIN.
    truncated.saturating_sub(i32::from(v < f64::from(truncated)))
}
fn fade(v: f32) -> f32 {
    v * v * v * (v * (v * 6.0 - 15.0) + 10.0)
}
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}
fn grad(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    const G: [[f32; 3]; 16] = [
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, -1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [1.0, 0.0, -1.0],
        [-1.0, 0.0, -1.0],
        [0.0, 1.0, 1.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, -1.0],
        [0.0, -1.0, -1.0],
        [1.0, 1.0, 0.0],
        [0.0, -1.0, 1.0],
        [-1.0, 1.0, 0.0],
        [0.0, -1.0, -1.0],
    ];
    let g = G[(hash & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[path = "noise_batch.rs"]
mod batch;

#[cfg(test)]
#[path = "noise_tests.rs"]
mod tests;
