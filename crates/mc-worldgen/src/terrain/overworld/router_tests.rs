use super::*;
use mc_world::chunk::OVERWORLD_GEOMETRY;
use std::collections::VecDeque;

#[test]
fn tellus_temperature_uses_latitude_and_surface_altitude() {
    let settings = TellusWorldgenSettings::default();
    let router = OverworldRouter::new(77, OVERWORLD_GEOMETRY, WorldgenMode::TellusLike(settings));
    let equator = router.temperature(0.0, 72.0, 0.0, Some(settings));
    let arctic = router.temperature(0.0, 72.0, -10_000_000.0, Some(settings));
    let summit = router.temperature(0.0, 180.0, 0.0, Some(settings));
    assert!(equator > arctic);
    assert!(equator > summit);
}

#[test]
fn terrain_is_deterministic_bounded_and_continuous() {
    for seed in -16..16 {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
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
fn chunk_borders_have_the_same_slope_budget_as_interior_columns() {
    for seed in [-9, 0, 27] {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for chunk_x in -24..=24 {
            for chunk_z in -24..=24 {
                let x = chunk_x * 16 + 15;
                let z = chunk_z * 16 + 15;
                let center = router.sample(x, z).surface_y;
                assert!((center - router.sample(x + 1, z).surface_y).abs() <= 3);
                assert!((center - router.sample(x, z + 1).surface_y).abs() <= 3);
            }
        }
    }
}

#[test]
fn rolling_terrain_changes_over_regions_not_single_chunks() {
    let mut near_change = 0.0;
    let mut regional_change = 0.0;
    let mut comparisons = 0u64;

    for seed in [-11, 0, 23] {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for z in (-2_048..=2_048).step_by(137) {
            for x in (-2_048..=2_048).step_by(131) {
                let center = landforms::rolling_hills(router, f64::from(x), f64::from(z), 1.0);
                for (near_x, near_z, far_x, far_z) in
                    [(x + 8, z, x + 128, z), (x, z + 8, x, z + 128)]
                {
                    near_change += (center
                        - landforms::rolling_hills(
                            router,
                            f64::from(near_x),
                            f64::from(near_z),
                            1.0,
                        ))
                    .abs();
                    regional_change += (center
                        - landforms::rolling_hills(
                            router,
                            f64::from(far_x),
                            f64::from(far_z),
                            1.0,
                        ))
                    .abs();
                    comparisons += 1;
                }
            }
        }
    }

    assert!(comparisons > 5_000);
    assert!(
        near_change * 4.0 < regional_change,
        "terrain changes too much at single-chunk scale: near={near_change}, regional={regional_change}"
    );
}

#[test]
fn landforms_do_not_create_isolated_craters() {
    for (seed, mode) in [
        (-11, WorldgenMode::VanillaLike),
        (23, WorldgenMode::VanillaLike),
        (
            91,
            WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
        ),
    ] {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, mode);
        for z in (-1_024..=1_024).step_by(31) {
            for x in (-1_024..=1_024).step_by(29) {
                let center = router.sample(x, z).surface_y;
                let surrounding_min = [
                    (x - 4, z),
                    (x + 4, z),
                    (x, z - 4),
                    (x, z + 4),
                    (x - 3, z - 3),
                    (x + 3, z - 3),
                    (x - 3, z + 3),
                    (x + 3, z + 3),
                ]
                .into_iter()
                .map(|(x, z)| router.sample(x, z).surface_y)
                .min()
                .unwrap();
                assert!(
                    surrounding_min - center <= 6,
                    "seed {seed} has an isolated crater at ({x},{z}): {center} below {surrounding_min}"
                );
            }
        }
    }
}

#[test]
fn origin_terrain_is_not_normalized_across_seeds() {
    let heights = (-16..16)
        .map(|seed| {
            OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike)
                .sample(0, 0)
                .surface_y
        })
        .collect::<std::collections::HashSet<_>>();
    assert!(
        heights.len() >= 8,
        "origin terrain still looks normalized across seeds: {heights:?}"
    );
}

#[test]
fn caves_never_open_the_surface_shell_across_seed_grid() {
    for seed in -16..16 {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for z in (-256..=256).step_by(31) {
            for x in (-256..=256).step_by(29) {
                let surface = router.sample(x, z).surface_y;
                for y in surface - CAVE_SURFACE_CLEARANCE..=surface {
                    assert!(
                        !router.is_cave(x, y, z, surface),
                        "seed {seed} cave opened the surface shell at ({x},{y},{z})"
                    );
                }
            }
        }
    }
}

#[test]
fn caves_are_sparse_connected_tunnels_without_shafts_or_chambers() {
    const SIDE: usize = 64;
    const MIN_Y: i32 = -48;
    const HEIGHT: usize = 81;
    let index = |x: usize, y: usize, z: usize| (y * SIDE + z) * SIDE + x;
    let mut total_caves = 0usize;

    for seed in [-3, 7] {
        let router = OverworldRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        let origin = i32::try_from(seed * 97).unwrap();
        let mut volume = vec![false; SIDE * HEIGHT * SIDE];
        let mut cave_count = 0usize;
        for y in 0..HEIGHT {
            for z in 0..SIDE {
                for x in 0..SIDE {
                    let world_x = origin + x as i32;
                    let world_z = -origin + z as i32;
                    let cave = router.is_cave(
                        world_x,
                        MIN_Y + y as i32,
                        world_z,
                        router.sample(world_x, world_z).surface_y,
                    );
                    volume[index(x, y, z)] = cave;
                    cave_count += usize::from(cave);
                }
            }
        }

        total_caves += cave_count;
        assert!(
            cave_count * 100 <= volume.len() * 6,
            "seed {seed} cave field is too open"
        );

        for z in 0..SIDE {
            for x in 0..SIDE {
                let mut longest = 0usize;
                let mut current = 0usize;
                for y in 0..HEIGHT {
                    if volume[index(x, y, z)] {
                        current += 1;
                        longest = longest.max(current);
                    } else {
                        current = 0;
                    }
                }
                assert!(longest <= 10, "seed {seed} opened a {longest}-block shaft");
            }
        }

        for y in 0..HEIGHT {
            let mut prefix = vec![0usize; (SIDE + 1) * (SIDE + 1)];
            for z in 0..SIDE {
                for x in 0..SIDE {
                    let prefix_index = |x: usize, z: usize| z * (SIDE + 1) + x;
                    prefix[prefix_index(x + 1, z + 1)] = usize::from(volume[index(x, y, z)])
                        + prefix[prefix_index(x, z + 1)]
                        + prefix[prefix_index(x + 1, z)]
                        - prefix[prefix_index(x, z)];
                }
            }
            for z in 0..=SIDE - 9 {
                for x in 0..=SIDE - 9 {
                    let prefix_index = |x: usize, z: usize| z * (SIDE + 1) + x;
                    let open = prefix[prefix_index(x + 9, z + 9)] + prefix[prefix_index(x, z)]
                        - prefix[prefix_index(x, z + 9)]
                        - prefix[prefix_index(x + 9, z)];
                    assert!(
                        open <= 36,
                        "seed {seed} opened {open}/81 cells at y={} x={x} z={z}",
                        MIN_Y + y as i32,
                    );
                }
            }
        }

        let mut visited = vec![false; volume.len()];
        for y in 0..HEIGHT {
            for z in 0..SIDE {
                for x in 0..SIDE {
                    let start = index(x, y, z);
                    if !volume[start] || visited[start] {
                        continue;
                    }
                    let mut queue = VecDeque::from([(x, y, z)]);
                    visited[start] = true;
                    let mut cells = 0usize;
                    let (mut min_x, mut max_x) = (x, x);
                    let (mut min_y, mut max_y) = (y, y);
                    let (mut min_z, mut max_z) = (z, z);
                    while let Some((x, y, z)) = queue.pop_front() {
                        cells += 1;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                        min_z = min_z.min(z);
                        max_z = max_z.max(z);
                        for (dx, dy, dz) in [
                            (-1, 0, 0),
                            (1, 0, 0),
                            (0, -1, 0),
                            (0, 1, 0),
                            (0, 0, -1),
                            (0, 0, 1),
                        ] {
                            let next_x = x as isize + dx;
                            let next_y = y as isize + dy;
                            let next_z = z as isize + dz;
                            if next_x < 0
                                || next_y < 0
                                || next_z < 0
                                || next_x >= SIDE as isize
                                || next_y >= HEIGHT as isize
                                || next_z >= SIDE as isize
                            {
                                continue;
                            }
                            let next = index(next_x as usize, next_y as usize, next_z as usize);
                            if volume[next] && !visited[next] {
                                visited[next] = true;
                                queue.push_back((
                                    next_x as usize,
                                    next_y as usize,
                                    next_z as usize,
                                ));
                            }
                        }
                    }
                    let bounds = (max_x - min_x + 1) * (max_y - min_y + 1) * (max_z - min_z + 1);
                    assert!(
                        cells < 64 || cells * 100 <= bounds * 45,
                        "seed {seed} cave component fills {cells}/{bounds} bounding cells"
                    );
                }
            }
        }
    }
    assert!(total_caves > 24, "sampled volumes should include caves");
}
