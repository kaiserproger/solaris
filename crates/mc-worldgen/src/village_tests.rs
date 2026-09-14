//! Engine tests for checkpoint B's placement and terrain-adaptation layers.
//!
//! The jigsaw solver itself lives in `village_solver_tests.rs`; these are the
//! other pieces of the village engine: `minecraft:random_spread` placement, the
//! worldgen random's seeding functions, and the column-height beard analogue.

use std::path::{Path, PathBuf};

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry};

use crate::structures::{BlockFace, Joint, orientation_faces};
use crate::vanilla_features::{LegacyRandom, RandomSource, WorldgenRandom, XoroshiroRandom};
use crate::village::beard::{
    BEARD_KERNEL_RADIUS, BeardContribution, BeardJunction, affected_box, apply_columns,
    beard_contribution, kernel_value, vanilla_contribution,
};
use crate::village::placement::{
    RandomSpreadPlacement, set_large_feature_seed, set_large_feature_with_salt, start_origin,
};

/// `Mth.fastInvSqrt`, restated in the test so the expectation above is derived
/// rather than copied from the implementation.
fn fast_inv_sqrt_ref(value: f64) -> f64 {
    let x = value as f32;
    let i = 0x5f37_59df - (x.to_bits() >> 1);
    let mut y = f32::from_bits(i);
    y *= 1.5 - (x * 0.5) * y * y;
    f64::from(y)
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

/// `<SOLARIS_CONTENT_CACHE>`, then `<workspace>/data/vanilla`.
fn content_cache() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
        let dir = PathBuf::from(dir);
        return dir
            .join("data")
            .join("minecraft")
            .join("worldgen")
            .is_dir()
            .then_some(dir);
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let dir = workspace.join("data").join("vanilla");
    dir.join("data")
        .join("minecraft")
        .join("worldgen")
        .is_dir()
        .then_some(dir)
}

/// Reference values from the real 26.1.2 `RandomSpreadStructurePlacement`
/// (spacing 34, separation 8, salt 10387312, `LINEAR`), produced by running the
/// bundled decompiled class: `(seed, chunkX, chunkZ, candidateX, candidateZ,
/// isPlacementChunk)`.
const PLACEMENT_REFERENCE: &[(i64, i32, i32, i32, i32, bool)] = &[
    (0, 0, 0, 15, 2, false),
    (0, 0, 1, 15, 2, false),
    (0, 0, 17, 15, 2, false),
    (0, 0, -3, 19, -15, false),
    (0, 0, 100, 20, 72, false),
    (0, 1, 0, 15, 2, false),
    (0, 1, 1, 15, 2, false),
    (0, 1, 17, 15, 2, false),
    (0, 1, -3, 19, -15, false),
    (0, 1, 100, 20, 72, false),
    (0, 17, 0, 15, 2, false),
    (0, 17, 1, 15, 2, false),
    (0, 17, 17, 15, 2, false),
    (0, 17, -3, 19, -15, false),
    (0, 17, 100, 20, 72, false),
    (0, -3, 0, -27, 1, false),
    (0, -3, 1, -27, 1, false),
    (0, -3, 17, -27, 1, false),
    (0, -3, -3, -9, -34, false),
    (0, -3, 100, -18, 69, false),
    (0, 100, 0, 85, 23, false),
    (0, 100, 1, 85, 23, false),
    (0, 100, 17, 85, 23, false),
    (0, 100, -3, 74, -24, false),
    (0, 100, 100, 90, 71, false),
    (42, 0, 0, 9, 11, false),
    (42, 0, 1, 9, 11, false),
    (42, 0, 17, 9, 11, false),
    (42, 0, -3, 10, -27, false),
    (42, 0, 100, 11, 87, false),
    (42, 1, 0, 9, 11, false),
    (42, 1, 1, 9, 11, false),
    (42, 1, 17, 9, 11, false),
    (42, 1, -3, 10, -27, false),
    (42, 1, 100, 11, 87, false),
    (42, 17, 0, 9, 11, false),
    (42, 17, 1, 9, 11, false),
    (42, 17, 17, 9, 11, false),
    (42, 17, -3, 10, -27, false),
    (42, 17, 100, 11, 87, false),
    (42, -3, 0, -19, 10, false),
    (42, -3, 1, -19, 10, false),
    (42, -3, 17, -19, 10, false),
    (42, -3, -3, -29, -21, false),
    (42, -3, 100, -13, 84, false),
    (42, 100, 0, 68, 10, false),
    (42, 100, 1, 68, 10, false),
    (42, 100, 17, 68, 10, false),
    (42, 100, -3, 93, -13, false),
    (42, 100, 100, 84, 85, false),
    (1234567890, 0, 0, 16, 4, false),
    (1234567890, 0, 1, 16, 4, false),
    (1234567890, 0, 17, 16, 4, false),
    (1234567890, 0, -3, 5, -9, false),
    (1234567890, 0, 100, 21, 88, false),
    (1234567890, 1, 0, 16, 4, false),
    (1234567890, 1, 1, 16, 4, false),
    (1234567890, 1, 17, 16, 4, false),
    (1234567890, 1, -3, 5, -9, false),
    (1234567890, 1, 100, 21, 88, false),
    (1234567890, 17, 0, 16, 4, false),
    (1234567890, 17, 1, 16, 4, false),
    (1234567890, 17, 17, 16, 4, false),
    (1234567890, 17, -3, 5, -9, false),
    (1234567890, 17, 100, 21, 88, false),
    (1234567890, -3, 0, -31, 8, false),
    (1234567890, -3, 1, -31, 8, false),
    (1234567890, -3, 17, -31, 8, false),
    (1234567890, -3, -3, -10, -32, false),
    (1234567890, -3, 100, -33, 84, false),
    (1234567890, 100, 0, 87, 4, false),
    (1234567890, 100, 1, 87, 4, false),
    (1234567890, 100, 17, 87, 4, false),
    (1234567890, 100, -3, 78, -14, false),
    (1234567890, 100, 100, 92, 73, false),
];
/// The whole village closure, walked by reference on the real cache: the set,
/// its five structures, the pools and pieces the connectors reach, the processor
/// lists those pools name, and exactly the 13 decor features A1/A2 implemented.
#[test]
fn village_closure_walks_the_real_cache() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP village_closure_walks_the_real_cache: no vanilla content cache found \
             (set SOLARIS_CONTENT_CACHE)"
        );
        return;
    };
    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let closure =
        crate::village::load_village_closure(&cache, &identifier("minecraft:villages"), &blocks)
            .expect("the village closure loads");

    assert_eq!(closure.structures.len(), 5);
    for structure in &closure.structures {
        assert_eq!(structure.weight, 1);
        assert_eq!(structure.spec.size, 6);
        assert!(structure.spec.use_expansion_hack);
        assert_eq!(structure.spec.max_distance_from_center.horizontal, 80);
        assert!(structure.spec.start_pool.as_str().contains("town_centers"));
    }
    assert_eq!(closure.placement.spacing, 34);
    assert_eq!(closure.placement.separation, 8);
    assert_eq!(closure.placement.salt, 10387312);
    assert!(!closure.placement.triangular);

    println!(
        "village closure: pools={} pieces={} processor_lists={} features={:?}",
        closure.pools.len(),
        closure.pieces.len(),
        closure.processor_lists.len(),
        closure.placed_features
    );
    assert_eq!(closure.pools.len(), 62);
    assert_eq!(closure.pieces.len(), 478);
    assert_eq!(closure.processor_lists.len(), 16);
    assert_eq!(
        closure.placed_features,
        vec![
            identifier("minecraft:acacia"),
            identifier("minecraft:flower_plain"),
            identifier("minecraft:oak"),
            identifier("minecraft:patch_berry_bush"),
            identifier("minecraft:patch_cactus"),
            identifier("minecraft:patch_taiga_grass"),
            identifier("minecraft:pile_hay"),
            identifier("minecraft:pile_ice"),
            identifier("minecraft:pile_melon"),
            identifier("minecraft:pile_pumpkin"),
            identifier("minecraft:pile_snow"),
            identifier("minecraft:pine"),
            identifier("minecraft:spruce"),
        ],
        "the closure reaches exactly the 13 village decor features"
    );

    // Every jigsaw of every reached piece names a pool the walk also reached
    // (or `minecraft:empty`, which is where vanilla stops).
    let mut jigsaws = 0_usize;
    let mut empty_pieces = 0_usize;
    for piece in closure.pieces.values() {
        if piece.blocks().is_empty() {
            // A piece whose palette is entirely air/structure_void/jigsaw has no
            // placeable blocks (the loader drops those), which vanilla still
            // counts as a piece.
            empty_pieces += 1;
        }
        assert!(piece.size().iter().all(|axis| *axis > 0));
        for jigsaw in piece.jigsaws() {
            jigsaws += 1;
            assert!(
                jigsaw.pool.as_str() == "minecraft:empty"
                    || closure.pools.contains_key(&jigsaw.pool),
                "{} references unreached pool {}",
                piece.size().len(),
                jigsaw.pool
            );
        }
    }
    println!("village closure: {jigsaws} jigsaw blocks, {empty_pieces} block-less pieces");
    assert!(jigsaws > 900, "the closure holds {jigsaws} jigsaw blocks");
    // The block-less pieces are the ones whose palette is only air /
    // structure_void / jigsaw (terminators and similar): real pieces with no
    // placeable blocks, so the assertion is that most pieces do have blocks.
    assert!(
        empty_pieces * 2 < closure.pieces.len(),
        "{empty_pieces} empty pieces"
    );

    // Determinism: walking again reaches the same closure.
    let again =
        crate::village::load_village_closure(&cache, &identifier("minecraft:villages"), &blocks)
            .expect("the village closure loads again");
    assert_eq!(again.pools.len(), closure.pools.len());
    assert_eq!(again.pieces.len(), closure.pieces.len());
    assert_eq!(again.placed_features, closure.placed_features);
}

/// The jigsaw metadata the solver needs, read from a real village piece in the
/// content cache: `plains_small_house_1` carries a street connector and a
/// `bottom` villager slot.
#[test]
fn template_jigsaws_load_from_the_real_cache() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP template_jigsaws_load_from_the_real_cache: no vanilla content cache found \
             (set SOLARIS_CONTENT_CACHE)"
        );
        return;
    };
    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let piece = cache
        .join("data")
        .join("minecraft")
        .join("structure")
        .join("village")
        .join("plains")
        .join("houses")
        .join("plains_small_house_1.nbt");
    let template = crate::structures::StructureTemplate::from_nbt_file(&piece, &blocks)
        .expect("the piece template loads");
    assert_eq!(template.size(), [7, 7, 7]);
    let jigsaws = template.jigsaws();
    assert_eq!(jigsaws.len(), 2, "the house has two jigsaw blocks");

    let entrance = jigsaws
        .iter()
        .find(|jigsaw| jigsaw.name.as_str() == "minecraft:building_entrance")
        .expect("the street connector");
    assert_eq!(entrance.pos, [0, 0, 3]);
    assert_eq!(entrance.target.as_str(), "minecraft:building_entrance");
    assert_eq!(entrance.pool.as_str(), "minecraft:village/plains/streets");
    assert_eq!(entrance.joint, Joint::Aligned);
    // The piece's entrance jigsaw state is `minecraft:jigsaw[orientation=west_up]`.
    assert_eq!(entrance.front, BlockFace::West);
    assert_eq!(entrance.top, BlockFace::Up);
    assert_eq!(entrance.selection_priority, 0);
    let final_state = blocks.by_id(entrance.final_state).expect("a state");
    assert_eq!(final_state.block.id.as_str(), "minecraft:oak_stairs");
    assert!(
        final_state
            .properties
            .iter()
            .any(|(key, value)| key == "facing" && value == "east")
    );

    let bottom = jigsaws
        .iter()
        .find(|jigsaw| jigsaw.name.as_str() == "minecraft:bottom")
        .expect("the villager slot");
    assert_eq!(bottom.pos, [3, 0, 3]);
    assert_eq!(bottom.pool.as_str(), "minecraft:village/plains/villagers");
    assert_eq!(bottom.joint, Joint::Rollable);
    assert_eq!(
        blocks
            .by_id(bottom.final_state)
            .expect("a state")
            .block
            .id
            .as_str(),
        "minecraft:oak_planks"
    );
}

#[test]
fn orientation_names_map_to_front_and_top_faces() {
    assert_eq!(
        orientation_faces("north_up"),
        Some((BlockFace::North, BlockFace::Up))
    );
    assert_eq!(
        orientation_faces("up_east"),
        Some((BlockFace::Up, BlockFace::East))
    );
    assert_eq!(orientation_faces("nonsense"), None);
    assert_eq!(BlockFace::North.opposite(), BlockFace::South);
    assert_eq!(BlockFace::Up.step(), [0, 1, 0]);
    assert_eq!(BlockFace::West.step(), [-1, 0, 0]);
}

#[test]
fn random_spread_potential_chunk_matches_vanilla_reference() {
    let placement = RandomSpreadPlacement {
        spacing: 34,
        separation: 8,
        salt: 10387312,
        triangular: false,
    };
    assert!(!PLACEMENT_REFERENCE.is_empty());
    for (seed, chunk_x, chunk_z, candidate_x, candidate_z, is_placement) in PLACEMENT_REFERENCE {
        let candidate = placement.potential_chunk(*seed, *chunk_x, *chunk_z);
        assert_eq!(
            candidate,
            (*candidate_x, *candidate_z),
            "seed {seed} chunk ({chunk_x},{chunk_z})"
        );
        assert_eq!(
            placement.is_placement_chunk(*seed, *chunk_x, *chunk_z),
            *is_placement,
            "seed {seed} chunk ({chunk_x},{chunk_z})"
        );
    }
}

#[test]
fn large_feature_with_salt_matches_worldgen_random() {
    // `WorldgenRandom.setLargeFeatureWithSalt`: x * 341873128712 +
    // z * 132897987541 + seed + blend.
    assert_eq!(
        set_large_feature_with_salt(7, 1, 2, 3),
        341873128712 + 2 * 132897987541 + 10
    );
    assert_eq!(set_large_feature_with_salt(0, 0, 0, 0), 0);
    assert_eq!(
        set_large_feature_with_salt(-1, -1, -1, -5),
        -341873128712 - 132897987541 - 6
    );
}

/// `WorldgenRandom.setLargeFeatureSeed`: the seed the structure selection and
/// the growth both run on, and *not* the placement's salted one.
#[test]
fn large_feature_seed_matches_the_worldgen_random() {
    // Restated from the formula: a legacy source seeded with `seed`, two
    // `nextLong` scale factors, combined by `x * xScale ^ z * zScale ^ seed`.
    let derived = |seed: i64, chunk_x: i32, chunk_z: i32| {
        let mut random = LegacyRandom::new(seed);
        let x_scale = random.next_long();
        let z_scale = random.next_long();
        i64::from(chunk_x).wrapping_mul(x_scale) ^ i64::from(chunk_z).wrapping_mul(z_scale) ^ seed
    };
    assert_eq!(set_large_feature_seed(0, 5, 7), derived(0, 5, 7));
    assert_eq!(set_large_feature_seed(-3, -9, 12), derived(-3, -9, 12));
    // Pinned so an LCG or composition change cannot slip through unnoticed.
    assert_eq!(set_large_feature_seed(0, 5, 7), 624_053_714_787_533_610);
    assert_eq!(
        set_large_feature_seed(4242, 427, 0),
        7_101_243_444_056_765_349
    );
    // The placement's own source is a different derivation from the same seed.
    assert_ne!(
        set_large_feature_seed(0, 5, 7),
        set_large_feature_with_salt(0, 5, 7, 10387312)
    );
}

/// `WorldgenRandom.setDecorationSeed`/`setFeatureSeed`: the decor lane's seeding
/// and the first draws of the stream it seeds.
///
/// Every number here was read out of the real 26.1.2 classes —
/// `new WorldgenRandom(new XoroshiroRandomSource(seed))` with
/// `setDecorationSeed(seed, chunkX * 16, chunkZ * 16)` then
/// `setFeatureSeed(decorationSeed, 22, 4)`, the village set's index among the
/// `surface_structures` entries and that step's ordinal
/// (`.analysis/codex-logs/village-decor-random/`). The seed is a `WorldgenRandom`
/// *wrapper*, so both calls compose their draws from the legacy formulas over the
/// Xoroshiro bits: a raw [`XoroshiroRandom`] gives a different decoration seed
/// (and a different stream) from the same world seed.
#[test]
fn decor_seeds_match_the_worldgen_random() {
    let mut random = WorldgenRandom::over_xoroshiro(4242);
    let decoration_seed = random.set_decoration_seed(4242, 427 * 16, 0);
    assert_eq!(decoration_seed, 3_979_914_027_210_390_498);
    random.set_feature_seed(decoration_seed, 22, 4);
    // Every composed draw, in the order the probe read them: these are
    // `BitRandomSource`'s legacy formulas over the Xoroshiro bits, which is what
    // makes the wrapper a wrapper.
    assert_eq!(random.next_long(), -5_071_117_971_071_978_252);
    assert_eq!(random.next_int(), -127_179_403);
    assert_eq!(random.next_int_bounded(5), 0);
    assert_eq!(random.next_float(), 0.982_076_6);
    assert_eq!(random.next_double(), 0.590_878_973_655_357_9);
    assert!(random.next_boolean());

    // At the origin the mixed seed is 0, and the stream that follows is still a
    // wrapper's: a raw source seeded with 0 draws differently.
    let mut origin = WorldgenRandom::over_xoroshiro(0);
    let origin_seed = origin.set_decoration_seed(0, 0, 0);
    assert_eq!(origin_seed, 0);
    origin.set_feature_seed(origin_seed, 22, 4);
    assert_eq!(origin.next_long(), 7_615_636_877_602_399_696);

    // The structure's index is the set's position in its step's registry list,
    // so the desert village (index 21) runs a different stream.
    let mut desert = WorldgenRandom::over_xoroshiro(4242);
    let desert_seed = desert.set_decoration_seed(4242, 427 * 16, 0);
    desert.set_feature_seed(desert_seed, 21, 4);
    assert_eq!(desert.next_long(), 9_222_894_648_221_271_537);

    // The wrapper is the point: the same world seed through the raw source is a
    // different stream.
    assert_eq!(
        WorldgenRandom::over_xoroshiro(4242).next_long(),
        -8_923_083_906_554_867_932
    );
    assert_eq!(
        XoroshiroRandom::new(4242).next_long(),
        -8_923_083_901_228_911_911
    );
}

#[test]
fn start_origin_uses_the_projected_height() {
    // `project_start_to_heightmap = WORLD_SURFACE_WG` replaces the absolute
    // start height with the free height at the chunk's start column.
    let projected = start_origin(4, -2, 0, 71);
    assert_eq!(
        projected,
        BlockPos {
            x: 64,
            y: 71,
            z: -32
        }
    );
    let absolute = start_origin(4, -2, 40, 71);
    assert_eq!(absolute.y, 40);
}

/// The kernel is `e^(-(dx² + (dy+0.5)² + dz²)/16)` and `beard_contribution`
/// multiplies it by `-dyWithOffset * fastInvSqrt(d`²/2) / 2`, both quoted from
/// `Beardifier`.
#[test]
fn beard_kernel_matches_vanilla_formula() {
    for (dx, dy, dz) in [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (3, -2, 1),
        (12, 12, 12),
        (-4, 5, 2),
    ] {
        let expected = kernel_value(dx, dy, dz);
        let distance_sqr =
            f64::from(dx).powi(2) + (f64::from(dy) + 0.5).powi(2) + f64::from(dz).powi(2);
        assert!(
            (expected - (-distance_sqr / 16.0).exp()).abs() < 1e-15,
            "kernel ({dx},{dy},{dz})"
        );
    }

    // Outside the 24³ window the contribution is exactly zero.
    assert_eq!(beard_contribution(BEARD_KERNEL_RADIUS, 0, 0, 0), 0.0);
    assert_eq!(beard_contribution(-BEARD_KERNEL_RADIUS - 1, 0, 0, 0), 0.0);
    assert_ne!(beard_contribution(BEARD_KERNEL_RADIUS - 1, 0, 0, 0), 0.0);

    // A rigid piece pulls terrain toward `box.minY() + groundLevelDelta`.
    let piece = BeardContribution {
        min: BlockPos { x: 0, y: 64, z: 0 },
        max: BlockPos { x: 4, y: 70, z: 4 },
        ground_level_delta: 1,
    };
    // `getBeardContribution` always uses `dyWithOffset = yToGround + 0.5`, so
    // the sign follows the side of the ground line and the pull is toward it.
    let ground = 65;
    let above = vanilla_contribution(&[piece], &[], 2, ground + 2, 2);
    let below = vanilla_contribution(&[piece], &[], 2, ground - 2, 2);
    let at_ground = vanilla_contribution(&[piece], &[], 2, ground, 2);
    assert!(
        above < 0.0,
        "above the ground line the offset is negative: {above}"
    );
    assert!(
        below > 0.0,
        "below the ground line the offset is positive: {below}"
    );
    assert!(
        at_ground < above,
        "the pull is strongest at the ground line and decays with height: \
         at={at_ground} above={above}"
    );
    assert!(
        (at_ground - (-0.5 * fast_inv_sqrt_ref(0.25 / 2.0) / 2.0 * kernel_value(0, 0, 0)) * 0.8)
            .abs()
            < 1e-12,
        "the ground-line value is vanilla's formula at dyToGround = 0: {at_ground}"
    );

    // The weights are vanilla's 0.8 (rigid) and 0.4 (junction).
    let junction = BeardJunction {
        x: 2,
        ground_y: 66,
        z: 2,
    };
    // A junction sits at (2, 66, 2): at that exact sample the junction term is
    // `0.4 * getBeardContribution(0, 0, 0, 0)`, and the rigid term is the
    // piece's own value, so the sample is their sum.
    let rigid_only = vanilla_contribution(&[piece], &[], 2, 66, 2);
    let junction_only = vanilla_contribution(&[], &[junction], 2, 66, 2);
    assert!((junction_only - 0.4 * beard_contribution(0, 0, 0, 0)).abs() < 1e-15);
    let combined = vanilla_contribution(&[piece], &[junction], 2, 66, 2);
    assert!((combined - (rigid_only + junction_only)).abs() < 1e-15);

    // One block away the junction's `dx` is 1 and the two weights stay separate.
    let junction_offset = vanilla_contribution(&[], &[junction], 3, 66, 2);
    let rigid_contribution = vanilla_contribution(&[piece], &[], 3, 66, 2);
    let junction_manual = 0.4 * beard_contribution(1, 0, 0, 0);
    assert!((junction_offset - junction_manual).abs() < 1e-15);
    let combined_manual = rigid_contribution + junction_manual;
    let combined = vanilla_contribution(&[piece], &[junction], 3, 66, 2);
    assert!((combined - combined_manual).abs() < 1e-15);
}

#[test]
fn beard_affected_box_is_inflated_by_the_kernel_radius() {
    let piece = BeardContribution {
        min: BlockPos { x: -3, y: 60, z: 7 },
        max: BlockPos { x: 9, y: 66, z: 11 },
        ground_level_delta: 0,
    };
    let junction = BeardJunction {
        x: 20,
        ground_y: 64,
        z: -4,
    };
    let (min, max) = affected_box(&[piece], &[junction]).expect("a box");
    assert_eq!(min.x, -3 - BEARD_KERNEL_RADIUS);
    assert_eq!(min.y, 60 - BEARD_KERNEL_RADIUS);
    assert_eq!(min.z, -4 - BEARD_KERNEL_RADIUS);
    assert_eq!(max.x, 20 + BEARD_KERNEL_RADIUS);
    assert_eq!(max.y, 66 + BEARD_KERNEL_RADIUS);
    assert_eq!(max.z, 11 + BEARD_KERNEL_RADIUS);
    assert!(affected_box(&[], &[]).is_none());
}

/// The analogue's *shape*, not just its formula: a column inside the affected
/// box is pulled toward the rigid piece's ground target, a column outside is
/// untouched, the pull falls off with distance and vanishes at the kernel
/// radius, and a fixed input always produces the same columns.
#[test]
fn beard_pull_shape_is_monotone_and_bounded_by_the_kernel() {
    let piece = BeardContribution {
        min: BlockPos { x: 0, y: 64, z: 0 },
        max: BlockPos { x: 4, y: 70, z: 4 },
        ground_level_delta: 0,
    };
    let (min, max) = affected_box(&[piece], &[]).expect("an affected box");

    // Pull magnitude decreases with distance from the piece's box, and is zero
    // at the kernel radius.
    let at_edge = vanilla_contribution(&[piece], &[], -1, 60, -1).abs();
    let one_further = vanilla_contribution(&[piece], &[], -2, 60, -2).abs();
    let at_radius = vanilla_contribution(&[piece], &[], min.x, 60, min.z);
    assert!(
        at_edge > one_further,
        "the pull decays with distance: {at_edge} vs {one_further}"
    );
    assert_eq!(at_radius, 0.0, "zero at the kernel radius");

    // The pull point sits toward the piece's ground level: a column below the
    // target is raised, one above it is lowered.
    let below = vanilla_contribution(&[piece], &[], -1, 60, -1);
    let above = vanilla_contribution(&[piece], &[], -1, 72, -1);
    assert!(
        below > 0.0,
        "below the ground level the surface is pulled up"
    );
    assert!(
        above < 0.0,
        "above the ground level the surface is pulled down"
    );

    // Columns outside the affected box never move, and the whole pass is
    // deterministic.
    let columns = vec![
        (-1, -1, 60),
        (-1, 72 - 60, 0),
        (max.x + 1, max.z + 1, 64),
        (min.x - 1, min.z, 64),
    ];
    let moved = apply_columns(&[piece], &[], &columns, -64, 320);
    assert!(moved.iter().all(|column| {
        column.x >= min.x && column.x <= max.x && column.z >= min.z && column.z <= max.z
    }));
    assert_eq!(moved, apply_columns(&[piece], &[], &columns, -64, 320));
}

#[test]
fn beard_columns_report_and_clamp() {
    let piece = BeardContribution {
        min: BlockPos { x: 0, y: 64, z: 0 },
        max: BlockPos { x: 2, y: 68, z: 2 },
        ground_level_delta: 0,
    };
    let columns = vec![(1, 1, 60), (1, 1, 64), (1, 1, 70), (40, 40, 64)];
    let moved = apply_columns(&[piece], &[], &columns, -64, 320);
    assert!(!moved.is_empty());
    for column in &moved {
        assert_ne!(column.surface_y, column.adjusted_y);
        assert!(column.adjusted_y > -64 && column.adjusted_y < 320);
        // A column well outside the affected box never moves.
        assert!(!(column.x == 40 && column.z == 40));
    }
    // Determinism: the same input always yields the same columns.
    assert_eq!(moved, apply_columns(&[piece], &[], &columns, -64, 320));

    // Clamping keeps the surface inside the world.
    let clamped = apply_columns(
        &[BeardContribution {
            min: BlockPos { x: 0, y: 320, z: 0 },
            max: BlockPos { x: 0, y: 320, z: 0 },
            ground_level_delta: 0,
        }],
        &[],
        &[(0, 0, -63)],
        -64,
        320,
    );
    assert!(clamped.iter().all(|column| column.adjusted_y >= -63));
}
