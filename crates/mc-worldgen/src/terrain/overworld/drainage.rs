use std::cmp::Ordering;

use crate::noise::fbm_2d;

const BASE_CELL_BLOCKS: f64 = 128.0;
const MIN_CELL_BLOCKS: f64 = 32.0;
const ACCUMULATION_DEPTH: u8 = 1;
const MIN_CHANNEL_ACCUMULATION: f64 = 0.7;
const FULL_CHANNEL_ACCUMULATION: f64 = 2.5;
const MIN_CHANNEL_WIDTH_BLOCKS: f64 = 16.0;
const MAX_CHANNEL_WIDTH_BLOCKS: f64 = 32.0;
// Jitter removes the axis/diagonal lattice alignment; a smooth bend between
// shared endpoints avoids replacing one ruler-straight reach with two.
const ANCHOR_JITTER_FRACTION: f64 = 0.30;
const CHANNEL_BEND_FRACTION: f64 = 0.30;
const CHANNEL_CURVE_STEPS: usize = 8;
const FLOW_POTENTIAL_CENTER_RADIUS_CELLS: f64 = 24.0;
const FLOW_POTENTIAL_NORMALIZATION: f64 = 13_824.0;
const BASIN_FIELD_SCALE_CELLS: f64 = 11.0;
const BASIN_WARP_SCALE_CELLS: f64 = 23.0;
const BASIN_WARP_CELLS: f64 = 2.5;
const NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DrainageSample {
    pub(super) channel_weight: f64,
    pub(super) channel_strength: f64,
    pub(super) river_distance: f64,
    pub(super) accumulation: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DrainageCell {
    pub(super) x: i32,
    pub(super) z: i32,
}

pub(super) fn sample(
    seed: i64,
    block_x: i32,
    block_z: i32,
    scale: f64,
    minimum_bank_width: f64,
) -> DrainageSample {
    let cell_blocks = cell_blocks(scale);
    let point_x = f64::from(block_x);
    let point_z = f64::from(block_z);
    let cell = cell_at(point_x, point_z, cell_blocks);
    let mut best = DrainageSample {
        channel_weight: 0.0,
        channel_strength: 0.0,
        river_distance: 1.0,
        accumulation: 0.0,
    };

    // The curve lies in the convex hull of its endpoints and quadratic
    // control point. Include the relief-dependent bank width so the search
    // cannot clip a reach at a drainage-cell boundary.
    let extent = (1.0 + ANCHOR_JITTER_FRACTION)
        .max(0.5 + ANCHOR_JITTER_FRACTION + 2.0 * CHANNEL_BEND_FRACTION)
        * cell_blocks
        + (MAX_CHANNEL_WIDTH_BLOCKS * 1.25 * (cell_blocks / BASE_CELL_BLOCKS).sqrt())
            .max(minimum_bank_width);
    let radius = (extent / cell_blocks + 0.5).floor() as i32;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let from = DrainageCell {
                x: cell.x.saturating_add(dx),
                z: cell.z.saturating_add(dz),
            };
            let (center_x, center_z) = cell_center(from, cell_blocks);
            if (point_x - center_x).abs() > extent || (point_z - center_z).abs() > extent {
                continue;
            }
            evaluate_segment(
                seed,
                from,
                point_x,
                point_z,
                cell_blocks,
                minimum_bank_width,
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
    minimum_bank_width: f64,
    best: &mut DrainageSample,
) {
    // Shared cell anchors and the directed reach determine the whole curve,
    // independently of generation order and chunk borders.
    let (from_x, from_z) = cell_anchor(seed, from, cell_blocks);
    let to = downstream(seed, from);
    let (to_x, to_z) = cell_anchor(seed, to, cell_blocks);
    let axis_x = to_x - from_x;
    let axis_z = to_z - from_z;
    let length = axis_x.hypot(axis_z);
    let (bend_x, bend_z) = if length > f64::EPSILON {
        let salt =
            0x4348_4245_4E44 ^ (from.x as u64).rotate_left(17) ^ (from.z as u64).rotate_left(41);
        let bend =
            signed_unit(cell_hash(seed, to.x, to.z, salt)) * CHANNEL_BEND_FRACTION * cell_blocks;
        (-axis_z / length * bend, axis_x / length * bend)
    } else {
        (0.0, 0.0)
    };
    let width_scale = (cell_blocks / BASE_CELL_BLOCKS).sqrt();
    let maximum_width = (MAX_CHANNEL_WIDTH_BLOCKS * 1.25 * width_scale).max(minimum_bank_width);
    if point_x < from_x.min(to_x) - bend_x.abs() - maximum_width
        || point_x > from_x.max(to_x) + bend_x.abs() + maximum_width
        || point_z < from_z.min(to_z) - bend_z.abs() - maximum_width
        || point_z > from_z.max(to_z) + bend_z.abs() + maximum_width
    {
        return;
    }
    let mut distance = f64::INFINITY;
    let (mut previous_x, mut previous_z) = (from_x, from_z);
    for step in 1..=CHANNEL_CURVE_STEPS {
        let t = step as f64 / CHANNEL_CURVE_STEPS as f64;
        let bend_weight = 4.0 * t * (1.0 - t);
        let curve_x = from_x + axis_x * t + bend_x * bend_weight;
        let curve_z = from_z + axis_z * t + bend_z * bend_weight;
        distance = distance.min(point_segment_distance(
            point_x, point_z, previous_x, previous_z, curve_x, curve_z,
        ));
        (previous_x, previous_z) = (curve_x, curve_z);
    }
    if distance >= maximum_width {
        return;
    }

    let accumulation = accumulation(seed, from, ACCUMULATION_DEPTH);
    let strength = accumulation_strength(accumulation) * basin_weight(seed, from);
    if strength <= 0.0 {
        return;
    }
    // Keep the seeded base width, widening taller banks rather than cutting cliffs.
    let width = ((MIN_CHANNEL_WIDTH_BLOCKS + accumulation * 1.05)
        .clamp(MIN_CHANNEL_WIDTH_BLOCKS, MAX_CHANNEL_WIDTH_BLOCKS)
        * (0.80 + unit(cell_hash(seed, from.x, from.z, 0x5749_4454_4848)) * 0.45)
        * width_scale)
        .max(minimum_bank_width * strength);
    let proximity = 1.0 - smootherstep((distance / width).clamp(0.0, 1.0));
    let channel_weight = proximity * strength;
    if channel_weight > best.channel_weight {
        best.channel_weight = channel_weight;
        best.channel_strength = strength;
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

/// Jittered reach endpoint for a drainage cell. The offset is a pure function
/// of the cell, so neighbouring chunks agree on every shared reach without
/// any cross-chunk state.
fn cell_anchor(seed: i64, cell: DrainageCell, cell_blocks: f64) -> (f64, f64) {
    let (center_x, center_z) = cell_center(cell, cell_blocks);
    let jitter = ANCHOR_JITTER_FRACTION * cell_blocks;
    (
        center_x + signed_unit(cell_hash(seed, cell.x, cell.z, 0x414E_4348_5858)) * jitter,
        center_z + signed_unit(cell_hash(seed, cell.x, cell.z, 0x414E_4348_5A5A)) * jitter,
    )
}

pub(super) fn downstream(seed: i64, cell: DrainageCell) -> DrainageCell {
    let elevation = hydraulic_elevation(seed, cell);
    NEIGHBOR_OFFSETS
        .into_iter()
        .map(|(dx, dz)| DrainageCell {
            x: cell.x.saturating_add(dx),
            z: cell.z.saturating_add(dz),
        })
        .filter(|candidate| hydraulic_elevation(seed, *candidate) < elevation)
        .min_by(|left, right| {
            branch_score(seed, cell, *left)
                .partial_cmp(&branch_score(seed, cell, *right))
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.x.cmp(&right.x))
                .then_with(|| left.z.cmp(&right.z))
        })
        .expect("radial drainage potential always exposes a downhill neighbour")
}

pub(super) fn accumulation(seed: i64, cell: DrainageCell, depth: u8) -> f64 {
    let mut total = local_runoff(seed, cell);
    if depth == 0 {
        return total;
    }
    for upstream in upstream_candidates(cell) {
        if downstream(seed, upstream) == cell {
            total += accumulation(seed, upstream, depth - 1);
        }
    }
    total
}

pub(super) fn hydraulic_elevation(seed: i64, cell: DrainageCell) -> f64 {
    let (center_x, center_z, phase) = flow_potential_identity(seed);
    let x = f64::from(cell.x) + 0.5 - center_x;
    let z = f64::from(cell.z) + 0.5 - center_z;
    let real = x * x * x - 3.0 * x * z * z;
    let imaginary = 3.0 * x * x * z - z * z * z;
    (real * phase.cos() + imaginary * phase.sin()) / FLOW_POTENTIAL_NORMALIZATION
}

#[cfg(test)]
pub(super) fn configured_cell_blocks(scale: f64) -> f64 {
    cell_blocks(scale)
}

fn upstream_candidates(cell: DrainageCell) -> [DrainageCell; 8] {
    NEIGHBOR_OFFSETS.map(|(dx, dz)| DrainageCell {
        x: cell.x.saturating_add(dx),
        z: cell.z.saturating_add(dz),
    })
}

fn flow_potential_identity(seed: i64) -> (f64, f64, f64) {
    let x = signed_unit(mix64(seed as u64 ^ 0x4452_4149_4E58_5352))
        * FLOW_POTENTIAL_CENTER_RADIUS_CELLS;
    let z = signed_unit(mix64(seed as u64 ^ 0x4452_4149_4E5A_5352))
        * FLOW_POTENTIAL_CENTER_RADIUS_CELLS;
    let phase = unit(mix64(seed as u64 ^ 0x4452_4149_4E50_4841)) * std::f64::consts::TAU;
    (x, z, phase)
}

fn branch_score(seed: i64, from: DrainageCell, to: DrainageCell) -> f64 {
    let hash = cell_hash(
        seed,
        to.x,
        to.z,
        0x4252_414E ^ (from.x as u64).rotate_left(17) ^ (from.z as u64).rotate_left(41),
    );
    // The hydraulic elevation is a single smooth global cubic, so neighbouring
    // cells share near-identical downhill directions and the old 0.01 jitter
    // let that coherence through as long parallel rivers. The jitter only
    // breaks ties between already-downhill candidates, so raising it cannot
    // create uphill flow or local minima; it just varies which downhill
    // branch each cell takes.
    hydraulic_elevation(seed, to) + signed_unit(hash) * 0.22
}

fn basin_weight(seed: i64, cell: DrainageCell) -> f64 {
    let x = f64::from(cell.x);
    let z = f64::from(cell.z);
    let warp_x = fbm_2d(
        x / BASIN_WARP_SCALE_CELLS,
        z / BASIN_WARP_SCALE_CELLS,
        seed ^ 0x4241_5357_4152_5058,
        2,
        0.5,
    ) * BASIN_WARP_CELLS;
    let warp_z = fbm_2d(
        x / BASIN_WARP_SCALE_CELLS,
        z / BASIN_WARP_SCALE_CELLS,
        seed ^ 0x4241_5357_4152_505A,
        2,
        0.5,
    ) * BASIN_WARP_CELLS;
    let value = fbm_2d(
        (x + warp_x + z * 0.37) / BASIN_FIELD_SCALE_CELLS,
        (z + warp_z - x * 0.23) / BASIN_FIELD_SCALE_CELLS,
        seed ^ 0x4241_5349_4E32_4446,
        3,
        0.52,
    );
    0.72 + smootherstep(remap(value, -0.45, 0.45)) * 0.28
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
    accumulation_strength(accumulation) * basin_weight(seed, cell) >= 0.10
}

fn local_runoff(seed: i64, cell: DrainageCell) -> f64 {
    // Keep runoff slowly varying so channel depth does not jump at cell joins.
    0.88 + basin_weight(seed, cell) * 0.20
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
#[path = "drainage_geometry_tests.rs"]
mod geometry_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downstream_strictly_decreases_hydraulic_elevation() {
        for seed in [-17, 0, 712_816, i64::MAX] {
            for z in -64..=64 {
                for x in -64..=64 {
                    let cell = DrainageCell { x, z };
                    let next = downstream(seed, cell);
                    assert!(
                        hydraulic_elevation(seed, next) < hydraulic_elevation(seed, cell),
                        "{cell:?} did not drain downhill to {next:?}"
                    );
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
    fn drainage_samples_are_deterministic_and_seed_sensitive() {
        let fingerprint = |seed| {
            (-1_024..=1_024)
                .step_by(31)
                .flat_map(|z| {
                    (-1_024..=1_024).step_by(29).map(move |x| {
                        let value = sample(seed, x, z, 1.0, 0.0);
                        (
                            value.channel_weight.to_bits(),
                            value.river_distance.to_bits(),
                            value.accumulation.to_bits(),
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        let first = fingerprint(712_816);
        assert_eq!(first, fingerprint(712_816));
        assert_ne!(first, fingerprint(712_817));
    }

    #[test]
    fn confluences_increase_local_accumulation() {
        let seed = 712_816;
        let mut confluences = 0usize;
        for z in -32..=32 {
            for x in -32..=32 {
                let cell = DrainageCell { x, z };
                let incoming = upstream_candidates(cell)
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
                    assert!((0.72..=11.52).contains(&first), "{first}");
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
                let current = sample(seed, x, z, scale, 0.0).channel_weight;
                maximum_step = maximum_step
                    .max((current - sample(seed, x + 1, z, scale, 0.0).channel_weight).abs())
                    .max((current - sample(seed, x, z + 1, scale, 0.0).channel_weight).abs());
            }
        }
        assert!(cell_blocks >= MIN_CELL_BLOCKS);
        assert!(
            maximum_step <= 0.42,
            "channel step is too abrupt: {maximum_step}"
        );
    }

    #[test]
    fn drainage_sampling_is_chunk_order_and_border_independent() {
        let coordinates = [
            (15, 15),
            (16, 15),
            (15, 16),
            (16, 16),
            (-17, -17),
            (-16, -17),
            (-17, -16),
            (-16, -16),
        ];
        let fingerprint = |coordinates: &[(i32, i32)]| {
            coordinates
                .iter()
                .map(|&(x, z)| {
                    let value = sample(712_816, x, z, 1.0, 0.0);
                    (
                        (x, z),
                        (
                            value.channel_weight.to_bits(),
                            value.river_distance.to_bits(),
                            value.accumulation.to_bits(),
                        ),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let mut reversed = coordinates;
        reversed.reverse();
        assert_eq!(fingerprint(&coordinates), fingerprint(&reversed));
    }

    #[test]
    fn downstream_flow_uses_every_octant_across_region() {
        // A single smooth global potential drains whole regions along
        // near-identical headings, which renders as long parallel rivers.
        // Every 45-degree octant must carry flow somewhere in the region.
        for seed in [0, 7, 712_816, 5_617_830] {
            let mut octants = [false; 8];
            for z in (-96..=96).step_by(3) {
                for x in (-96..=96).step_by(3) {
                    let cell = DrainageCell { x, z };
                    let next = downstream(seed, cell);
                    let index = match (next.x - cell.x, next.z - cell.z) {
                        (-1, -1) => 0,
                        (0, -1) => 1,
                        (1, -1) => 2,
                        (-1, 0) => 3,
                        (1, 0) => 4,
                        (-1, 1) => 5,
                        (0, 1) => 6,
                        (1, 1) => 7,
                        _ => continue,
                    };
                    octants[index] = true;
                }
            }
            assert!(
                octants.iter().all(|used| *used),
                "seed {seed} leaves flow octants unused: {octants:?}"
            );
        }
    }

    #[test]
    fn channel_centrelines_bend_between_cells() {
        // Reaches rendered centre-to-centre are straight by construction.
        // Walk downstream paths and require the jittered anchors to deviate
        // laterally from the straight chord: that deviation is the meander.
        let cell_blocks = configured_cell_blocks(1.0);
        for seed in [0, 712_816, 5_617_830] {
            let mut bending_paths = 0usize;
            let mut paths = 0usize;
            for z in (-64..=64).step_by(7) {
                for x in (-64..=64).step_by(7) {
                    let mut cell = DrainageCell { x, z };
                    let mut anchors = Vec::with_capacity(10);
                    for _ in 0..10 {
                        anchors.push(cell_anchor(seed, cell, cell_blocks));
                        cell = downstream(seed, cell);
                    }
                    let (start_x, start_z) = anchors[0];
                    let (end_x, end_z) = anchors[anchors.len() - 1];
                    let chord_x = end_x - start_x;
                    let chord_z = end_z - start_z;
                    let chord_length = chord_x.hypot(chord_z);
                    if chord_length <= cell_blocks {
                        continue;
                    }
                    paths += 1;
                    let deviation = anchors[1..anchors.len() - 1]
                        .iter()
                        .map(|(ax, az)| {
                            ((az - start_z) * chord_x - (ax - start_x) * chord_z).abs()
                                / chord_length
                        })
                        .fold(0.0_f64, f64::max);
                    if deviation >= 0.10 * cell_blocks {
                        bending_paths += 1;
                    }
                }
            }
            assert!(paths > 32, "seed {seed} sampled only {paths} paths");
            assert!(
                bending_paths * 2 >= paths,
                "seed {seed}: only {bending_paths}/{paths} downstream paths meander"
            );
        }
    }
}
