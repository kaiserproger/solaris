use super::goal_policy::{
    ControlFlags, GoalDefinition, GoalId, GoalInput, GoalPlanError, GoalPolicyScratch,
    GoalTransition, plan_goal_transitions,
};

fn goal(id: u16, priority: u16, controls: ControlFlags, interruptible: bool) -> GoalDefinition {
    GoalDefinition::new(GoalId(id), priority, controls, interruptible)
}

fn input(goal: GoalDefinition, running: bool, can_start: bool, can_continue: bool) -> GoalInput {
    GoalInput::new(goal, running, can_start, can_continue)
}

fn plan(inputs: &[GoalInput]) -> Result<Vec<GoalTransition>, GoalPlanError> {
    let mut scratch = GoalPolicyScratch::with_capacity(inputs.len());
    let mut output = Vec::with_capacity(inputs.len().saturating_mul(2));
    plan_goal_transitions(inputs, &mut scratch, &mut output)?;
    Ok(output)
}

#[test]
fn ordered_preemption_stops_owner_before_starting_candidate() {
    let wander = goal(1, 5, ControlFlags::MOVE, true);
    let panic = goal(2, 1, ControlFlags::MOVE, true);

    let transitions = plan(&[
        input(wander, true, false, true),
        input(panic, false, true, false),
    ])
    .unwrap();

    assert_eq!(
        transitions,
        vec![
            GoalTransition::Stop(GoalId(1)),
            GoalTransition::Start(GoalId(2))
        ]
    );
}

#[test]
fn disjoint_controls_start_in_priority_order() {
    let movement = goal(1, 5, ControlFlags::MOVE, true);
    let low_priority_look = goal(2, 8, ControlFlags::LOOK, true);
    let high_priority_target = goal(3, 2, ControlFlags::TARGET, true);

    let transitions = plan(&[
        input(movement, true, false, true),
        input(low_priority_look, false, true, false),
        input(high_priority_target, false, true, false),
    ])
    .unwrap();

    assert_eq!(
        transitions,
        vec![
            GoalTransition::Start(GoalId(3)),
            GoalTransition::Start(GoalId(2))
        ]
    );
}

#[test]
fn noninterruptible_owner_blocks_conflicting_candidate() {
    let owner = goal(1, 5, ControlFlags::MOVE | ControlFlags::LOOK, false);
    let candidate = goal(2, 1, ControlFlags::MOVE, true);

    let transitions = plan(&[
        input(owner, true, false, true),
        input(candidate, false, true, false),
    ])
    .unwrap();

    assert!(transitions.is_empty());
}

#[test]
fn failed_continuation_stops_running_goal() {
    let owner = goal(1, 5, ControlFlags::MOVE, true);

    let transitions = plan(&[input(owner, true, false, false)]).unwrap();

    assert_eq!(transitions, vec![GoalTransition::Stop(GoalId(1))]);
}

#[test]
fn candidate_starts_after_failed_continuation_releases_controls() {
    let owner = goal(1, 1, ControlFlags::MOVE, false);
    let candidate = goal(2, 9, ControlFlags::MOVE, true);

    let transitions = plan(&[
        input(owner, true, false, false),
        input(candidate, false, true, false),
    ])
    .unwrap();

    assert_eq!(
        transitions,
        vec![
            GoalTransition::Stop(GoalId(1)),
            GoalTransition::Start(GoalId(2))
        ]
    );
}

#[test]
fn duplicate_goal_ids_are_rejected() {
    let inputs = [
        input(goal(7, 1, ControlFlags::MOVE, true), false, true, false),
        input(goal(7, 2, ControlFlags::LOOK, true), false, true, false),
    ];
    let mut scratch = GoalPolicyScratch::with_capacity(inputs.len());
    let mut output = vec![GoalTransition::Stop(GoalId(99))];

    let result = plan_goal_transitions(&inputs, &mut scratch, &mut output);

    assert_eq!(result, Err(GoalPlanError::DuplicateGoalId(GoalId(7))));
    assert_eq!(output, vec![GoalTransition::Stop(GoalId(99))]);
}

#[test]
fn priority_sort_prevents_start_then_lost_transitions() {
    let low_priority = goal(1, 9, ControlFlags::MOVE, true);
    let high_priority = goal(2, 1, ControlFlags::MOVE, true);

    let transitions = plan(&[
        input(low_priority, false, true, false),
        input(high_priority, false, true, false),
    ])
    .unwrap();

    assert_eq!(transitions, vec![GoalTransition::Start(GoalId(2))]);
}

#[test]
fn equal_priority_conflicts_keep_input_order() {
    let first = goal(1, 3, ControlFlags::JUMP, true);
    let second = goal(2, 3, ControlFlags::JUMP, true);

    let transitions = plan(&[
        input(first, false, true, false),
        input(second, false, true, false),
    ])
    .unwrap();

    assert_eq!(transitions, vec![GoalTransition::Start(GoalId(1))]);
}

#[test]
fn overlapping_continuing_controls_are_rejected_without_mutating_output() {
    let first = goal(1, 1, ControlFlags::MOVE | ControlFlags::LOOK, true);
    let second = goal(2, 2, ControlFlags::LOOK | ControlFlags::TARGET, true);
    let inputs = [
        input(first, true, false, true),
        input(second, true, false, true),
    ];
    let mut scratch = GoalPolicyScratch::with_capacity(inputs.len());
    let mut output = Vec::with_capacity(8);
    output.push(GoalTransition::Start(GoalId(99)));
    let output_pointer = output.as_ptr();
    let output_capacity = output.capacity();

    let result = plan_goal_transitions(&inputs, &mut scratch, &mut output);

    assert_eq!(
        result,
        Err(GoalPlanError::OverlappingRunningControls {
            first: GoalId(1),
            second: GoalId(2),
            controls: ControlFlags::LOOK,
        })
    );
    assert_eq!(output, vec![GoalTransition::Start(GoalId(99))]);
    assert_eq!(output.as_ptr(), output_pointer);
    assert_eq!(output.capacity(), output_capacity);
}

#[test]
fn warmed_planner_reuses_all_buffer_capacities_and_preserves_ordering() {
    let continuing = goal(1, 5, ControlFlags::MOVE, true);
    let stopping = goal(2, 2, ControlFlags::LOOK, false);
    let preempting = goal(3, 1, ControlFlags::MOVE, true);
    let released_start = goal(4, 8, ControlFlags::LOOK, true);
    let inputs = [
        input(continuing, true, false, true),
        input(stopping, true, false, false),
        input(preempting, false, true, false),
        input(released_start, false, true, false),
    ];
    let expected = [
        GoalTransition::Stop(GoalId(2)),
        GoalTransition::Stop(GoalId(1)),
        GoalTransition::Start(GoalId(3)),
        GoalTransition::Start(GoalId(4)),
    ];
    let mut scratch = GoalPolicyScratch::with_capacity(0);
    let mut output = Vec::new();

    plan_goal_transitions(&inputs, &mut scratch, &mut output).unwrap();
    assert_eq!(output, expected);
    let capacities = (
        scratch.selected.capacity(),
        scratch.priority_order.capacity(),
        scratch.stopped.capacity(),
        scratch.started.capacity(),
        output.capacity(),
    );
    assert!(capacities.0 >= inputs.len());
    assert!(capacities.1 >= inputs.len());
    assert!(capacities.2 >= inputs.len());
    assert!(capacities.3 >= inputs.len());
    assert!(capacities.4 >= inputs.len().saturating_mul(2));

    for _ in 0..32 {
        plan_goal_transitions(&inputs, &mut scratch, &mut output).unwrap();
        assert_eq!(output, expected);
        assert_eq!(
            (
                scratch.selected.capacity(),
                scratch.priority_order.capacity(),
                scratch.stopped.capacity(),
                scratch.started.capacity(),
                output.capacity(),
            ),
            capacities
        );
    }
}
