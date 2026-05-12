//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! In M0 this crate only exposes the configuration types used by the binary
//! and its integration test. Runtime wiring arrives in later milestones.
//!
//! Part of the Solaris engine.

use serde::{Deserialize, Serialize};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level server configuration loaded from a TOML file at startup.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub network: NetworkSection,
}

/// Identity-level server settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerSection {
    pub name: String,
    pub motd: String,
}

/// Network-level server settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config_shape() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.server.name, "S");
        assert_eq!(cfg.network.port, 25565);
    }
}
