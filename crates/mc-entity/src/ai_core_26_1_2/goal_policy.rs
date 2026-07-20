use std::ops::{BitOr, BitOrAssign};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GoalId(pub(crate) u16);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct ControlFlags(u8);

impl ControlFlags {
    pub(crate) const MOVE: Self = Self(1 << 0);
    pub(crate) const LOOK: Self = Self(1 << 1);
    pub(crate) const JUMP: Self = Self(1 << 2);
    pub(crate) const TARGET: Self = Self(1 << 3);

    const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

impl BitOr for ControlFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ControlFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoalDefinition {
    id: GoalId,
    priority: u16,
    controls: ControlFlags,
    interruptible: bool,
}

impl GoalDefinition {
    pub(crate) const fn new(
        id: GoalId,
        priority: u16,
        controls: ControlFlags,
        interruptible: bool,
    ) -> Self {
        Self {
            id,
            priority,
            controls,
            interruptible,
        }
    }
}

/// One caller-owned snapshot used for a single policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoalInput {
    goal: GoalDefinition,
    running: bool,
    can_start: bool,
    can_continue: bool,
}

impl GoalInput {
    pub(crate) const fn new(
        goal: GoalDefinition,
        running: bool,
        can_start: bool,
        can_continue: bool,
    ) -> Self {
        Self {
            goal,
            running,
            can_start,
            can_continue,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalTransition {
    Stop(GoalId),
    Start(GoalId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalPlanError {
    DuplicateGoalId(GoalId),
    OverlappingRunningControls {
        first: GoalId,
        second: GoalId,
        controls: ControlFlags,
    },
}

/// Reusable planner buffers owned by one regional worker or batch lane.
///
/// Retain the scratch value across evaluations. Sharing one value between concurrent workers would
/// serialize otherwise independent policy work.
#[derive(Debug)]
pub(crate) struct GoalPolicyScratch {
    pub(super) selected: Vec<bool>,
    pub(super) priority_order: Vec<usize>,
    pub(super) stopped: Vec<GoalId>,
    pub(super) started: Vec<GoalId>,
}

impl GoalPolicyScratch {
    pub(crate) fn with_capacity(goal_capacity: usize) -> Self {
        Self {
            selected: Vec::with_capacity(goal_capacity),
            priority_order: Vec::with_capacity(goal_capacity),
            stopped: Vec::with_capacity(goal_capacity),
            started: Vec::with_capacity(goal_capacity),
        }
    }

    fn prepare(&mut self, goal_count: usize) {
        self.selected.clear();
        self.selected.reserve(goal_count);
        self.selected.resize(goal_count, false);
        self.priority_order.clear();
        self.priority_order.reserve(goal_count);
        self.priority_order.extend(0..goal_count);
        self.stopped.clear();
        self.stopped.reserve(goal_count);
        self.started.clear();
        self.started.reserve(goal_count);
    }
}

/// Plans control transitions without reading or mutating authoritative entity state.
///
/// Lower numeric priorities win and input order breaks ties. On success, `output` is replaced with
/// every stop followed by every start; each start is a final winner for this evaluation. Validation
/// errors leave `output` unchanged.
pub(crate) fn plan_goal_transitions(
    inputs: &[GoalInput],
    scratch: &mut GoalPolicyScratch,
    output: &mut Vec<GoalTransition>,
) -> Result<(), GoalPlanError> {
    for (index, input) in inputs.iter().enumerate() {
        if inputs[..index]
            .iter()
            .any(|previous| previous.goal.id == input.goal.id)
        {
            return Err(GoalPlanError::DuplicateGoalId(input.goal.id));
        }
    }

    for (second_index, second) in inputs.iter().enumerate() {
        if !second.running {
            continue;
        }
        for first in &inputs[..second_index] {
            if !first.running {
                continue;
            }
            let controls = first.goal.controls.intersection(second.goal.controls);
            if controls.0 != 0 {
                return Err(GoalPlanError::OverlappingRunningControls {
                    first: first.goal.id,
                    second: second.goal.id,
                    controls,
                });
            }
        }
    }

    scratch.prepare(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        scratch.selected[index] = input.running && input.can_continue;
        if input.running && !input.can_continue {
            scratch.stopped.push(input.goal.id);
        }
    }
    scratch
        .priority_order
        .sort_unstable_by_key(|&index| (inputs[index].goal.priority, index));

    for &candidate_index in &scratch.priority_order {
        let candidate = inputs[candidate_index];
        if scratch.selected[candidate_index] || !candidate.can_start {
            continue;
        }

        let can_replace_every_conflict = inputs.iter().enumerate().all(|(owner_index, owner)| {
            !scratch.selected[owner_index]
                || !candidate.goal.controls.intersects(owner.goal.controls)
                || (owner.goal.interruptible && candidate.goal.priority < owner.goal.priority)
        });
        if !can_replace_every_conflict {
            continue;
        }

        for (owner_index, owner) in inputs.iter().enumerate() {
            if scratch.selected[owner_index]
                && candidate.goal.controls.intersects(owner.goal.controls)
            {
                scratch.selected[owner_index] = false;
                scratch.stopped.push(owner.goal.id);
            }
        }

        scratch.selected[candidate_index] = true;
        scratch.started.push(candidate.goal.id);
    }

    output.clear();
    output.reserve(inputs.len().saturating_mul(2));
    output.extend(scratch.stopped.iter().copied().map(GoalTransition::Stop));
    output.extend(scratch.started.iter().copied().map(GoalTransition::Start));
    Ok(())
}
