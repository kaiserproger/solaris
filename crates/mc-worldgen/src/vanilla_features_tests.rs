//! Executor tests for the checkpoint A1 vanilla placed-feature layer.
//!
//! These exercise the partial layer end to end with synthetic Solaris fixtures
//! and in-memory levels, plus one loud-skipping live proof against a real local
//! vanilla content cache. Nothing here activates a runtime path.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::{
    BlockColumnLayer, BlockPredicateSpec, BlockStateSpec, ColumnDirection, ConfiguredFeatureKind,
    ConfiguredFeatureSpec, IntProviderSpec, NoiseParametersSpec, NoiseThresholdSpec,
    PlacedFeatureSpec, PlacementModifierSpec, RuleBasedRule, StateProviderSpec, WeightedInt,
    WeightedState,
};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

use crate::vanilla_features::{
    BlockSemantics, BlockTagIndex, CacheTags, CompileError, CompiledPlacedFeature,
    CompiledStateProvider, FeatureLevel, JavaHashSet, LegacyPositionalRandomFactory, LegacyRandom,
    NormalNoise, PlaceError, RandomSource, java_hash, java_string_hash,
};

// ---------------------------------------------------------------- fixtures

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}

/// Resolve a state by name, filling the block's required properties with the
/// defaults used across these tests (`snowy=false`, `distance=7`, ...).
fn state_id(blocks: &BlockRegistry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    let properties: Vec<(String, String)> = properties
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let id = identifier(name);
    let selected: Vec<(String, String)> = properties
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let mut resolved = selected.clone();
    if let Some(block) = blocks.block(&id) {
        for (key, _) in &block.properties {
            if resolved.iter().any(|(name, _)| name == key) {
                continue;
            }
            let value = match key.as_str() {
                "snowy" | "persistent" | "waterlogged" | "powered" | "lit" => "false",
                "distance" => "7",
                "half" => "lower",
                "age" => "0",
                "layers" => "1",
                "axis" => "y",
                "stage" => "0",
                other => panic!("no default for property {other} of {name}"),
            };
            resolved.push((key.to_owned(), value.to_owned()));
        }
    }
    let properties = if properties.is_empty() {
        resolved
    } else {
        selected
    };
    blocks
        .by_name_and_props(&id, &properties)
        .unwrap_or_else(|| panic!("{name} {properties:?} is not a registered state"))
}

/// Synthetic tag index: the ids are vanilla's, the contents are ours.
#[derive(Default)]
struct TestTags {
    tags: BTreeMap<Identifier, BTreeSet<Identifier>>,
}

impl TestTags {
    fn new(tags: &[(&str, &[&str])]) -> Self {
        Self {
            tags: tags
                .iter()
                .map(|(tag, blocks)| {
                    (
                        identifier(tag),
                        blocks.iter().map(|block| identifier(block)).collect(),
                    )
                })
                .collect(),
        }
    }

    fn village_surface() -> Self {
        Self::new(&[
            ("minecraft:air", &["minecraft:air", "minecraft:cave_air"]),
            (
                "minecraft:supports_vegetation",
                &[
                    "minecraft:grass_block",
                    "minecraft:dirt",
                    "minecraft:coarse_dirt",
                    "minecraft:podzol",
                ],
            ),
            (
                "minecraft:supports_cactus",
                &["minecraft:sand", "minecraft:red_sand"],
            ),
        ])
    }
}

impl BlockTagIndex for TestTags {
    fn block_in_tag(&self, tag: &Identifier, block: &Identifier) -> bool {
        self.tags
            .get(tag)
            .is_some_and(|blocks| blocks.contains(block))
    }
}

/// In-memory level: `ground` at and below `ground_top`, air above, plus edits.
struct ProbeLevel {
    min_y: i32,
    air: BlockStateId,
    ground: BlockStateId,
    ground_top: i32,
    edits: HashMap<BlockPos, BlockStateId>,
}

impl ProbeLevel {
    fn new(blocks: &BlockRegistry, ground: &str, ground_top: i32) -> Self {
        Self {
            min_y: -64,
            air: state_id(blocks, "minecraft:air", &[]),
            ground: state_id(blocks, ground, &[]),
            ground_top,
            edits: HashMap::new(),
        }
    }

    /// Placed states as `(position, block)` pairs in a stable order, so tests
    /// can compare exact shapes.
    fn placed(&self, blocks: &BlockRegistry) -> Vec<(BlockPos, String)> {
        let mut placed: Vec<(BlockPos, String)> = self
            .edits
            .iter()
            .map(|(pos, state)| (*pos, blocks.by_id(*state).unwrap().block.id.to_string()))
            .collect();
        placed.sort_by_key(|(pos, _)| (pos.y, pos.z, pos.x));
        placed
    }

    fn placed_blocks(&self, blocks: &BlockRegistry, name: &str) -> Vec<BlockPos> {
        self.placed(blocks)
            .into_iter()
            .filter(|(_, block)| block == name)
            .map(|(pos, _)| pos)
            .collect()
    }
}

impl FeatureLevel for ProbeLevel {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn max_y(&self) -> i32 {
        319
    }

    fn is_water_source_at(&self, _pos: BlockPos) -> bool {
        false
    }

    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        self.edits
            .get(&pos)
            .copied()
            .unwrap_or(if pos.y <= self.ground_top {
                self.ground
            } else {
                self.air
            })
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        self.edits.insert(pos, state);
    }
}

/// Scripted `RandomSource`: each `next(bits)` pops the next value and records
/// the requested width, so a test pins both the value and the draw order.
struct ScriptedRandom {
    draws: VecDeque<i32>,
    bits: Vec<u32>,
}

impl ScriptedRandom {
    fn new(draws: &[i32]) -> Self {
        Self {
            draws: draws.iter().copied().collect(),
            bits: Vec::new(),
        }
    }

    fn remaining(&self) -> usize {
        self.draws.len()
    }
}

impl RandomSource for ScriptedRandom {
    fn next(&mut self, bits: u32) -> i32 {
        self.bits.push(bits);
        self.draws.pop_front().expect("scripted random exhausted")
    }

    fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        panic!("scripted random has no positional factory")
    }
}

/// Legacy source that records the width of every draw.
struct RecordingRandom {
    inner: LegacyRandom,
    bits: Vec<u32>,
}

impl RecordingRandom {
    fn new(seed: i64) -> Self {
        Self {
            inner: LegacyRandom::new(seed),
            bits: Vec::new(),
        }
    }
}

impl RandomSource for RecordingRandom {
    fn next(&mut self, bits: u32) -> i32 {
        self.bits.push(bits);
        self.inner.next(bits)
    }

    fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        self.inner.fork_positional()
    }
}

fn block_state(name: &str, properties: &[(&str, &str)]) -> BlockStateSpec {
    BlockStateSpec {
        block: identifier(name),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn simple(name: &str, properties: &[(&str, &str)]) -> StateProviderSpec {
    StateProviderSpec::Simple {
        state: block_state(name, properties),
    }
}

fn simple_provider(
    blocks: &BlockRegistry,
    tags: &dyn BlockTagIndex,
    spec: &StateProviderSpec,
) -> CompiledStateProvider {
    let semantics = BlockSemantics::new(blocks, tags);
    CompiledStateProvider::compile(&semantics, &identifier("minecraft:fixture"), spec)
        .expect("fixture provider compiles")
}

fn placed(
    configured: &str,
    kind: ConfiguredFeatureKind,
    placement: Vec<PlacementModifierSpec>,
) -> PlacedFeatureSpec {
    PlacedFeatureSpec {
        id: identifier(configured),
        feature: ConfiguredFeatureSpec {
            id: identifier(configured),
            kind,
        },
        placement,
    }
}

fn count(value: i32) -> PlacementModifierSpec {
    PlacementModifierSpec::Count(IntProviderSpec::Constant(value))
}

fn air_filter() -> PlacementModifierSpec {
    PlacementModifierSpec::BlockPredicateFilter(BlockPredicateSpec::MatchingBlockTag {
        offset: (0, 0, 0),
        tag: identifier("minecraft:air"),
    })
}

fn triangle_xz(range: i32) -> IntProviderSpec {
    IntProviderSpec::Trapezoid {
        min_inclusive: -range,
        max_inclusive: range,
        plateau: 0,
    }
}

fn semantics_for<'a>(blocks: &'a BlockRegistry, tags: &'a dyn BlockTagIndex) -> BlockSemantics<'a> {
    BlockSemantics::new(blocks, tags)
}

// ------------------------------------------------------------------ random

/// Reference values from `java.util.Random`, whose 48-bit LCG and bounded-int
/// algorithm are what `LegacyRandomSource`/`BitRandomSource` reimplement.
#[test]
fn legacy_random_matches_java_reference() {
    #[allow(clippy::type_complexity)]
    let cases: [(i64, [i32; 5], [i32; 4], [i32; 4], i64, [bool; 2], u32, u64); 3] = [
        (
            42,
            [-1170105035, 234785527, -1360544799, 205897768, 1325939940],
            [25, 5, 18, 19],
            [3, 3, 4, 0],
            7040072382879853816,
            [true, false],
            1061361751,
            4605226971863729423,
        ),
        (
            -1234567890123456789,
            [-1299346621, -1598166229, 989122942, 145235978, -882839779],
            [53, 25, 4, 92],
            [0, 1, 0, 1],
            6911455454112599179,
            [true, true],
            1063444707,
            4605580189029145344,
        ),
        (
            2345,
            [-284797816, 49532468, -1883052629, -1162614278, 233442744],
            [35, 79, 94, 18],
            [1, 6, 2, 3],
            -3914124124438457868,
            [false, true],
            1061600978,
            4600359552169262562,
        ),
    ];
    for (seed, ints, int100, int7, long, booleans, float, double) in cases {
        let mut random = LegacyRandom::new(seed);
        for expected in ints {
            assert_eq!(random.next_int(), expected, "seed {seed}");
        }
        for expected in int100 {
            assert_eq!(random.next_int_bounded(100), expected, "seed {seed}");
        }
        for expected in int7 {
            assert_eq!(random.next_int_bounded(7), expected, "seed {seed}");
        }
        assert_eq!(random.next_long(), long, "seed {seed}");
        assert_eq!(random.next_boolean(), booleans[0], "seed {seed}");
        assert_eq!(random.next_boolean(), booleans[1], "seed {seed}");
        assert_eq!(random.next_float().to_bits(), float, "seed {seed}");
        assert_eq!(random.next_double().to_bits(), double, "seed {seed}");
    }
}

#[test]
fn java_string_hash_matches_reference() {
    assert_eq!(java_string_hash("octave_0"), 1261148513);
    assert_eq!(java_string_hash("octave_1"), 1261148514);
    assert_eq!(java_string_hash("octave_-2"), 440898196);
}

/// `BitRandomSource.nextInt` rejects a sample whose `sample % bound` would
/// overflow the bound check and redraws; a plain `%` would return early.
#[test]
fn bounded_int_redraws_after_rejection() {
    let mut random = ScriptedRandom::new(&[i32::MAX, 20]);
    assert_eq!(random.next_int_bounded(7), 6);
    assert_eq!(random.remaining(), 0, "both samples must be consumed");
}

/// Reference values produced by running the 26.1.2 `NormalNoise` bytecode
/// (extracted from the bundled server jar, decompiled, run with guava/fastutil
/// on the classpath): `LegacyRandomSource` → `WorldgenRandom` → positional
/// factory → `PerlinNoise` octaves → `NormalNoise.getValue`.
#[test]
fn noise_matches_vanilla_26_1_2_reference() {
    let mut source = LegacyRandom::new(2345);
    let noise = NormalNoise::create(&mut source, 0, &[1.0]);
    let scale = 0.005_f32;
    let expected = [
        (0, 64, 0, -0.458_591_847_491_976_8),
        (1, 64, 0, -0.459_683_397_776_694_6),
        (8, 64, -3, -0.451_027_764_753_275),
        (123, 64, 456, -0.020_609_873_646_637_92),
        (-17, 70, 33, -0.587_701_643_187_750_8),
        (512, 63, -512, 0.058_558_752_722_049_59),
        (7, 64, 7, -0.491_959_811_695_988_85),
        (-1, -64, 1, -0.262_398_748_190_536),
    ];
    for (x, y, z, value) in expected {
        let got = noise.value(
            f64::from(x) * f64::from(scale),
            f64::from(y) * f64::from(scale),
            f64::from(z) * f64::from(scale),
        );
        assert!(
            (got - value).abs() < 1e-15,
            "({x},{y},{z}): got {got}, want {value}"
        );
    }

    let mut source = LegacyRandom::new(987_654_321);
    let noise = NormalNoise::create(&mut source, -2, &[0.5, 1.0, 2.0]);
    let expected = [
        (0.0, 0.003_313_760_474_997_866_4),
        (1.5, 0.000_266_307_286_963_200_1),
        (-33.25, 0.057_854_298_222_347_965),
        (1000.75, -0.413_376_694_738_908_1),
    ];
    for (x, value) in expected {
        let got = noise.value(x, 64.0, -7.0);
        assert!(
            (got - value).abs() < 1e-15,
            "x={x}: got {got}, want {value}"
        );
    }
}

// --------------------------------------------------------------- providers

#[test]
fn simple_state_provider_is_constant_and_draws_nothing() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let provider = simple_provider(&blocks, &tags, &simple("minecraft:melon", &[]));
    let level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = ScriptedRandom::new(&[]);
    let state = provider.state(
        &level,
        &semantics,
        &mut random,
        BlockPos { x: 0, y: 64, z: 0 },
    );
    assert_eq!(
        blocks.by_id(state).unwrap().block.id,
        identifier("minecraft:melon")
    );
    assert!(random.bits.is_empty(), "a simple provider draws nothing");
}

/// `WeightedStateProvider` draws `nextInt(totalWeight)` and walks the
/// cumulative weights, so selections 0..weight-1 map to the first entry.
#[test]
fn weighted_state_provider_selects_by_cumulative_weight() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let provider = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::Weighted {
            entries: vec![
                WeightedState {
                    state: block_state("minecraft:packed_ice", &[]),
                    weight: 1,
                },
                WeightedState {
                    state: block_state("minecraft:blue_ice", &[]),
                    weight: 5,
                },
            ],
        },
    );
    let level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    for (selection, expected) in [
        (0, "minecraft:packed_ice"),
        (1, "minecraft:blue_ice"),
        (5, "minecraft:blue_ice"),
    ] {
        let mut random = ScriptedRandom::new(&[selection]);
        let state = provider.state(
            &level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        );
        assert_eq!(blocks.by_id(state).unwrap().block.id, identifier(expected));
    }
}

#[test]
fn rotated_block_provider_sets_random_axis_and_leaves_axisless_blocks_alone() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let level = ProbeLevel::new(&blocks, "minecraft:sand", 63);

    let hay = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::Rotated {
            block: identifier("minecraft:hay_block"),
        },
    );
    for (axis_index, expected) in ["x", "y", "z"].into_iter().enumerate() {
        let mut random = ScriptedRandom::new(&[axis_index as i32]);
        let state = hay.state(
            &level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        );
        let resolved = blocks.by_id(state).unwrap();
        assert_eq!(resolved.block.id, identifier("minecraft:hay_block"));
        assert!(
            resolved
                .properties
                .iter()
                .any(|(name, value)| name == "axis" && value == expected),
            "expected axis {expected}, got {resolved:?}"
        );
    }

    let melon = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::Rotated {
            block: identifier("minecraft:melon"),
        },
    );
    let mut random = ScriptedRandom::new(&[2]);
    assert_eq!(
        melon.state(
            &level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 }
        ),
        state_id(&blocks, "minecraft:melon", &[]),
        "a block without an axis property keeps its default state"
    );
}

/// `NoiseThresholdProvider`: below the threshold the low list is drawn, above it
/// `nextFloat() < highChance` picks the high list or the default state. The
/// thresholds sit outside the reachable noise range so the branch is pinned
/// without depending on the noise value itself.
#[test]
fn noise_threshold_provider_branches_on_threshold() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    let pos = BlockPos { x: 3, y: 64, z: -5 };

    let provider = |threshold: f32, high_chance: f32| {
        simple_provider(
            &blocks,
            &tags,
            &StateProviderSpec::NoiseThreshold(NoiseThresholdSpec {
                seed: 2345,
                noise: NoiseParametersSpec {
                    first_octave: 0,
                    amplitudes: vec![1.0],
                },
                scale: 0.005,
                threshold,
                high_chance,
                default_state: block_state("minecraft:dandelion", &[]),
                low_states: vec![
                    block_state("minecraft:red_tulip", &[]),
                    block_state("minecraft:orange_tulip", &[]),
                ],
                high_states: vec![
                    block_state("minecraft:poppy", &[]),
                    block_state("minecraft:azure_bluet", &[]),
                ],
            }),
        )
    };

    // `nextInt(2)` is a power-of-two bound: only samples above 2^30 select the
    // second entry.
    const SECOND: i32 = 1 << 30;

    let always_low = provider(2.0, 0.0);
    for (selection, expected) in [
        (0, "minecraft:red_tulip"),
        (SECOND, "minecraft:orange_tulip"),
    ] {
        let mut random = ScriptedRandom::new(&[selection]);
        let state = always_low.state(&level, &semantics, &mut random, pos);
        assert_eq!(blocks.by_id(state).unwrap().block.id, identifier(expected));
    }

    let always_high = provider(-2.0, 1.0);
    let mut random = ScriptedRandom::new(&[0, SECOND]);
    let state = always_high.state(&level, &semantics, &mut random, pos);
    assert_eq!(
        blocks.by_id(state).unwrap().block.id,
        identifier("minecraft:azure_bluet")
    );

    let never_high = provider(-2.0, 0.0);
    let mut random = ScriptedRandom::new(&[0]);
    let state = never_high.state(&level, &semantics, &mut random, pos);
    assert_eq!(
        blocks.by_id(state).unwrap().block.id,
        identifier("minecraft:dandelion")
    );
}

#[test]
fn rule_based_state_provider_takes_first_rule_then_fallback() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let mut level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    let provider = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::RuleBased {
            fallback: Some(Box::new(simple("minecraft:dirt", &[]))),
            rules: vec![RuleBasedRule {
                predicate: BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, -1, 0),
                    tag: identifier("minecraft:supports_vegetation"),
                },
                then: simple("minecraft:coarse_dirt", &[]),
            }],
        },
    );
    let mut random = ScriptedRandom::new(&[]);
    let on_grass = BlockPos { x: 0, y: 64, z: 0 };
    assert_eq!(
        blocks
            .by_id(provider.state(&level, &semantics, &mut random, on_grass))
            .unwrap()
            .block
            .id,
        identifier("minecraft:coarse_dirt"),
        "the first matching rule wins"
    );

    let on_sand = BlockPos { x: 4, y: 64, z: 0 };
    level.set_block(
        BlockPos { x: 4, y: 63, z: 0 },
        state_id(&blocks, "minecraft:sand", &[]),
    );
    assert_eq!(
        blocks
            .by_id(provider.state(&level, &semantics, &mut random, on_sand))
            .unwrap()
            .block
            .id,
        identifier("minecraft:dirt"),
        "no rule matched, so the fallback applies"
    );

    // Without a fallback `getOptionalState` yields nothing and `getState`
    // leaves the world's own state in place.
    let no_fallback = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::RuleBased {
            fallback: None,
            rules: Vec::new(),
        },
    );
    assert!(
        no_fallback
            .optional_state(&level, &semantics, &mut random, on_sand)
            .is_none()
    );
    assert_eq!(
        no_fallback.state(&level, &semantics, &mut random, on_sand),
        level.block_state(on_sand)
    );
}

// --------------------------------------------------------- placement order

/// `count` then `random_offset` then `block_predicate_filter`: the constant
/// count draws nothing, each position draws its `xz` spread twice with the
/// constant `y` spread drawing nothing, and the filter sees the offset
/// position.
#[test]
fn modifiers_place_and_filter_in_list_order() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let mut level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    let spec = placed(
        "minecraft:fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:dandelion", &[]),
            schedule_tick: false,
        },
        vec![
            count(3),
            PlacementModifierSpec::RandomOffset {
                xz_spread: triangle_xz(2),
                y_spread: IntProviderSpec::Constant(0),
            },
            air_filter(),
        ],
    );
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    // Triangle sampling is `nextInt(3) - nextInt(3)` per axis: (4,3) → +1,
    // (0,5) → -2, (1,1) → 0, (2,2) → 0, (0,0) → 0, (2,0) → +2.
    let mut random = ScriptedRandom::new(&[4, 3, 0, 5, 1, 1, 2, 2, 0, 0, 2, 0]);
    assert!(
        compiled
            .place(
                &mut level,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 },
            )
            .unwrap()
    );
    assert_eq!(random.remaining(), 0, "every scripted draw is used");
    assert_eq!(
        level.placed(&blocks),
        vec![
            (
                BlockPos { x: 1, y: 64, z: -2 },
                "minecraft:dandelion".to_owned()
            ),
            (
                BlockPos { x: 0, y: 64, z: 0 },
                "minecraft:dandelion".to_owned()
            ),
            (
                BlockPos { x: 0, y: 64, z: 2 },
                "minecraft:dandelion".to_owned()
            ),
        ]
    );
}

/// Vanilla folds the modifiers with `Stream.flatMap`, so the feature runs (and
/// draws) for the first position before the next modifier draw happens for the
/// second. A collect-then-place pipeline would draw both offsets first.
#[test]
fn feature_placements_interleave_with_later_modifier_draws() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let mut level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let spec = placed(
        "minecraft:fixture",
        ConfiguredFeatureKind::BlockPile {
            state_provider: simple("minecraft:melon", &[]),
        },
        vec![
            count(2),
            PlacementModifierSpec::RandomOffset {
                xz_spread: triangle_xz(2),
                y_spread: IntProviderSpec::Constant(0),
            },
            air_filter(),
        ],
    );
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    let mut random = RecordingRandom::new(12345);
    compiled
        .place(
            &mut level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();

    // Draw widths in vanilla order: offset #1 is `nextInt(3) - nextInt(3)` per
    // horizontal axis, so four `next(31)` draws (indices 0..4); pile #1 then
    // draws its two `nextInt(2)` extents (31, 31 at indices 4..6) and its first
    // two `nextFloat` values (24, 24). A pipeline that collected both offsets
    // before placing would leave index 6 at 31, because offset #2 also draws
    // four times.
    assert_eq!(&random.bits[..8], &[31, 31, 31, 31, 31, 31, 24, 24]);
}

// ---------------------------------------------------------------- features

/// Fixed-seed pile shape. The expected list was produced by a primitive
/// transcription of the 26.1.2 `BlockPileFeature.place` body (same 48-bit LCG,
/// same float arithmetic) over the same stub level: air everywhere, a full-cube
/// floor at y ≤ 63.
#[test]
fn block_pile_shape_on_fixed_seed() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let mut level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let spec = placed(
        "minecraft:pile_hay",
        ConfiguredFeatureKind::BlockPile {
            state_provider: StateProviderSpec::Rotated {
                block: identifier("minecraft:hay_block"),
            },
        },
        Vec::new(),
    );
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();
    let mut random = LegacyRandom::new(12345);
    assert!(
        compiled
            .place(
                &mut level,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 },
            )
            .unwrap()
    );

    let expected = [
        ((-1, 0, -2), "x"),
        ((0, 0, -1), "z"),
        ((2, 0, -1), "x"),
        ((-2, 0, 0), "x"),
        ((-1, 0, 0), "y"),
        ((0, 0, 0), "y"),
        ((1, 0, 0), "x"),
        ((2, 0, 0), "y"),
        ((-1, 0, 1), "z"),
        ((0, 0, 1), "x"),
        ((1, 0, 1), "x"),
        ((0, 0, 2), "x"),
        ((0, 1, -1), "x"),
        ((-1, 1, 0), "x"),
        ((0, 1, 0), "x"),
        ((1, 1, 0), "x"),
        ((-1, 1, 1), "z"),
        ((0, 1, 1), "z"),
    ];
    let placed = level.placed(&blocks);
    assert_eq!(placed.len(), expected.len(), "placed {placed:?}");
    assert!(
        placed
            .iter()
            .all(|(_, block)| block == "minecraft:hay_block")
    );
    for ((dx, dy, dz), axis) in expected {
        let pos = BlockPos {
            x: dx,
            y: 64 + dy,
            z: dz,
        };
        let state = blocks.by_id(level.block_state(pos)).unwrap();
        let got = state
            .properties
            .iter()
            .find(|(name, _)| name == "axis")
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no axis at {pos:?}"));
        assert_eq!(got, axis, "axis at {pos:?}");
    }

    // Determinism: the same seed reproduces the same pile.
    let mut again = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = LegacyRandom::new(12345);
    compiled
        .place(
            &mut again,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert_eq!(again.placed(&blocks), placed);
}

fn cactus_column() -> PlacedFeatureSpec {
    placed(
        "minecraft:cactus",
        ConfiguredFeatureKind::BlockColumn {
            layers: vec![
                BlockColumnLayer {
                    height: IntProviderSpec::BiasedToBottom {
                        min_inclusive: 1,
                        max_inclusive: 3,
                    },
                    provider: simple("minecraft:cactus", &[("age", "0")]),
                },
                BlockColumnLayer {
                    height: IntProviderSpec::WeightedList(vec![
                        WeightedInt {
                            provider: IntProviderSpec::Constant(0),
                            weight: 3,
                        },
                        WeightedInt {
                            provider: IntProviderSpec::Constant(1),
                            weight: 1,
                        },
                    ]),
                    provider: simple("minecraft:cactus_flower", &[]),
                },
            ],
            direction: ColumnDirection::Up,
            allowed_placement: BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: identifier("minecraft:air"),
            },
            prioritize_tip: false,
        },
        Vec::new(),
    )
}

/// Fixed-seed column shape (`biased_to_bottom` base plus a `weighted_list`
/// flower layer) and the `prioritize_tip = false` truncation, whose expected
/// values come from the same primitive transcription of
/// `BlockColumnFeature.place`.
#[test]
fn block_column_shape_and_truncation_on_fixed_seed() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let compiled = CompiledPlacedFeature::compile(&cactus_column(), &semantics).unwrap();

    let mut level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = LegacyRandom::new(48);
    compiled
        .place(
            &mut level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert_eq!(
        level.placed(&blocks),
        vec![
            (
                BlockPos { x: 0, y: 64, z: 0 },
                "minecraft:cactus".to_owned()
            ),
            (
                BlockPos { x: 0, y: 65, z: 0 },
                "minecraft:cactus".to_owned()
            ),
            (
                BlockPos { x: 0, y: 66, z: 0 },
                "minecraft:cactus".to_owned()
            ),
            (
                BlockPos { x: 0, y: 67, z: 0 },
                "minecraft:cactus_flower".to_owned()
            ),
        ]
    );

    // The same seed with the column obstructed at y = 67: vanilla truncates the
    // tip (flower) layer first, so only two cactus blocks remain.
    let mut level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    level.set_block(
        BlockPos { x: 0, y: 67, z: 0 },
        state_id(&blocks, "minecraft:stone", &[]),
    );
    let mut random = LegacyRandom::new(48);
    compiled
        .place(
            &mut level,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert_eq!(
        level.placed(&blocks),
        vec![
            (
                BlockPos { x: 0, y: 64, z: 0 },
                "minecraft:cactus".to_owned()
            ),
            (
                BlockPos { x: 0, y: 65, z: 0 },
                "minecraft:cactus".to_owned()
            ),
            (BlockPos { x: 0, y: 67, z: 0 }, "minecraft:stone".to_owned()),
        ]
    );

    // Nothing under the column (its `allowed_placement` check starts one block
    // above the origin) truncates the whole column away.
    let mut blocked = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    blocked.set_block(
        BlockPos { x: 0, y: 65, z: 0 },
        state_id(&blocks, "minecraft:stone", &[]),
    );
    let mut random = LegacyRandom::new(48);
    assert!(
        compiled
            .place(
                &mut blocked,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 },
            )
            .unwrap(),
        "vanilla still reports a block column even when it truncates every layer"
    );
    assert!(
        blocked
            .placed_blocks(&blocks, "minecraft:cactus")
            .is_empty()
    );
}

#[test]
fn simple_block_places_only_where_it_can_survive() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let spec = placed(
        "minecraft:flower_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:dandelion", &[]),
            schedule_tick: false,
        },
        vec![count(1)],
    );
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    let mut level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    level.set_block(
        BlockPos { x: 1, y: 63, z: 0 },
        state_id(&blocks, "minecraft:stone", &[]),
    );
    let on_grass = BlockPos { x: 0, y: 64, z: 0 };
    let on_stone = BlockPos { x: 1, y: 64, z: 0 };
    let mut random = LegacyRandom::new(7);
    assert!(
        compiled
            .place(&mut level, &semantics, &mut random, on_grass)
            .unwrap()
    );
    let mut random = LegacyRandom::new(7);
    assert!(
        !compiled
            .place(&mut level, &semantics, &mut random, on_stone)
            .unwrap(),
        "stone is not in supports_vegetation"
    );
    assert_eq!(
        level.placed_blocks(&blocks, "minecraft:dandelion"),
        vec![on_grass]
    );

    // Cactus survival needs `supports_cactus` below and no liquid above.
    let cactus = placed(
        "minecraft:cactus_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:cactus", &[("age", "0")]),
            schedule_tick: false,
        },
        vec![count(1), air_filter()],
    );
    let compiled = CompiledPlacedFeature::compile(&cactus, &semantics).unwrap();
    let mut on_sand = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = LegacyRandom::new(11);
    assert!(
        compiled
            .place(
                &mut on_sand,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
    let mut random = LegacyRandom::new(11);
    assert!(
        !compiled
            .place(&mut level, &semantics, &mut random, on_stone)
            .unwrap(),
        "stone cannot support a cactus"
    );
}

#[test]
fn block_pile_requires_a_sturdy_floor_and_height_above_min_y() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let spec = placed(
        "minecraft:pile_snow",
        ConfiguredFeatureKind::BlockPile {
            state_provider: simple("minecraft:snow", &[("layers", "1")]),
        },
        Vec::new(),
    );
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    // Sand is a full cube, so its top face is sturdy and the pile places.
    let mut level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = LegacyRandom::new(3);
    assert!(
        compiled
            .place(
                &mut level,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
    assert!(!level.placed_blocks(&blocks, "minecraft:snow").is_empty());

    // Inside the bottom five blocks of the world vanilla refuses outright.
    let mut low = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let mut random = LegacyRandom::new(3);
    assert!(
        !compiled
            .place(
                &mut low,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: -61, z: 0 }
            )
            .unwrap()
    );
    assert!(low.placed(&blocks).is_empty());

    // On dirt path vanilla flips a coin per candidate instead of asking for a
    // sturdy face, which shows up as `next(1)` draws in the trace.
    let mut path = ProbeLevel::new(&blocks, "minecraft:dirt_path", 63);
    let mut random = RecordingRandom::new(3);
    compiled
        .place(
            &mut path,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert!(
        random.bits.contains(&1),
        "dirt path placement must draw nextBoolean"
    );
    assert!(!path.placed(&blocks).is_empty());

    // A layer of snow is only an eighth of a block tall, so its top face is
    // not sturdy and no pile block may sit on it.
    let mut leafy = ProbeLevel::new(&blocks, "minecraft:snow", 63);
    let mut random = LegacyRandom::new(3);
    compiled
        .place(
            &mut leafy,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert!(leafy.placed(&blocks).is_empty());
}

// -------------------------------------------------------- fail-closed path

#[test]
fn compile_fails_closed_for_unmodelled_block_behaviour() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);

    let double_plant = placed(
        "minecraft:tall_grass_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:tall_grass", &[("half", "lower")]),
            schedule_tick: false,
        },
        vec![count(1)],
    );
    let error = CompiledPlacedFeature::compile(&double_plant, &semantics)
        .expect_err("double plants are not placed by the A1 layer");
    assert!(matches!(
        error,
        CompileError::UnsupportedMultiBlockPlacement { .. }
    ));
    assert!(
        error.to_string().contains("minecraft:tall_grass"),
        "{error}"
    );

    let mushroom = placed(
        "minecraft:mushroom_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:brown_mushroom", &[]),
            schedule_tick: false,
        },
        vec![count(1)],
    );
    let error = CompiledPlacedFeature::compile(&mushroom, &semantics)
        .expect_err("unmodelled survival must fail closed");
    assert!(matches!(
        error,
        CompileError::UnsupportedBlockBehaviour { .. }
    ));
    assert!(
        error.to_string().contains("minecraft:brown_mushroom"),
        "{error}"
    );

    let ticking = placed(
        "minecraft:ticking_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:dandelion", &[]),
            schedule_tick: true,
        },
        Vec::new(),
    );
    assert!(matches!(
        CompiledPlacedFeature::compile(&ticking, &semantics),
        Err(CompileError::UnsupportedScheduleTick { .. })
    ));

    // A `would_survive` predicate on an unmodelled block is refused too.
    let predicate = placed(
        "minecraft:predicate_fixture",
        ConfiguredFeatureKind::BlockColumn {
            layers: vec![BlockColumnLayer {
                height: IntProviderSpec::Constant(1),
                provider: simple("minecraft:cactus", &[("age", "0")]),
            }],
            direction: ColumnDirection::Up,
            allowed_placement: BlockPredicateSpec::WouldSurvive {
                offset: (0, 0, 0),
                state: block_state("minecraft:lily_pad", &[]),
            },
            prioritize_tip: false,
        },
        Vec::new(),
    );
    let error = CompiledPlacedFeature::compile(&predicate, &semantics)
        .expect_err("would_survive on an unmodelled block must fail closed");
    assert!(error.to_string().contains("minecraft:lily_pad"), "{error}");

    // Unknown blocks are data errors, not silent no-ops.
    let unknown = placed(
        "minecraft:unknown_state_fixture",
        ConfiguredFeatureKind::SimpleBlock {
            to_place: simple("minecraft:not_a_block", &[]),
            schedule_tick: false,
        },
        Vec::new(),
    );
    assert!(matches!(
        CompiledPlacedFeature::compile(&unknown, &semantics),
        Err(CompileError::UnknownBlockState { .. })
    ));
}

// -------------------------------------------------------------- live proof

/// `<SOLARIS_CONTENT_CACHE>`, then `<workspace>/data/vanilla`.
fn content_cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
        let dir = PathBuf::from(dir);
        if live_worldgen_dir(&dir).is_some() {
            return Some(dir);
        }
        println!(
            "SKIP live proof: SOLARIS_CONTENT_CACHE={} has no data/minecraft/worldgen",
            dir.display()
        );
        return None;
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let dir = workspace.join("data").join("vanilla");
    live_worldgen_dir(&dir).map(|_| dir)
}

fn live_worldgen_dir(cache: &Path) -> Option<PathBuf> {
    let worldgen = cache.join("data").join("minecraft").join("worldgen");
    worldgen.is_dir().then_some(worldgen)
}

/// Loads the real local vanilla content cache and runs the executor over the
/// whole village decor closure, then repeats the run to prove determinism.
///
/// Loud skip, never a silent pass: without a cache the test prints why it did
/// not run. Point it at a cache with `SOLARIS_CONTENT_CACHE=/path/to/cache`.
#[test]
fn village_decor_closure_places_from_the_real_cache() {
    let Some(cache) = content_cache_dir() else {
        println!(
            "SKIP village_decor_closure_places_from_the_real_cache: \
             no vanilla content cache found (set SOLARIS_CONTENT_CACHE)"
        );
        return;
    };
    let worldgen = live_worldgen_dir(&cache).expect("checked by content_cache_dir");
    let closure = mc_data::vanilla_feature_closure::FeatureClosure::new(&worldgen);

    // Every village decor pool now resolves: the closure's four `minecraft:tree`
    // entries landed with checkpoint A2.
    let mut pool_features: Vec<(String, Vec<String>)> = Vec::new();
    for pool in [
        "desert/decor",
        "plains/decor",
        "savanna/decor",
        "snowy/decor",
        "taiga/decor",
        "plains/trees",
        "savanna/trees",
        "snowy/trees",
    ] {
        let pool_id = identifier(&format!("minecraft:village/{pool}"));
        let entries = closure
            .load_pool_features(&pool_id)
            .unwrap_or_else(|error| panic!("{pool_id}: {error}"));
        let mut names: Vec<String> = entries
            .iter()
            .map(|entry| entry.placed_feature.to_string())
            .collect();
        names.sort();
        println!("live proof: {pool_id} -> {names:?}");
        pool_features.push((pool.to_owned(), names));
    }
    assert_eq!(
        pool_features
            .iter()
            .map(|(_, names)| names.len())
            .sum::<usize>(),
        19,
        "the decor and tree pools hold the 19 reachable feature elements: {pool_features:?}"
    );
    let mut distinct: Vec<String> = pool_features
        .iter()
        .flat_map(|(_, names)| names.iter().cloned())
        .collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 13, "13 distinct features: {distinct:?}");

    // Fail-closed discipline still holds outside the village closure: asking by
    // reference for `patch_berry_common`, which uses a `rarity_filter`
    // modifier, must name the type and the entry.
    let error = closure
        .load_placed_feature(&identifier("minecraft:patch_berry_common"))
        .expect_err("rarity_filter is outside the village closure");
    println!("live proof: patch_berry_common fails closed with: {error}");
    let message = error.to_string();
    assert!(message.contains("minecraft:rarity_filter"), "{message}");
    assert!(
        message.contains("minecraft:patch_berry_common"),
        "{message}"
    );

    let blocks = Arc::new(registry());
    let vanilla = Arc::new(mc_data::load(cache.clone()).expect("cache registry index loads"));
    let tags = Arc::new(mc_data::tags::load(&cache, &vanilla).expect("cache block tags load"));
    let tags = CacheTags::new(Arc::clone(&blocks), vanilla, tags);
    let semantics = BlockSemantics::new(&blocks, &tags);

    // Every one of the 13 reachable features, placed at a fixed seed over the
    // surface its pool expects.
    let origin = BlockPos {
        x: 128,
        y: 64,
        z: -96,
    };
    let tree_origin = BlockPos { x: 0, y: 64, z: 0 };
    let surfaces = [
        (
            "minecraft:oak",
            "minecraft:grass_block",
            tree_origin,
            Some(OAK_TREE),
        ),
        (
            "minecraft:spruce",
            "minecraft:grass_block",
            tree_origin,
            Some(SPRUCE_TREE),
        ),
        (
            "minecraft:pine",
            "minecraft:grass_block",
            tree_origin,
            Some(PINE_TREE),
        ),
        (
            "minecraft:acacia",
            "minecraft:grass_block",
            tree_origin,
            Some(ACACIA_TREE),
        ),
        (
            "minecraft:flower_plain",
            "minecraft:grass_block",
            origin,
            None,
        ),
        (
            "minecraft:patch_taiga_grass",
            "minecraft:grass_block",
            origin,
            None,
        ),
        (
            "minecraft:patch_berry_bush",
            "minecraft:grass_block",
            origin,
            None,
        ),
        ("minecraft:patch_cactus", "minecraft:sand", origin, None),
        ("minecraft:pile_hay", "minecraft:sand", origin, None),
        ("minecraft:pile_ice", "minecraft:snow_block", origin, None),
        (
            "minecraft:pile_melon",
            "minecraft:grass_block",
            origin,
            None,
        ),
        (
            "minecraft:pile_pumpkin",
            "minecraft:grass_block",
            origin,
            None,
        ),
        ("minecraft:pile_snow", "minecraft:snow_block", origin, None),
    ];

    let run = || {
        let mut report: Vec<(String, Vec<(BlockPos, String)>)> = Vec::new();
        for (feature, ground, at, expected) in surfaces {
            let spec = closure
                .load_placed_feature(&identifier(feature))
                .unwrap_or_else(|error| panic!("{feature}: {error}"));
            let compiled = CompiledPlacedFeature::compile(&spec, &semantics)
                .unwrap_or_else(|error| panic!("{feature}: {error}"));
            let mut level = ProbeLevel::new(&blocks, ground, 63);
            let before: HashSet<BlockPos> = level.edits.keys().copied().collect();
            // The tree expectations below are pinned at this seed.
            let mut random = LegacyRandom::new(12345);
            assert!(
                compiled
                    .place(&mut level, &semantics, &mut random, at)
                    .unwrap(),
                "{feature} placed nothing on {ground}"
            );
            let placed: Vec<(BlockPos, String)> = level
                .placed(&blocks)
                .into_iter()
                .filter(|(pos, _)| !before.contains(pos))
                .collect();
            assert!(!placed.is_empty(), "{feature} changed no blocks");
            if let Some(expected) = expected {
                let lines: Vec<String> = {
                    let mut lines: Vec<String> = placed
                        .iter()
                        .map(|(pos, block)| {
                            let state = blocks.by_id(level.block_state(*pos)).unwrap();
                            let mut props: Vec<String> = state
                                .properties
                                .iter()
                                .map(|(key, value)| format!("{key}={value}"))
                                .collect();
                            props.sort();
                            if props.is_empty() {
                                format!("({}, {}, {}) {}", pos.x, pos.y, pos.z, block)
                            } else {
                                format!(
                                    "({}, {}, {}) {}[{}]",
                                    pos.x,
                                    pos.y,
                                    pos.z,
                                    block,
                                    props.join(",")
                                )
                            }
                        })
                        .collect();
                    lines.sort();
                    lines
                };
                assert_eq!(lines, expected.to_vec(), "{feature} block set");
            }
            report.push(((*feature).to_owned(), placed));
        }
        report
    };

    let first = run();
    for (feature, placed) in &first {
        println!("live proof: {feature} placed {} blocks", placed.len());
        for (pos, block) in placed.iter().take(4) {
            println!("  {block} at ({}, {}, {})", pos.x, pos.y, pos.z);
        }
    }
    assert_eq!(
        first.len(),
        13,
        "all 13 reachable features placed something"
    );

    let second = run();
    assert_eq!(first, second, "the same seed must replay identically");
    println!("live proof: deterministic replay matched for all 13 features");
}

// ------------------------------------------------------------------- trees

/// Canonical `(x, y, z) block[props]` lines for the states this placement wrote
/// (not the terrain or anything a test pre-placed), ordered like the vanilla
/// oracle's output so a whole tree shape can be compared at once.
fn block_lines(
    level: &ProbeLevel,
    blocks: &BlockRegistry,
    before: &HashSet<BlockPos>,
) -> Vec<String> {
    let mut lines: Vec<String> = level
        .placed(blocks)
        .into_iter()
        .filter(|(pos, _)| !before.contains(pos))
        .map(|(pos, block)| {
            let state = blocks.by_id(level.block_state(pos)).unwrap();
            let mut props: Vec<String> = state
                .properties
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            props.sort();
            if props.is_empty() {
                format!("({}, {}, {}) {}", pos.x, pos.y, pos.z, block)
            } else {
                format!(
                    "({}, {}, {}) {}[{}]",
                    pos.x,
                    pos.y,
                    pos.z,
                    block,
                    props.join(",")
                )
            }
        })
        .collect();
    lines.sort();
    lines
}

/// Synthetic Solaris-authored tree configurations mirroring the four reachable
/// village pairs. Expected block sets under each are the real 26.1.2
/// `TreeFeature` output for the same configuration, seed and floor (produced by
/// running the bundled decompiled `minecraft:tree` against a stub world).
const OAK_TREE_CONFIG: &str = r#"{
    "type": "minecraft:tree",
    "config": {
        "below_trunk_provider": {
            "type": "minecraft:rule_based_state_provider",
            "rules": [{
                "if_true": {
                    "type": "minecraft:not",
                    "predicate": {
                        "type": "minecraft:matching_block_tag",
                        "tag": "minecraft:cannot_replace_below_tree_trunk"
                    }
                },
                "then": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:dirt" }
                }
            }]
        },
        "decorators": [],
        "foliage_placer": { "type": "minecraft:blob_foliage_placer", "height": 3, "offset": 0, "radius": 2 },
        "foliage_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:oak_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
        },
        "ignore_vines": true,
        "minimum_size": { "type": "minecraft:two_layers_feature_size", "limit": 1, "lower_size": 0, "upper_size": 1 },
        "trunk_placer": { "type": "minecraft:straight_trunk_placer", "base_height": 4, "height_rand_a": 2, "height_rand_b": 0 },
        "trunk_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:oak_log", "Properties": { "axis": "y" } }
        }
    }
}"#;

const SPRUCE_TREE_CONFIG: &str = r#"{
    "type": "minecraft:tree",
    "config": {
        "decorators": [],
        "foliage_placer": {
            "type": "minecraft:spruce_foliage_placer",
            "offset": { "type": "minecraft:uniform", "max_inclusive": 2, "min_inclusive": 0 },
            "radius": { "type": "minecraft:uniform", "max_inclusive": 3, "min_inclusive": 2 },
            "trunk_height": { "type": "minecraft:uniform", "max_inclusive": 2, "min_inclusive": 1 }
        },
        "foliage_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:spruce_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
        },
        "ignore_vines": true,
        "minimum_size": { "type": "minecraft:two_layers_feature_size", "limit": 2, "lower_size": 0, "upper_size": 2 },
        "trunk_placer": { "type": "minecraft:straight_trunk_placer", "base_height": 5, "height_rand_a": 2, "height_rand_b": 1 },
        "trunk_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:spruce_log", "Properties": { "axis": "y" } }
        }
    }
}"#;

const PINE_TREE_CONFIG: &str = r#"{
    "type": "minecraft:tree",
    "config": {
        "decorators": [],
        "foliage_placer": {
            "type": "minecraft:pine_foliage_placer",
            "height": { "type": "minecraft:uniform", "max_inclusive": 4, "min_inclusive": 3 },
            "offset": 1,
            "radius": 1
        },
        "foliage_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:spruce_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
        },
        "ignore_vines": true,
        "minimum_size": { "type": "minecraft:two_layers_feature_size", "limit": 2, "lower_size": 0, "upper_size": 2 },
        "trunk_placer": { "type": "minecraft:straight_trunk_placer", "base_height": 6, "height_rand_a": 4, "height_rand_b": 0 },
        "trunk_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:spruce_log", "Properties": { "axis": "y" } }
        }
    }
}"#;

const ACACIA_TREE_CONFIG: &str = r#"{
    "type": "minecraft:tree",
    "config": {
        "decorators": [],
        "foliage_placer": { "type": "minecraft:acacia_foliage_placer", "offset": 0, "radius": 2 },
        "foliage_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:acacia_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
        },
        "ignore_vines": true,
        "minimum_size": { "type": "minecraft:two_layers_feature_size", "limit": 1, "lower_size": 0, "upper_size": 2 },
        "trunk_placer": { "type": "minecraft:forking_trunk_placer", "base_height": 5, "height_rand_a": 2, "height_rand_b": 2 },
        "trunk_provider": {
            "type": "minecraft:simple_state_provider",
            "state": { "Name": "minecraft:acacia_log", "Properties": { "axis": "y" } }
        }
    }
}"#;

/// `oak`: 59 blocks, vanilla 26.1.2 (see the module docs).
const OAK_TREE: &[&str] = &[
    "(-1, 66, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, -2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 66, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 66, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 67, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 67, -2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 67, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 67, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 67, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 68, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 69, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 66, -1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 66, -2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 66, 0) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 66, 1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 67, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 67, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 67, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 67, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(0, 63, 0) minecraft:dirt",
    "(0, 64, 0) minecraft:oak_log[axis=y]",
    "(0, 65, 0) minecraft:oak_log[axis=y]",
    "(0, 66, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, -2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 66, 0) minecraft:oak_log[axis=y]",
    "(0, 66, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, 2) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, -2) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, 0) minecraft:oak_log[axis=y]",
    "(0, 67, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, 2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 68, -1) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 68, 0) minecraft:oak_log[axis=y]",
    "(0, 68, 1) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 69, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 69, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 69, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 66, -2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 66, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, 2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, -2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 67, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 68, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(1, 69, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 66, -1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 66, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 66, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 66, 2) minecraft:oak_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 67, -1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 67, 0) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 67, 1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 67, 2) minecraft:oak_leaves[distance=5,persistent=false,waterlogged=false]",
];

/// `spruce`: 62 blocks, vanilla 26.1.2 (see the module docs).
const SPRUCE_TREE: &[&str] = &[
    "(-1, 66, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 66, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 66, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 67, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 68, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 68, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 68, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 68, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 68, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 69, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 71, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 66, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 66, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 66, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 68, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 68, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 68, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 63, 0) minecraft:dirt",
    "(0, 64, 0) minecraft:spruce_log[axis=y]",
    "(0, 65, 0) minecraft:spruce_log[axis=y]",
    "(0, 66, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, -2) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, 0) minecraft:spruce_log[axis=y]",
    "(0, 66, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, 2) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, 0) minecraft:spruce_log[axis=y]",
    "(0, 67, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, -2) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, 0) minecraft:spruce_log[axis=y]",
    "(0, 68, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 69, -1) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 69, 0) minecraft:spruce_log[axis=y]",
    "(0, 69, 1) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 70, 0) minecraft:spruce_log[axis=y]",
    "(0, 71, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 71, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 71, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 72, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 66, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 66, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 68, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 68, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 68, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 68, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 68, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 69, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(1, 71, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 66, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 66, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 66, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 68, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 68, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 68, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
];

/// `pine`: 98 blocks, vanilla 26.1.2 (see the module docs).
const PINE_TREE: &[&str] = &[
    "(-1, 68, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 68, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 68, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 68, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 68, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 69, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 69, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 69, -3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-1, 69, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 69, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 69, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 69, 3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-1, 70, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 70, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 70, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 70, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 70, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 71, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 68, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 68, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 68, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 69, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 69, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 69, -3) minecraft:spruce_leaves[distance=6,persistent=false,waterlogged=false]",
    "(-2, 69, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 69, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 69, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 69, 3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-2, 70, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-2, 70, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 70, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-3, 69, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-3, 69, -2) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-3, 69, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-3, 69, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-3, 69, 2) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(0, 63, 0) minecraft:dirt",
    "(0, 64, 0) minecraft:spruce_log[axis=y]",
    "(0, 65, 0) minecraft:spruce_log[axis=y]",
    "(0, 66, 0) minecraft:spruce_log[axis=y]",
    "(0, 67, 0) minecraft:spruce_log[axis=y]",
    "(0, 68, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 68, 0) minecraft:spruce_log[axis=y]",
    "(0, 68, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 68, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 69, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 69, -2) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 69, -3) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 69, 0) minecraft:spruce_log[axis=y]",
    "(0, 69, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 69, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 69, 3) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 70, -1) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 70, -2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 70, 0) minecraft:spruce_log[axis=y]",
    "(0, 70, 1) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 70, 2) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 71, -1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 71, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 71, 1) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 72, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 68, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 68, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 68, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 68, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 68, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 69, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 69, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 69, -3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(1, 69, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 69, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 69, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 69, 3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(1, 70, -1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 70, -2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 70, 0) minecraft:spruce_leaves[distance=1,persistent=false,waterlogged=false]",
    "(1, 70, 1) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 70, 2) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 71, 0) minecraft:spruce_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 68, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 68, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 68, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 69, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 69, -2) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 69, -3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 69, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 69, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 69, 2) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 69, 3) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 70, -1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 70, 0) minecraft:spruce_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 70, 1) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(3, 69, -1) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(3, 69, -2) minecraft:spruce_leaves[distance=6,persistent=false,waterlogged=false]",
    "(3, 69, 0) minecraft:spruce_leaves[distance=4,persistent=false,waterlogged=false]",
    "(3, 69, 1) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
    "(3, 69, 2) minecraft:spruce_leaves[distance=5,persistent=false,waterlogged=false]",
];

/// `acacia`: 93 blocks, vanilla 26.1.2 (see the module docs).
const ACACIA_TREE: &[&str] = &[
    "(-1, 66, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 66, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, -3) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 66, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 66, 1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 67, -1) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 67, -2) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 67, 0) minecraft:acacia_log[axis=y]",
    "(-1, 70, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 70, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 70, -3) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 70, 0) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 70, 1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 70, 2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 70, 3) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-1, 71, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 71, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 71, 1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 66, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 66, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 66, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 68, 0) minecraft:acacia_log[axis=y]",
    "(-2, 69, 0) minecraft:acacia_log[axis=y]",
    "(-2, 70, -1) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-2, 70, -2) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 70, -3) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 70, 0) minecraft:acacia_log[axis=y]",
    "(-2, 70, 1) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-2, 70, 2) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 70, 3) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 71, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 71, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 71, 0) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-2, 71, 1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 71, 2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-3, 70, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-3, 70, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-3, 70, -3) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-3, 70, 0) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-3, 70, 1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-3, 70, 2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-3, 70, 3) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-3, 71, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-3, 71, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-3, 71, 1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-4, 70, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-4, 70, -2) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-4, 70, -3) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-4, 70, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-4, 70, 1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-4, 70, 2) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-4, 70, 3) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-4, 71, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-5, 70, -1) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-5, 70, -2) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(-5, 70, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-5, 70, 1) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(-5, 70, 2) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(0, 63, 0) minecraft:dirt",
    "(0, 64, 0) minecraft:acacia_log[axis=y]",
    "(0, 65, 0) minecraft:acacia_log[axis=y]",
    "(0, 66, -1) minecraft:acacia_log[axis=y]",
    "(0, 66, -2) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 66, -3) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, 0) minecraft:acacia_log[axis=y]",
    "(0, 66, 1) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 67, -1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 67, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 67, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 70, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 70, -2) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(0, 70, -3) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(0, 70, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 70, 1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 70, 2) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(0, 70, 3) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(0, 71, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 66, -1) minecraft:acacia_leaves[distance=1,persistent=false,waterlogged=false]",
    "(1, 66, -2) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, -3) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 66, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 66, 1) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 67, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 67, -2) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 67, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 70, -1) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 70, -2) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(1, 70, 0) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 70, 1) minecraft:acacia_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 70, 2) minecraft:acacia_leaves[distance=5,persistent=false,waterlogged=false]",
    "(2, 66, -1) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 66, -2) minecraft:acacia_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 66, 0) minecraft:acacia_leaves[distance=2,persistent=false,waterlogged=false]",
];

/// `oak` clipped by a block at (0, 68, 0) with `min_clipped_height = 2`: 34 blocks.
const OAK_TREE_CLIPPED: &[&str] = &[
    "(-1, 64, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 64, -2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 64, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 64, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-1, 64, 2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-1, 65, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(-1, 66, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 64, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 64, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(-2, 64, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(-2, 64, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(0, 63, 0) minecraft:dirt",
    "(0, 64, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 64, -2) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 64, 0) minecraft:oak_log[axis=y]",
    "(0, 64, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 64, 2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(0, 65, -1) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 65, 0) minecraft:oak_log[axis=y]",
    "(0, 65, 1) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 66, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(0, 66, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(0, 66, 1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 64, -1) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 64, -2) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 64, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(1, 64, 1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(1, 64, 2) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(1, 65, 0) minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]",
    "(1, 66, 0) minecraft:oak_leaves[distance=2,persistent=false,waterlogged=false]",
    "(2, 64, -1) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 64, 0) minecraft:oak_leaves[distance=3,persistent=false,waterlogged=false]",
    "(2, 64, 1) minecraft:oak_leaves[distance=4,persistent=false,waterlogged=false]",
    "(2, 64, 2) minecraft:oak_leaves[distance=5,persistent=false,waterlogged=false]",
];

/// The clipped case: the same oak config, but with `min_clipped_height = 2`, so a
/// block at (0, 68, 0) shortens the trunk instead of refusing the tree. Expected
/// block set from the real 26.1.2 `TreeFeature` under the same conditions.
#[test]
fn clipped_tree_shape_matches_vanilla_reference() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let dir = tree_worldgen(&OAK_TREE_CONFIG.replace(
        "\"upper_size\": 1 }",
        "\"upper_size\": 1, \"min_clipped_height\": 2 }",
    ));
    let spec = mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
        .load_placed_feature(&identifier("minecraft:tree_fixture"))
        .unwrap();
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    let mut level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    level.set_block(
        BlockPos { x: 0, y: 68, z: 0 },
        state_id(&blocks, "minecraft:stone", &[]),
    );
    let before: HashSet<BlockPos> = level.edits.keys().copied().collect();
    let mut random = LegacyRandom::new(12345);
    assert!(
        compiled
            .place(
                &mut level,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
    assert_eq!(block_lines(&level, &blocks, &before), OAK_TREE_CLIPPED);
}

fn tree_worldgen(config: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    for sub in ["placed_feature", "configured_feature"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    std::fs::write(
        dir.path().join("configured_feature/tree_fixture.json"),
        config,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("placed_feature/tree_fixture.json"),
        r#"{ "feature": "minecraft:tree_fixture", "placement": [] }"#,
    )
    .unwrap();
    dir
}

/// The four reachable tree pairs, placed at a fixed seed, against the block sets
/// the real 26.1.2 `TreeFeature` produced for the same configurations —
/// positions, block states and the leaf `distance` pass included.
#[test]
fn tree_shapes_match_vanilla_reference() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    for (name, config, expected) in [
        ("oak", OAK_TREE_CONFIG, OAK_TREE),
        ("spruce", SPRUCE_TREE_CONFIG, SPRUCE_TREE),
        ("pine", PINE_TREE_CONFIG, PINE_TREE),
        ("acacia", ACACIA_TREE_CONFIG, ACACIA_TREE),
    ] {
        let dir = tree_worldgen(config);
        let spec = mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
            .load_placed_feature(&identifier("minecraft:tree_fixture"))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();
        let mut level = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
        let before: HashSet<BlockPos> = level.edits.keys().copied().collect();
        let mut random = LegacyRandom::new(12345);
        assert!(
            compiled
                .place(
                    &mut level,
                    &semantics,
                    &mut random,
                    BlockPos { x: 0, y: 64, z: 0 }
                )
                .unwrap(),
            "{name} placed nothing"
        );
        assert_eq!(block_lines(&level, &blocks, &before), expected, "{name}");

        // Same seed, same tree.
        let mut again = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
        let mut random = LegacyRandom::new(12345);
        compiled
            .place(
                &mut again,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 },
            )
            .unwrap();
        assert_eq!(again.placed(&blocks), level.placed(&blocks), "{name}");
    }
}

/// Tree placement consults the terrain. With the reachable configs (no
/// `min_clipped_height`) an obstruction that lowers `getMaxFreeTreeHeight` below
/// the rolled height refuses the whole tree — the real 26.1.2 feature returns
/// `false` and writes nothing there, which the stub-level oracle confirms for
/// this exact seed and obstruction.
#[test]
fn tree_refuses_against_a_blocking_obstruction() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let dir = tree_worldgen(OAK_TREE_CONFIG);
    let spec = mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
        .load_placed_feature(&identifier("minecraft:tree_fixture"))
        .unwrap();
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    let mut blocked = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    blocked.set_block(
        BlockPos { x: 0, y: 66, z: 0 },
        state_id(&blocks, "minecraft:stone", &[]),
    );
    let before: HashSet<BlockPos> = blocked.edits.keys().copied().collect();
    let mut random = LegacyRandom::new(12345);
    assert!(
        !compiled
            .place(
                &mut blocked,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
    assert_eq!(
        block_lines(&blocked, &blocks, &before),
        Vec::<String>::new(),
        "the obstructed tree writes nothing at all"
    );

    // The same seed without the obstruction is the pinned full tree.
    let mut clear = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    let clear_before: HashSet<BlockPos> = clear.edits.keys().copied().collect();
    let mut random = LegacyRandom::new(12345);
    assert!(
        compiled
            .place(
                &mut clear,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
    assert_eq!(block_lines(&clear, &blocks, &clear_before), OAK_TREE);
}

#[test]
fn tree_refuses_when_the_column_is_occupied_or_out_of_bounds() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let dir = tree_worldgen(OAK_TREE_CONFIG);
    let spec = mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
        .load_placed_feature(&identifier("minecraft:tree_fixture"))
        .unwrap();
    let compiled = CompiledPlacedFeature::compile(&spec, &semantics).unwrap();

    // A column of leaves leaves no room for the trunk.
    let mut leafy = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    for y in 64..=70 {
        leafy.set_block(
            BlockPos { x: 0, y, z: 0 },
            state_id(&blocks, "minecraft:oak_leaves", &[]),
        );
    }
    let before = leafy.placed(&blocks);
    let mut random = LegacyRandom::new(12345);
    compiled
        .place(
            &mut leafy,
            &semantics,
            &mut random,
            BlockPos { x: 0, y: 64, z: 0 },
        )
        .unwrap();
    assert_eq!(
        leafy.placed(&blocks),
        before,
        "a full column of leaves leaves no room for the trunk"
    );

    // Below the world floor vanilla refuses outright.
    let mut low = ProbeLevel::new(&blocks, "minecraft:grass_block", 63);
    low.min_y = 64;
    let mut random = LegacyRandom::new(12345);
    assert!(
        !compiled
            .place(
                &mut low,
                &semantics,
                &mut random,
                BlockPos { x: 0, y: 64, z: 0 }
            )
            .unwrap()
    );
}

#[test]
fn tree_compile_fails_closed_for_unmodelled_parts() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let with_config = |config: &str| {
        let dir = tree_worldgen(config);
        mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
            .load_placed_feature(&identifier("minecraft:tree_fixture"))
    };

    let error =
        with_config(&OAK_TREE_CONFIG.replace("straight_trunk_placer", "fancy_trunk_placer"))
            .expect_err("fancy trunks are not implemented");
    let message = error.to_string();
    assert!(
        message.contains("minecraft:fancy_trunk_placer"),
        "{message}"
    );
    assert!(message.contains("minecraft:tree_fixture"), "{message}");

    let error = with_config(
        &OAK_TREE_CONFIG.replace("blob_foliage_placer", "random_spread_foliage_placer"),
    )
    .expect_err("random spread foliage is not implemented");
    let message = error.to_string();
    assert!(
        message.contains("minecraft:random_spread_foliage_placer"),
        "{message}"
    );
    assert!(message.contains("minecraft:tree_fixture"), "{message}");

    let error = with_config(&OAK_TREE_CONFIG.replace(
        "\"decorators\": []",
        "\"decorators\": [{ \"type\": \"minecraft:trunk_vine\" }]",
    ))
    .expect_err("tree decorators are not implemented");
    assert!(
        error.to_string().contains("minecraft:trunk_vine"),
        "{error}"
    );

    let error = with_config(
        &OAK_TREE_CONFIG.replace("two_layers_feature_size", "three_layers_feature_size"),
    )
    .expect_err("three layer feature sizes are not implemented");
    let message = error.to_string();
    assert!(
        message.contains("minecraft:three_layers_feature_size"),
        "{message}"
    );
    assert!(message.contains("minecraft:tree_fixture"), "{message}");

    let error = with_config(&OAK_TREE_CONFIG.replace(
        "\"decorators\": []",
        "\"root_placer\": { \"type\": \"minecraft:mangrove_root_placer\" }, \"decorators\": []",
    ))
    .expect_err("root placers are not implemented");
    let message = error.to_string();
    assert!(
        message.contains("minecraft:mangrove_root_placer"),
        "{message}"
    );
    assert!(message.contains("minecraft:tree_fixture"), "{message}");

    // A foliage provider whose states are not leaves would make vanilla's
    // distance pass throw (`setValue(DISTANCE, ...)` on a state without the
    // property), so it is refused at compile time.
    let dir = tree_worldgen(&OAK_TREE_CONFIG.replace(
        "\"Name\": \"minecraft:oak_leaves\"",
        "\"Name\": \"minecraft:melon\"",
    ));
    let spec = mc_data::vanilla_feature_closure::FeatureClosure::new(dir.path())
        .load_placed_feature(&identifier("minecraft:tree_fixture"))
        .unwrap();
    let error = CompiledPlacedFeature::compile(&spec, &semantics)
        .expect_err("a non-leaf foliage provider is a data error");
    assert!(matches!(error, CompileError::MissingStateProperty { .. }));
}

/// The `java.util.HashMap` emulation must stop loudly at the treeify boundary
/// rather than keep an order vanilla does not have. `Vec3i.hashCode` is
/// `31y + 961z + x`, so `(x=10000-31a, y=a, z=0)` all collide on one hash.
#[test]
fn java_hash_set_fails_loudly_at_the_treeify_boundary() {
    /// `HashMap.TREEIFY_THRESHOLD`.
    const TREEIFY_THRESHOLD: usize = 8;

    let colliding = |a: i32| BlockPos {
        x: 10_000 - 31 * a,
        y: a,
        z: 0,
    };
    assert_eq!(java_hash(colliding(0)), java_hash(colliding(9)));

    // 30 entries push the table past 24 entries, i.e. to 64 buckets, where a
    // bin of 8 is treeified by real `HashMap`. Filler positions may share the
    // colliding bin, so the assertion is the boundary condition the error
    // reports, not a fixed insert index.
    let mut set = JavaHashSet::new();
    for index in 0..30 {
        set.add(BlockPos {
            x: index,
            y: 70,
            z: 5,
        })
        .expect("a bin of 8 below 64 buckets only resizes");
    }

    let mut boundary = None;
    for a in 0..12 {
        if let Err(error) = set.add(colliding(a)) {
            boundary = Some(error);
            break;
        }
    }
    let error = boundary.expect("colliding entries must cross the treeify boundary");
    let PlaceError::TreeOrderBoundary {
        bin_size,
        capacity,
        pos,
    } = error;
    assert!(bin_size >= TREEIFY_THRESHOLD, "bin_size {bin_size}");
    assert!(capacity >= 64, "capacity {capacity}");
    assert!(
        (0..12).any(|a| colliding(a) == pos),
        "the reported block is the one that crossed: {pos:?}"
    );
}

/// Nested rule-based providers resolve to the existing block state, as vanilla's
/// `getOptionalState` does, instead of reporting "nothing to place".
#[test]
fn nested_rule_based_provider_falls_back_to_the_existing_state() {
    let blocks = registry();
    let tags = TestTags::village_surface();
    let semantics = semantics_for(&blocks, &tags);
    let level = ProbeLevel::new(&blocks, "minecraft:sand", 63);
    let pos = BlockPos { x: 0, y: 64, z: 0 };

    let inner = StateProviderSpec::RuleBased {
        fallback: None,
        rules: Vec::new(),
    };
    let outer = StateProviderSpec::RuleBased {
        rules: vec![RuleBasedRule {
            predicate: BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: identifier("minecraft:air"),
            },
            then: inner,
        }],
        fallback: None,
    };
    let provider = simple_provider(&blocks, &tags, &outer);
    let mut random = ScriptedRandom::new(&[]);
    assert_eq!(
        provider.optional_state(&level, &semantics, &mut random, pos),
        Some(level.block_state(pos)),
        "the outer rule matches, so the inner no-match result is the world's state"
    );

    // The outermost provider with no matching rule and no fallback still
    // reports nothing to place.
    let unmatched = simple_provider(
        &blocks,
        &tags,
        &StateProviderSpec::RuleBased {
            rules: Vec::new(),
            fallback: None,
        },
    );
    assert!(
        unmatched
            .optional_state(&level, &semantics, &mut random, pos)
            .is_none()
    );
}
