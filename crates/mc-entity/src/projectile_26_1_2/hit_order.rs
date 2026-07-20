use std::cmp::Ordering;

use super::{BlockStateId, EntityId, EntityIdentity, ProjectileState, Vec3};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CandidateWork {
    pub candidates: usize,
    pub duplicate_adjacencies_checked: usize,
    pub hit_candidates_visited: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitEligibility {
    pub can_be_hit_by_projectile: bool,
    /// AbstractArrow's player-owner/player-target PvP result.
    ///
    /// Generic projectile eligibility intentionally ignores this field.
    pub arrow_pvp_permitted: bool,
    pub shares_owner_vehicle: bool,
}

impl HitEligibility {
    pub(crate) fn permits_projectile(
        self,
        projectile: &ProjectileState,
        resolved_owner: Option<EntityIdentity>,
    ) -> bool {
        self.can_be_hit_by_projectile
            && (resolved_owner.is_none() || projectile.left_owner || !self.shares_owner_vehicle)
    }

    pub(crate) fn permits_arrow(
        self,
        projectile: &ProjectileState,
        resolved_owner: Option<EntityIdentity>,
    ) -> bool {
        self.arrow_pvp_permitted && self.permits_projectile(projectile, resolved_owner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedDeflection {
    pub velocity: Vec3,
    pub yaw_delta: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityHitResolution {
    Impact,
    Deflected(ResolvedDeflection),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThrowableEntityHit {
    pub entity: EntityId,
    pub location: Vec3,
    pub eligibility: HitEligibility,
    pub resolution: EntityHitResolution,
    /// Kernel-owned stable tie key, overwritten from the supplied slice order.
    pub input_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHit {
    pub block_state: BlockStateId,
    pub location: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitTarget {
    Miss,
    Entity {
        entity: EntityId,
        location: Vec3,
    },
    Block {
        block_state: BlockStateId,
        location: Vec3,
    },
}

pub(crate) fn compare_distance(origin: Vec3, left: Vec3, right: Vec3) -> Ordering {
    let left_distance = finite_distance(origin, left);
    let right_distance = finite_distance(origin, right);
    match (left_distance.is_finite(), right_distance.is_finite()) {
        (true, true) => left_distance.total_cmp(&right_distance),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            let scale = [
                origin.x.abs(),
                origin.y.abs(),
                origin.z.abs(),
                left.x.abs(),
                left.y.abs(),
                left.z.abs(),
                right.x.abs(),
                right.y.abs(),
                right.z.abs(),
            ]
            .into_iter()
            .fold(0.0_f64, f64::max);
            scaled_distance(origin, left, scale).total_cmp(&scaled_distance(origin, right, scale))
        }
    }
}

pub(crate) fn strictly_before(origin: Vec3, point: Vec3, endpoint: Vec3) -> bool {
    compare_distance(origin, point, endpoint) == Ordering::Less
}

fn finite_distance(origin: Vec3, point: Vec3) -> f64 {
    let x = point.x - origin.x;
    let y = point.y - origin.y;
    let z = point.z - origin.z;
    x.hypot(y).hypot(z)
}

fn scaled_distance(origin: Vec3, point: Vec3, scale: f64) -> f64 {
    if scale == 0.0 {
        return 0.0;
    }
    let x = point.x / scale - origin.x / scale;
    let y = point.y / scale - origin.y / scale;
    let z = point.z / scale - origin.z / scale;
    x.hypot(y).hypot(z)
}

pub(crate) fn select_throwable_entity(
    projectile: &ProjectileState,
    resolved_owner: Option<EntityIdentity>,
    block_hit: Option<BlockHit>,
    candidates: &[ThrowableEntityHit],
) -> (Option<ThrowableEntityHit>, usize) {
    let mut visited = 0;
    for candidate in candidates {
        visited += 1;
        if !candidate
            .eligibility
            .permits_projectile(projectile, resolved_owner)
        {
            continue;
        }
        if block_hit.is_some_and(|block| {
            !strictly_before(projectile.position, candidate.location, block.location)
        }) {
            continue;
        }
        return (Some(*candidate), visited);
    }
    (None, visited)
}
