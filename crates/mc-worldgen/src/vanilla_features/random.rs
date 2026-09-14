//! Vanilla random sources, reduced to what the executor and village placement
//! consume.
//!
//! `minecraft`'s `LegacyRandomSource` is a 48-bit LCG with `java.util.Random`'s
//! bit layout, and every draw here is written to reproduce it exactly: same
//! modulus, same multipliers, same `next(bits)` shifts, same bounded-integer
//! rejection loop, same float/double composition, same per-position seeding.
//!
//! Three sources live here, and which one a lane uses is a fidelity question,
//! not a preference: [`LegacyRandom`] for the legacy draws (noise seeding,
//! `setLargeFeatureSeed`), [`XoroshiroRandom`] where vanilla uses a raw
//! `XoroshiroRandomSource` (the router's positional forks), and
//! [`WorldgenRandom`] where vanilla wraps one (the `FEATURES` decoration step).

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
///
/// `XoroshiroRandomSource` overrides several of these defaults
/// (`nextInt`, `nextInt(int)`, `nextLong`, `nextBoolean`, `nextDouble`,
/// `consumeCount`), so [`XoroshiroRandom`] implements them rather than
/// inheriting them.
pub trait RandomSource {
    /// Vanilla `RandomSource.next(bits)`.
    fn next(&mut self, bits: u32) -> i32;

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
}

/// A source whose vanilla counterpart can be forked positionally.
///
/// Only the legacy source is forked in this layer: every `NormalNoise` the
/// reachable closure builds comes from `NoiseBasedStateProvider`'s
/// `WorldgenRandom(new LegacyRandomSource(seed))`, whose `forkPositional`
/// delegates to that legacy source. The village decor lane runs on
/// [`WorldgenRandom`] over an [`XoroshiroRandom`], and neither it nor a raw
/// [`XoroshiroRandom`] ever reaches a `NormalNoise` here — so `XoroshiroRandom`
/// does not claim this capability rather than answer it with a legacy factory
/// and silently seed a different noise.
pub trait PositionalRandomSource: RandomSource {
    /// Vanilla `LegacyRandomSource.forkPositional()`. Draws one `nextLong`.
    fn fork_positional(&mut self) -> LegacyPositionalRandomFactory;
}

impl PositionalRandomSource for LegacyRandom {
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

/// `RandomSupport.GOLDEN_RATIO_64`.
const GOLDEN_RATIO_64: i64 = -7_046_029_254_386_353_131;
/// `RandomSupport.SILVER_RATIO_64`.
const SILVER_RATIO_64: i64 = 7_640_891_576_956_012_809;

/// `RandomSupport.mixStafford13`.
fn mix_stafford13(mut value: i64) -> i64 {
    value = (value ^ ((value as u64 >> 30) as i64)).wrapping_mul(-4_658_895_280_553_007_687);
    value = (value ^ ((value as u64 >> 27) as i64)).wrapping_mul(-7_723_592_293_110_705_685);
    value ^ ((value as u64 >> 31) as i64)
}

/// Vanilla `Xoroshiro128PlusPlus` (`XoroshiroRandomSource`'s generator).
#[derive(Debug, Clone)]
struct Xoroshiro128PlusPlus {
    seed_lo: u64,
    seed_hi: u64,
}

impl Xoroshiro128PlusPlus {
    /// `Xoroshiro128PlusPlus(RandomSupport.upgradeSeedTo128bit(seed))`: the two
    /// halves are `seed ^ SILVER_RATIO_64` and `that + GOLDEN_RATIO_64`, each
    /// run through `mixStafford13`.
    fn from_seed(seed: i64) -> Self {
        let lo = seed ^ SILVER_RATIO_64;
        let hi = lo.wrapping_add(GOLDEN_RATIO_64);
        let mut generator = Self {
            seed_lo: mix_stafford13(lo) as u64,
            seed_hi: mix_stafford13(hi) as u64,
        };
        // The constructor's zero guard: an all-zero state is not a valid
        // xorshift state, so vanilla substitutes the two ratio constants.
        if generator.seed_lo | generator.seed_hi == 0 {
            generator.seed_lo = GOLDEN_RATIO_64 as u64;
            generator.seed_hi = SILVER_RATIO_64 as u64;
        }
        generator
    }

    /// Vanilla `Xoroshiro128PlusPlus.nextLong`.
    fn next_long(&mut self) -> i64 {
        let s0 = self.seed_lo;
        let s1 = self.seed_hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        let s1 = s1 ^ s0;
        self.seed_lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.seed_hi = s1.rotate_left(28);
        result as i64
    }
}

/// Vanilla `XoroshiroRandomSource`, the random vanilla runs the `FEATURES`
/// decoration step on (`ChunkGenerator.applyBiomeDecoration`).
///
/// Every method vanilla overrides is overridden here: `nextInt`,
/// `nextInt(bound)`, `nextLong`, `nextBoolean`, `nextDouble` and `consumeCount`
/// are *not* `BitRandomSource`'s legacy compositions, so inheriting the
/// trait's defaults would draw a different stream.
#[derive(Debug, Clone)]
pub struct XoroshiroRandom {
    generator: Xoroshiro128PlusPlus,
}

impl XoroshiroRandom {
    /// `new XoroshiroRandomSource(seed)`.
    #[must_use]
    pub fn new(seed: i64) -> Self {
        Self {
            generator: Xoroshiro128PlusPlus::from_seed(seed),
        }
    }

    /// `XoroshiroRandomSource.nextBits(bits)`.
    fn next_bits(&mut self, bits: u32) -> i64 {
        (self.generator.next_long() as u64 >> (64 - bits)) as i64
    }
}

impl RandomSource for XoroshiroRandom {
    fn next(&mut self, bits: u32) -> i32 {
        self.next_bits(bits) as i32
    }

    fn next_int(&mut self) -> i32 {
        self.generator.next_long() as i32
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "random bound must be positive");
        // `XoroshiroRandomSource.nextInt(int)`: a 32x32 multiply whose high half
        // is the result, rejecting the biased low fraction.
        let mut random_bits = i64::from(self.next_int() as u32);
        let mut multiplied = random_bits.wrapping_mul(i64::from(bound));
        let mut fractional = multiplied & 0xFFFF_FFFF;
        if fractional < i64::from(bound) {
            let threshold = i64::from((bound as u32).wrapping_neg() % (bound as u32));
            while fractional < threshold {
                random_bits = i64::from(self.next_int() as u32);
                multiplied = random_bits.wrapping_mul(i64::from(bound));
                fractional = multiplied & 0xFFFF_FFFF;
            }
        }
        (multiplied >> 32) as i32
    }

    fn next_long(&mut self) -> i64 {
        self.generator.next_long()
    }

    fn next_boolean(&mut self) -> bool {
        self.generator.next_long() & 1 != 0
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_MULTIPLIER
    }

    fn next_double(&mut self) -> f64 {
        self.next_bits(53) as f64 * DOUBLE_MULTIPLIER
    }

    fn consume_count(&mut self, rounds: u32) {
        for _ in 0..rounds {
            self.generator.next_long();
        }
    }
}

/// Vanilla `WorldgenRandom`: a [`RandomSource`] whose `next(bits)` comes from a
/// wrapped *bit* source while every composed draw keeps `BitRandomSource`'s
/// legacy composition.
///
/// The distinction is the whole reason the wrapper exists. Vanilla's
/// `WorldgenRandom extends LegacyRandomSource`, so `nextInt`, `nextInt(bound)`,
/// `nextLong`, `nextBoolean`, `nextFloat`, `nextDouble` and `consumeCount` are
/// the legacy formulas *over the wrapped bits* — not the wrapped source's own
/// overrides. `ChunkGenerator.applyBiomeDecoration`, which places a village's
/// structures and their decor, runs on
/// `new WorldgenRandom(new XoroshiroRandomSource(...))`, so a raw
/// [`XoroshiroRandom`] draws a different stream from the same seed: seeded at
/// 4242, the wrapper's first `nextLong()` after `setFeatureSeed` is
/// `-5071117971071978252` and the raw source's is `-8923083901228911911`.
///
/// Only the wrapper's methods are implemented, so the trait's legacy defaults
/// are what every composed draw uses.
#[derive(Debug, Clone)]
pub struct WorldgenRandom {
    bits: XoroshiroRandom,
}

impl WorldgenRandom {
    /// `new WorldgenRandom(new XoroshiroRandomSource(seed))`.
    #[must_use]
    pub fn over_xoroshiro(seed: i64) -> Self {
        Self {
            bits: XoroshiroRandom::new(seed),
        }
    }

    /// Vanilla `WorldgenRandom.setSeed`, which re-seeds the wrapped generator.
    pub fn set_seed(&mut self, seed: i64) {
        self.bits = XoroshiroRandom::new(seed);
    }

    /// Vanilla `WorldgenRandom.setDecorationSeed`: seed from `seed`, draw the
    /// two odd scales the chunk coordinates are multiplied by, re-seed with the
    /// mixed value and return it. `ChunkGenerator.applyBiomeDecoration` calls
    /// this once per chunk with the chunk's minimum block coordinates.
    pub fn set_decoration_seed(&mut self, seed: i64, chunk_x: i32, chunk_z: i32) -> i64 {
        self.set_seed(seed);
        let x_scale = self.next_long() | 1;
        let z_scale = self.next_long() | 1;
        let decoration_seed = i64::from(chunk_x)
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(chunk_z).wrapping_mul(z_scale))
            ^ seed;
        self.set_seed(decoration_seed);
        decoration_seed
    }

    /// Vanilla `WorldgenRandom.setFeatureSeed`: one structure of a decoration
    /// step gets `decorationSeed + index + 10000 * step`.
    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, step: i32) {
        self.set_seed(
            decoration_seed
                .wrapping_add(i64::from(index))
                .wrapping_add(i64::from(step).wrapping_mul(10_000)),
        );
    }
}

impl RandomSource for WorldgenRandom {
    fn next(&mut self, bits: u32) -> i32 {
        // Vanilla's non-legacy branch: `(int)(randomSource.nextLong() >>> 64 - bits)`.
        self.bits.next(bits)
    }
}
