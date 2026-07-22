//! Vanilla 26.1.2 entity-contact calculations.
//!
//! This module computes push-selection eligibility, independently gated
//! recipient impulses, and cramming damage eligibility. Collision discovery,
//! team and vehicle fact ownership, velocity publication, and damage commits
//! remain with their owning callers.

use crate::{Aabb, Vec3};

/// `0.01F` from `Entity.push(Entity)`, widened exactly as Java widens it.
pub const MIN_PUSH_DISTANCE: f64 = 0.01_f32 as f64;
/// `0.05F` from `Entity.push(Entity)`, widened exactly as Java widens it.
pub const PUSH_STRENGTH: f64 = 0.05_f32 as f64;
/// `LivingEntity.pushEntities()` accepts one damage roll out of four.
pub const CRAMMING_ROLL_DENOMINATOR: u8 = 4;
/// Damage passed to `hurtServer` after a successful cramming roll.
pub const CRAMMING_DAMAGE: f32 = 6.0;

// `Attributes.SCALE` bounds from the bundled Java Edition 26.1.2 server.
const MIN_ENTITY_SCALE: f32 = 0.0625;
const MAX_ENTITY_SCALE: f32 = 16.0;

/// Unscaled living-entity geometry from the 26.1.2 entity contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityCollisionDimensions {
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub fixed: bool,
}

/// Collision and eye geometry after applying the authoritative live scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaledEntityCollisionGeometry {
    pub aabb: Aabb,
    pub eye_height: f64,
}

/// Applies `EntityDimensions.scale(float)` semantics from Java Edition 26.1.2.
pub fn scale_entity_collision_geometry(
    dimensions: EntityCollisionDimensions,
    scale: f32,
) -> Result<ScaledEntityCollisionGeometry, EntityContactError> {
    if !scale.is_finite() || !(MIN_ENTITY_SCALE..=MAX_ENTITY_SCALE).contains(&scale) {
        return Err(EntityContactError::InvalidScale);
    }
    if !dimensions.width.is_finite()
        || !dimensions.height.is_finite()
        || !dimensions.eye_height.is_finite()
    {
        return Err(EntityContactError::InvalidDimensions);
    }

    let effective_scale = if dimensions.fixed { 1.0 } else { scale };
    let width = dimensions.width * effective_scale;
    let height = dimensions.height * effective_scale;
    let eye_height = dimensions.eye_height * effective_scale;
    if !width.is_finite() || !height.is_finite() || !eye_height.is_finite() {
        return Err(EntityContactError::InvalidDimensions);
    }
    Ok(ScaledEntityCollisionGeometry {
        aabb: Aabb {
            half_width: f64::from(width / 2.0),
            height: f64::from(height),
        },
        eye_height: f64::from(eye_height),
    })
}

/// Vanilla scoreboard collision rules used by `EntitySelector.pushableBy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamCollisionRule {
    Always,
    Never,
    PushOwnTeam,
    PushOtherTeams,
}

/// The already-resolved alliance relation between the two participants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamRelationship {
    Allied,
    NotAllied,
}

/// Applies vanilla's server-side `EntitySelector.pushableBy` predicate.
#[must_use]
pub fn vanilla_pushable_by(
    pusher_rule: TeamCollisionRule,
    contact_rule: TeamCollisionRule,
    relationship: TeamRelationship,
    contact_pushable: bool,
    contact_spectator: bool,
) -> bool {
    if !contact_pushable
        || contact_spectator
        || pusher_rule == TeamCollisionRule::Never
        || contact_rule == TeamCollisionRule::Never
    {
        return false;
    }

    let allied = relationship == TeamRelationship::Allied;
    if allied
        && (pusher_rule == TeamCollisionRule::PushOwnTeam
            || contact_rule == TeamCollisionRule::PushOwnTeam)
    {
        return false;
    }

    allied
        || (pusher_rule != TeamCollisionRule::PushOtherTeams
            && contact_rule != TeamCollisionRule::PushOtherTeams)
}

/// The participant that should receive this invocation's single impulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushRecipient {
    Caller,
    Other,
}

/// Compact input for one invocation of vanilla's `Entity.push(Entity)` math.
///
/// The horizontal delta is `other.position - caller.position`. The caller
/// supplies relationship and eligibility decisions because those are owned by
/// the entity/runtime layer rather than this contact kernel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityPushInput {
    pub caller_to_other_x: f64,
    pub caller_to_other_z: f64,
    pub recipient: PushRecipient,
    pub caller_physics_enabled: bool,
    pub other_physics_enabled: bool,
    pub passenger_of_same_vehicle: bool,
    pub recipient_pushable: bool,
    pub recipient_is_vehicle: bool,
}

/// Inputs shared by both independently eligible recipients of `Entity.push`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityPushPairInput {
    pub caller_to_other_x: f64,
    pub caller_to_other_z: f64,
    pub caller_physics_enabled: bool,
    pub other_physics_enabled: bool,
    pub passenger_of_same_vehicle: bool,
    pub caller_pushable: bool,
    pub caller_is_vehicle: bool,
    pub other_pushable: bool,
    pub other_is_vehicle: bool,
}

/// The two independently gated velocity additions from one `Entity.push` call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityPushImpulses {
    pub caller: Vec3,
    pub other: Vec3,
}

impl EntityPushImpulses {
    pub const ZERO: Self = Self {
        caller: Vec3::ZERO,
        other: Vec3::ZERO,
    };
}

/// Invalid caller input to the entity-contact kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityContactError {
    NonFinitePushDelta,
    InvalidCrammingRoll { roll: u8 },
    InvalidDimensions,
    InvalidScale,
}

/// Computes the vanilla impulse for exactly one caller-selected recipient.
///
/// Exact horizontal overlap returns zero through vanilla's distance threshold.
/// Solaris-specific deterministic overlap handling, if needed, must remain a
/// separately named opt-in policy outside this vanilla function.
pub fn vanilla_push_impulse(input: EntityPushInput) -> Result<Vec3, EntityContactError> {
    if !input.caller_to_other_x.is_finite() || !input.caller_to_other_z.is_finite() {
        return Err(EntityContactError::NonFinitePushDelta);
    }
    if !input.caller_physics_enabled
        || !input.other_physics_enabled
        || input.passenger_of_same_vehicle
        || !input.recipient_pushable
        || input.recipient_is_vehicle
    {
        return Ok(Vec3::ZERO);
    }

    let mut x = input.caller_to_other_x;
    let mut z = input.caller_to_other_z;
    let mut distance = x.abs().max(z.abs());
    if distance < MIN_PUSH_DISTANCE {
        return Ok(Vec3::ZERO);
    }

    distance = distance.sqrt();
    x /= distance;
    z /= distance;
    let attenuation = (1.0 / distance).min(1.0);
    x *= attenuation;
    z *= attenuation;
    x *= PUSH_STRENGTH;
    z *= PUSH_STRENGTH;

    if input.recipient == PushRecipient::Caller {
        x = -x;
        z = -z;
    }
    Ok(Vec3::new(x, 0.0, z))
}

/// Computes both independently eligible recipients from one `Entity.push` call.
pub fn vanilla_push_impulses(
    input: EntityPushPairInput,
) -> Result<EntityPushImpulses, EntityContactError> {
    let shared = |recipient, recipient_pushable, recipient_is_vehicle| EntityPushInput {
        caller_to_other_x: input.caller_to_other_x,
        caller_to_other_z: input.caller_to_other_z,
        recipient,
        caller_physics_enabled: input.caller_physics_enabled,
        other_physics_enabled: input.other_physics_enabled,
        passenger_of_same_vehicle: input.passenger_of_same_vehicle,
        recipient_pushable,
        recipient_is_vehicle,
    };
    Ok(EntityPushImpulses {
        caller: vanilla_push_impulse(shared(
            PushRecipient::Caller,
            input.caller_pushable,
            input.caller_is_vehicle,
        ))?,
        other: vanilla_push_impulse(shared(
            PushRecipient::Other,
            input.other_pushable,
            input.other_is_vehicle,
        ))?,
    })
}

/// One already-selected pushable entity intersecting the subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrammingContact {
    pub is_passenger: bool,
}

/// Counts and coarse roll requirement from vanilla's cramming gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrammingGate {
    pub pushable_contacts: usize,
    pub non_passenger_contacts: usize,
    pub roll_required: bool,
    max_entity_cramming: usize,
}

/// Opaque second-stage request produced only after vanilla's coarse gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrammingRollRequest {
    gate: CrammingGate,
}

/// Counts caller-selected pushable contacts before any random roll is drawn.
pub fn vanilla_cramming_gate(
    contacts: &[CrammingContact],
    max_entity_cramming: u32,
) -> CrammingGate {
    let pushable_contacts = contacts.len();
    let non_passenger_contacts = contacts
        .iter()
        .filter(|contact| !contact.is_passenger)
        .count();
    let cap = max_entity_cramming as usize;
    let roll_required = max_entity_cramming > 0 && pushable_contacts >= cap;

    CrammingGate {
        pushable_contacts,
        non_passenger_contacts,
        roll_required,
        max_entity_cramming: cap,
    }
}

/// Requests one deterministic caller-supplied roll when the coarse gate passes.
#[must_use]
pub fn vanilla_cramming_roll_request(
    contacts: &[CrammingContact],
    max_entity_cramming: u32,
) -> Option<CrammingRollRequest> {
    let gate = vanilla_cramming_gate(contacts, max_entity_cramming);
    gate.roll_required.then_some(CrammingRollRequest { gate })
}

/// Evaluates a caller-provided `nextInt(4)` roll after the coarse gate passes.
///
/// Callers should only draw a roll and invoke this function when
/// [`CrammingGate::roll_required`] is true. This function does not select
/// contacts, consume randomness, or commit damage.
pub fn evaluate_cramming_roll(gate: CrammingGate, roll: u8) -> Result<bool, EntityContactError> {
    if roll >= CRAMMING_ROLL_DENOMINATOR {
        return Err(EntityContactError::InvalidCrammingRoll { roll });
    }

    Ok(gate.roll_required && gate.non_passenger_contacts >= gate.max_entity_cramming && roll == 0)
}

/// Applies an explicit `nextInt(4)` result to a previously requested roll.
pub fn apply_cramming_roll(
    request: CrammingRollRequest,
    roll: u8,
) -> Result<Option<f32>, EntityContactError> {
    evaluate_cramming_roll(request.gate, roll).map(|damage| damage.then_some(CRAMMING_DAMAGE))
}

#[cfg(test)]
#[path = "entity_collision_26_1_2_tests.rs"]
mod tests;
