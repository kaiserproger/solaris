//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! Part of the Solaris engine.

use std::net::{IpAddr, SocketAddr};

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
    #[serde(default = "default_max_players")]
    pub max_players: u32,
}

/// Network-level server settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}

fn default_max_players() -> u32 {
    20
}

impl ServerConfig {
    /// Convert a parsed TOML config into the network-layer [`mc_net::ServerConfig`].
    ///
    /// Returns an error if `bind_address` is not a valid IP literal — we
    /// do not do hostname resolution at startup, which keeps boot
    /// deterministic.
    pub fn to_network(&self) -> Result<mc_net::ServerConfig, std::net::AddrParseError> {
        let ip: IpAddr = self.network.bind_address.parse()?;
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
        })
    }
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
        assert_eq!(cfg.server.max_players, 20);
        assert_eq!(cfg.network.port, 25565);
    }

    #[test]
    fn translates_to_network_config() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "Howdy"
            max_players = 50

            [network]
            bind_address = "127.0.0.1"
            port = 25000
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let net = cfg.to_network().unwrap();
        assert_eq!(net.motd, "Howdy");
        assert_eq!(net.max_players, 50);
        assert_eq!(net.bind_address.port(), 25000);
    }

    #[test]
    fn invalid_bind_address_is_rejected() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = ""

            [network]
            bind_address = "not-an-ip"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        assert!(cfg.to_network().is_err());
    }
}
