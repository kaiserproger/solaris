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
            assert!((-1.0..=1.0).contains(&v), "value out of [-1,1]: {v}");
        }
    }
}

#[test]
fn different_seeds_differ() {
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
        "two seeds produced identical noise across 100 samples"
    );
}

#[test]
fn fbm_is_bounded() {
    for x in -20..=20 {
        for z in -20..=20 {
            let v = fbm_2d(x as f64 * 0.1, z as f64 * 0.1, 13, 4, 0.5);
            assert!((-1.0..=1.0).contains(&v), "fbm out of [-1,1]: {v}");
        }
    }
}

#[test]
fn neighbour_samples_are_continuous() {
    let a = value_noise_2d(5.0, 5.0, 99);
    let b = value_noise_2d(5.001, 5.0, 99);
    assert!((a - b).abs() < 0.05, "expected smooth noise: {a} vs {b}");
}

#[test]
fn two_octave_grid_matches_scalar_noise_bits() {
    for (min_x, min_z, y, divisors, seed) in [
        (-129, -65, -32, [240.0, 120.0, 240.0], 29),
        (-17, 31, -7, [34.0, 8.0, 34.0], -91),
        (2_047, -2_049, 18, [40.0, 10.0, 40.0], i64::MAX),
    ] {
        let side = 18;
        let grid = Fbm3dTwoOctaveGrid::new(min_x, min_z, side, y, divisors, seed);
        for z in 0..side {
            for x in 0..side {
                let world_x = min_x + x as i32;
                let world_z = min_z + z as i32;
                let scalar = fbm_3d(
                    f64::from(world_x) / divisors[0],
                    f64::from(y) / divisors[1],
                    f64::from(world_z) / divisors[2],
                    seed,
                    2,
                    0.5,
                );
                assert_eq!(
                    grid.sample(x, z).to_bits(),
                    scalar.to_bits(),
                    "grid changed scalar noise at ({world_x}, {y}, {world_z})"
                );
            }
        }
    }
}

#[test]
fn three_dimensional_noise_is_bounded_deterministic_and_continuous() {
    for x in -8..=8 {
        for y in -8..=8 {
            for z in -8..=8 {
                let point = (x as f64 * 0.17, y as f64 * 0.13, z as f64 * 0.19);
                let value = fbm_3d(point.0, point.1, point.2, 29, 3, 0.5);
                assert_eq!(value, fbm_3d(point.0, point.1, point.2, 29, 3, 0.5));
                assert!((-1.0..=1.0).contains(&value));
            }
        }
    }
    let a = value_noise_3d(5.0, -2.0, 7.0, 91);
    let b = value_noise_3d(5.001, -1.999, 7.001, 91);
    assert!((a - b).abs() < 0.05, "expected smooth 3D noise: {a} vs {b}");
}

#[test]
fn three_dimensional_seed_is_not_a_shifted_x_coordinate() {
    for x in -16..=16 {
        assert_ne!(
            hash4(x, 7, -11, 41),
            hash4(x + 1, 7, -11, 40),
            "seed must be mixed independently from the X coordinate"
        );
    }
}
