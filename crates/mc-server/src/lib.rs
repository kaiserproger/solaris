//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! Part of the Solaris engine.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use mc_data::VanillaData;
use mc_data::tags::TagsData;
use mc_net::WorldHandle;
use mc_world::BlockRegistry;
use serde::{Deserialize, Serialize};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level server configuration loaded from a TOML file at startup.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub network: NetworkSection,
    #[serde(default)]
    pub data: DataSection,
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

/// Where the vanilla data sidecar lives on disk. Defaults to
/// `./data/vanilla` relative to the working directory the server is
/// launched from, which matches the layout `tools/extract-vanilla-data.sh`
/// produces.
///
/// `world_dir` is the on-disk world save the server reads chunks from
/// at runtime. `None` (or the default) means "no world wired up yet";
/// the server will start and log a warning, and chunk queries will
/// resolve to `None` everywhere. Plumbing the world into the network
/// layer happens in M3.
#[derive(Debug, Serialize, Deserialize)]
pub struct DataSection {
    #[serde(default = "default_vanilla_dir")]
    pub vanilla_dir: PathBuf,
    #[serde(default)]
    pub world_dir: Option<PathBuf>,
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            vanilla_dir: default_vanilla_dir(),
            world_dir: None,
        }
    }
}

fn default_max_players() -> u32 {
    20
}

fn default_vanilla_dir() -> PathBuf {
    PathBuf::from("data/vanilla")
}

impl ServerConfig {
    /// Convert a parsed TOML config into the network-layer
    /// [`mc_net::ServerConfig`], using the pre-loaded vanilla data,
    /// block registry, and (optionally) a shared world handle.
    pub fn to_network(
        &self,
        data: Arc<VanillaData>,
        blocks: Arc<BlockRegistry>,
        world: Option<WorldHandle>,
        tags: Arc<TagsData>,
        block_light: Option<Arc<mc_data::block_light::BlockLightTable>>,
    ) -> Result<mc_net::ServerConfig, std::net::AddrParseError> {
        let ip: IpAddr = self.network.bind_address.parse()?;
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
            data,
            blocks,
            world,
            tags,
            block_light,
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
        assert_eq!(cfg.data.vanilla_dir, PathBuf::from("data/vanilla"));
    }

    fn stub_blocks() -> Arc<BlockRegistry> {
        Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"))
    }

    fn stub_tags() -> Arc<TagsData> {
        Arc::new(TagsData::default())
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

            [data]
            vanilla_dir = "/tmp/vanilla"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let data = Arc::new(mc_data::testing::stub());
        let net = cfg
            .to_network(data, stub_blocks(), None, stub_tags(), None)
            .unwrap();
        assert_eq!(net.motd, "Howdy");
        assert_eq!(net.max_players, 50);
        assert_eq!(net.bind_address.port(), 25000);
        assert!(net.world.is_none());
        assert_eq!(cfg.data.vanilla_dir, PathBuf::from("/tmp/vanilla"));
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
        let data = Arc::new(mc_data::testing::stub());
        assert!(
            cfg.to_network(data, stub_blocks(), None, stub_tags(), None)
                .is_err()
        );
    }
}
