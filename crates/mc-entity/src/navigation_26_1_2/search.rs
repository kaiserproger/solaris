use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use super::{MAX_NEIGHBORS, NodeEvaluator, NodePos, PathNode, SearchGoal};

const NOT_IN_HEAP: usize = usize::MAX;

pub const MAX_SEARCH_NODES: usize = 16_384;
pub const MAX_CELL_EVALUATIONS: usize = MAX_SEARCH_NODES * MAX_NEIGHBORS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBudget {
    max_nodes: usize,
    max_cell_evaluations: usize,
}

impl SearchBudget {
    pub fn new(max_nodes: usize, max_cell_evaluations: usize) -> Result<Self, SearchBudgetError> {
        if max_nodes == 0 {
            return Err(SearchBudgetError::ZeroNodes);
        }
        if max_nodes > MAX_SEARCH_NODES {
            return Err(SearchBudgetError::TooManyNodes);
        }
        if max_cell_evaluations == 0 {
            return Err(SearchBudgetError::ZeroCellEvaluations);
        }
        if max_cell_evaluations > MAX_CELL_EVALUATIONS {
            return Err(SearchBudgetError::TooManyCellEvaluations);
        }
        Ok(Self {
            max_nodes,
            max_cell_evaluations,
        })
    }

    #[must_use]
    pub const fn max_nodes(self) -> usize {
        self.max_nodes
    }

    #[must_use]
    pub const fn max_cell_evaluations(self) -> usize {
        self.max_cell_evaluations
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBudgetError {
    ZeroNodes,
    TooManyNodes,
    ZeroCellEvaluations,
    TooManyCellEvaluations,
}

impl fmt::Display for SearchBudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroNodes => formatter.write_str("search budget must allow at least one node"),
            Self::TooManyNodes => formatter.write_str("search node budget exceeds the hard cap"),
            Self::ZeroCellEvaluations => {
                formatter.write_str("search budget must allow at least one cell evaluation")
            }
            Self::TooManyCellEvaluations => {
                formatter.write_str("cell-evaluation budget exceeds the hard cap")
            }
        }
    }
}

impl std::error::Error for SearchBudgetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchTermination {
    Reached,
    NoPath,
    NodeBudgetExhausted,
    CellEvaluationBudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCost {
    StartMalus,
    Edge,
    Malus,
    Heuristic,
    Accumulated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostViolation {
    NonFinite,
    Negative,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    InvalidCost {
        cost: SearchCost,
        violation: CostViolation,
        at: NodePos,
    },
    NeighborLimitExceeded {
        at: NodePos,
    },
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCost {
                cost,
                violation,
                at,
            } => write!(
                formatter,
                "invalid {cost:?} cost ({violation:?}) at ({}, {}, {})",
                at.x, at.y, at.z
            ),
            Self::NeighborLimitExceeded { at } => write!(
                formatter,
                "evaluator exceeded the neighbor limit at ({}, {}, {})",
                at.x, at.y, at.z
            ),
        }
    }
}

impl std::error::Error for SearchError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScratchError {
    AllocationFailed,
}

impl fmt::Display for SearchScratchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to allocate bounded search scratch")
    }
}

impl std::error::Error for SearchScratchError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScratchCapacities {
    pub records: usize,
    pub index: usize,
    pub open_heap: usize,
    pub path: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchResult<'a> {
    pub termination: SearchTermination,
    pub path: &'a [PathNode],
    pub visited_nodes: usize,
    pub discovered_nodes: usize,
    pub cell_evaluations: usize,
    pub total_cost: f32,
}

impl SearchResult<'_> {
    #[must_use]
    pub const fn reached(&self) -> bool {
        matches!(self.termination, SearchTermination::Reached)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy)]
struct Record {
    node: PathNode,
    parent: Option<usize>,
    g: f32,
    h: f32,
    f: f32,
    sequence: usize,
    heap_slot: usize,
    state: RecordState,
}

/// Allocation-reusing A* state sized once from a validated [`SearchBudget`].
///
/// A successful construction reserves every container to its hard search
/// bound. The returned path borrows this scratch and remains valid until its
/// next mutable use.
#[derive(Debug)]
pub struct SearchScratch {
    budget: SearchBudget,
    records: Vec<Record>,
    index: HashMap<NodePos, usize>,
    open_heap: Vec<usize>,
    path: Vec<PathNode>,
}

impl SearchScratch {
    pub fn try_new(budget: SearchBudget) -> Result<Self, SearchScratchError> {
        let mut records = Vec::new();
        records
            .try_reserve_exact(budget.max_nodes)
            .map_err(|_| SearchScratchError::AllocationFailed)?;

        let mut index = HashMap::new();
        index
            .try_reserve(budget.max_nodes)
            .map_err(|_| SearchScratchError::AllocationFailed)?;

        let mut open_heap = Vec::new();
        open_heap
            .try_reserve_exact(budget.max_nodes)
            .map_err(|_| SearchScratchError::AllocationFailed)?;

        let mut path = Vec::new();
        path.try_reserve_exact(budget.max_nodes)
            .map_err(|_| SearchScratchError::AllocationFailed)?;

        Ok(Self {
            budget,
            records,
            index,
            open_heap,
            path,
        })
    }

    #[must_use]
    pub const fn budget(&self) -> SearchBudget {
        self.budget
    }

    #[must_use]
    pub fn capacities(&self) -> ScratchCapacities {
        ScratchCapacities {
            records: self.records.capacity(),
            index: self.index.capacity(),
            open_heap: self.open_heap.capacity(),
            path: self.path.capacity(),
        }
    }

    pub fn search<'a, E: NodeEvaluator + ?Sized>(
        &'a mut self,
        evaluator: &mut E,
        start: PathNode,
        goal: SearchGoal,
    ) -> Result<SearchResult<'a>, SearchError> {
        self.reset();
        validate_cost(start.malus, SearchCost::StartMalus, start.pos)?;

        let start_h = evaluator.heuristic(start.pos, goal);
        validate_cost(start_h, SearchCost::Heuristic, start.pos)?;
        let start_f = checked_accumulation(0.0, start_h, start.pos)?;
        self.records.push(Record {
            node: start,
            parent: None,
            g: 0.0,
            h: start_h,
            f: start_f,
            sequence: 0,
            heap_slot: NOT_IN_HEAP,
            state: RecordState::Open,
        });
        self.index.insert(start.pos, 0);
        self.heap_push(0);

        let mut visited_nodes = 0usize;
        let mut cell_evaluations = 0usize;
        let mut reached = None;

        let termination = loop {
            let Some(current_index) = self.heap_pop() else {
                break SearchTermination::NoPath;
            };
            visited_nodes += 1;
            self.records[current_index].state = RecordState::Closed;
            let current = self.records[current_index];

            if goal.contains(current.node.pos) {
                reached = Some(current_index);
                break SearchTermination::Reached;
            }

            let candidate_count = evaluator.neighbor_count(current.node);
            if candidate_count > MAX_NEIGHBORS {
                return Err(SearchError::NeighborLimitExceeded {
                    at: current.node.pos,
                });
            }
            if candidate_count == 0 {
                continue;
            }

            let remaining_evaluations = self
                .budget
                .max_cell_evaluations
                .saturating_sub(cell_evaluations);
            if remaining_evaluations == 0 {
                break SearchTermination::CellEvaluationBudgetExhausted;
            }

            let evaluated_candidates = candidate_count.min(remaining_evaluations);
            let mut node_budget_exhausted = false;

            for neighbor_index in 0..evaluated_candidates {
                cell_evaluations += 1;
                let Some(neighbor) = evaluator.neighbor(current.node, neighbor_index) else {
                    continue;
                };
                validate_cost(neighbor.edge_cost, SearchCost::Edge, neighbor.node.pos)?;
                validate_cost(neighbor.node.malus, SearchCost::Malus, neighbor.node.pos)?;

                let transition_cost = checked_accumulation(
                    neighbor.edge_cost,
                    neighbor.node.malus,
                    neighbor.node.pos,
                )?;
                let tentative_g =
                    checked_accumulation(current.g, transition_cost, neighbor.node.pos)?;

                if let Some(&existing_index) = self.index.get(&neighbor.node.pos) {
                    let existing = self.records[existing_index];
                    if tentative_g.total_cmp(&existing.g) != Ordering::Less {
                        continue;
                    }

                    let f = checked_accumulation(tentative_g, existing.h, neighbor.node.pos)?;
                    self.records[existing_index].node = neighbor.node;
                    self.records[existing_index].parent = Some(current_index);
                    self.records[existing_index].g = tentative_g;
                    self.records[existing_index].f = f;
                    match existing.state {
                        RecordState::Open => self.heap_decrease(existing_index),
                        RecordState::Closed => {
                            self.records[existing_index].state = RecordState::Open;
                            self.heap_push(existing_index);
                        }
                    }
                    continue;
                }

                if self.records.len() >= self.budget.max_nodes {
                    node_budget_exhausted = true;
                    continue;
                }

                let h = evaluator.heuristic(neighbor.node.pos, goal);
                validate_cost(h, SearchCost::Heuristic, neighbor.node.pos)?;
                let f = checked_accumulation(tentative_g, h, neighbor.node.pos)?;
                let record_index = self.records.len();
                self.records.push(Record {
                    node: neighbor.node,
                    parent: Some(current_index),
                    g: tentative_g,
                    h,
                    f,
                    sequence: record_index,
                    heap_slot: NOT_IN_HEAP,
                    state: RecordState::Open,
                });
                self.index.insert(neighbor.node.pos, record_index);
                self.heap_push(record_index);
            }

            if evaluated_candidates < candidate_count {
                break SearchTermination::CellEvaluationBudgetExhausted;
            }
            if node_budget_exhausted {
                break SearchTermination::NodeBudgetExhausted;
            }
        };

        let end = reached.unwrap_or_else(|| self.closest_record(goal));
        self.reconstruct(end);
        Ok(SearchResult {
            termination,
            path: &self.path,
            visited_nodes,
            discovered_nodes: self.records.len(),
            cell_evaluations,
            total_cost: self.records[end].g,
        })
    }

    fn reset(&mut self) {
        self.records.clear();
        self.index.clear();
        self.open_heap.clear();
        self.path.clear();
    }

    fn closest_record(&self, goal: SearchGoal) -> usize {
        let mut best = 0usize;
        for candidate in 1..self.records.len() {
            if self.closest_order(candidate, best, goal) == Ordering::Less {
                best = candidate;
            }
        }
        best
    }

    fn closest_order(&self, left: usize, right: usize, goal: SearchGoal) -> Ordering {
        let left_record = self.records[left];
        let right_record = self.records[right];
        goal.euclidean_lower_bound(left_record.node.pos)
            .total_cmp(&goal.euclidean_lower_bound(right_record.node.pos))
            .then_with(|| left_record.g.total_cmp(&right_record.g))
            .then_with(|| left_record.sequence.cmp(&right_record.sequence))
            .then_with(|| left_record.node.pos.cmp(&right_record.node.pos))
    }

    fn reconstruct(&mut self, mut record_index: usize) {
        self.path.clear();
        loop {
            let record = self.records[record_index];
            self.path.push(record.node);
            let Some(parent) = record.parent else {
                break;
            };
            record_index = parent;
        }
        self.path.reverse();
    }

    fn heap_precedes(&self, left: usize, right: usize) -> bool {
        let left_record = self.records[left];
        let right_record = self.records[right];
        left_record
            .f
            .total_cmp(&right_record.f)
            .then_with(|| left_record.h.total_cmp(&right_record.h))
            .then_with(|| left_record.sequence.cmp(&right_record.sequence))
            .then_with(|| left_record.node.pos.cmp(&right_record.node.pos))
            == Ordering::Less
    }

    fn heap_push(&mut self, record_index: usize) {
        let slot = self.open_heap.len();
        self.open_heap.push(record_index);
        self.records[record_index].heap_slot = slot;
        self.heap_sift_up(slot);
    }

    fn heap_pop(&mut self) -> Option<usize> {
        let root = *self.open_heap.first()?;
        let last = self.open_heap.pop()?;
        self.records[root].heap_slot = NOT_IN_HEAP;
        if !self.open_heap.is_empty() {
            self.open_heap[0] = last;
            self.records[last].heap_slot = 0;
            self.heap_sift_down(0);
        }
        Some(root)
    }

    fn heap_decrease(&mut self, record_index: usize) {
        let slot = self.records[record_index].heap_slot;
        debug_assert_ne!(slot, NOT_IN_HEAP);
        if slot != NOT_IN_HEAP {
            self.heap_sift_up(slot);
        }
    }

    fn heap_sift_up(&mut self, mut slot: usize) {
        while slot > 0 {
            let parent = (slot - 1) / 2;
            if !self.heap_precedes(self.open_heap[slot], self.open_heap[parent]) {
                break;
            }
            self.heap_swap(slot, parent);
            slot = parent;
        }
    }

    fn heap_sift_down(&mut self, mut slot: usize) {
        loop {
            let left = slot * 2 + 1;
            if left >= self.open_heap.len() {
                break;
            }
            let right = left + 1;
            let mut next = left;
            if right < self.open_heap.len()
                && self.heap_precedes(self.open_heap[right], self.open_heap[left])
            {
                next = right;
            }
            if !self.heap_precedes(self.open_heap[next], self.open_heap[slot]) {
                break;
            }
            self.heap_swap(slot, next);
            slot = next;
        }
    }

    fn heap_swap(&mut self, left: usize, right: usize) {
        self.open_heap.swap(left, right);
        self.records[self.open_heap[left]].heap_slot = left;
        self.records[self.open_heap[right]].heap_slot = right;
    }
}

fn validate_cost(value: f32, cost: SearchCost, at: NodePos) -> Result<(), SearchError> {
    if !value.is_finite() {
        return Err(SearchError::InvalidCost {
            cost,
            violation: CostViolation::NonFinite,
            at,
        });
    }
    if value < 0.0 {
        return Err(SearchError::InvalidCost {
            cost,
            violation: CostViolation::Negative,
            at,
        });
    }
    Ok(())
}

fn checked_accumulation(left: f32, right: f32, at: NodePos) -> Result<f32, SearchError> {
    let total = left + right;
    if total.is_finite() {
        Ok(total)
    } else {
        Err(SearchError::InvalidCost {
            cost: SearchCost::Accumulated,
            violation: CostViolation::Overflow,
            at,
        })
    }
}
