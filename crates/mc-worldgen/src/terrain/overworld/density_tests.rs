use super::*;
use mc_world::chunk::OVERWORLD_GEOMETRY;

#[test]
fn tellus_temperature_uses_latitude_and_surface_altitude() {
    let settings = TellusWorldgenSettings::default();
    let router = DensityRouter::new(77, OVERWORLD_GEOMETRY, WorldgenMode::TellusLike(settings));
    let equator = router.temperature(0.0, 72.0, 0.0, Some(settings));
    let arctic = router.temperature(0.0, 72.0, -10_000_000.0, Some(settings));
    let summit = router.temperature(0.0, 180.0, 0.0, Some(settings));
    assert!(equator > arctic);
    assert!(equator > summit);
}

#[test]
fn surface_is_deterministic_bounded_and_spawn_safe() {
    for seed in -16..16 {
        let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        assert!(router.sample(0, 0).surface_y >= SEA_LEVEL + 3);
        for z in (-2_048..=2_048).step_by(97) {
            for x in (-2_048..=2_048).step_by(31) {
                let current = router.sample(x, z).surface_y;
                assert_eq!(current, router.sample(x, z).surface_y);
                for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                    let next = router.sample(nx, nz).surface_y;
                    assert!(
                        (current - next).abs() <= 3,
                        "terrain step for seed {seed} at ({x},{z}): {current} -> {next}"
                    );
                }
            }
        }
    }
}

#[test]
fn caves_are_sparse_connected_tunnels_without_large_vertical_openings() {
    let mut caves = 0;
    let mut isolated = 0;
    let mut sampled = 0;
    for seed in -4..4 {
        let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for x in (-96..=96).step_by(8) {
            for z in (-96..=96).step_by(8) {
                let mut longest = 0;
                let mut current = 0;
                for y in -48..=32 {
                    sampled += 1;
                    if router.is_cave(x, y, z) {
                        caves += 1;
                        current += 1;
                        longest = longest.max(current);
                        if ![
                            (1, 0, 0),
                            (-1, 0, 0),
                            (0, 1, 0),
                            (0, -1, 0),
                            (0, 0, 1),
                            (0, 0, -1),
                        ]
                        .into_iter()
                        .any(|(dx, dy, dz)| router.is_cave(x + dx, y + dy, z + dz))
                        {
                            isolated += 1;
                        }
                    } else {
                        current = 0;
                    }
                }
                assert!(
                    longest <= 12,
                    "seed {seed} opened a {longest}-block shaft at {x},{z}"
                );
            }
        }
    }
    assert!(caves > 24, "sample should include caves");
    assert!(
        caves * 100 <= sampled * 8,
        "caves occupied {caves} of {sampled} sampled underground cells"
    );
    assert!(
        isolated * 100 <= caves,
        "{isolated} of {caves} cave cells were isolated"
    );
}

#[test]
fn caves_do_not_form_broad_horizontal_chambers() {
    let mut maximum_open_cells = 0usize;
    for seed in [-3, 7] {
        let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for y in [-40, -20, 0, 20] {
            for center_x in (-64..=64).step_by(8) {
                for center_z in (-64..=64).step_by(8) {
                    let open_cells = (-4..=4)
                        .flat_map(|dx| (-4..=4).map(move |dz| (dx, dz)))
                        .filter(|(dx, dz)| router.is_cave(center_x + dx, y, center_z + dz))
                        .count();
                    maximum_open_cells = maximum_open_cells.max(open_cells);
                }
            }
        }
    }
    assert!(
        maximum_open_cells <= 45,
        "cave field opened {maximum_open_cells}/81 cells in a horizontal 9x9 window"
    );
}
