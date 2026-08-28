//! Protocol-neutral chunk-stream coordinate planning for Java Edition 26.1.2.

/// Returns every chunk in Chebyshev-ring order around the center.
/// The caller supplies its own policy ceiling so this module does not depend on
/// protocol or transport limits.
#[must_use]
pub fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    max_view_distance: i32,
) -> Vec<(i32, i32)> {
    let vd = view_distance.clamp(0, max_view_distance.max(0));
    let diameter = (2 * vd + 1) as usize;
    let mut out = Vec::with_capacity(diameter * diameter);
    out.push((center_x, center_z));

    for radius in 1..=vd {
        for dx in -radius..=radius {
            out.push((center_x + dx, center_z - radius));
        }
        for dz in (-radius + 1)..radius {
            out.push((center_x - radius, center_z + dz));
            out.push((center_x + radius, center_z + dz));
        }
        for dx in -radius..=radius {
            out.push((center_x + dx, center_z + radius));
        }
    }

    out
}

#[must_use]
pub fn forward_from_yaw(yaw: f32) -> (f64, f64) {
    let radians = f64::from(yaw).to_radians();
    (-radians.sin(), radians.cos())
}

#[must_use]
pub fn directional_score(dx: i32, dz: i32, forward_x: f64, forward_z: f64) -> f64 {
    f64::from(dx) * forward_x + f64::from(dz) * forward_z
}

#[must_use]
pub fn directional_lateral(dx: i32, dz: i32, forward_x: f64, forward_z: f64) -> f64 {
    (f64::from(dx) * forward_z - f64::from(dz) * forward_x).abs()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedChunkPriority {
    pub ring: u32,
    pub sequence: u32,
}

#[must_use]
pub fn prioritized_spiral(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    max_view_distance: i32,
    direction_yaw: f32,
) -> Vec<(i32, i32, PlannedChunkPriority)> {
    prioritize_chunks(
        center_x,
        center_z,
        direction_yaw,
        spiral_chunks(center_x, center_z, view_distance, max_view_distance),
    )
}

#[must_use]
pub fn prioritize_chunks(
    center_x: i32,
    center_z: i32,
    direction_yaw: f32,
    mut chunks: Vec<(i32, i32)>,
) -> Vec<(i32, i32, PlannedChunkPriority)> {
    let (forward_x, forward_z) = forward_from_yaw(direction_yaw);
    chunks.sort_by(|&(left_x, left_z), &(right_x, right_z)| {
        let left_dx = left_x - center_x;
        let left_dz = left_z - center_z;
        let right_dx = right_x - center_x;
        let right_dz = right_z - center_z;
        let left_ring = left_dx.abs().max(left_dz.abs());
        let right_ring = right_dx.abs().max(right_dz.abs());
        left_ring
            .cmp(&right_ring)
            .then_with(|| {
                directional_score(right_dx, right_dz, forward_x, forward_z)
                    .total_cmp(&directional_score(left_dx, left_dz, forward_x, forward_z))
            })
            .then_with(|| {
                directional_lateral(left_dx, left_dz, forward_x, forward_z).total_cmp(
                    &directional_lateral(right_dx, right_dz, forward_x, forward_z),
                )
            })
            .then_with(|| left_z.cmp(&right_z))
            .then_with(|| left_x.cmp(&right_x))
    });
    chunks
        .into_iter()
        .enumerate()
        .map(|(sequence, (cx, cz))| {
            (
                cx,
                cz,
                PlannedChunkPriority {
                    ring: (cx - center_x).abs().max((cz - center_z).abs()) as u32,
                    sequence: sequence as u32,
                },
            )
        })
        .collect()
}

/// Plans the full ring one chunk beyond the active view, front edge first and
/// then the opposite edge, with the remainder ordered by view direction.
#[must_use]
pub fn prewarm_edge_ring_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    max_view_distance: i32,
    direction_yaw: f32,
) -> Vec<(i32, i32)> {
    let vd = view_distance.clamp(0, max_view_distance.max(0));
    let radius = vd + 1;
    let (forward_x, forward_z) = forward_from_yaw(direction_yaw);
    let mut chunks = Vec::new();
    if forward_x.abs() > forward_z.abs() {
        let forward_sign = if forward_x.is_sign_negative() { -1 } else { 1 };
        push_x_edge(&mut chunks, center_x, center_z, radius, vd, forward_sign);
        push_x_edge(&mut chunks, center_x, center_z, radius, vd, -forward_sign);
    } else {
        let forward_sign = if forward_z.is_sign_negative() { -1 } else { 1 };
        push_z_edge(&mut chunks, center_x, center_z, radius, vd, forward_sign);
        push_z_edge(&mut chunks, center_x, center_z, radius, vd, -forward_sign);
    }

    let mut remaining = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx.abs().max(dz.abs()) == radius {
                remaining.push((center_x + dx, center_z + dz));
            }
        }
    }
    remaining.sort_by(|&(left_x, left_z), &(right_x, right_z)| {
        let left_dx = left_x - center_x;
        let left_dz = left_z - center_z;
        let right_dx = right_x - center_x;
        let right_dz = right_z - center_z;
        directional_score(right_dx, right_dz, forward_x, forward_z)
            .total_cmp(&directional_score(left_dx, left_dz, forward_x, forward_z))
            .then_with(|| {
                directional_lateral(left_dx, left_dz, forward_x, forward_z).total_cmp(
                    &directional_lateral(right_dx, right_dz, forward_x, forward_z),
                )
            })
            .then_with(|| left_z.cmp(&right_z))
            .then_with(|| left_x.cmp(&right_x))
    });
    for chunk in remaining {
        push_unique(&mut chunks, chunk);
    }
    chunks
}

#[must_use]
pub fn prewarm_edge_batch_limit(
    view_distance: i32,
    max_view_distance: i32,
    edge_ring_limit: usize,
) -> usize {
    let vd = view_distance.clamp(0, max_view_distance.max(0)) as usize;
    if vd == 0 {
        return 0;
    }
    (3 * (2 * vd + 1)).min(edge_ring_limit)
}

#[must_use]
pub fn prewarm_edge_batch_chunks(
    center: (i32, i32),
    view_distance: i32,
    max_view_distance: i32,
    direction_yaw: f32,
    player: (f64, f64),
    edge_ring_limit: usize,
) -> Vec<(i32, i32)> {
    let (center_x, center_z) = center;
    let (player_x, player_z) = player;
    let vd = view_distance.clamp(0, max_view_distance.max(0));
    if vd == 0 {
        return Vec::new();
    }
    let radius = vd + 1;
    let (forward_x, forward_z) = forward_from_yaw(direction_yaw);
    let limit = prewarm_edge_batch_limit(vd, max_view_distance, edge_ring_limit);
    let mut chunks = Vec::with_capacity(limit);
    if forward_x.abs() > forward_z.abs() {
        let forward_sign = if forward_x.is_sign_negative() { -1 } else { 1 };
        let local_z = player_z - f64::from(center_z) * 16.0;
        let lateral_sign = if local_z <= 8.0 { -1 } else { 1 };
        let local_x = player_x - f64::from(center_x) * 16.0;
        let mut edges = [
            (
                distance_to_signed_chunk_edge(local_x, forward_sign),
                0u8,
                true,
                forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_x, -forward_sign),
                2u8,
                true,
                -forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_z, lateral_sign),
                1u8,
                false,
                lateral_sign,
            ),
        ];
        edges.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (_, _, x_edge, sign) in edges {
            if x_edge {
                push_x_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            } else {
                push_z_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            }
        }
    } else {
        let forward_sign = if forward_z.is_sign_negative() { -1 } else { 1 };
        let local_x = player_x - f64::from(center_x) * 16.0;
        let lateral_sign = if local_x <= 8.0 { -1 } else { 1 };
        let local_z = player_z - f64::from(center_z) * 16.0;
        let mut edges = [
            (
                distance_to_signed_chunk_edge(local_z, forward_sign),
                0u8,
                false,
                forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_z, -forward_sign),
                2u8,
                false,
                -forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_x, lateral_sign),
                1u8,
                true,
                lateral_sign,
            ),
        ];
        edges.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (_, _, x_edge, sign) in edges {
            if x_edge {
                push_x_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            } else {
                push_z_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            }
        }
    }

    if chunks.len() < limit {
        for chunk in
            prewarm_edge_ring_chunks(center_x, center_z, vd, max_view_distance, direction_yaw)
        {
            push_unique(&mut chunks, chunk);
            if chunks.len() == limit {
                break;
            }
        }
    }
    chunks
}

#[must_use]
pub fn initial_window_target(view_distance: i32, minimum_ring: i32) -> usize {
    let ring = view_distance.clamp(0, minimum_ring.max(0)) as usize;
    (2 * ring + 1).pow(2)
}

#[must_use]
pub fn desired_chunk_set(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    max_view_distance: i32,
) -> std::collections::HashSet<(i32, i32)> {
    spiral_chunks(center_x, center_z, view_distance, max_view_distance)
        .into_iter()
        .collect()
}

#[must_use]
pub fn distance_to_signed_chunk_edge(local: f64, sign: i32) -> f64 {
    let local = local.clamp(0.0, 16.0);
    if sign < 0 { local } else { 16.0 - local }
}

fn push_z_edge(
    chunks: &mut Vec<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
    view_distance: i32,
    sign: i32,
) {
    let edge_z = center_z + sign * radius;
    for dx in -view_distance..=view_distance {
        push_unique(chunks, (center_x + dx, edge_z));
    }
}

fn push_x_edge(
    chunks: &mut Vec<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
    view_distance: i32,
    sign: i32,
) {
    let edge_x = center_x + sign * radius;
    for dz in -view_distance..=view_distance {
        push_unique(chunks, (edge_x, center_z + dz));
    }
}

fn push_unique(chunks: &mut Vec<(i32, i32)>, chunk: (i32, i32)) {
    if !chunks.contains(&chunk) {
        chunks.push(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_covers_each_chunk_once_and_clamps_to_policy_limit() {
        let chunks = spiral_chunks(5, -2, 9, 2);
        assert_eq!(chunks.len(), 25);
        assert_eq!(chunks[0], (5, -2));
        let unique = chunks
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), chunks.len());
        assert!(
            chunks
                .iter()
                .all(|(x, z)| (x - 5).abs().max((z + 2).abs()) <= 2)
        );
    }

    #[test]
    fn prioritized_spiral_prefers_forward_chunks_within_the_same_ring() {
        let chunks = prioritized_spiral(0, 0, 1, 32, 0.0);
        assert_eq!(
            chunks[0],
            (
                0,
                0,
                PlannedChunkPriority {
                    ring: 0,
                    sequence: 0
                }
            )
        );
        let forward = chunks
            .iter()
            .position(|(x, z, _)| (*x, *z) == (0, 1))
            .unwrap();
        let backward = chunks
            .iter()
            .position(|(x, z, _)| (*x, *z) == (0, -1))
            .unwrap();
        assert!(forward < backward);
        assert!(
            chunks
                .iter()
                .enumerate()
                .all(|(index, (_, _, priority))| priority.sequence == index as u32)
        );
    }

    #[test]
    fn prewarm_ring_is_unique_and_one_chunk_beyond_view() {
        let chunks = prewarm_edge_ring_chunks(0, 0, 2, 32, 0.0);
        assert_eq!(chunks.len(), 24);
        let unique = chunks
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), chunks.len());
        assert!(chunks.iter().all(|(x, z)| x.abs().max(z.abs()) == 3));
    }

    #[test]
    fn prewarm_batch_respects_limit_and_nearest_lateral_edge() {
        let chunks = prewarm_edge_batch_chunks((0, 0), 4, 32, 0.0, (0.5, 8.5), 40);
        assert_eq!(chunks.len(), prewarm_edge_batch_limit(4, 32, 40));
        assert!(chunks.iter().all(|(x, z)| x.abs().max(z.abs()) == 5));
        assert!(
            chunks.contains(&(-5, 0)),
            "near x=0 should include the west edge early"
        );
        assert_eq!(prewarm_edge_batch_limit(0, 32, 40), 0);
        assert_eq!(initial_window_target(8, 2), 25);
    }

    #[test]
    fn desired_set_matches_spiral_coverage() {
        let desired = desired_chunk_set(3, -4, 2, 32);
        assert_eq!(desired.len(), 25);
        assert!(desired.contains(&(3, -4)));
        assert!(desired.contains(&(5, -2)));
        assert!(!desired.contains(&(6, -4)));
    }

    #[test]
    fn directional_helpers_follow_minecraft_yaw_and_edge_distance() {
        let (x, z) = forward_from_yaw(0.0);
        assert!(x.abs() < f64::EPSILON);
        assert!((z - 1.0).abs() < f64::EPSILON);
        assert_eq!(directional_score(0, 2, x, z), 2.0);
        assert_eq!(distance_to_signed_chunk_edge(3.0, -1), 3.0);
        assert_eq!(distance_to_signed_chunk_edge(3.0, 1), 13.0);
    }
}
