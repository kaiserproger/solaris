//! Vanilla `NormalNoise`, as `noise_threshold_provider` uses it.
//!
//! `NormalNoise` sums two `PerlinNoise` fields — each holding one
//! `ImprovedNoise` per non-zero amplitude octave, forked from
//! `PositionalRandomFactory.fromHashOf("octave_<n>")` — and scales the result by
//! `0.16666666666666666 / expectedDeviation(octaveSpan)`. Only this one entry
//! point is implemented because it is all `noise_threshold_provider` reaches.

use super::random::{PositionalRandomSource, RandomSource};

/// `NormalNoise.INPUT_FACTOR`.
const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;
/// `PerlinNoise.ROUND_OFF`, the wrap period of `PerlinNoise.wrap`.
const ROUND_OFF: f64 = 3.355_443_2e7;

/// `SimplexNoise.GRADIENT`, indexed by `hash & 15`.
const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// Vanilla `NormalNoise`.
#[derive(Debug, Clone)]
pub struct NormalNoise {
    first: PerlinNoise,
    second: PerlinNoise,
    value_factor: f64,
}

impl NormalNoise {
    /// Vanilla `NormalNoise.create(random, firstOctave, amplitudes)`. Vanilla
    /// wraps the seed in `WorldgenRandom`, whose `next`/`forkPositional` simply
    /// delegate to the legacy source; a `LegacyRandom` is therefore equivalent.
    #[must_use]
    pub fn create(
        random: &mut impl PositionalRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let first = PerlinNoise::create(random, first_octave, amplitudes);
        let second = PerlinNoise::create(random, first_octave, amplitudes);

        let mut min_octave = i32::MAX;
        let mut max_octave = i32::MIN;
        for (index, amplitude) in amplitudes.iter().enumerate() {
            if *amplitude != 0.0 {
                let octave = index as i32;
                min_octave = min_octave.min(octave);
                max_octave = max_octave.max(octave);
            }
        }
        let span = max_octave.wrapping_sub(min_octave);
        let value_factor = 0.166_666_666_666_666_66 / expected_deviation(span);
        Self {
            first,
            second,
            value_factor,
        }
    }

    /// Vanilla `NormalNoise.getValue`.
    #[must_use]
    pub fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        let x2 = x * INPUT_FACTOR;
        let y2 = y * INPUT_FACTOR;
        let z2 = z * INPUT_FACTOR;
        (self.first.value(x, y, z) + self.second.value(x2, y2, z2)) * self.value_factor
    }
}

/// `NormalNoise.expectedDeviation`.
fn expected_deviation(octave_span: i32) -> f64 {
    0.1 * (1.0 + 1.0 / (f64::from(octave_span) + 1.0))
}

/// Vanilla `PerlinNoise`.
#[derive(Debug, Clone)]
struct PerlinNoise {
    levels: Vec<Option<ImprovedNoise>>,
    amplitudes: Vec<f64>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
}

impl PerlinNoise {
    /// Vanilla `PerlinNoise.create(random, firstOctave, amplitudes)` with the
    /// new (positional-factory) initialization.
    fn create(
        random: &mut impl PositionalRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let octaves = amplitudes.len();
        let zero_octave_index = -first_octave;
        let factory = random.fork_positional();
        let mut levels = Vec::with_capacity(octaves);
        for (index, amplitude) in amplitudes.iter().enumerate() {
            if *amplitude != 0.0 {
                let octave = first_octave + index as i32;
                let mut source = factory.from_hash_of(&format!("octave_{octave}"));
                levels.push(Some(ImprovedNoise::new(&mut source)));
            } else {
                levels.push(None);
            }
        }
        let factor = 2f64.powi(-zero_octave_index);
        Self {
            levels,
            amplitudes: amplitudes.to_vec(),
            lowest_freq_input_factor: factor,
            lowest_freq_value_factor: 2f64.powi(octaves as i32 - 1)
                / (2f64.powi(octaves as i32) - 1.0),
        }
    }

    /// Vanilla `PerlinNoise.getValue(x, y, z)` (the `yScale`/`yFudge` overload
    /// with both zero, which is what `NoiseBasedStateProvider` calls).
    fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut value = 0.0;
        let mut factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;
        for (index, level) in self.levels.iter().enumerate() {
            if let Some(noise) = level {
                value += self.amplitudes[index]
                    * noise.noise(wrap(x * factor), wrap(y * factor), wrap(z * factor))
                    * value_factor;
            }
            factor *= 2.0;
            value_factor /= 2.0;
        }
        value
    }
}

/// `PerlinNoise.wrap`.
fn wrap(value: f64) -> f64 {
    value - ((value / ROUND_OFF + 0.5).floor()) * ROUND_OFF
}

/// Vanilla `ImprovedNoise`.
#[derive(Debug, Clone)]
struct ImprovedNoise {
    p: [u8; 256],
    xo: f64,
    yo: f64,
    zo: f64,
}

impl ImprovedNoise {
    fn new(random: &mut impl RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (index, value) in p.iter_mut().enumerate() {
            *value = index as u8;
        }
        for index in 0..256 {
            let offset = random.next_int_bounded(256 - index as i32) as usize;
            p.swap(index, index + offset);
        }
        Self { p, xo, yo, zo }
    }

    /// Vanilla `ImprovedNoise.noise(x, y, z)`: the `noise(x, y, z, 0.0, 0.0)`
    /// overload leaves `yFudge` zero, so `sampleAndLerp` gets `yr` twice.
    fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        let x = x + self.xo;
        let y = y + self.yo;
        let z = z + self.zo;
        let xf = floor(x);
        let yf = floor(y);
        let zf = floor(z);
        self.sample_and_lerp(
            xf,
            yf,
            zf,
            x - f64::from(xf),
            y - f64::from(yf),
            z - f64::from(zf),
        )
    }

    fn p(&self, x: i32) -> i32 {
        i32::from(self.p[(x & 0xFF) as usize])
    }

    fn sample_and_lerp(&self, x: i32, y: i32, z: i32, xr: f64, yr: f64, zr: f64) -> f64 {
        let x0 = self.p(x);
        let x1 = self.p(x + 1);
        let xy00 = self.p(x0 + y);
        let xy01 = self.p(x0 + y + 1);
        let xy10 = self.p(x1 + y);
        let xy11 = self.p(x1 + y + 1);
        let d000 = grad_dot(self.p(xy00 + z), xr, yr, zr);
        let d100 = grad_dot(self.p(xy10 + z), xr - 1.0, yr, zr);
        let d010 = grad_dot(self.p(xy01 + z), xr, yr - 1.0, zr);
        let d110 = grad_dot(self.p(xy11 + z), xr - 1.0, yr - 1.0, zr);
        let d001 = grad_dot(self.p(xy00 + z + 1), xr, yr, zr - 1.0);
        let d101 = grad_dot(self.p(xy10 + z + 1), xr - 1.0, yr, zr - 1.0);
        let d011 = grad_dot(self.p(xy01 + z + 1), xr, yr - 1.0, zr - 1.0);
        let d111 = grad_dot(self.p(xy11 + z + 1), xr - 1.0, yr - 1.0, zr - 1.0);
        let x_alpha = smoothstep(xr);
        let y_alpha = smoothstep(yr);
        let z_alpha = smoothstep(zr);
        lerp3(
            x_alpha, y_alpha, z_alpha, d000, d100, d010, d110, d001, d101, d011, d111,
        )
    }
}

fn grad_dot(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    let gradient = GRADIENT[(hash & 15) as usize];
    f64::from(gradient[0]) * x + f64::from(gradient[1]) * y + f64::from(gradient[2]) * z
}

/// `Mth.floor` (`(int) Math.floor(v)`).
fn floor(value: f64) -> i32 {
    value.floor() as i32
}

/// `Mth.smoothstep`.
fn smoothstep(value: f64) -> f64 {
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

/// `Mth.lerp`.
fn lerp(alpha: f64, first: f64, second: f64) -> f64 {
    first + alpha * (second - first)
}

/// `Mth.lerp2`.
fn lerp2(alpha1: f64, alpha2: f64, x00: f64, x10: f64, x01: f64, x11: f64) -> f64 {
    lerp(alpha2, lerp(alpha1, x00, x10), lerp(alpha1, x01, x11))
}

/// `Mth.lerp3`.
#[allow(clippy::too_many_arguments)]
fn lerp3(
    alpha1: f64,
    alpha2: f64,
    alpha3: f64,
    x000: f64,
    x100: f64,
    x010: f64,
    x110: f64,
    x001: f64,
    x101: f64,
    x011: f64,
    x111: f64,
) -> f64 {
    lerp(
        alpha3,
        lerp2(alpha1, alpha2, x000, x100, x010, x110),
        lerp2(alpha1, alpha2, x001, x101, x011, x111),
    )
}
