//! Vanilla random sources, reduced to what the A1 executor consumes.
//!
//! `minecraft`'s `LegacyRandomSource` is a 48-bit LCG with `java.util.Random`'s
//! bit layout, and every draw here is written to reproduce it exactly: same
//! modulus, same multipliers, same `next(bits)` shifts, same bounded-integer
//! rejection loop, same float/double composition, same per-position seeding.

/// `LegacyRandomSource.MODULUS_MASK`.
const MODULUS_MASK: u64 = (1 << 48) - 1;
/// `LegacyRandomSource.MULTIPLIER`.
const MULTIPLIER: u64 = 25_214_903_917;
/// `LegacyRandomSource.INCREMENT`.
const INCREMENT: u64 = 11;
/// `BitRandomSource.FLOAT_MULTIPLIER` (`2^-24`).
const FLOAT_MULTIPLIER: f32 = 5.960_464_5e-8;
/// `BitRandomSource.DOUBLE_MULTIPLIER`. Vanilla declares it as
/// `double DOUBLE_MULTIPLIER = 1.110223E-16F;` — a *float* literal widened to
/// double — which is exactly `2^-53`. Keep it written through `f32` so nobody
/// "simplifies" it into a differently-rounded double literal: any other value
/// silently changes every `nextDouble`.
const DOUBLE_MULTIPLIER: f64 = 1.110_223e-16_f32 as f64;

/// The `RandomSource` surface the executor uses, with `BitRandomSource`'s
/// default compositions.
pub trait RandomSource {
    /// Vanilla `RandomSource.next(bits)`.
    fn next(&mut self, bits: u32) -> i32;

    /// Vanilla `LegacyRandomSource.forkPositional()`. Draws one `nextLong`.
    fn fork_positional(&mut self) -> LegacyPositionalRandomFactory;

    fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// Vanilla `BitRandomSource.nextInt(bound)`. `bound` must be positive, as
    /// vanilla's `IllegalArgumentException` precondition requires.
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "random bound must be positive");
        if bound & (bound - 1) == 0 {
            let sample = i64::from(self.next(31));
            return ((sample * i64::from(bound)) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    fn next_long(&mut self) -> i64 {
        let upper = i64::from(self.next(32));
        let lower = i64::from(self.next(32));
        (upper << 32).wrapping_add(lower)
    }

    fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_MULTIPLIER
    }

    fn next_double(&mut self) -> f64 {
        let upper = i64::from(self.next(26));
        let lower = i64::from(self.next(27));
        ((upper << 27) + lower) as f64 * DOUBLE_MULTIPLIER
    }

    /// Vanilla `RandomSource.consumeCount`.
    fn consume_count(&mut self, rounds: u32) {
        for _ in 0..rounds {
            self.next_int();
        }
    }
}

/// Vanilla `LegacyRandomSource` (`java.util.Random`'s truncated 48-bit LCG).
#[derive(Debug, Clone)]
pub struct LegacyRandom {
    seed: u64,
}

impl LegacyRandom {
    #[must_use]
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ MULTIPLIER) & MODULUS_MASK,
        }
    }

    /// The current 48-bit state, exposed so tests can assert draw counts.
    #[must_use]
    pub fn state(&self) -> u64 {
        self.seed
    }
}

impl RandomSource for LegacyRandom {
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT) & MODULUS_MASK;
        (self.seed >> (48 - bits)) as i32
    }

    fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        LegacyPositionalRandomFactory {
            seed: self.next_long(),
        }
    }
}

/// Vanilla `LegacyRandomSource.LegacyPositionalRandomFactory`: seeds a legacy
/// source from `Mth.getSeed(x, y, z)` or from a `String.hashCode`.
#[derive(Debug, Clone, Copy)]
pub struct LegacyPositionalRandomFactory {
    seed: i64,
}

impl LegacyPositionalRandomFactory {
    /// Vanilla `LegacyPositionalRandomFactory.fromHashOf`, i.e.
    /// `Mth.getSeed`-free `String.hashCode` seeding. Used by `PerlinNoise` to
    /// fork each octave from `"octave_<n>"`.
    #[must_use]
    pub fn from_hash_of(&self, name: &str) -> LegacyRandom {
        LegacyRandom::new(i64::from(java_string_hash(name)) ^ self.seed)
    }
}

/// `String.hashCode`: UTF-16 code units, `h = 31h + c` in wrapping `i32`.
#[must_use]
pub fn java_string_hash(value: &str) -> i32 {
    let mut hash = 0i32;
    for unit in value.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    hash
}
