use crate::noise::{Fbm3dTwoOctaveGrid, fbm_3d};

use super::{CAVE_SURFACE_CLEARANCE, OverworldRouter};

pub(super) fn contains(router: OverworldRouter, x: i32, y: i32, z: i32, surface_y: i32) -> bool {
    if y <= router.geometry.min_y().saturating_add(10)
        || y >= surface_y.saturating_sub(CAVE_SURFACE_CLEARANCE).min(32)
    {
        return false;
    }

    raw(router, x, y, z)
        && [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .any(|(dx, dz)| {
                x.checked_add(dx)
                    .zip(z.checked_add(dz))
                    .is_some_and(|(x, z)| raw(router, x, y, z))
            })
}

pub(super) fn fill_raw_layer(
    router: OverworldRouter,
    min_world_x: i32,
    min_world_z: i32,
    side: usize,
    y: i32,
    output: &mut [u8],
) {
    assert_eq!(output.len(), side * side);
    let cave_region = Fbm3dTwoOctaveGrid::new(
        min_world_x,
        min_world_z,
        side,
        y,
        [240.0, 120.0, 240.0],
        router.seed ^ 0x4341_5652,
    );
    let tunnel_a = Fbm3dTwoOctaveGrid::new(
        min_world_x,
        min_world_z,
        side,
        y,
        [34.0, 8.0, 34.0],
        router.seed ^ 0x4341_5641,
    );
    let tunnel_b = Fbm3dTwoOctaveGrid::new(
        min_world_x,
        min_world_z,
        side,
        y,
        [40.0, 10.0, 40.0],
        router.seed ^ 0x4341_5642,
    );
    for z in 0..side {
        for x in 0..side {
            let raw = cave_region.sample(x, z) >= -0.12
                && tunnel_a.sample(x, z).abs() < 0.030
                && tunnel_b.sample(x, z).abs() < 0.040;
            output[z * side + x] = if raw { 2 } else { 1 };
        }
    }
}

pub(super) fn raw(router: OverworldRouter, x: i32, y: i32, z: i32) -> bool {
    let x = f64::from(x);
    let y = f64::from(y);
    let z = f64::from(z);
    let cave_region = fbm_3d(
        x / 240.0,
        y / 120.0,
        z / 240.0,
        router.seed ^ 0x4341_5652,
        2,
        0.5,
    );
    if cave_region < -0.12 {
        return false;
    }

    let tunnel_a = fbm_3d(
        x / 34.0,
        y / 8.0,
        z / 34.0,
        router.seed ^ 0x4341_5641,
        2,
        0.5,
    );
    if tunnel_a.abs() >= 0.030 {
        return false;
    }
    let tunnel_b = fbm_3d(
        x / 40.0,
        y / 10.0,
        z / 40.0,
        router.seed ^ 0x4341_5642,
        2,
        0.5,
    );
    tunnel_b.abs() < 0.040
}
