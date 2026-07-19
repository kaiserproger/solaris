//! # mc-test-harness
//!
//! Diff testing infrastructure.
//!
//! Part of the Solaris engine.

pub mod client;
pub mod parity;
pub mod replay;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
