use super::control_math::*;

const EPSILON: f64 = 1.0E-5_f32 as f64;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LookControlState {
    pub revision: u64,
    pub max_yaw_speed: f32,
    pub max_pitch_angle: f32,
    pub cooldown: u8,
    pub wanted: Vec3,
}

impl Revisioned for LookControlState {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn same_state(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.max_yaw_speed.to_bits() == other.max_yaw_speed.to_bits()
            && self.max_pitch_angle.to_bits() == other.max_pitch_angle.to_bits()
            && self.cooldown == other.cooldown
            && self.wanted.x.to_bits() == other.wanted.x.to_bits()
            && self.wanted.y.to_bits() == other.wanted.y.to_bits()
            && self.wanted.z.to_bits() == other.wanted.z.to_bits()
    }
}

impl LookControlState {
    pub fn look_at(&mut self, wanted: Vec3, max_yaw_speed: f32, max_pitch_angle: f32) {
        self.wanted = wanted;
        self.max_yaw_speed = max_yaw_speed;
        self.max_pitch_angle = max_pitch_angle;
        self.cooldown = 2;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookFacts {
    pub position: Vec3,
    pub eye_y: f64,
    pub pitch: f32,
    pub head_yaw: f32,
    pub body_yaw: f32,
    pub max_head_yaw: f32,
    pub navigation_done: Option<bool>,
    /// Mirrors the overridable `resetXRotOnTick`; callers select the subclass behavior.
    pub reset_pitch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwimmingLookConfig {
    pub max_yaw_from_center: i32,
}

pub fn prepare_look(
    state: &LookControlState,
    stamp: InputStamp,
    facts: LookFacts,
) -> Result<Prepared<LookControlState>, PrepareError> {
    validate(state, facts)?;
    let navigation_done = facts
        .navigation_done
        .ok_or(PrepareError::Deferred(MissingInput::NavigationState))?;
    let mut next = *state;
    let mut output = ControlOutput::default();
    let mut pitch = if facts.reset_pitch {
        output.pitch = Some(0.0);
        0.0
    } else {
        facts.pitch
    };
    let mut head_yaw = facts.head_yaw;
    if state.cooldown > 0 {
        next.cooldown -= 1;
        let xd = state.wanted.x - facts.position.x;
        let yd = state.wanted.y - facts.eye_y;
        let zd = state.wanted.z - facts.position.z;
        if zd.abs() > EPSILON || xd.abs() > EPSILON {
            head_yaw = rotate_towards(head_yaw, target_yaw(xd, zd), state.max_yaw_speed);
            output.head_yaw = Some(head_yaw);
        }
        let horizontal = (xd * xd + zd * zd).sqrt();
        if yd.abs() > EPSILON || horizontal.abs() > EPSILON {
            pitch = rotate_towards(pitch, target_pitch(yd, horizontal), state.max_pitch_angle);
            output.pitch = Some(pitch);
        }
    } else {
        head_yaw = rotate_towards(head_yaw, facts.body_yaw, 10.0);
        output.head_yaw = Some(head_yaw);
    }
    if !navigation_done {
        head_yaw = rotate_if_necessary(head_yaw, facts.body_yaw, facts.max_head_yaw);
        output.head_yaw = Some(head_yaw);
    }
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_look(
    state: &mut LookControlState,
    stamp: InputStamp,
    plan: Prepared<LookControlState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}

pub fn prepare_smooth_swimming_look(
    state: &LookControlState,
    stamp: InputStamp,
    facts: LookFacts,
    config: SwimmingLookConfig,
) -> Result<Prepared<LookControlState>, PrepareError> {
    validate(state, facts)?;
    let mut next = *state;
    let mut output = ControlOutput::default();
    let mut pitch = facts.pitch;
    let mut head_yaw = facts.head_yaw;
    if state.cooldown > 0 {
        next.cooldown -= 1;
        let xd = state.wanted.x - facts.position.x;
        let yd = state.wanted.y - facts.eye_y;
        let zd = state.wanted.z - facts.position.z;
        if zd.abs() > EPSILON || xd.abs() > EPSILON {
            head_yaw = rotate_towards(head_yaw, target_yaw(xd, zd) + 20.0, state.max_yaw_speed);
            output.head_yaw = Some(head_yaw);
        }
        let horizontal = (xd * xd + zd * zd).sqrt();
        if yd.abs() > EPSILON || horizontal.abs() > EPSILON {
            pitch = rotate_towards(
                pitch,
                target_pitch(yd, horizontal) + 10.0,
                state.max_pitch_angle,
            );
            output.pitch = Some(pitch);
        }
    } else {
        let navigation_done = facts
            .navigation_done
            .ok_or(PrepareError::Deferred(MissingInput::NavigationState))?;
        if navigation_done {
            pitch = rotate_towards(pitch, 0.0, 5.0);
            output.pitch = Some(pitch);
        }
        head_yaw = rotate_towards(head_yaw, facts.body_yaw, state.max_yaw_speed);
        output.head_yaw = Some(head_yaw);
    }

    let difference = wrap_degrees(head_yaw - facts.body_yaw);
    if difference < -(config.max_yaw_from_center as f32) {
        output.body_yaw = Some(facts.body_yaw - 4.0);
    } else if difference > config.max_yaw_from_center as f32 {
        output.body_yaw = Some(facts.body_yaw + 4.0);
    }

    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_smooth_swimming_look(
    state: &mut LookControlState,
    stamp: InputStamp,
    plan: Prepared<LookControlState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}

fn validate(state: &LookControlState, facts: LookFacts) -> Result<(), PrepareError> {
    if !facts.position.is_finite() || !facts.eye_y.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Position));
    }
    if !state.wanted.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Target));
    }
    if !facts.pitch.is_finite()
        || !facts.head_yaw.is_finite()
        || !facts.body_yaw.is_finite()
        || !facts.max_head_yaw.is_finite()
        || !state.max_yaw_speed.is_finite()
        || !state.max_pitch_angle.is_finite()
    {
        return Err(PrepareError::NonFinite(InputField::Rotation));
    }
    Ok(())
}
