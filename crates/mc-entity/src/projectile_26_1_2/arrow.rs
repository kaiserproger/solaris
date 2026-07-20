use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    BlockStateId, CandidateWork, DiscardReason, EntityId, EntityIdentity, HitEligibility,
    InputStamp, OwnerCollisionInput, OwnerInputError, ProjectileLifecycle, ProjectilePublication,
    ProjectileState, PublicationBatch, ResolvedDeflection, RevisionTransition, StateError,
    TickCommitError, Vec3, compare_distance, lerp_rotation, move_projectile,
    outside_owner_collision_range, resolved_owner, strictly_before, target_rotation,
    validate_owner_collision_input,
};

pub const ARROW_DESPAWN_TICKS: i32 = 1_200;
pub const MAX_PIERCED_ENTITIES: usize = 128;
const WATER_INERTIA: f64 = 0.6_f32 as f64;
const AIR_INERTIA: f64 = 0.99_f32 as f64;
const ARROW_GRAVITY: f64 = 0.05;
const BLOCK_EMBED_OFFSET: f64 = 0.05_f32 as f64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPosition {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickupMode {
    Disallowed,
    Allowed,
    CreativeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerRecord {
    Inserted,
    AlreadyPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiercingLedgerError {
    CapacityExceeded { capacity: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiercingLedger {
    entries: [Option<EntityId>; MAX_PIERCED_ENTITIES],
    len: usize,
}

impl PiercingLedger {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PIERCED_ENTITIES],
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

    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.entries[..self.len].contains(&Some(entity))
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<EntityId> {
        if index < self.len {
            self.entries[index]
        } else {
            None
        }
    }

    pub fn record(&mut self, entity: EntityId) -> Result<LedgerRecord, PiercingLedgerError> {
        if self.contains(entity) {
            return Ok(LedgerRecord::AlreadyPresent);
        }
        if self.len == MAX_PIERCED_ENTITIES {
            return Err(PiercingLedgerError::CapacityExceeded {
                capacity: MAX_PIERCED_ENTITIES,
            });
        }
        self.entries[self.len] = Some(entity);
        self.len += 1;
        Ok(LedgerRecord::Inserted)
    }

    pub fn clear(&mut self) {
        self.entries[..self.len].fill(None);
        self.len = 0;
    }
}

impl Default for PiercingLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for PiercingLedger {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.len))?;
        for entity in self.entries[..self.len].iter().flatten() {
            sequence.serialize_element(entity)?;
        }
        sequence.end()
    }
}

struct PiercingLedgerVisitor;

impl<'de> Visitor<'de> for PiercingLedgerVisitor {
    type Value = PiercingLedger;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("at most 128 distinct projectile entity ids")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut ledger = PiercingLedger::new();
        while let Some(entity) = sequence.next_element::<EntityId>()? {
            if ledger.len == MAX_PIERCED_ENTITIES {
                return Err(de::Error::invalid_length(MAX_PIERCED_ENTITIES + 1, &self));
            }
            if ledger.contains(entity) {
                return Err(de::Error::custom("duplicate pierced entity"));
            }
            ledger.entries[ledger.len] = Some(entity);
            ledger.len += 1;
        }
        Ok(ledger)
    }
}

impl<'de> Deserialize<'de> for PiercingLedger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(PiercingLedgerVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArrowState {
    pub projectile: ProjectileState,
    pub in_ground: bool,
    pub no_physics: bool,
    pub no_gravity: bool,
    pub in_ground_time: i32,
    pub shake_time: i32,
    pub despawn_age: i32,
    pub pickup: PickupMode,
    pub pierce_level: i8,
    pub pierced_entities: PiercingLedger,
    pub pierced_and_killed: PiercingLedger,
    pub last_block_state: Option<BlockStateId>,
    pub last_block_position: Option<BlockPosition>,
}

impl ArrowState {
    #[must_use]
    pub const fn new(projectile: ProjectileState, pickup: PickupMode, pierce_level: i8) -> Self {
        Self {
            projectile,
            in_ground: false,
            no_physics: false,
            no_gravity: false,
            in_ground_time: 0,
            shake_time: 0,
            despawn_age: 0,
            pickup,
            pierce_level,
            pierced_entities: PiercingLedger::new(),
            pierced_and_killed: PiercingLedger::new(),
            last_block_state: None,
            last_block_position: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowOwnerKind {
    Player,
    OminousItemSpawner,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowOwner {
    pub identity: EntityIdentity,
    pub kind: ArrowOwnerKind,
}

impl ArrowOwner {
    #[must_use]
    pub const fn new(identity: EntityIdentity, kind: ArrowOwnerKind) -> Self {
        Self { identity, kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowMutationError {
    StaleState { expected: u64, actual: u64 },
    RevisionExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowOwnerMutation {
    pub previous: Option<EntityIdentity>,
    pub current: Option<EntityIdentity>,
    pub pickup: PickupMode,
    pub revision: RevisionTransition,
}

pub fn assign_arrow_owner(
    state: &mut ArrowState,
    expected_revision: u64,
    owner: Option<ArrowOwner>,
) -> Result<ArrowOwnerMutation, ArrowMutationError> {
    ensure_arrow_revision(state, expected_revision)?;
    let next_revision = state
        .projectile
        .revision
        .checked_add(1)
        .ok_or(ArrowMutationError::RevisionExhausted)?;
    let previous = state.projectile.owner;
    let identity = owner.map(|owner| owner.identity);
    let pickup = match owner.map(|owner| owner.kind) {
        Some(ArrowOwnerKind::Player) if state.pickup == PickupMode::Disallowed => {
            PickupMode::Allowed
        }
        Some(ArrowOwnerKind::OminousItemSpawner) => PickupMode::Disallowed,
        _ => state.pickup,
    };
    state.projectile.owner = identity;
    state.pickup = pickup;
    state.projectile.revision = next_revision;
    Ok(ArrowOwnerMutation {
        previous,
        current: identity,
        pickup,
        revision: RevisionTransition {
            from: expected_revision,
            to: next_revision,
        },
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickupInput {
    pub has_infinite_materials: bool,
    pub inventory_inserted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupRejection {
    Discarded,
    NotAccessible,
    Shaking,
    Disallowed,
    InventoryRejected,
    RequiresInfiniteMaterials,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickupOutcome {
    Rejected(PickupRejection),
    PickedUp {
        revision: RevisionTransition,
        publication: ProjectilePublication,
    },
}

pub fn pickup_arrow(
    state: &mut ArrowState,
    expected_revision: u64,
    input: PickupInput,
) -> Result<PickupOutcome, ArrowMutationError> {
    ensure_arrow_revision(state, expected_revision)?;
    let rejection = if state.projectile.lifecycle == ProjectileLifecycle::Discarded {
        Some(PickupRejection::Discarded)
    } else if !state.in_ground && !state.no_physics {
        Some(PickupRejection::NotAccessible)
    } else if state.shake_time > 0 {
        Some(PickupRejection::Shaking)
    } else {
        match state.pickup {
            PickupMode::Disallowed => Some(PickupRejection::Disallowed),
            PickupMode::Allowed if !input.inventory_inserted => {
                Some(PickupRejection::InventoryRejected)
            }
            PickupMode::CreativeOnly if !input.has_infinite_materials => {
                Some(PickupRejection::RequiresInfiniteMaterials)
            }
            PickupMode::Allowed | PickupMode::CreativeOnly => None,
        }
    };
    if let Some(rejection) = rejection {
        return Ok(PickupOutcome::Rejected(rejection));
    }

    let next_revision = expected_revision
        .checked_add(1)
        .ok_or(ArrowMutationError::RevisionExhausted)?;
    state.projectile.lifecycle = ProjectileLifecycle::Discarded;
    state.projectile.revision = next_revision;
    Ok(PickupOutcome::PickedUp {
        revision: RevisionTransition {
            from: expected_revision,
            to: next_revision,
        },
        publication: ProjectilePublication::Discarded {
            reason: DiscardReason::PickedUp,
        },
    })
}

fn ensure_arrow_revision(
    state: &ArrowState,
    expected_revision: u64,
) -> Result<(), ArrowMutationError> {
    if state.projectile.revision != expected_revision {
        return Err(ArrowMutationError::StaleState {
            expected: expected_revision,
            actual: state.projectile.revision,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrowMotionUpdate {
    Shot {
        velocity: Vec3,
        rotation: super::Rotation,
    },
    Lerp {
        velocity: Vec3,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowMotionError {
    StaleState { expected: u64, actual: u64 },
    RevisionExhausted,
    InvalidState(StateError),
    NonFiniteVelocity,
    NonFiniteRotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowMotionOutcome {
    pub revision: RevisionTransition,
    pub velocity_applied: bool,
    pub cleared_in_ground: bool,
}

pub fn update_arrow_motion(
    state: &mut ArrowState,
    expected_revision: u64,
    update: ArrowMotionUpdate,
) -> Result<ArrowMotionOutcome, ArrowMotionError> {
    if state.projectile.revision != expected_revision {
        return Err(ArrowMotionError::StaleState {
            expected: expected_revision,
            actual: state.projectile.revision,
        });
    }
    state
        .projectile
        .validate()
        .map_err(ArrowMotionError::InvalidState)?;
    let (velocity, rotation, velocity_applied) = match update {
        ArrowMotionUpdate::Shot { velocity, rotation } => {
            if !velocity.is_finite() {
                return Err(ArrowMotionError::NonFiniteVelocity);
            }
            (velocity, Some(rotation), true)
        }
        ArrowMotionUpdate::Lerp { velocity } => (velocity, None, velocity.is_finite()),
    };
    if rotation.is_some_and(|rotation| !rotation.is_finite()) {
        return Err(ArrowMotionError::NonFiniteRotation);
    }
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or(ArrowMotionError::RevisionExhausted)?;

    let mut next = *state;
    if velocity_applied {
        next.projectile.velocity = velocity;
    }
    if let Some(rotation) = rotation {
        next.projectile.rotation = rotation;
    }
    next.despawn_age = 0;
    let cleared_in_ground = matches!(update, ArrowMotionUpdate::Lerp { .. })
        && next.in_ground
        && velocity.length_squared() > 0.0;
    if cleared_in_ground {
        next.in_ground = false;
    }
    next.projectile.revision = next_revision;
    *state = next;
    Ok(ArrowMotionOutcome {
        revision: RevisionTransition {
            from: expected_revision,
            to: next_revision,
        },
        velocity_applied,
        cleared_in_ground,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrowDamageResolution {
    Accepted {
        enderman: bool,
        living: bool,
        killed: bool,
    },
    Rejected {
        reverse: ResolvedDeflection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrowEntityResolution {
    Deflected(ResolvedDeflection),
    Damage(ArrowDamageResolution),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowEntityHit {
    pub entity: EntityId,
    pub location: Vec3,
    pub entity_position: Vec3,
    pub eligibility: HitEligibility,
    pub resolution: ArrowEntityResolution,
    /// Kernel-owned stable tie key, overwritten from the supplied slice order.
    pub input_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowBlockHit {
    pub block_state: BlockStateId,
    pub block_position: Option<BlockPosition>,
    pub location: Vec3,
    pub world_border_deflection: Option<ResolvedDeflection>,
}

impl ArrowBlockHit {
    #[must_use]
    pub const fn block(
        block_state: BlockStateId,
        block_position: BlockPosition,
        location: Vec3,
    ) -> Self {
        Self {
            block_state,
            block_position: Some(block_position),
            location,
            world_border_deflection: None,
        }
    }

    #[must_use]
    pub const fn world_border(
        block_state: BlockStateId,
        location: Vec3,
        deflection: ResolvedDeflection,
    ) -> Self {
        Self {
            block_state,
            block_position: None,
            location,
            world_border_deflection: Some(deflection),
        }
    }
}

#[derive(Debug)]
pub struct ArrowTickInput<'a> {
    pub stamp: InputStamp,
    pub owner_collision: OwnerCollisionInput<'a>,
    pub embedded_in_block: bool,
    pub current_block_state: BlockStateId,
    pub should_fall: bool,
    pub fall_velocity_scale: Option<Vec3>,
    pub in_water: bool,
    pub in_water_or_rain: bool,
    pub no_gravity: bool,
    pub block_hit: Option<ArrowBlockHit>,
    /// Mutable complete raycast candidates in deterministic caller order.
    ///
    /// Preparation reorders this caller-owned working slice in place and does
    /// not retain it. This provides unbounded candidate count without per-tick
    /// allocation.
    pub entity_hits: &'a mut [ArrowEntityHit],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowInputField {
    EntityHitLocation,
    EntityPosition,
    BlockHitLocation,
    DeflectionVelocity,
    DeflectionYaw,
    ComputedVelocity,
    ComputedRotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowPrepareError {
    Discarded,
    RevisionExhausted,
    InvalidState(StateError),
    Owner(OwnerInputError),
    DuplicateCandidate(EntityId),
    NonFinite(ArrowInputField),
    MissingFallVelocityScale,
    FallVelocityScaleOutOfRange,
    PublicationCapacityExceeded { capacity: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowTickMutation {
    Grounded {
        started_falling: bool,
        despawned: bool,
    },
    Flight {
        no_physics: bool,
        ordered_entity_hits: usize,
        block_processed: bool,
        target_deflection: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowTickOutcome {
    pub revision: RevisionTransition,
    pub mutation: ArrowTickMutation,
    pub candidate_work: CandidateWork,
    pub publications: PublicationBatch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedArrowTick {
    expected_state: ArrowState,
    stamp: InputStamp,
    next: ArrowState,
    outcome: ArrowTickOutcome,
}

pub fn prepare_arrow_tick(
    current: &ArrowState,
    mut input: ArrowTickInput<'_>,
) -> Result<PreparedArrowTick, ArrowPrepareError> {
    if current.projectile.lifecycle == ProjectileLifecycle::Discarded {
        return Err(ArrowPrepareError::Discarded);
    }
    current
        .projectile
        .validate()
        .map_err(ArrowPrepareError::InvalidState)?;
    let next_revision = current
        .projectile
        .revision
        .checked_add(1)
        .ok_or(ArrowPrepareError::RevisionExhausted)?;
    let mut next = *current;
    let mut publications = PublicationBatch::new();
    let movement = current.projectile.velocity;
    let physics_enabled = !current.no_physics;

    if input.block_hit.is_some() && physics_enabled {
        next.in_ground = false;
    } else if input.embedded_in_block && physics_enabled {
        next.projectile.velocity = Vec3::ZERO;
        next.in_ground = true;
    }
    if next.shake_time > 0 {
        next.shake_time -= 1;
    }

    let (mutation, candidate_work) = if next.in_ground && physics_enabled {
        (
            prepare_grounded_tick(&mut next, &input, &mut publications)?,
            CandidateWork::default(),
        )
    } else {
        prepare_flight_tick(&mut next, movement, &mut input, &mut publications)?
    };

    next.projectile.revision = next_revision;
    let outcome = ArrowTickOutcome {
        revision: RevisionTransition {
            from: current.projectile.revision,
            to: next_revision,
        },
        mutation,
        candidate_work,
        publications,
    };
    Ok(PreparedArrowTick {
        expected_state: *current,
        stamp: input.stamp,
        next,
        outcome,
    })
}

pub fn commit_arrow_tick(
    current: &mut ArrowState,
    stamp: InputStamp,
    plan: PreparedArrowTick,
) -> Result<ArrowTickOutcome, TickCommitError> {
    if current.projectile.revision != plan.expected_state.projectile.revision {
        return Err(TickCommitError::StaleState {
            expected: plan.expected_state.projectile.revision,
            actual: current.projectile.revision,
        });
    }
    if *current != plan.expected_state {
        return Err(TickCommitError::StateChangedAtRevision {
            revision: current.projectile.revision,
        });
    }
    if stamp.world_revision != plan.stamp.world_revision {
        return Err(TickCommitError::StaleWorld {
            expected: plan.stamp.world_revision,
            actual: stamp.world_revision,
        });
    }
    if stamp.collision_revision != plan.stamp.collision_revision {
        return Err(TickCommitError::StaleCollisions {
            expected: plan.stamp.collision_revision,
            actual: stamp.collision_revision,
        });
    }
    if stamp.resolution_revision != plan.stamp.resolution_revision {
        return Err(TickCommitError::StaleResolutions {
            expected: plan.stamp.resolution_revision,
            actual: stamp.resolution_revision,
        });
    }
    *current = plan.next;
    Ok(plan.outcome)
}

fn prepare_grounded_tick(
    state: &mut ArrowState,
    input: &ArrowTickInput<'_>,
    publications: &mut PublicationBatch,
) -> Result<ArrowTickMutation, ArrowPrepareError> {
    let support_changed = state.last_block_state != Some(input.current_block_state);
    let mut started_falling = false;
    let mut despawned = false;
    if support_changed && input.should_fall {
        let scale = input
            .fall_velocity_scale
            .ok_or(ArrowPrepareError::MissingFallVelocityScale)?;
        if !scale.is_finite()
            || [scale.x, scale.y, scale.z]
                .into_iter()
                .any(|component| !(0.0..0.2).contains(&component))
        {
            return Err(ArrowPrepareError::FallVelocityScaleOutOfRange);
        }
        state.in_ground = false;
        state.projectile.velocity = state.projectile.velocity.multiply(scale);
        state.despawn_age = 0;
        started_falling = true;
    } else {
        state.despawn_age = state.despawn_age.wrapping_add(1);
        if state.despawn_age >= ARROW_DESPAWN_TICKS {
            state.projectile.lifecycle = ProjectileLifecycle::Discarded;
            publications
                .push(ProjectilePublication::Discarded {
                    reason: DiscardReason::DespawnAge,
                })
                .expect("grounded tick emits one bounded publication");
            despawned = true;
        }
    }
    state.in_ground_time = state.in_ground_time.wrapping_add(1);
    Ok(ArrowTickMutation::Grounded {
        started_falling,
        despawned,
    })
}

fn prepare_flight_tick(
    state: &mut ArrowState,
    movement: Vec3,
    input: &mut ArrowTickInput<'_>,
    publications: &mut PublicationBatch,
) -> Result<(ArrowTickMutation, CandidateWork), ArrowPrepareError> {
    let original_position = state.projectile.position;
    state.in_ground_time = 0;
    if input.in_water {
        state.projectile.velocity = state.projectile.velocity.scale(WATER_INERTIA);
    }

    let target = target_rotation(movement, state.no_physics);
    state.projectile.rotation.pitch = lerp_rotation(state.projectile.rotation.pitch, target.pitch)
        .ok_or(ArrowPrepareError::NonFinite(
            ArrowInputField::ComputedRotation,
        ))?;
    state.projectile.rotation.yaw = lerp_rotation(state.projectile.rotation.yaw, target.yaw)
        .ok_or(ArrowPrepareError::NonFinite(
            ArrowInputField::ComputedRotation,
        ))?;
    if !state.projectile.rotation.is_finite() {
        return Err(ArrowPrepareError::NonFinite(
            ArrowInputField::ComputedRotation,
        ));
    }

    validate_owner_collision_input(&state.projectile, input.owner_collision)
        .map_err(ArrowPrepareError::Owner)?;
    if !state.projectile.left_owner {
        state.projectile.left_owner =
            outside_owner_collision_range(&state.projectile, input.owner_collision)
                .map_err(ArrowPrepareError::Owner)?;
    }
    let owner = resolved_owner(&state.projectile, input.owner_collision);
    let mut ordered_entity_hits = 0;
    let mut block_processed = false;
    let mut target_deflection = false;
    let mut candidate_work = CandidateWork::default();

    if state.no_physics {
        let endpoint = original_position.plus(movement);
        move_projectile(&mut state.projectile, endpoint)
            .map_err(ArrowPrepareError::InvalidState)?;
    } else {
        candidate_work = order_candidates(original_position, input.entity_hits)?;
        if input.block_hit.is_some_and(|hit| !hit.location.is_finite()) {
            return Err(ArrowPrepareError::NonFinite(
                ArrowInputField::BlockHitLocation,
            ));
        }
        for hit in input.entity_hits.iter().copied() {
            candidate_work.hit_candidates_visited += 1;
            if !arrow_candidate_is_eligible(state, owner, original_position, input.block_hit, &hit)
            {
                continue;
            }
            ordered_entity_hits += 1;
            if state.projectile.lifecycle == ProjectileLifecycle::Discarded || target_deflection {
                continue;
            }
            move_projectile(&mut state.projectile, hit.location)
                .map_err(ArrowPrepareError::InvalidState)?;
            if let ArrowEntityResolution::Deflected(deflection) = hit.resolution {
                let applied = state.projectile.last_deflected_by != Some(hit.entity);
                if applied {
                    validate_deflection(deflection)?;
                    state.projectile.velocity = deflection.velocity;
                    state.projectile.rotation.yaw += deflection.yaw_delta;
                    state.projectile.last_deflected_by = Some(hit.entity);
                }
                publish(
                    publications,
                    ProjectilePublication::Deflected {
                        by: Some(hit.entity),
                        applied,
                    },
                )?;
                target_deflection = true;
                continue;
            }
            process_arrow_damage_hit(state, hit, publications)?;
        }

        let may_continue_to_block = state.projectile.lifecycle == ProjectileLifecycle::Active
            && !target_deflection
            && (ordered_entity_hits == 0 || state.pierce_level > 0);
        if may_continue_to_block {
            let endpoint = input
                .block_hit
                .map_or_else(|| original_position.plus(movement), |hit| hit.location);
            move_projectile(&mut state.projectile, endpoint)
                .map_err(ArrowPrepareError::InvalidState)?;
            if let Some(block_hit) = input.block_hit {
                process_arrow_block_hit(state, block_hit, publications)?;
                block_processed = true;
            }
        }
    }

    if !input.in_water {
        state.projectile.velocity = state.projectile.velocity.scale(AIR_INERTIA);
    }
    if !state.no_physics && !state.in_ground && !input.no_gravity {
        state.projectile.velocity.y -= ARROW_GRAVITY;
    }
    if !state.projectile.velocity.is_finite() {
        return Err(ArrowPrepareError::NonFinite(
            ArrowInputField::ComputedVelocity,
        ));
    }

    if !state.projectile.has_been_shot {
        publish(
            publications,
            ProjectilePublication::ProjectileShot { owner },
        )?;
        state.projectile.has_been_shot = true;
    }
    let _ = input.in_water_or_rain;
    Ok((
        ArrowTickMutation::Flight {
            no_physics: state.no_physics,
            ordered_entity_hits,
            block_processed,
            target_deflection,
        },
        candidate_work,
    ))
}

fn process_arrow_damage_hit(
    state: &mut ArrowState,
    hit: ArrowEntityHit,
    publications: &mut PublicationBatch,
) -> Result<(), ArrowPrepareError> {
    publish(
        publications,
        ProjectilePublication::EntityImpact {
            entity: hit.entity,
            location: hit.location,
        },
    )?;

    if state.pierce_level > 0 {
        let limit = usize::from(state.pierce_level as u8) + 1;
        if state.pierced_entities.len() >= limit {
            discard(state, publications, DiscardReason::PiercingLimit)?;
            publish(
                publications,
                ProjectilePublication::ProjectileLandedEntity {
                    entity: hit.entity,
                    location: hit.location,
                },
            )?;
            return Ok(());
        }
        state
            .pierced_entities
            .record(hit.entity)
            .expect("positive byte pierce limit cannot exceed fixed ledger capacity");
    }

    match hit.resolution {
        ArrowEntityResolution::Deflected(_) => unreachable!("deflection handled before damage"),
        ArrowEntityResolution::Damage(ArrowDamageResolution::Accepted {
            enderman,
            living,
            killed,
        }) => {
            publish(
                publications,
                ProjectilePublication::ArrowDamageAccepted {
                    entity: hit.entity,
                    killed_living: living && killed,
                    enderman,
                },
            )?;
            if living && killed && state.pierce_level > 0 {
                state
                    .pierced_and_killed
                    .record(hit.entity)
                    .expect("killed ledger cannot exceed pierced ledger");
            }
            if !enderman && state.pierce_level <= 0 {
                discard(state, publications, DiscardReason::EntityHit)?;
            }
        }
        ArrowEntityResolution::Damage(ArrowDamageResolution::Rejected { reverse }) => {
            validate_deflection(reverse)?;
            state.projectile.velocity = reverse.velocity.scale(0.2);
            state.projectile.rotation.yaw += reverse.yaw_delta;
            publish(
                publications,
                ProjectilePublication::ArrowDamageRejected { entity: hit.entity },
            )?;
            if state.projectile.velocity.length_squared() < 1.0e-7 {
                if state.pickup == PickupMode::Allowed {
                    publish(publications, ProjectilePublication::PickupItemRequested)?;
                }
                discard(state, publications, DiscardReason::RejectedHitStopped)?;
            }
        }
    }
    publish(
        publications,
        ProjectilePublication::ProjectileLandedEntity {
            entity: hit.entity,
            location: hit.location,
        },
    )?;
    Ok(())
}

fn process_arrow_block_hit(
    state: &mut ArrowState,
    hit: ArrowBlockHit,
    publications: &mut PublicationBatch,
) -> Result<(), ArrowPrepareError> {
    if !hit.location.is_finite() {
        return Err(ArrowPrepareError::NonFinite(
            ArrowInputField::BlockHitLocation,
        ));
    }
    if let Some(deflection) = hit.world_border_deflection {
        validate_deflection(deflection)?;
        state.projectile.velocity = deflection.velocity.scale(0.2);
        state.projectile.rotation.yaw += deflection.yaw_delta;
        publish(
            publications,
            ProjectilePublication::Deflected {
                by: None,
                applied: true,
            },
        )?;
        return Ok(());
    }

    state.last_block_state = Some(hit.block_state);
    state.last_block_position = hit.block_position;
    publish(
        publications,
        ProjectilePublication::BlockImpact {
            block_state: hit.block_state,
            location: hit.location,
        },
    )?;
    let velocity = state.projectile.velocity;
    let offset = Vec3::new(
        java_signum(velocity.x) * BLOCK_EMBED_OFFSET,
        java_signum(velocity.y) * BLOCK_EMBED_OFFSET,
        java_signum(velocity.z) * BLOCK_EMBED_OFFSET,
    );
    let embedded_position = state.projectile.position.subtract(offset);
    move_projectile(&mut state.projectile, embedded_position)
        .map_err(ArrowPrepareError::InvalidState)?;
    state.projectile.velocity = Vec3::ZERO;
    state.in_ground = true;
    state.shake_time = 7;
    state.pierce_level = 0;
    state.pierced_entities.clear();
    state.pierced_and_killed.clear();
    publish(
        publications,
        ProjectilePublication::ProjectileLandedBlock {
            block_state: hit.block_state,
            location: hit.location,
        },
    )?;
    Ok(())
}

fn java_signum(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

fn discard(
    state: &mut ArrowState,
    publications: &mut PublicationBatch,
    reason: DiscardReason,
) -> Result<(), ArrowPrepareError> {
    state.projectile.lifecycle = ProjectileLifecycle::Discarded;
    publish(publications, ProjectilePublication::Discarded { reason })
}

fn publish(
    publications: &mut PublicationBatch,
    publication: ProjectilePublication,
) -> Result<(), ArrowPrepareError> {
    publications.push(publication).map_err(|error| match error {
        super::PublicationError::CapacityExceeded { capacity } => {
            ArrowPrepareError::PublicationCapacityExceeded { capacity }
        }
    })
}

fn order_candidates(
    origin: Vec3,
    candidates: &mut [ArrowEntityHit],
) -> Result<CandidateWork, ArrowPrepareError> {
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if !candidate.location.is_finite() {
            return Err(ArrowPrepareError::NonFinite(
                ArrowInputField::EntityHitLocation,
            ));
        }
        if !candidate.entity_position.is_finite() {
            return Err(ArrowPrepareError::NonFinite(
                ArrowInputField::EntityPosition,
            ));
        }
        candidate.input_order = index;
    }
    candidates.sort_unstable_by_key(|candidate| candidate.entity);
    for pair in candidates.windows(2) {
        if pair[0].entity == pair[1].entity {
            return Err(ArrowPrepareError::DuplicateCandidate(pair[1].entity));
        }
    }
    candidates.sort_unstable_by(|left, right| {
        compare_distance(origin, left.entity_position, right.entity_position)
            .then_with(|| left.input_order.cmp(&right.input_order))
    });
    Ok(CandidateWork {
        candidates: candidates.len(),
        duplicate_adjacencies_checked: candidates.len().saturating_sub(1),
        hit_candidates_visited: 0,
    })
}

fn arrow_candidate_is_eligible(
    state: &ArrowState,
    owner: Option<EntityIdentity>,
    origin: Vec3,
    block_hit: Option<ArrowBlockHit>,
    candidate: &ArrowEntityHit,
) -> bool {
    if !candidate
        .eligibility
        .permits_arrow(&state.projectile, owner)
        || state.pierced_entities.contains(candidate.entity)
    {
        return false;
    }
    block_hit.is_none_or(|hit| strictly_before(origin, candidate.location, hit.location))
}

fn validate_deflection(deflection: ResolvedDeflection) -> Result<(), ArrowPrepareError> {
    if !deflection.velocity.is_finite() {
        return Err(ArrowPrepareError::NonFinite(
            ArrowInputField::DeflectionVelocity,
        ));
    }
    if !deflection.yaw_delta.is_finite() {
        return Err(ArrowPrepareError::NonFinite(ArrowInputField::DeflectionYaw));
    }
    Ok(())
}
