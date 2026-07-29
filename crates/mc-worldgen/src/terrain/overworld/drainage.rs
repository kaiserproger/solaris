use std::cmp::Ordering;

const BASE_CELL_BLOCKS: f64 = 128.0;
const MIN_CELL_BLOCKS: f64 = 32.0;
const ACCUMULATION_DEPTH: u8 = 2;
const MIN_CHANNEL_ACCUMULATION: f64 = 2.0;
const FULL_CHANNEL_ACCUMULATION: f64 = 8.5;
const BASIN_SPACING_CELLS: i64 = 12;
const BASIN_CORE_CELLS: f64 = 0.6;
const BASIN_EDGE_CELLS: f64 = 2.4;
const MIN_CHANNEL_WIDTH_BLOCKS: f64 = 5.0;
const MAX_CHANNEL_WIDTH_BLOCKS: f64 = 22.0;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DrainageSample {
    pub(super) channel_weight: f64,
    pub(super) river_distance: f64,
    pub(super) accumulation: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DrainageCell {
    pub(super) x: i32,
    pub(super) z: i32,
}

pub(super) fn sample(seed: i64, block_x: i32, block_z: i32, scale: f64) -> DrainageSample {
    let cell_blocks = cell_blocks(scale);
    let point_x = f64::from(block_x);
    let point_z = f64::from(block_z);
    let cell = cell_at(point_x, point_z, cell_blocks);
    let mut best = DrainageSample {
        channel_weight: 0.0,
        river_distance: 1.0,
        accumulation: 0.0,
    };

    let upstream = upstream_candidates(seed, cell);
    let mandatory = [cell, upstream[0], upstream[1], upstream[2]];
    for source in mandatory {
        evaluate_segment(
            seed,
            source,
            point_x,
            point_z,
            cell_blocks,
            ACCUMULATION_DEPTH,
            &mut best,
        );
    }

    let maximum_reach = MAX_CHANNEL_WIDTH_BLOCKS * (cell_blocks / BASE_CELL_BLOCKS).sqrt();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let source = DrainageCell {
                x: cell.x.saturating_add(dx),
                z: cell.z.saturating_add(dz),
            };
            if mandatory.contains(&source)
                || point_cell_distance(point_x, point_z, source, cell_blocks) > maximum_reach
            {
                continue;
            }
            evaluate_segment(
                seed,
                source,
                point_x,
                point_z,
                cell_blocks,
                ACCUMULATION_DEPTH,
                &mut best,
            );
        }
    }
    best
}

fn evaluate_segment(
    seed: i64,
    from: DrainageCell,
    point_x: f64,
    point_z: f64,
    cell_blocks: f64,
    accumulation_depth: u8,
    best: &mut DrainageSample,
) {
    let basin = basin_weight(seed, from);
    if basin <= 0.0 {
        return;
    }

    let to = downstream(seed, from);
    let (from_x, from_z) = cell_center(from, cell_blocks);
    let (to_x, to_z) = cell_center(to, cell_blocks);
    let distance = point_segment_distance(point_x, point_z, from_x, from_z, to_x, to_z);
    let width_scale = (cell_blocks / BASE_CELL_BLOCKS).sqrt();
    if distance >= MAX_CHANNEL_WIDTH_BLOCKS * width_scale {
        return;
    }

    let accumulation = accumulation(seed, from, accumulation_depth);
    let strength = accumulation_strength(accumulation) * basin;
    if strength <= 0.0 {
        return;
    }
    let width = (MIN_CHANNEL_WIDTH_BLOCKS + accumulation * 1.05)
        .clamp(MIN_CHANNEL_WIDTH_BLOCKS, MAX_CHANNEL_WIDTH_BLOCKS)
        * width_scale;
    let proximity = 1.0 - smootherstep((distance / width).clamp(0.0, 1.0));
    let channel_weight = proximity * strength;
    if channel_weight > best.channel_weight {
        best.channel_weight = channel_weight;
        best.river_distance = 0.10 * (1.0 - channel_weight);
        best.accumulation = accumulation;
    }
}

fn cell_blocks(scale: f64) -> f64 {
    (BASE_CELL_BLOCKS * scale).max(MIN_CELL_BLOCKS)
}

fn cell_at(x: f64, z: f64, cell_blocks: f64) -> DrainageCell {
    DrainageCell {
        x: (x / cell_blocks).floor() as i32,
        z: (z / cell_blocks).floor() as i32,
    }
}

pub(super) fn cell_center(cell: DrainageCell, cell_blocks: f64) -> (f64, f64) {
    (
        (f64::from(cell.x) + 0.5) * cell_blocks,
        (f64::from(cell.z) + 0.5) * cell_blocks,
    )
}

pub(super) fn downstream(seed: i64, cell: DrainageCell) -> DrainageCell {
    let offsets = forward_offsets(flow_direction(seed));
    offsets
        .into_iter()
        .map(|(dx, dz)| DrainageCell {
            x: cell.x.saturating_add(dx),
            z: cell.z.saturating_add(dz),
        })
        .min_by(|left, right| {
            branch_score(seed, cell, *left)
                .partial_cmp(&branch_score(seed, cell, *right))
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.x.cmp(&right.x))
                .then_with(|| left.z.cmp(&right.z))
        })
        .expect("drainage has three forward candidates")
}

pub(super) fn accumulation(seed: i64, cell: DrainageCell, depth: u8) -> f64 {
    let mut total = local_runoff(seed, cell);
    if depth == 0 {
        return total;
    }
    for upstream in upstream_candidates(seed, cell) {
        if downstream(seed, upstream) == cell {
            total += accumulation(seed, upstream, depth - 1);
        }
    }
    total
}

#[cfg(test)]
pub(super) fn hydraulic_rank(seed: i64, cell: DrainageCell) -> i64 {
    let (dx, dz) = flow_direction(seed);
    i64::from(cell.x) * i64::from(dx) + i64::from(cell.z) * i64::from(dz)
}

#[cfg(test)]
pub(super) fn configured_cell_blocks(scale: f64) -> f64 {
    cell_blocks(scale)
}

fn upstream_candidates(seed: i64, cell: DrainageCell) -> [DrainageCell; 3] {
    forward_offsets(flow_direction(seed)).map(|(dx, dz)| DrainageCell {
        x: cell.x.saturating_sub(dx),
        z: cell.z.saturating_sub(dz),
    })
}

fn flow_direction(seed: i64) -> (i32, i32) {
    const DIRECTIONS: [(i32, i32); 8] = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];
    DIRECTIONS[(mix64(seed as u64 ^ 0x4452_4149_4E41_4745) & 7) as usize]
}

fn forward_offsets((dx, dz): (i32, i32)) -> [(i32, i32); 3] {
    match (dx, dz) {
        (0, dz) => [(0, dz), (1, dz), (-1, dz)],
        (dx, 0) => [(dx, 0), (dx, 1), (dx, -1)],
        (dx, dz) => [(dx, dz), (dx, 0), (0, dz)],
    }
}

fn branch_score(seed: i64, from: DrainageCell, to: DrainageCell) -> f64 {
    let hash = cell_hash(
        seed,
        to.x,
        to.z,
        0x4252_414E ^ (from.x as u64).rotate_left(17) ^ (from.z as u64).rotate_left(41),
    );
    basin_distance_cells(seed, to) * 0.82 + signed_unit(hash) * 0.36
}

fn basin_weight(seed: i64, cell: DrainageCell) -> f64 {
    1.0 - smootherstep(remap(
        basin_distance_cells(seed, cell),
        BASIN_CORE_CELLS,
        BASIN_EDGE_CELLS,
    ))
}

fn accumulation_strength(accumulation: f64) -> f64 {
    smootherstep(remap(
        accumulation,
        MIN_CHANNEL_ACCUMULATION,
        FULL_CHANNEL_ACCUMULATION,
    ))
}

#[cfg(test)]
pub(super) fn active_cell(seed: i64, cell: DrainageCell) -> bool {
    let accumulation = accumulation(seed, cell, ACCUMULATION_DEPTH);
    accumulation_strength(accumulation) * basin_weight(seed, cell) >= 0.55
}

fn basin_distance_cells(seed: i64, cell: DrainageCell) -> f64 {
    let (dx, dz) = flow_direction(seed);
    let cross = i64::from(cell.x) * -i64::from(dz) + i64::from(cell.z) * i64::from(dx);
    let offset = (mix64(seed as u64 ^ 0x4241_5349_4E4F_4646) % BASIN_SPACING_CELLS as u64) as i64;
    let wrapped = (cross - offset).rem_euclid(BASIN_SPACING_CELLS);
    wrapped.min(BASIN_SPACING_CELLS - wrapped) as f64
}

fn local_runoff(seed: i64, cell: DrainageCell) -> f64 {
    0.72 + unit(cell_hash(seed, cell.x, cell.z, 0x5255_4E4F_4646)) * 0.56
}

fn cell_hash(seed: i64, x: i32, z: i32, salt: u64) -> u64 {
    let mut value = seed as u64 ^ salt;
    value ^= (x as u32 as u64).wrapping_mul(0x9E37_79B1_85EB_CA87);
    value = mix64(value);
    value ^= (z as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    mix64(value)
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn unit(value: u64) -> f64 {
    let mantissa = value >> 11;
    mantissa as f64 / ((1u64 << 53) - 1) as f64
}

fn signed_unit(value: u64) -> f64 {
    unit(value) * 2.0 - 1.0
}

fn point_cell_distance(px: f64, pz: f64, cell: DrainageCell, cell_blocks: f64) -> f64 {
    let min_x = f64::from(cell.x) * cell_blocks;
    let max_x = min_x + cell_blocks;
    let min_z = f64::from(cell.z) * cell_blocks;
    let max_z = min_z + cell_blocks;
    let dx = if px < min_x {
        min_x - px
    } else if px > max_x {
        px - max_x
    } else {
        0.0
    };
    let dz = if pz < min_z {
        min_z - pz
    } else if pz > max_z {
        pz - max_z
    } else {
        0.0
    };
    dx.hypot(dz)
}

fn point_segment_distance(px: f64, pz: f64, ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let ab_x = bx - ax;
    let ab_z = bz - az;
    let length_squared = ab_x * ab_x + ab_z * ab_z;
    if length_squared <= f64::EPSILON {
        return (px - ax).hypot(pz - az);
    }
    let projection = (((px - ax) * ab_x + (pz - az) * ab_z) / length_squared).clamp(0.0, 1.0);
    let closest_x = ax + ab_x * projection;
    let closest_z = az + ab_z * projection;
    (px - closest_x).hypot(pz - closest_z)
}

fn remap(value: f64, low: f64, high: f64) -> f64 {
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn smootherstep(value: f64) -> f64 {
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downstream_strictly_increases_hydraulic_rank() {
        for seed in [-17, 0, 712_816, i64::MAX] {
            for z in -32..=32 {
                for x in -32..=32 {
                    let cell = DrainageCell { x, z };
                    let next = downstream(seed, cell);
                    assert!(hydraulic_rank(seed, next) > hydraulic_rank(seed, cell));
                }
            }
        }
    }

    #[test]
    fn different_seeds_change_the_coarse_flow_network() {
        let fingerprint = |seed| {
            (-12..=12)
                .flat_map(|z| (-12..=12).map(move |x| DrainageCell { x, z }))
                .map(|cell| downstream(seed, cell))
                .collect::<Vec<_>>()
        };
        assert_ne!(fingerprint(0), fingerprint(1));
        assert_ne!(fingerprint(0), fingerprint(712_816));
    }

    #[test]
    fn confluences_increase_local_accumulation() {
        let seed = 712_816;
        let mut confluences = 0usize;
        for z in -32..=32 {
            for x in -32..=32 {
                let cell = DrainageCell { x, z };
                let incoming = upstream_candidates(seed, cell)
                    .into_iter()
                    .filter(|upstream| downstream(seed, *upstream) == cell)
                    .count();
                if incoming < 2 {
                    continue;
                }
                confluences += 1;
                assert!(
                    accumulation(seed, cell, ACCUMULATION_DEPTH) > local_runoff(seed, cell) + 1.4,
                    "confluence {cell:?} did not accumulate upstream runoff"
                );
            }
        }
        assert!(confluences > 32, "sampled only {confluences} confluences");
    }

    #[test]
    fn accumulation_is_deterministic_and_bounded() {
        for seed in [-1, 0, 712_816] {
            for z in -24..=24 {
                for x in -24..=24 {
                    let cell = DrainageCell { x, z };
                    let first = accumulation(seed, cell, ACCUMULATION_DEPTH);
                    let second = accumulation(seed, cell, ACCUMULATION_DEPTH);
                    assert_eq!(first.to_bits(), second.to_bits());
                    assert!((0.72..=16.64).contains(&first), "{first}");
                }
            }
        }
    }

    #[test]
    fn channel_weight_is_continuous_across_cell_boundaries() {
        let seed = 712_816;
        let scale = 1.0;
        let cell_blocks = configured_cell_blocks(scale);
        let mut maximum_step = 0.0_f64;
        for z in (-1_024..=1_024).step_by(17) {
            for x in (-1_024..=1_024).step_by(17) {
                let current = sample(seed, x, z, scale).channel_weight;
                maximum_step = maximum_step
                    .max((current - sample(seed, x + 1, z, scale).channel_weight).abs())
                    .max((current - sample(seed, x, z + 1, scale).channel_weight).abs());
            }
        }
        assert!(cell_blocks >= MIN_CELL_BLOCKS);
        assert!(
            maximum_step <= 0.42,
            "channel step is too abrupt: {maximum_step}"
        );
    }
}
