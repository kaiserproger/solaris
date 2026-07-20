use std::panic::catch_unwind;

use super::{
    CostViolation, MAX_CELL_EVALUATIONS, MAX_NEIGHBORS, MAX_SEARCH_NODES, Neighbor, NodeEvaluator,
    NodePos, PathNode, PathType, SearchBudget, SearchBudgetError, SearchCost, SearchError,
    SearchGoal, SearchGoalError, SearchScratch, SearchTermination,
};

#[derive(Debug, Clone, Copy)]
struct Edge {
    from: NodePos,
    node: PathNode,
    cost: f32,
}

impl Edge {
    fn new(from: NodePos, to: NodePos, cost: f32) -> Self {
        Self {
            from,
            node: PathNode::new(to, PathType::Open, 0.0),
            cost,
        }
    }

    fn with_malus(from: NodePos, to: NodePos, cost: f32, malus: f32) -> Self {
        Self {
            from,
            node: PathNode::new(to, PathType::Open, malus),
            cost,
        }
    }

    fn with_type(from: NodePos, to: NodePos, cost: f32, path_type: PathType) -> Self {
        Self {
            from,
            node: PathNode::new(to, path_type, 0.0),
            cost,
        }
    }
}

struct GraphEvaluator {
    edges: Vec<Edge>,
    heuristic: fn(NodePos, SearchGoal) -> f32,
    neighbor_calls: usize,
    heuristic_calls: usize,
}

impl GraphEvaluator {
    fn new(edges: Vec<Edge>) -> Self {
        Self {
            edges,
            heuristic: region_heuristic,
            neighbor_calls: 0,
            heuristic_calls: 0,
        }
    }

    fn with_heuristic(edges: Vec<Edge>, heuristic: fn(NodePos, SearchGoal) -> f32) -> Self {
        Self {
            edges,
            heuristic,
            neighbor_calls: 0,
            heuristic_calls: 0,
        }
    }
}

impl NodeEvaluator for GraphEvaluator {
    fn neighbor_count(&self, current: PathNode) -> usize {
        self.edges
            .iter()
            .filter(|edge| edge.from == current.pos)
            .count()
    }

    fn neighbor(&mut self, current: PathNode, index: usize) -> Option<Neighbor> {
        self.neighbor_calls += 1;
        self.edges
            .iter()
            .filter(|edge| edge.from == current.pos)
            .nth(index)
            .map(|edge| Neighbor::new(edge.node, edge.cost))
    }

    fn heuristic(&mut self, pos: NodePos, goal: SearchGoal) -> f32 {
        self.heuristic_calls += 1;
        (self.heuristic)(pos, goal)
    }
}

fn pos(x: i32, y: i32, z: i32) -> NodePos {
    NodePos::new(x, y, z)
}

fn open(pos: NodePos) -> PathNode {
    PathNode::new(pos, PathType::Open, 0.0)
}

fn goal(center: NodePos, reach_radius: f32) -> SearchGoal {
    SearchGoal::new(center, reach_radius).expect("test goal must be valid")
}

fn scratch(max_nodes: usize, max_cell_evaluations: usize) -> SearchScratch {
    let budget =
        SearchBudget::new(max_nodes, max_cell_evaluations).expect("test budget must be valid");
    SearchScratch::try_new(budget).expect("small test scratch allocation must succeed")
}

fn path_positions(result: &super::SearchResult<'_>) -> Vec<NodePos> {
    result.path.iter().map(|node| node.pos).collect()
}

fn region_heuristic(pos: NodePos, goal: SearchGoal) -> f32 {
    goal.euclidean_lower_bound(pos)
}

fn zero_heuristic(_pos: NodePos, _goal: SearchGoal) -> f32 {
    0.0
}

fn nan_heuristic(_pos: NodePos, _goal: SearchGoal) -> f32 {
    f32::NAN
}

fn infinite_heuristic(_pos: NodePos, _goal: SearchGoal) -> f32 {
    f32::INFINITY
}

fn negative_heuristic(_pos: NodePos, _goal: SearchGoal) -> f32 {
    -1.0
}

fn admissible_inconsistent_heuristic(pos: NodePos, _goal: SearchGoal) -> f32 {
    if pos == NodePos::new(0, 1, 0) {
        10.0
    } else {
        0.0
    }
}

#[test]
fn budgets_accept_exact_hard_caps_and_reject_invalid_limits() {
    assert!(SearchBudget::new(MAX_SEARCH_NODES, MAX_CELL_EVALUATIONS).is_ok());
    assert_eq!(SearchBudget::new(0, 1), Err(SearchBudgetError::ZeroNodes));
    assert_eq!(
        SearchBudget::new(MAX_SEARCH_NODES + 1, 1),
        Err(SearchBudgetError::TooManyNodes)
    );
    assert_eq!(
        SearchBudget::new(1, 0),
        Err(SearchBudgetError::ZeroCellEvaluations)
    );
    assert_eq!(
        SearchBudget::new(1, MAX_CELL_EVALUATIONS + 1),
        Err(SearchBudgetError::TooManyCellEvaluations)
    );
}

#[test]
fn usize_max_budgets_are_rejected_without_panicking() {
    let result = catch_unwind(|| {
        (
            SearchBudget::new(usize::MAX, 1),
            SearchBudget::new(1, usize::MAX),
        )
    });

    let (nodes, evaluations) = result.expect("budget validation must not panic");
    assert_eq!(nodes, Err(SearchBudgetError::TooManyNodes));
    assert_eq!(evaluations, Err(SearchBudgetError::TooManyCellEvaluations));
}

#[test]
fn goals_reject_non_finite_and_negative_reach_radii() {
    let center = pos(0, 0, 0);

    assert_eq!(
        SearchGoal::new(center, f32::NAN),
        Err(SearchGoalError::NonFiniteReachRadius)
    );
    assert_eq!(
        SearchGoal::new(center, f32::INFINITY),
        Err(SearchGoalError::NonFiniteReachRadius)
    );
    assert_eq!(
        SearchGoal::new(center, -1.0),
        Err(SearchGoalError::NegativeReachRadius)
    );
}

#[test]
fn scratch_construction_is_fallible_and_reserves_every_search_container() {
    let budget = SearchBudget::new(8, 16).expect("test budget must be valid");
    let scratch = SearchScratch::try_new(budget).expect("small allocation must succeed");
    let capacities = scratch.capacities();

    assert!(capacities.records >= budget.max_nodes());
    assert!(capacities.index >= budget.max_nodes());
    assert!(capacities.open_heap >= budget.max_nodes());
    assert!(capacities.path >= budget.max_nodes());
}

#[test]
fn reach_radius_uses_an_admissible_goal_region_lower_bound() {
    let start = pos(0, 0, 0);
    let center = pos(2, 0, 0);
    let cheaper_boundary = pos(1, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::new(start, center, 5.0),
        Edge::new(start, cheaper_boundary, 4.5),
    ]);
    let mut scratch = scratch(3, 2);

    let result = scratch
        .search(&mut evaluator, open(start), goal(center, 1.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::Reached);
    assert_eq!(
        result.path.last().map(|node| node.pos),
        Some(cheaper_boundary)
    );
    assert_eq!(result.total_cost, 4.5);
}

#[test]
fn goal_region_lower_bound_never_rounds_above_the_exact_distance() {
    let center = pos(0, 0, 0);
    let candidate = pos(1, 5, 0);
    let goal = goal(center, 0.1);
    let exact = 1.0_f64.hypot(5.0) - 0.1_f64;

    assert!(f64::from(goal.euclidean_lower_bound(candidate)) <= exact);
}

#[test]
fn reaching_on_the_exact_evaluation_cap_is_not_exhaustion() {
    let start = pos(0, 0, 0);
    let middle = pos(1, 0, 0);
    let target = pos(2, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::new(start, middle, 1.0),
        Edge::new(start, target, 2.0),
    ]);
    let mut scratch = scratch(3, 2);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::Reached);
    assert_eq!(result.cell_evaluations, 2);
    assert_eq!(result.discovered_nodes, 3);
}

#[test]
fn evaluation_exhaustion_returns_a_bounded_partial_path() {
    let start = pos(0, 0, 0);
    let first = pos(1, 0, 0);
    let target = pos(3, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::new(start, first, 1.0),
        Edge::new(start, pos(2, 0, 0), 2.0),
    ]);
    let mut scratch = scratch(3, 1);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(
        result.termination,
        SearchTermination::CellEvaluationBudgetExhausted
    );
    assert_eq!(result.cell_evaluations, 1);
    assert_eq!(result.discovered_nodes, 2);
    assert_eq!(result.path.last().map(|node| node.pos), Some(first));
}

#[test]
fn duplicate_neighbors_each_consume_evaluation_budget() {
    let start = pos(0, 0, 0);
    let duplicate = pos(1, 0, 0);
    let target = pos(3, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::new(start, duplicate, 1.0),
        Edge::new(start, duplicate, 1.0),
        Edge::new(start, duplicate, 1.0),
    ]);
    let mut scratch = scratch(3, 2);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(
        result.termination,
        SearchTermination::CellEvaluationBudgetExhausted
    );
    assert_eq!(result.cell_evaluations, 2);
    assert_eq!(result.discovered_nodes, 2);
    assert_eq!(evaluator.neighbor_calls, 2);
}

struct RejectingEvaluator {
    candidate_count: usize,
    calls: usize,
}

impl NodeEvaluator for RejectingEvaluator {
    fn neighbor_count(&self, _current: PathNode) -> usize {
        self.candidate_count
    }

    fn neighbor(&mut self, _current: PathNode, _index: usize) -> Option<Neighbor> {
        self.calls += 1;
        None
    }
}

#[test]
fn rejected_candidates_consume_evaluation_budget_before_the_call() {
    let start = pos(0, 0, 0);
    let mut evaluator = RejectingEvaluator {
        candidate_count: 2,
        calls: 0,
    };
    let mut scratch = scratch(2, 1);

    let result = scratch
        .search(&mut evaluator, open(start), goal(pos(3, 0, 0), 0.0))
        .expect("default heuristic is valid");

    assert_eq!(
        result.termination,
        SearchTermination::CellEvaluationBudgetExhausted
    );
    assert_eq!(result.cell_evaluations, 1);
    assert_eq!(result.discovered_nodes, 1);
    assert_eq!(evaluator.calls, 1);
}

#[test]
fn node_exhaustion_stops_at_the_exact_record_cap() {
    let start = pos(0, 0, 0);
    let accepted = pos(1, 0, 0);
    let target = pos(3, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::new(start, accepted, 1.0),
        Edge::new(start, pos(2, 0, 0), 2.0),
    ]);
    let mut scratch = scratch(2, 2);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::NodeBudgetExhausted);
    assert_eq!(result.discovered_nodes, 2);
    assert_eq!(result.cell_evaluations, 2);
    assert_eq!(result.path.last().map(|node| node.pos), Some(accepted));
}

#[test]
fn invalid_start_malus_is_rejected_before_evaluator_calls() {
    for (malus, violation) in [
        (f32::NAN, CostViolation::NonFinite),
        (f32::INFINITY, CostViolation::NonFinite),
        (-1.0, CostViolation::Negative),
    ] {
        let start = pos(0, 0, 0);
        let mut evaluator = GraphEvaluator::new(Vec::new());
        let mut scratch = scratch(1, 1);
        let error = scratch
            .search(
                &mut evaluator,
                PathNode::new(start, PathType::Open, malus),
                goal(pos(1, 0, 0), 0.0),
            )
            .expect_err("invalid start malus must fail");

        assert_eq!(
            error,
            SearchError::InvalidCost {
                cost: SearchCost::StartMalus,
                violation,
                at: start,
            }
        );
        assert_eq!(evaluator.neighbor_calls, 0);
        assert_eq!(evaluator.heuristic_calls, 0);
    }
}

#[test]
fn non_finite_and_negative_edge_costs_are_rejected() {
    for (cost, violation) in [
        (f32::NAN, CostViolation::NonFinite),
        (f32::INFINITY, CostViolation::NonFinite),
        (-1.0, CostViolation::Negative),
    ] {
        let start = pos(0, 0, 0);
        let neighbor = pos(1, 0, 0);
        let mut evaluator = GraphEvaluator::new(vec![Edge::new(start, neighbor, cost)]);
        let mut scratch = scratch(2, 1);
        let error = scratch
            .search(&mut evaluator, open(start), goal(pos(2, 0, 0), 0.0))
            .expect_err("invalid edge cost must fail");

        assert_eq!(
            error,
            SearchError::InvalidCost {
                cost: SearchCost::Edge,
                violation,
                at: neighbor,
            }
        );
    }
}

#[test]
fn non_finite_and_negative_neighbor_malus_is_rejected() {
    for (malus, violation) in [
        (f32::NAN, CostViolation::NonFinite),
        (f32::INFINITY, CostViolation::NonFinite),
        (-1.0, CostViolation::Negative),
    ] {
        let start = pos(0, 0, 0);
        let neighbor = pos(1, 0, 0);
        let mut evaluator =
            GraphEvaluator::new(vec![Edge::with_malus(start, neighbor, 1.0, malus)]);
        let mut scratch = scratch(2, 1);
        let error = scratch
            .search(&mut evaluator, open(start), goal(pos(2, 0, 0), 0.0))
            .expect_err("invalid neighbor malus must fail");

        assert_eq!(
            error,
            SearchError::InvalidCost {
                cost: SearchCost::Malus,
                violation,
                at: neighbor,
            }
        );
    }
}

#[test]
fn non_finite_and_negative_heuristics_are_rejected() {
    for (heuristic, violation) in [
        (
            nan_heuristic as fn(NodePos, SearchGoal) -> f32,
            CostViolation::NonFinite,
        ),
        (infinite_heuristic, CostViolation::NonFinite),
        (negative_heuristic, CostViolation::Negative),
    ] {
        let start = pos(0, 0, 0);
        let mut evaluator = GraphEvaluator::with_heuristic(Vec::new(), heuristic);
        let mut scratch = scratch(1, 1);
        let error = scratch
            .search(&mut evaluator, open(start), goal(pos(1, 0, 0), 0.0))
            .expect_err("invalid heuristic must fail");

        assert_eq!(
            error,
            SearchError::InvalidCost {
                cost: SearchCost::Heuristic,
                violation,
                at: start,
            }
        );
    }
}

#[test]
fn accumulated_cost_overflow_is_rejected() {
    let start = pos(0, 0, 0);
    let middle = pos(1, 0, 0);
    let target = pos(2, 0, 0);
    let mut evaluator = GraphEvaluator::with_heuristic(
        vec![
            Edge::new(start, middle, f32::MAX),
            Edge::new(middle, target, f32::MAX),
        ],
        zero_heuristic,
    );
    let mut scratch = scratch(3, 2);
    let error = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect_err("overflowed accumulated cost must fail");

    assert_eq!(
        error,
        SearchError::InvalidCost {
            cost: SearchCost::Accumulated,
            violation: CostViolation::Overflow,
            at: target,
        }
    );
}

#[test]
fn decrease_key_updates_parent_and_heap_slot() {
    let start = pos(0, 0, 0);
    let improved = pos(2, 0, 0);
    let detour = pos(1, 0, 0);
    let distractor = pos(0, 1, 0);
    let target = pos(3, 0, 0);
    let mut evaluator = GraphEvaluator::with_heuristic(
        vec![
            Edge::new(start, improved, 10.0),
            Edge::new(start, distractor, 4.0),
            Edge::new(start, detour, 1.0),
            Edge::new(detour, improved, 1.0),
            Edge::new(improved, target, 1.0),
        ],
        zero_heuristic,
    );
    let mut scratch = scratch(5, 5);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::Reached);
    assert_eq!(
        path_positions(&result),
        vec![start, detour, improved, target]
    );
    assert_eq!(result.total_cost, 3.0);
}

#[test]
fn better_path_reopens_a_closed_node_and_repairs_its_parent() {
    let start = pos(0, 0, 0);
    let reopened = pos(1, 0, 0);
    let detour = pos(0, 1, 0);
    let target = pos(2, 0, 0);
    let mut evaluator = GraphEvaluator::with_heuristic(
        vec![
            Edge::new(start, reopened, 3.0),
            Edge::new(start, detour, 1.0),
            Edge::new(detour, reopened, 1.0),
            Edge::new(reopened, target, 10.0),
        ],
        admissible_inconsistent_heuristic,
    );
    let mut scratch = scratch(4, 5);

    let result = scratch
        .search(&mut evaluator, open(start), goal(target, 0.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::Reached);
    assert_eq!(
        path_positions(&result),
        vec![start, detour, reopened, target]
    );
    assert_eq!(result.total_cost, 12.0);
}

#[test]
fn equal_cost_ties_follow_stable_emission_order_across_reuse() {
    let start = pos(0, 0, 0);
    let first = pos(1, 1, 0);
    let second = pos(1, -1, 0);
    let target = pos(2, 0, 0);
    let mut evaluator = GraphEvaluator::with_heuristic(
        vec![
            Edge::new(start, first, 1.0),
            Edge::new(start, second, 1.0),
            Edge::new(first, target, 1.0),
            Edge::new(second, target, 1.0),
        ],
        zero_heuristic,
    );
    let mut scratch = scratch(4, 4);

    for _ in 0..16 {
        let result = scratch
            .search(&mut evaluator, open(start), goal(target, 0.0))
            .expect("fixture costs are valid");
        assert_eq!(path_positions(&result), vec![start, first, target]);
    }
}

#[test]
fn unreachable_search_returns_the_start_path() {
    let start = pos(0, 0, 0);
    let mut evaluator = GraphEvaluator::new(Vec::new());
    let mut scratch = scratch(2, 1);

    let result = scratch
        .search(&mut evaluator, open(start), goal(pos(3, 0, 0), 0.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::NoPath);
    assert_eq!(result.path, &[open(start)]);
    assert_eq!(result.visited_nodes, 1);
    assert_eq!(result.cell_evaluations, 0);
}

#[test]
fn start_inside_goal_region_reaches_without_expansion() {
    let start = pos(1, 0, 0);
    let mut evaluator = GraphEvaluator::new(Vec::new());
    let mut scratch = scratch(1, 1);

    let result = scratch
        .search(&mut evaluator, open(start), goal(pos(2, 0, 0), 1.0))
        .expect("fixture costs are valid");

    assert_eq!(result.termination, SearchTermination::Reached);
    assert_eq!(result.path, &[open(start)]);
    assert_eq!(result.cell_evaluations, 0);
    assert_eq!(evaluator.neighbor_calls, 0);
}

#[test]
fn evaluator_cannot_exceed_the_per_expansion_neighbor_bound() {
    let start = pos(0, 0, 0);
    let edges = (0..=MAX_NEIGHBORS)
        .map(|index| {
            Edge::new(
                start,
                pos(
                    i32::try_from(index + 1).expect("fixture index fits i32"),
                    0,
                    0,
                ),
                1.0,
            )
        })
        .collect();
    let mut evaluator = GraphEvaluator::new(edges);
    let mut scratch = scratch(MAX_NEIGHBORS + 1, MAX_NEIGHBORS + 1);
    let error = scratch
        .search(&mut evaluator, open(start), goal(pos(100, 0, 0), 0.0))
        .expect_err("evaluator overflow must fail");

    assert_eq!(error, SearchError::NeighborLimitExceeded { at: start });
    assert_eq!(evaluator.neighbor_calls, 0);
}

#[test]
fn scratch_reuse_preserves_capacity_and_clears_previous_search_state() {
    let start = pos(0, 0, 0);
    let middle = pos(1, 0, 0);
    let target = pos(2, 0, 0);
    let mut evaluator = GraphEvaluator::new(vec![
        Edge::with_type(start, middle, 1.0, PathType::Walkable),
        Edge::with_type(middle, target, 1.0, PathType::Water),
    ]);
    let mut scratch = scratch(3, 2);
    let capacities = scratch.capacities();

    for _ in 0..16 {
        let result = scratch
            .search(&mut evaluator, open(start), goal(target, 0.0))
            .expect("fixture costs are valid");
        assert_eq!(result.termination, SearchTermination::Reached);
        assert_eq!(result.cell_evaluations, 2);
        assert_eq!(result.path[1].path_type, PathType::Walkable);
        assert_eq!(result.path[2].path_type, PathType::Water);
    }

    assert_eq!(scratch.capacities(), capacities);
}
