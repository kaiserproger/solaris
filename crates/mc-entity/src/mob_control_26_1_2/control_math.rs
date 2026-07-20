//! Shared scalar types, arithmetic, and prepare/apply transaction fences.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputStamp {
    pub entity_revision: u64,
    pub facts_revision: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ControlOutput {
    pub speed: Option<f32>,
    pub forward: Option<f32>,
    pub strafe: Option<f32>,
    pub vertical: Option<f32>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub head_yaw: Option<f32>,
    pub body_yaw: Option<f32>,
    pub jump_requested: Option<bool>,
    pub jumping: Option<bool>,
    pub no_gravity: Option<bool>,
    pub vertical_velocity_change: Option<VerticalVelocityChange>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalVelocityChange {
    /// Apply `delta` only when the caller's current value still bit-matches
    /// `expected_current`; `result` is vanilla's resulting absolute Y velocity.
    Additive {
        expected_current: f64,
        delta: f64,
        result: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prepared<S> {
    pub(crate) expected_control_revision: u64,
    pub(crate) expected_control_state: S,
    pub(crate) stamp: InputStamp,
    pub next: S,
    pub output: ControlOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingInput {
    Walkability,
    Collision,
    NavigationState,
    NavigationPresence,
    NodeEvaluatorPresence,
    CurrentVerticalVelocity,
    FlyingSpeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputField {
    Position,
    Target,
    Rotation,
    Speed,
    BodyDimensions,
    Collision,
    VerticalVelocity,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareError {
    Deferred(MissingInput),
    StaleFact(MissingInput),
    NonFinite(InputField),
    RevisionExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    StaleControl {
        expected: u64,
        actual: u64,
    },
    StaleEntity {
        expected: u64,
        actual: u64,
    },
    StaleFacts {
        expected: u64,
        actual: u64,
    },
    ControlChangedAtRevision {
        revision: u64,
    },
    MissingFact(MissingInput),
    StaleVerticalVelocity {
        expected_bits: u64,
        actual_bits: u64,
    },
}

pub(crate) trait Revisioned {
    fn revision(&self) -> u64;
    fn same_state(&self, other: &Self) -> bool;
}

pub(crate) fn prepared<S: Copy + Revisioned>(
    current: &S,
    stamp: InputStamp,
    mut next: S,
    output: ControlOutput,
    set_revision: impl FnOnce(&mut S, u64),
) -> Result<Prepared<S>, PrepareError> {
    let revision = current.revision();
    let next_revision = revision
        .checked_add(1)
        .ok_or(PrepareError::RevisionExhausted)?;
    set_revision(&mut next, next_revision);
    Ok(Prepared {
        expected_control_revision: revision,
        expected_control_state: *current,
        stamp,
        next,
        output,
    })
}

pub(crate) fn apply<S: Copy + Revisioned>(
    current: &mut S,
    stamp: InputStamp,
    plan: Prepared<S>,
) -> Result<ControlOutput, ApplyError> {
    validate_apply(current, stamp, &plan)?;
    Ok(commit_apply(current, plan))
}

pub(crate) fn validate_apply<S: Copy + Revisioned>(
    current: &S,
    stamp: InputStamp,
    plan: &Prepared<S>,
) -> Result<(), ApplyError> {
    if current.revision() != plan.expected_control_revision {
        return Err(ApplyError::StaleControl {
            expected: plan.expected_control_revision,
            actual: current.revision(),
        });
    }
    if !current.same_state(&plan.expected_control_state) {
        return Err(ApplyError::ControlChangedAtRevision {
            revision: current.revision(),
        });
    }
    if stamp.entity_revision != plan.stamp.entity_revision {
        return Err(ApplyError::StaleEntity {
            expected: plan.stamp.entity_revision,
            actual: stamp.entity_revision,
        });
    }
    if stamp.facts_revision != plan.stamp.facts_revision {
        return Err(ApplyError::StaleFacts {
            expected: plan.stamp.facts_revision,
            actual: stamp.facts_revision,
        });
    }
    Ok(())
}

pub(crate) fn commit_apply<S: Copy>(current: &mut S, plan: Prepared<S>) -> ControlOutput {
    *current = plan.next;
    plan.output
}

pub fn wrap_degrees(angle: f32) -> f32 {
    let mut normalized = angle % 360.0;
    if normalized >= 180.0 {
        normalized -= 360.0;
    }
    if normalized < -180.0 {
        normalized += 360.0;
    }
    normalized
}

pub fn rotate_towards(from: f32, to: f32, max_rotation: f32) -> f32 {
    from + java_clamp(wrap_degrees(to - from), -max_rotation, max_rotation)
}

pub fn rotate_if_necessary(base: f32, target: f32, max_difference: f32) -> f32 {
    target - java_clamp(wrap_degrees(target - base), -max_difference, max_difference)
}

pub fn rotlerp(from: f32, to: f32, max_rotation: f32) -> f32 {
    let mut difference = wrap_degrees(to - from);
    if difference > max_rotation {
        difference = max_rotation;
    }
    if difference < -max_rotation {
        difference = -max_rotation;
    }
    let result = from + difference;
    if result < 0.0 {
        result + 360.0
    } else if result > 360.0 {
        result - 360.0
    } else {
        result
    }
}

pub(crate) fn java_clamp(value: f32, minimum: f32, maximum: f32) -> f32 {
    if value < minimum {
        minimum
    } else if value.is_nan() || maximum.is_nan() {
        f32::NAN
    } else {
        value.min(maximum)
    }
}

pub(crate) fn target_yaw(x_delta: f64, z_delta: f64) -> f32 {
    (vanilla_atan2(z_delta, x_delta) * 180.0 / f64::from(std::f32::consts::PI)) as f32 - 90.0
}

pub(crate) fn target_pitch(y_delta: f64, horizontal: f64) -> f32 {
    (-(vanilla_atan2(y_delta, horizontal) * 180.0 / f64::from(std::f32::consts::PI))) as f32
}

fn vanilla_atan2(mut y: f64, mut x: f64) -> f64 {
    let length_squared = x * x + y * y;
    if length_squared.is_nan() {
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

    let inverse_length = fast_inverse_sqrt(length_squared);
    x *= inverse_length;
    y *= inverse_length;
    let fraction_bias = f64::from_bits(4_805_340_802_404_319_232);
    let biased_y = fraction_bias + y;
    let index = biased_y.to_bits() as u32;
    let phi = (f64::from(index) / 256.0).asin();
    let cosine_phi = phi.cos();
    let sine_phi = biased_y - fraction_bias;
    let error = y * cosine_phi - x * sine_phi;
    let correction = (6.0 + error * error) * error * 0.16666666666666666;
    let mut theta = phi + correction;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if negative_x {
        theta = std::f64::consts::PI - theta;
    }
    if negative_y {
        theta = -theta;
    }
    theta
}

fn fast_inverse_sqrt(mut value: f64) -> f64 {
    let half = 0.5 * value;
    let bits = 6_910_469_410_427_058_090_i64 - ((value.to_bits() as i64) >> 1);
    value = f64::from_bits(bits as u64);
    value * (1.5 - half * value * value)
}

// Mth's 65,536-entry table is reconstructed at the selected index, avoiding
// per-kernel storage while retaining its quantized angle selection.
pub(crate) fn vanilla_sin(angle: f64) -> f32 {
    const SCALE: f64 = 10430.378350470453;
    let index = ((angle * SCALE) as i64 & 65535) as u32;
    (f64::from(index) / SCALE).sin() as f32
}

pub(crate) fn vanilla_cos(angle: f64) -> f32 {
    const SCALE: f64 = 10430.378350470453;
    let index = ((angle * SCALE + 16384.0) as i64 & 65535) as u32;
    (f64::from(index) / SCALE).sin() as f32
}
