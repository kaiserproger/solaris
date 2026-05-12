//! Tiny hash-based 2D value noise for the M7 baseline terrain.
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

/// Lattice value at integer `(x, z)`. `[-1.0, 1.0]`.
#[inline]
fn lattice(x: i32, z: i32, seed: i64) -> f64 {
    let h = hash3(x, z, seed);
    // Map `0..=u32::MAX` to `-1..=1` linearly.
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
/// `seed` mixes into every lattice sample so two different seeds
/// produce uncorrelated fields. The noise period is `2^31` cells
/// (the input ints), which is far beyond any reasonable world size.
#[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_calls() {
        let a = value_noise_2d(3.7, -12.4, 42);
        let b = value_noise_2d(3.7, -12.4, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn output_is_bounded() {
        for x in -50..=50 {
            for z in -50..=50 {
                let v = value_noise_2d(x as f64 * 0.37, z as f64 * 0.41, 7);
                assert!(v >= -1.0 && v <= 1.0, "value out of [-1,1]: {v}");
            }
        }
    }

    #[test]
    fn different_seeds_differ() {
        // Pick a handful of points and assert at least one differs
        // between seeds 0 and 1. We can't assert *every* point
        // differs (two seeds may collide on any single lattice cell)
        // but pairs ought to disagree somewhere.
        let mut any_diff = false;
        for x in 0..10 {
            for z in 0..10 {
                let a = value_noise_2d(x as f64, z as f64, 0);
                let b = value_noise_2d(x as f64, z as f64, 1);
                if (a - b).abs() > 1e-9 {
                    any_diff = true;
                }
            }
        }
        assert!(
            any_diff,
            "two seeds produced identical noise across 100 samples — implausible"
        );
    }

    #[test]
    fn fbm_is_bounded() {
        for x in -20..=20 {
            for z in -20..=20 {
                let v = fbm_2d(x as f64 * 0.1, z as f64 * 0.1, 13, 4, 0.5);
                assert!(v >= -1.0 && v <= 1.0, "fbm out of [-1,1]: {v}");
            }
        }
    }

    #[test]
    fn neighbour_samples_are_continuous() {
        // Smooth noise should produce small deltas between very
        // close inputs. Pick a step size much smaller than 1
        // (the lattice spacing) and confirm.
        let a = value_noise_2d(5.0, 5.0, 99);
        let b = value_noise_2d(5.001, 5.0, 99);
        assert!((a - b).abs() < 0.05, "expected smooth noise: {a} vs {b}");
    }
}
