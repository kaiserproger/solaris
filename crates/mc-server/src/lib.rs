//! # mc-server
//!
//! Public server configuration facade and shared server utilities.
//!
//! The binary composes startup; `config` owns schema, access-control file policy,
//! and runtime translation.

mod config;
pub use config::*;

pub mod dashboard;
pub mod dashboard_stats;
#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod dashboard_tests;
pub mod profile;
pub mod startup_data;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
