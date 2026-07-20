use serde::{Deserialize, Serialize};

/// Fixed output capacity covering the full positive-byte piercing path.
///
/// Longer non-discarding hit sequences return a typed prepare error.
pub const MAX_PUBLICATIONS: usize = 520;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct EntityIdentity(u128);

impl EntityIdentity {
    #[must_use]
    pub const fn new(raw: u128) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u128 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct EntityId(i32);

impl EntityId {
    #[must_use]
    pub const fn new(raw: i32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct BlockStateId(u32);

impl BlockStateId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    #[must_use]
    pub fn subtract(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    #[must_use]
    pub fn scale(self, scale: f64) -> Self {
        Self::new(self.x * scale, self.y * scale, self.z * scale)
    }

    #[must_use]
    pub fn multiply(self, scale: Self) -> Self {
        Self::new(self.x * scale.x, self.y * scale.y, self.z * scale.z)
    }

    #[must_use]
    pub fn horizontal_distance(self) -> f64 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    #[must_use]
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[must_use]
    pub fn distance_squared(self, other: Self) -> f64 {
        other.subtract(self).length_squared()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
}

impl Rotation {
    #[must_use]
    pub const fn new(yaw: f32, pitch: f32) -> Self {
        Self { yaw, pitch }
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.yaw.is_finite() && self.pitch.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl Aabb {
    /// Builds a canonical finite box, swapping inverted endpoints like vanilla.
    ///
    /// Vanilla's constructor also retains NaN and infinite coordinates. This
    /// bounded kernel rejects them explicitly because downstream state and
    /// rotation policy requires finite geometry.
    pub fn new(
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> Result<Self, GeometryError> {
        for (field, value) in [
            (GeometryField::MinX, min_x),
            (GeometryField::MinY, min_y),
            (GeometryField::MinZ, min_z),
            (GeometryField::MaxX, max_x),
            (GeometryField::MaxY, max_y),
            (GeometryField::MaxZ, max_z),
        ] {
            if value.is_nan() {
                return Err(GeometryError::NaN(field));
            }
            if value.is_infinite() {
                return Err(GeometryError::Infinite(field));
            }
        }
        Ok(Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            min_z: min_z.min(max_z),
            max_x: min_x.max(max_x),
            max_y: min_y.max(max_y),
            max_z: min_z.max(max_z),
        })
    }

    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.min_x < other.max_x
            && self.max_x > other.min_x
            && self.min_y < other.max_y
            && self.max_y > other.min_y
            && self.min_z < other.max_z
            && self.max_z > other.min_z
    }

    pub(crate) fn expand_towards(self, movement: Vec3) -> Result<Self, GeometryError> {
        Self::new(
            self.min_x + movement.x.min(0.0),
            self.min_y + movement.y.min(0.0),
            self.min_z + movement.z.min(0.0),
            self.max_x + movement.x.max(0.0),
            self.max_y + movement.y.max(0.0),
            self.max_z + movement.z.max(0.0),
        )
    }

    pub(crate) fn inflate(self, amount: f64) -> Result<Self, GeometryError> {
        Self::new(
            self.min_x - amount,
            self.min_y - amount,
            self.min_z - amount,
            self.max_x + amount,
            self.max_y + amount,
            self.max_z + amount,
        )
    }

    pub(crate) fn move_by(self, movement: Vec3) -> Result<Self, GeometryError> {
        Self::new(
            self.min_x + movement.x,
            self.min_y + movement.y,
            self.min_z + movement.z,
            self.max_x + movement.x,
            self.max_y + movement.y,
            self.max_z + movement.z,
        )
    }

    fn validate(self) -> Result<(), GeometryError> {
        let canonical = Self::new(
            self.min_x, self.min_y, self.min_z, self.max_x, self.max_y, self.max_z,
        )?;
        if self.min_x > self.max_x {
            return Err(GeometryError::NonCanonical(GeometryAxis::X));
        }
        if self.min_y > self.max_y {
            return Err(GeometryError::NonCanonical(GeometryAxis::Y));
        }
        if self.min_z > self.max_z {
            return Err(GeometryError::NonCanonical(GeometryAxis::Z));
        }
        debug_assert_eq!(self, canonical);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryField {
    MinX,
    MinY,
    MinZ,
    MaxX,
    MaxY,
    MaxZ,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryAxis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    NaN(GeometryField),
    Infinite(GeometryField),
    NonCanonical(GeometryAxis),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectileLifecycle {
    Active,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectileState {
    pub revision: u64,
    pub owner: Option<EntityIdentity>,
    pub left_owner: bool,
    pub has_been_shot: bool,
    pub last_deflected_by: Option<EntityId>,
    pub position: Vec3,
    pub bounds: Aabb,
    pub velocity: Vec3,
    pub rotation: Rotation,
    pub lifecycle: ProjectileLifecycle,
}

impl ProjectileState {
    pub fn new(
        owner: Option<EntityIdentity>,
        position: Vec3,
        bounds: Aabb,
        velocity: Vec3,
        rotation: Rotation,
    ) -> Result<Self, StateError> {
        if !position.is_finite() {
            return Err(StateError::NonFinite(StateField::Position));
        }
        if !velocity.is_finite() {
            return Err(StateError::NonFinite(StateField::Velocity));
        }
        if !rotation.is_finite() {
            return Err(StateError::NonFinite(StateField::Rotation));
        }
        Ok(Self {
            revision: 0,
            owner,
            left_owner: false,
            has_been_shot: false,
            last_deflected_by: None,
            position,
            bounds,
            velocity,
            rotation,
            lifecycle: ProjectileLifecycle::Active,
        })
    }

    pub(crate) fn validate(self) -> Result<(), StateError> {
        if !self.position.is_finite() {
            return Err(StateError::NonFinite(StateField::Position));
        }
        if !self.velocity.is_finite() {
            return Err(StateError::NonFinite(StateField::Velocity));
        }
        if !self.rotation.is_finite() {
            return Err(StateError::NonFinite(StateField::Rotation));
        }
        self.bounds.validate().map_err(StateError::InvalidBounds)?;
        Ok(())
    }

    pub(crate) fn set_position(&mut self, position: Vec3) -> Result<(), StateError> {
        if !position.is_finite() {
            return Err(StateError::NonFinite(StateField::Position));
        }
        let movement = position.subtract(self.position);
        self.bounds = self
            .bounds
            .move_by(movement)
            .map_err(StateError::InvalidBounds)?;
        self.position = position;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateField {
    Position,
    Bounds,
    Velocity,
    Rotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    NonFinite(StateField),
    InvalidBounds(GeometryError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnerVehicleMember {
    pub pickable: bool,
    pub bounds: Aabb,
}

#[derive(Debug, Clone, Copy)]
pub struct OwnerCollisionInput<'a> {
    pub resolved_owner: Option<EntityIdentity>,
    /// Complete root-vehicle self/passenger membership in deterministic order.
    ///
    /// This slice is streamed without retention. A caller that cannot provide
    /// complete membership must defer left-owner evaluation for this tick.
    pub vehicle_members: &'a [OwnerVehicleMember],
}

impl<'a> OwnerCollisionInput<'a> {
    #[must_use]
    pub const fn missing() -> Self {
        Self {
            resolved_owner: None,
            vehicle_members: &[],
        }
    }

    #[must_use]
    pub const fn resolved(
        owner: EntityIdentity,
        vehicle_members: &'a [OwnerVehicleMember],
    ) -> Self {
        Self {
            resolved_owner: Some(owner),
            vehicle_members,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerInputError {
    OwnerMismatch {
        expected: EntityIdentity,
        actual: EntityIdentity,
    },
    UnexpectedResolvedOwner(EntityIdentity),
    MembersWithoutResolvedOwner,
    InvalidVehicleMemberBounds {
        index: usize,
        error: GeometryError,
    },
    InvalidProjectileState(StateError),
    SweptBoundsOverflow,
}

pub fn outside_owner_collision_range(
    state: &ProjectileState,
    input: OwnerCollisionInput<'_>,
) -> Result<bool, OwnerInputError> {
    validate_owner_collision_input(state, input)?;
    if input.resolved_owner.is_none() {
        return Ok(true);
    }
    let swept = state
        .bounds
        .expand_towards(state.velocity)
        .and_then(|bounds| bounds.inflate(1.0))
        .map_err(|_| OwnerInputError::SweptBoundsOverflow)?;
    Ok(input
        .vehicle_members
        .iter()
        .filter(|member| member.pickable)
        .all(|member| !swept.intersects(member.bounds)))
}

pub fn validate_owner_collision_input(
    state: &ProjectileState,
    input: OwnerCollisionInput<'_>,
) -> Result<(), OwnerInputError> {
    state
        .validate()
        .map_err(OwnerInputError::InvalidProjectileState)?;
    match (state.owner, input.resolved_owner) {
        (Some(expected), Some(actual)) if expected != actual => {
            return Err(OwnerInputError::OwnerMismatch { expected, actual });
        }
        (None, Some(actual)) => {
            return Err(OwnerInputError::UnexpectedResolvedOwner(actual));
        }
        (_, None) if !input.vehicle_members.is_empty() => {
            return Err(OwnerInputError::MembersWithoutResolvedOwner);
        }
        (_, None) => return Ok(()),
        _ => {}
    }
    for (index, member) in input.vehicle_members.iter().enumerate() {
        if member.pickable {
            member
                .bounds
                .validate()
                .map_err(|error| OwnerInputError::InvalidVehicleMemberBounds { index, error })?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionTransition {
    pub from: u64,
    pub to: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputStamp {
    pub world_revision: u64,
    pub collision_revision: u64,
    pub resolution_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickCommitError {
    StaleState { expected: u64, actual: u64 },
    StateChangedAtRevision { revision: u64 },
    StaleWorld { expected: u64, actual: u64 },
    StaleCollisions { expected: u64, actual: u64 },
    StaleResolutions { expected: u64, actual: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerMutation {
    pub previous: Option<EntityIdentity>,
    pub current: Option<EntityIdentity>,
    pub revision: RevisionTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationError {
    StaleState { expected: u64, actual: u64 },
    RevisionExhausted,
}

pub fn assign_owner(
    state: &mut ProjectileState,
    expected_revision: u64,
    owner: Option<EntityIdentity>,
) -> Result<OwnerMutation, MutationError> {
    if state.revision != expected_revision {
        return Err(MutationError::StaleState {
            expected: expected_revision,
            actual: state.revision,
        });
    }
    let next_revision = state
        .revision
        .checked_add(1)
        .ok_or(MutationError::RevisionExhausted)?;
    let previous = state.owner;
    state.owner = owner;
    state.revision = next_revision;
    Ok(OwnerMutation {
        previous,
        current: owner,
        revision: RevisionTransition {
            from: expected_revision,
            to: next_revision,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectilePublication {
    ProjectileShot {
        owner: Option<EntityIdentity>,
    },
    EntityImpact {
        entity: EntityId,
        location: Vec3,
    },
    BlockImpact {
        block_state: BlockStateId,
        location: Vec3,
    },
    ProjectileLandedEntity {
        entity: EntityId,
        location: Vec3,
    },
    ProjectileLandedBlock {
        block_state: BlockStateId,
        location: Vec3,
    },
    Deflected {
        by: Option<EntityId>,
        applied: bool,
    },
    ArrowDamageAccepted {
        entity: EntityId,
        killed_living: bool,
        enderman: bool,
    },
    ArrowDamageRejected {
        entity: EntityId,
    },
    PickupItemRequested,
    Discarded {
        reason: DiscardReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    DespawnAge,
    EntityHit,
    PiercingLimit,
    RejectedHitStopped,
    PickedUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationError {
    CapacityExceeded { capacity: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PublicationBatch {
    entries: [Option<ProjectilePublication>; MAX_PUBLICATIONS],
    len: usize,
}

impl PublicationBatch {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PUBLICATIONS],
            len: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, publication: ProjectilePublication) -> Result<(), PublicationError> {
        if self.len == MAX_PUBLICATIONS {
            return Err(PublicationError::CapacityExceeded {
                capacity: MAX_PUBLICATIONS,
            });
        }
        self.entries[self.len] = Some(publication);
        self.len += 1;
        Ok(())
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<ProjectilePublication> {
        if index >= self.len {
            None
        } else {
            self.entries[index]
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = ProjectilePublication> + '_ {
        self.entries[..self.len].iter().copied().flatten()
    }
}

impl Default for PublicationBatch {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn move_projectile(
    state: &mut ProjectileState,
    position: Vec3,
) -> Result<(), StateError> {
    state.set_position(position)
}

pub(crate) fn resolved_owner(
    state: &ProjectileState,
    input: OwnerCollisionInput<'_>,
) -> Option<EntityIdentity> {
    input
        .resolved_owner
        .filter(|identity| Some(*identity) == state.owner)
}

pub(crate) fn vanilla_atan2(mut y: f64, mut x: f64) -> f64 {
    let squared = x * x + y * y;
    if squared.is_nan() {
        return f64::NAN;
    }
    let negative_y = y < 0.0;
    if negative_y {
        y = -y;
    }
    let negative_x = x < 0.0;
    if negative_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }

    let half = 0.5 * squared;
    let bits = 6_910_469_410_427_058_090_u64.wrapping_sub(squared.to_bits() >> 1);
    let inverse = f64::from_bits(bits);
    let inverse = inverse * (1.5 - half * inverse * inverse);
    x *= inverse;
    y *= inverse;

    const FRAC_BIAS: f64 = f64::from_bits(4_805_340_802_404_319_232_u64);
    let biased = FRAC_BIAS + y;
    let index = (biased.to_bits() as u32) as usize;
    let value = index as f64 / 256.0;
    let phi = value.asin();
    let cosine = phi.cos();
    let sine = biased - FRAC_BIAS;
    let delta = y * cosine - x * sine;
    let correction = (6.0 + delta * delta) * delta * (1.0 / 6.0);
    let mut angle = phi + correction;
    if steep {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if negative_x {
        angle = std::f64::consts::PI - angle;
    }
    if negative_y {
        angle = -angle;
    }
    angle
}

pub(crate) fn target_rotation(velocity: Vec3, reverse_yaw: bool) -> Rotation {
    let horizontal = velocity.horizontal_distance();
    let yaw = if reverse_yaw {
        vanilla_atan2(-velocity.x, -velocity.z)
    } else {
        vanilla_atan2(velocity.x, velocity.z)
    };
    let degrees = f64::from(180.0_f32) / f64::from(std::f32::consts::PI);
    Rotation::new(
        (yaw * degrees) as f32,
        (vanilla_atan2(velocity.y, horizontal) * degrees) as f32,
    )
}

pub(crate) fn lerp_rotation(mut previous: f32, target: f32) -> Option<f32> {
    let difference = target - previous;
    if !difference.is_finite() {
        return None;
    }
    if difference < -180.0 {
        if previous - 360.0 == previous {
            return None;
        }
        let turns = ((-180.0 - difference) / 360.0).ceil();
        previous -= turns * 360.0;
    } else if difference >= 180.0 {
        if previous + 360.0 == previous {
            return None;
        }
        let turns = ((difference - 180.0) / 360.0).floor() + 1.0;
        previous += turns * 360.0;
    }

    let adjusted_difference = target - previous;
    if !(-180.0..180.0).contains(&adjusted_difference) {
        return None;
    }
    let result = previous + 0.2_f32 * adjusted_difference;
    result.is_finite().then_some(result)
}
