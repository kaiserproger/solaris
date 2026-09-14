//! Terrain adaptation as a column-height analogue.
//!
//! ## What vanilla does
//!
//! `terrain_adaptation = beard_thin` is a **density** term. `Beardifier`
//! precomputes a 24³ kernel from
//! `computeBeardContribution(dx, dy, dz) = e^(-(dx² + dy² + dz²) / 16)`
//! (`BEARD_KERNEL[zi * 24 * 24 + xi * 24 + yi]`, offsets `+12`), collects one
//! `Rigid(boundingBox, terrainAdjustment, groundLevelDelta)` per
//! RIGID-projection `PoolElementStructurePiece` and every `JigsawJunction`,
//! limits itself to the union of those boxes inflated by 24, and in `compute`
//! adds, per sample `(x, y, z)`:
//!
//! - per rigid piece: `0.8 * getBeardContribution(dx, dyToGround, dz, dyToGround)`
//!   with `dx`/`dz` the axis distances outside the piece's box and
//!   `groundY = box.minY() + groundLevelDelta`;
//! - per junction: `0.4 * getBeardContribution(dx, dy, dz, dy)` around the
//!   junction's `(sourceX, sourceGroundY, sourceZ)`;
//!
//! where `getBeardContribution` is
//! `-dyWithOffset * fastInvSqrt(distanceSqr / 2) / 2 * BEARD_KERNEL[...]` with
//! `dyWithOffset = yToGround + 0.5`, or 0 outside the kernel window.
//!
//! ## What this module does instead
//!
//! Solaris terrain is a 2D per-column surface (`TerrainGenerator::surface_height`
//! is the router's `surface_y`) plus cave carving: there is no 3D density array
//! for the term to enter. The analogue keeps **the same numbers** — the same 24³
//! window, the same `e^(-d²/16)` kernel, the same 0.8/0.4 weights and the same
//! `groundY = box.minY() + groundLevelDelta` target — and applies the resulting
//! offset to the column's `surface_y` inside the affected box, pulling the
//! terrain toward each rigid piece's ground level.
//!
//! The divergence is the medium and nothing else: a future full-fidelity version
//! would have to carry a 3D density/solidity field through chunk generation and
//! evaluate [`vanilla_contribution`] there, exactly where `NoiseChunk` evaluates
//! the `BeardifierOrMarker` density function. Until then the applied offset is
//! reported per column in [`BeardColumn`], so callers can log exactly how much
//! terrain moved.

use mc_world::BlockPos;

/// `Beardifier.BEARD_KERNEL_RADIUS`.
pub const BEARD_KERNEL_RADIUS: i32 = 12;
/// `Beardifier.BEARD_KERNEL_SIZE`.
pub const BEARD_KERNEL_SIZE: usize = 24;
/// `Beardifier`'s rigid-piece weight.
pub const RIGID_WEIGHT: f64 = 0.8;
/// `Beardifier`'s junction weight.
pub const JUNCTION_WEIGHT: f64 = 0.4;

/// One rigid piece's contribution: its bounding box and ground level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeardContribution {
    pub min: BlockPos,
    pub max: BlockPos,
    pub ground_level_delta: i32,
}

/// A junction's contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeardJunction {
    pub x: i32,
    pub ground_y: i32,
    pub z: i32,
}

/// `Beardifier.compute` for one sample `(x, y, z)`: the density offset vanilla
/// would add. Kept public so a future 3D-density implementation can reuse it
/// unchanged.
#[must_use]
pub fn vanilla_contribution(
    pieces: &[BeardContribution],
    junctions: &[BeardJunction],
    x: i32,
    y: i32,
    z: i32,
) -> f64 {
    let mut value = 0.0;
    for piece in pieces {
        let dx = 0.max((piece.min.x - x).max(x - piece.max.x));
        let dz = 0.max((piece.min.z - z).max(z - piece.max.z));
        let ground_y = piece.min.y + piece.ground_level_delta;
        let dy_to_ground = y - ground_y;
        value += RIGID_WEIGHT * beard_contribution(dx, dy_to_ground, dz, dy_to_ground);
    }
    for junction in junctions {
        let dx = x - junction.x;
        let dy = y - junction.ground_y;
        let dz = z - junction.z;
        value += JUNCTION_WEIGHT * beard_contribution(dx, dy, dz, dy);
    }
    value
}

/// `Beardifier.getBeardContribution`.
#[must_use]
pub fn beard_contribution(dx: i32, dy: i32, dz: i32, y_to_ground: i32) -> f64 {
    let xi = dx + BEARD_KERNEL_RADIUS;
    let yi = dy + BEARD_KERNEL_RADIUS;
    let zi = dz + BEARD_KERNEL_RADIUS;
    if !(0..BEARD_KERNEL_SIZE as i32).contains(&xi)
        || !(0..BEARD_KERNEL_SIZE as i32).contains(&yi)
        || !(0..BEARD_KERNEL_SIZE as i32).contains(&zi)
    {
        return 0.0;
    }
    let dy_with_offset = f64::from(y_to_ground) + 0.5;
    let distance_sqr = f64::from(dx).powi(2) + dy_with_offset.powi(2) + f64::from(dz).powi(2);
    let value = -dy_with_offset * fast_inv_sqrt(distance_sqr / 2.0) / 2.0;
    value * kernel(xi as usize, yi as usize, zi as usize)
}

/// `Beardifier.computeBeardContribution(dx, dy + 0.5, dz)`.
#[must_use]
pub fn kernel_value(dx: i32, dy: i32, dz: i32) -> f64 {
    let dy = f64::from(dy) + 0.5;
    let distance_sqr = f64::from(dx).powi(2) + dy.powi(2) + f64::from(dz).powi(2);
    (-distance_sqr / 16.0).exp()
}

/// `BEARD_KERNEL[zi * 24 * 24 + xi * 24 + yi]`.
fn kernel(xi: usize, yi: usize, zi: usize) -> f64 {
    kernel_value(
        xi as i32 - BEARD_KERNEL_RADIUS,
        yi as i32 - BEARD_KERNEL_RADIUS,
        zi as i32 - BEARD_KERNEL_RADIUS,
    )
}

/// `Mth.fastInvSqrt` (`Float.intBitsToFloat(0x5f3759df - (Float.floatToRawIntBits(x) >> 1))`).
fn fast_inv_sqrt(value: f64) -> f64 {
    let x = value as f32;
    let half = x * 0.5;
    let mut i = x.to_bits();
    i = 0x5f37_59df - (i >> 1);
    let mut y = f32::from_bits(i);
    // One Newton step, as vanilla's `Mth.fastInvSqrt` does.
    y *= 1.5 - half * y * y;
    f64::from(y)
}

/// One column the analogue moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeardColumn {
    pub x: i32,
    pub z: i32,
    /// The column's unmodified surface.
    pub surface_y: i32,
    /// The surface after the analogue.
    pub adjusted_y: i32,
}

/// One plan's beard inputs: its RIGID pieces and its junctions.
pub type BeardSource<'a> = (&'a [BeardContribution], &'a [BeardJunction]);

/// Apply the analogue over `columns` (`(x, z, surface_y)`), returning the moved
/// columns. A column moves only when the analogue is non-zero and at least one
/// block, and is clamped to `min_y + 1 ..= max_y - 1`.
///
/// Vanilla's `Beardifier` collects the contributions of *every* structure the
/// chunk references and sums them into one density offset, so several plans can
/// reach one column: [`apply_columns_over`] sums their contributions and rounds
/// once, and this single-source call is that same function with one source.
#[must_use]
pub fn apply_columns(
    pieces: &[BeardContribution],
    junctions: &[BeardJunction],
    columns: &[(i32, i32, i32)],
    min_y: i32,
    max_y: i32,
) -> Vec<BeardColumn> {
    apply_columns_over(&[(pieces, junctions)], columns, min_y, max_y)
}

/// [`apply_columns`] over several plans at once: the offsets of every source are
/// summed per column, then rounded and clamped once, as vanilla sums the
/// contributions before the terrain is decided.
#[must_use]
pub fn apply_columns_over(
    sources: &[BeardSource<'_>],
    columns: &[(i32, i32, i32)],
    min_y: i32,
    max_y: i32,
) -> Vec<BeardColumn> {
    let mut moved = Vec::new();
    for (x, z, surface_y) in columns {
        let offset: f64 = sources
            .iter()
            .map(|(pieces, junctions)| vanilla_contribution(pieces, junctions, *x, *surface_y, *z))
            .sum();
        if offset == 0.0 {
            continue;
        }
        let adjusted = (*surface_y + offset.round() as i32).clamp(min_y + 1, max_y - 1);
        if adjusted != *surface_y {
            moved.push(BeardColumn {
                x: *x,
                z: *z,
                surface_y: *surface_y,
                adjusted_y: adjusted,
            });
        }
    }
    moved
}

/// The union of the pieces' and junctions' boxes inflated by
/// `BEARD_KERNEL_RADIUS`, vanilla's `affectedBox`.
#[must_use]
pub fn affected_box(
    pieces: &[BeardContribution],
    junctions: &[BeardJunction],
) -> Option<(BlockPos, BlockPos)> {
    let mut bounds: Option<(BlockPos, BlockPos)> = None;
    let mut include = |min: BlockPos, max: BlockPos| {
        bounds = Some(match bounds {
            None => (min, max),
            Some((current_min, current_max)) => (
                BlockPos {
                    x: current_min.x.min(min.x),
                    y: current_min.y.min(min.y),
                    z: current_min.z.min(min.z),
                },
                BlockPos {
                    x: current_max.x.max(max.x),
                    y: current_max.y.max(max.y),
                    z: current_max.z.max(max.z),
                },
            ),
        });
    };
    for piece in pieces {
        include(piece.min, piece.max);
    }
    for junction in junctions {
        let pos = BlockPos {
            x: junction.x,
            y: junction.ground_y,
            z: junction.z,
        };
        include(pos, pos);
    }
    bounds.map(|(min, max)| {
        (
            BlockPos {
                x: min.x - BEARD_KERNEL_RADIUS,
                y: min.y - BEARD_KERNEL_RADIUS,
                z: min.z - BEARD_KERNEL_RADIUS,
            },
            BlockPos {
                x: max.x + BEARD_KERNEL_RADIUS,
                y: max.y + BEARD_KERNEL_RADIUS,
                z: max.z + BEARD_KERNEL_RADIUS,
            },
        )
    })
}
