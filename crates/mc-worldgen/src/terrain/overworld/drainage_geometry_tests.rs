use super::{DrainageCell, DrainageSample, cell_at, cell_blocks, evaluate_segment, sample};

#[test]
fn sampling_matches_complete_reaches_at_cell_boundaries() {
    let seed = 5_617_830;
    for (scale, minimum_bank_width) in [(0.25, 0.0), (1.0, 0.0), (8.0, 0.0), (0.25, 64.0)] {
        let size = cell_blocks(scale);
        for cell_z in (-16..=16).step_by(4) {
            for cell_x in -16..=16 {
                for offset in [-1.0, 0.0, 1.0] {
                    let edge = (f64::from(cell_x) * size + offset) as i32;
                    let middle = ((f64::from(cell_z) + 0.5) * size) as i32;
                    for (x, z) in [(edge, middle), (middle, edge)] {
                        let cell = cell_at(f64::from(x), f64::from(z), size);
                        let mut expected = DrainageSample {
                            river_distance: 1.0,
                            ..DrainageSample::default()
                        };
                        // Unpruned reference includes every geometrically reachable origin.
                        for dz in -4..=4 {
                            for dx in -4..=4 {
                                evaluate_segment(
                                    seed,
                                    DrainageCell {
                                        x: cell.x + dx,
                                        z: cell.z + dz,
                                    },
                                    f64::from(x),
                                    f64::from(z),
                                    size,
                                    minimum_bank_width,
                                    &mut expected,
                                );
                            }
                        }
                        let actual = sample(seed, x, z, scale, minimum_bank_width);
                        assert_eq!(
                            (
                                actual.channel_weight,
                                actual.river_distance,
                                actual.accumulation
                            ),
                            (
                                expected.channel_weight,
                                expected.river_distance,
                                expected.accumulation
                            ),
                            "river contribution clipped at ({x}, {z}), scale {scale}",
                        );
                    }
                }
            }
        }
    }
}
