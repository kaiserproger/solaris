use super::control_math::*;

const MIN_SPEED_SQUARED: f64 = 2.5000003E-7_f32 as f64;
const EPSILON: f64 = 1.0E-5_f32 as f64;
const SWIMMING_GRAVITY_DELTA: f64 = 0.005;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MoveOperation {
    #[default]
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoveControlState {
    pub revision: u64,
    pub wanted: Vec3,
    pub speed_modifier: f64,
    pub strafe_forwards: f32,
    pub strafe_right: f32,
    pub operation: MoveOperation,
}

impl Revisioned for MoveControlState {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn same_state(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.wanted.x.to_bits() == other.wanted.x.to_bits()
            && self.wanted.y.to_bits() == other.wanted.y.to_bits()
            && self.wanted.z.to_bits() == other.wanted.z.to_bits()
            && self.speed_modifier.to_bits() == other.speed_modifier.to_bits()
            && self.strafe_forwards.to_bits() == other.strafe_forwards.to_bits()
            && self.strafe_right.to_bits() == other.strafe_right.to_bits()
            && self.operation == other.operation
    }
}

impl MoveControlState {
    pub fn move_to(&mut self, wanted: Vec3, speed_modifier: f64) {
        self.wanted = wanted;
        self.speed_modifier = speed_modifier;
        if self.operation != MoveOperation::Jumping {
            self.operation = MoveOperation::MoveTo;
        }
    }

    pub fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = MoveOperation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }

    pub fn set_wait(&mut self) {
        self.operation = MoveOperation::Wait;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionFact {
    pub top_y: f64,
    pub is_door: bool,
    pub is_fence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizontalDelta {
    pub x: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkabilityFact {
    pub probe: HorizontalDelta,
    pub is_walkable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurrentVerticalVelocityFact {
    /// Entity/fact revisions at which `value` was observed.
    pub stamp: InputStamp,
    pub value: f64,
}

impl CurrentVerticalVelocityFact {
    pub const fn new(stamp: InputStamp, value: f64) -> Self {
        Self { stamp, value }
    }
}

impl WalkabilityFact {
    pub const fn new(probe: HorizontalDelta, is_walkable: bool) -> Self {
        Self { probe, is_walkable }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveFacts {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub movement_speed: f64,
    pub flying_speed: Option<f64>,
    pub max_up_step: f32,
    pub body_width: f32,
    pub on_ground: bool,
    pub in_liquid: bool,
    pub affected_by_fluids: bool,
    pub in_water: bool,
    pub navigation_done: Option<bool>,
    pub strafe_navigation_present: Option<bool>,
    pub strafe_evaluator_present: Option<bool>,
    pub walkability: Option<WalkabilityFact>,
    pub current_vertical_velocity: Option<CurrentVerticalVelocityFact>,
    /// `None` means not queried; `Some(None)` means an empty collision shape.
    pub collision: Option<Option<CollisionFact>>,
}

impl MoveFacts {
    pub fn with_body(mut self, max_up_step: f32, body_width: f32) -> Self {
        self.max_up_step = max_up_step;
        self.body_width = body_width;
        self
    }

    pub fn with_collision(mut self, value: Option<CollisionFact>) -> Self {
        self.collision = Some(value);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyingConfig {
    pub max_pitch_turn: i32,
    pub hovers_in_place: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwimmingConfig {
    pub max_pitch: i32,
    pub max_yaw_turn: i32,
    pub in_water_speed_modifier: f32,
    pub outside_water_speed_modifier: f32,
    pub apply_gravity: bool,
}

pub fn prepare_move(
    state: &MoveControlState,
    stamp: InputStamp,
    facts: MoveFacts,
) -> Result<Prepared<MoveControlState>, PrepareError> {
    validate(state, facts)?;
    let mut next = *state;
    let mut output = ControlOutput::default();
    match state.operation {
        MoveOperation::Strafe => {
            let probe = strafe_walkability_probe(state, facts)?;
            let navigation_present = facts
                .strafe_navigation_present
                .ok_or(PrepareError::Deferred(MissingInput::NavigationPresence))?;
            let walkable = if navigation_present {
                let evaluator_present = facts
                    .strafe_evaluator_present
                    .ok_or(PrepareError::Deferred(MissingInput::NodeEvaluatorPresence))?;
                if evaluator_present {
                    let walkability = facts
                        .walkability
                        .ok_or(PrepareError::Deferred(MissingInput::Walkability))?;
                    if walkability.probe.x.to_bits() != probe.x.to_bits()
                        || walkability.probe.z.to_bits() != probe.z.to_bits()
                    {
                        return Err(PrepareError::StaleFact(MissingInput::Walkability));
                    }
                    walkability.is_walkable
                } else {
                    true
                }
            } else {
                true
            };
            let speed = state.speed_modifier as f32 * facts.movement_speed as f32;
            let mut forward = state.strafe_forwards;
            let mut right = state.strafe_right;
            if !walkable {
                forward = 1.0;
                right = 0.0;
            }
            output.speed = Some(speed);
            output.forward = Some(forward);
            output.strafe = Some(right);
            next.strafe_forwards = forward;
            next.strafe_right = right;
            next.operation = MoveOperation::Wait;
        }
        MoveOperation::MoveTo => {
            next.operation = MoveOperation::Wait;
            let xd = state.wanted.x - facts.position.x;
            let yd = state.wanted.y - facts.position.y;
            let zd = state.wanted.z - facts.position.z;
            let distance_squared = xd * xd + yd * yd + zd * zd;
            if distance_squared < MIN_SPEED_SQUARED {
                output.forward = Some(0.0);
            } else {
                output.yaw = Some(rotlerp(facts.yaw, target_yaw(xd, zd), 90.0));
                output.speed = Some((state.speed_modifier * facts.movement_speed) as f32);
                let collision = facts
                    .collision
                    .ok_or(PrepareError::Deferred(MissingInput::Collision))?;
                let step_jump = yd > f64::from(facts.max_up_step)
                    && xd * xd + zd * zd < f64::from(facts.body_width.max(1.0));
                let collision_jump = collision.is_some_and(|shape| {
                    facts.position.y < shape.top_y && !shape.is_door && !shape.is_fence
                });
                if step_jump || collision_jump {
                    output.jump_requested = Some(true);
                    next.operation = MoveOperation::Jumping;
                }
            }
        }
        MoveOperation::Jumping => {
            output.speed = Some((state.speed_modifier * facts.movement_speed) as f32);
            if facts.on_ground || facts.in_liquid && facts.affected_by_fluids {
                next.operation = MoveOperation::Wait;
            }
        }
        MoveOperation::Wait => output.forward = Some(0.0),
    }
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn strafe_walkability_probe(
    state: &MoveControlState,
    facts: MoveFacts,
) -> Result<HorizontalDelta, PrepareError> {
    validate(state, facts)?;
    let speed = state.speed_modifier as f32 * facts.movement_speed as f32;
    let distance_squared =
        state.strafe_forwards * state.strafe_forwards + state.strafe_right * state.strafe_right;
    let mut distance = f64::from(distance_squared).sqrt() as f32;
    if distance < 1.0 {
        distance = 1.0;
    }
    let scale = speed / distance;
    let xa = state.strafe_forwards * scale;
    let za = state.strafe_right * scale;
    let radians = f64::from(facts.yaw * (std::f32::consts::PI / 180.0));
    let sin = vanilla_sin(radians);
    let cos = vanilla_cos(radians);
    Ok(HorizontalDelta {
        x: xa * cos - za * sin,
        z: za * cos + xa * sin,
    })
}

pub fn apply_move(
    state: &mut MoveControlState,
    stamp: InputStamp,
    plan: Prepared<MoveControlState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}

pub fn prepare_flying_move(
    state: &MoveControlState,
    stamp: InputStamp,
    facts: MoveFacts,
    config: FlyingConfig,
) -> Result<Prepared<MoveControlState>, PrepareError> {
    validate(state, facts)?;
    let mut next = *state;
    let mut output = ControlOutput::default();
    if state.operation == MoveOperation::MoveTo {
        next.operation = MoveOperation::Wait;
        output.no_gravity = Some(true);
        let xd = state.wanted.x - facts.position.x;
        let yd = state.wanted.y - facts.position.y;
        let zd = state.wanted.z - facts.position.z;
        if xd * xd + yd * yd + zd * zd < MIN_SPEED_SQUARED {
            output.vertical = Some(0.0);
            output.forward = Some(0.0);
        } else {
            output.yaw = Some(rotlerp(facts.yaw, target_yaw(xd, zd), 90.0));
            let attribute = if facts.on_ground {
                facts.movement_speed
            } else {
                facts
                    .flying_speed
                    .ok_or(PrepareError::Deferred(MissingInput::FlyingSpeed))?
            };
            let speed = (state.speed_modifier * attribute) as f32;
            output.speed = Some(speed);
            let horizontal = (xd * xd + zd * zd).sqrt();
            if yd.abs() > EPSILON || horizontal.abs() > EPSILON {
                output.pitch = Some(rotlerp(
                    facts.pitch,
                    target_pitch(yd, horizontal),
                    config.max_pitch_turn as f32,
                ));
                output.vertical = Some(if yd > 0.0 { speed } else { -speed });
            }
        }
    } else {
        if !config.hovers_in_place {
            output.no_gravity = Some(false);
        }
        output.vertical = Some(0.0);
        output.forward = Some(0.0);
    }
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_flying_move(
    state: &mut MoveControlState,
    stamp: InputStamp,
    plan: Prepared<MoveControlState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}

pub fn prepare_smooth_swimming_move(
    state: &MoveControlState,
    stamp: InputStamp,
    facts: MoveFacts,
    config: SwimmingConfig,
) -> Result<Prepared<MoveControlState>, PrepareError> {
    validate(state, facts)?;
    if !config.in_water_speed_modifier.is_finite()
        || !config.outside_water_speed_modifier.is_finite()
    {
        return Err(PrepareError::NonFinite(InputField::Configuration));
    }
    let next = *state;
    let mut output = ControlOutput::default();
    if config.apply_gravity && facts.in_water {
        let current = facts
            .current_vertical_velocity
            .ok_or(PrepareError::Deferred(
                MissingInput::CurrentVerticalVelocity,
            ))?;
        if current.stamp != stamp {
            return Err(PrepareError::StaleFact(
                MissingInput::CurrentVerticalVelocity,
            ));
        }
        if !current.value.is_finite() {
            return Err(PrepareError::NonFinite(InputField::VerticalVelocity));
        }
        output.vertical_velocity_change = Some(VerticalVelocityChange::Additive {
            expected_current: current.value,
            delta: SWIMMING_GRAVITY_DELTA,
            result: current.value + SWIMMING_GRAVITY_DELTA,
        });
    }
    let active = if state.operation == MoveOperation::MoveTo {
        !facts
            .navigation_done
            .ok_or(PrepareError::Deferred(MissingInput::NavigationState))?
    } else {
        false
    };
    if active {
        let xd = state.wanted.x - facts.position.x;
        let yd = state.wanted.y - facts.position.y;
        let zd = state.wanted.z - facts.position.z;
        if xd * xd + yd * yd + zd * zd < MIN_SPEED_SQUARED {
            output.forward = Some(0.0);
        } else {
            let wanted_yaw = target_yaw(xd, zd);
            let yaw = rotlerp(facts.yaw, wanted_yaw, config.max_yaw_turn as f32);
            output.yaw = Some(yaw);
            output.body_yaw = Some(yaw);
            output.head_yaw = Some(yaw);
            let speed = (state.speed_modifier * facts.movement_speed) as f32;
            if facts.in_water {
                output.speed = Some(speed * config.in_water_speed_modifier);
                let horizontal = (xd * xd + zd * zd).sqrt();
                let mut pitch = facts.pitch;
                if yd.abs() > EPSILON || horizontal.abs() > EPSILON {
                    let wanted_pitch = java_clamp(
                        wrap_degrees(target_pitch(yd, horizontal)),
                        -(config.max_pitch as f32),
                        config.max_pitch as f32,
                    );
                    pitch = rotate_towards(pitch, wanted_pitch, 5.0);
                    output.pitch = Some(pitch);
                }
                let radians = f64::from(pitch * (std::f32::consts::PI / 180.0));
                output.forward = Some(vanilla_cos(radians) * speed);
                output.vertical = Some(-vanilla_sin(radians) * speed);
            } else {
                let left_to_turn = wrap_degrees(yaw - wanted_yaw).abs();
                let factor = 1.0 - java_clamp((left_to_turn - 10.0) / 50.0, 0.0, 1.0);
                output.speed = Some(speed * config.outside_water_speed_modifier * factor);
            }
        }
    } else {
        output.speed = Some(0.0);
        output.strafe = Some(0.0);
        output.vertical = Some(0.0);
        output.forward = Some(0.0);
    }
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_smooth_swimming_move(
    state: &mut MoveControlState,
    stamp: InputStamp,
    current_vertical_velocity: Option<f64>,
    plan: Prepared<MoveControlState>,
) -> Result<ControlOutput, ApplyError> {
    validate_apply(state, stamp, &plan)?;
    if let Some(VerticalVelocityChange::Additive {
        expected_current, ..
    }) = plan.output.vertical_velocity_change
    {
        let actual = current_vertical_velocity.ok_or(ApplyError::MissingFact(
            MissingInput::CurrentVerticalVelocity,
        ))?;
        if actual.to_bits() != expected_current.to_bits() {
            return Err(ApplyError::StaleVerticalVelocity {
                expected_bits: expected_current.to_bits(),
                actual_bits: actual.to_bits(),
            });
        }
    }
    Ok(commit_apply(state, plan))
}

fn validate(state: &MoveControlState, facts: MoveFacts) -> Result<(), PrepareError> {
    if !facts.position.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Position));
    }
    if !state.wanted.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Target));
    }
    if !facts.yaw.is_finite() || !facts.pitch.is_finite() {
        return Err(PrepareError::NonFinite(InputField::Rotation));
    }
    if !state.speed_modifier.is_finite()
        || !facts.movement_speed.is_finite()
        || facts.flying_speed.is_some_and(|v| !v.is_finite())
        || !state.strafe_forwards.is_finite()
        || !state.strafe_right.is_finite()
    {
        return Err(PrepareError::NonFinite(InputField::Speed));
    }
    if !facts.max_up_step.is_finite() || !facts.body_width.is_finite() {
        return Err(PrepareError::NonFinite(InputField::BodyDimensions));
    }
    if facts
        .collision
        .flatten()
        .is_some_and(|shape| !shape.top_y.is_finite())
    {
        return Err(PrepareError::NonFinite(InputField::Collision));
    }
    Ok(())
}
