use super::{NodePos, PathNode, SearchGoal};

/// Maximum candidate slots one expansion may expose.
pub const MAX_NEIGHBORS: usize = 26;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbor {
    pub node: PathNode,
    pub edge_cost: f32,
}

impl Neighbor {
    #[must_use]
    pub const fn new(node: PathNode, edge_cost: f32) -> Self {
        Self { node, edge_cost }
    }
}

/// Caller-owned topology and cost adapter for the bounded search kernel.
pub trait NodeEvaluator {
    /// Returns the deterministic number of candidate slots for `current`.
    /// The kernel rejects values above [`MAX_NEIGHBORS`] before evaluation.
    /// This method is topology-only: it must not inspect cells, collision, or
    /// other state whose work belongs under the cell-evaluation budget.
    fn neighbor_count(&self, current: PathNode) -> usize;

    /// Evaluates one candidate slot.
    ///
    /// Every invocation consumes one cell-evaluation budget unit before this
    /// method runs. Returning `None` rejects the candidate; duplicate nodes at
    /// different indices still consume separate units.
    fn neighbor(&mut self, current: PathNode, index: usize) -> Option<Neighbor>;

    /// Returns a finite nonnegative lower bound to any point accepted by
    /// `goal`. The default is admissible when `edge_cost + node.malus` is never
    /// less than Euclidean displacement. Cost models that do not guarantee
    /// that must override this with another lower bound, including zero. The
    /// value must remain stable for a position throughout one search.
    fn heuristic(&mut self, pos: NodePos, goal: SearchGoal) -> f32 {
        goal.euclidean_lower_bound(pos)
    }
}
