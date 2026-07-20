//! Deterministic bounded path-search mechanics reusable by 26.1.2 adapters.
//!
//! This module does not classify world collision, implement movement-mode
//! evaluators, or claim full vanilla navigation parity. Search output borrows
//! reusable scratch; consumers retaining a path must copy it into owned state.

mod evaluator;
mod node;
mod search;

pub use evaluator::{MAX_NEIGHBORS, Neighbor, NodeEvaluator};
pub use node::{NodePos, PathNode, PathType, SearchGoal, SearchGoalError};
pub use search::{
    CostViolation, MAX_CELL_EVALUATIONS, MAX_SEARCH_NODES, ScratchCapacities, SearchBudget,
    SearchBudgetError, SearchCost, SearchError, SearchResult, SearchScratch, SearchScratchError,
    SearchTermination,
};

#[cfg(test)]
mod tests;
