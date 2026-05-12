//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Part of the Solaris engine.

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
