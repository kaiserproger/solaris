use super::control_math::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JumpControlState {
    pub revision: u64,
    pub requested: bool,
}

impl Revisioned for JumpControlState {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn same_state(&self, other: &Self) -> bool {
        self == other
    }
}

impl JumpControlState {
    pub fn jump(&mut self) {
        self.requested = true;
    }
}

pub fn prepare_jump(
    state: &JumpControlState,
    stamp: InputStamp,
) -> Result<Prepared<JumpControlState>, PrepareError> {
    let mut next = *state;
    next.requested = false;
    let output = ControlOutput {
        jumping: Some(state.requested),
        ..ControlOutput::default()
    };
    prepared(state, stamp, next, output, |value, revision| {
        value.revision = revision
    })
}

pub fn apply_jump(
    state: &mut JumpControlState,
    stamp: InputStamp,
    plan: Prepared<JumpControlState>,
) -> Result<ControlOutput, ApplyError> {
    apply(state, stamp, plan)
}
