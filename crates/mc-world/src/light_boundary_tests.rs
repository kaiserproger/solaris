use std::collections::VecDeque;

use super::{N_X, N_Z, bfs, grid_idx, grid_volume, seed_sky_from_open_columns};

#[test]
fn irregular_sky_boundaries_match_full_source_propagation() {
    let height = 32;
    let mut opacity = vec![0; grid_volume(height)];
    let mut propagates_sky = vec![true; grid_volume(height)];
    let mut expected = vec![0; grid_volume(height)];
    let mut full_sources = VecDeque::new();
    for gz in 0..N_Z {
        for gx in 0..N_X {
            // Include fully open, fully blocked and uneven neighbouring columns.
            let bottom = (gx * 17 + gz * 31) % (height + 1);
            if bottom > 0 {
                let roof = grid_idx(gx, bottom - 1, gz);
                propagates_sky[roof] = false;
                opacity[roof] = if (gx + gz).is_multiple_of(2) { 15 } else { 1 };
            }
            for y in bottom..height {
                let index = grid_idx(gx, y, gz);
                expected[index] = 15;
                full_sources.push_back(index as u32);
            }
        }
    }
    bfs(&opacity, &mut expected, &mut full_sources, height);

    let mut actual = vec![0; grid_volume(height)];
    let mut boundary_sources = VecDeque::new();
    seed_sky_from_open_columns(&propagates_sky, &mut actual, &mut boundary_sources, height);
    bfs(&opacity, &mut actual, &mut boundary_sources, height);

    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "light at grid index {index}");
    }
}
