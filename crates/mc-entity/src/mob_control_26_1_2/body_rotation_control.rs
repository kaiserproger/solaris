use super::control_math::*;

const MOVEMENT_SQUARED: f64 = 2.5000003E-7_f32 as f64;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BodyRotationState {
    pub revision: u64,
    pub head_stable_time: i32,
    pub last_stable_head_yaw: f32,
}

impl Revisioned for BodyRotationState {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn same_state(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.head_stable_time == other.head_stable_time
            && self.last_stable_head_yaw.to_bits() == other.last_stable_head_yaw.to_bits()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyFacts {
    pub position: Vec3,
    pub previous_position: Vec3,
    pub yaw: f32,
    pub head_yaw: f32,
    pub body_yaw: f32,
    pub max_head_yaw: f32,
    pub carrying_mob_passenger: bool,
}

pub fn prepare_body_rotation(
    state: &BodyRotationState,
    stamp: InputStamp,
    facts: BodyFacts,
) -> Result<Prepared<BodyRotationState>, PrepareError> {
    validate(state, facts)?;
    let mut next = *state;
    let mut output = ControlOutput::default();
    let xd = facts.position.x - facts.previous_position.x;
    let zd = facts.position.z - facts.previous_position.z;
    if xd * xd + zd * zd > MOVEMENT_SQUARED {
        output.body_yaw = Some(facts.yaw);
        output.head_yaw = Some(rotate_if_necessary(
            facts.head_yaw,
            facts.yaw,
            facts.max_head_yaw,
        ));
        next.last_stable_head_yaw = output.head_yaw.unwrap();
        next.head_stable_time = 0;
    } else if !facts.carrying_mob_passenger {
        if (facts.head_yaw - state.last_stable_head_yaw).abs() > 15.0 {
            next.head_stable_time = 0;
            next.last_stable_head_yaw = facts.head_yaw;
            output.body_yaw = Some(rotate_if_necessary(
                facts.body_yaw,
                facts.head_yaw,
                facts.max_head_yaw,
            ));
        } else {
            next.head_stable_time = state.head_stable_time.wrapping_add(1);
            if next.head_stable_time > 10 {
                let elapsed = next.head_stable_time - 10;
                let fraction = (elapsed as f32 / 10.0).clamp(0.0, 1.0);
                let remaining = facts.max_head_yaw * (1.0 - fraction);
                output.body_yaw = Some(rotate_if_necessary(
                    facts.body_yaw,
                    facts.head_yaw,
                    remaining,
                ));
            }
        }
    }
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_body_rotation(
    state: &mut BodyRotationState,
    stamp: InputStamp,
    plan: Prepared<BodyRotationState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}

fn validate(state: &BodyRotationState, facts: BodyFacts) -> Result<(), PrepareError> {
    if !facts.position.is_finite() || !facts.previous_position.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Position));
    }
    if !state.last_stable_head_yaw.is_finite()
        || !facts.yaw.is_finite()
        || !facts.head_yaw.is_finite()
        || !facts.body_yaw.is_finite()
        || !facts.max_head_yaw.is_finite()
    {
        return Err(PrepareError::NonFinite(InputField::Rotation));
    }
    Ok(())
}
