//! Status state handler — the vanilla "server list ping" choreography.

use bytes::BytesMut;
use mc_protocol::frame::Compression;
use mc_protocol::packets::status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
use mc_protocol::{PROTOCOL_VERSION, State, TARGET_RELEASE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::RuntimeControlHandle;
use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_packet_with_timeout, write_packet};
use crate::error::ConnectionError;
use crate::play::SessionRegistry;
use crate::server::ServerConfig;

/// Build the JSON payload the vanilla client renders in its server list.
///
/// See <https://minecraft.wiki/w/Java_Edition_protocol/Status>.
pub(crate) fn build_status_json(
    config: &ServerConfig,
    online_players: usize,
    runtime_control: Option<&RuntimeControlHandle>,
) -> String {
    serde_json::json!({
        "version": {
            "name": TARGET_RELEASE,
            "protocol": PROTOCOL_VERSION,
        },
        "players": {
            "max": config.max_players,
            "online": online_players,
            "sample": [],
        },
        "description": { "text": config.motd },
        "enforcesSecureChat": false,
        "solaris": {
            "health": status_health_json(config, online_players, runtime_control),
        },
    })
    .to_string()
}

fn status_health_json(
    config: &ServerConfig,
    online_players: usize,
    runtime_control: Option<&RuntimeControlHandle>,
) -> serde_json::Value {
    let shutdown_requested = config.shutdown.is_requested();
    let runtime_draining = runtime_control.is_some_and(|control| control.snapshot().draining);
    let world_available = config.world.is_some();
    let capacity_available = online_players < config.max_players as usize;
    let state = if shutdown_requested {
        "shutting_down"
    } else if runtime_draining {
        "draining"
    } else if !world_available {
        "world_unavailable"
    } else if !capacity_available {
        "full"
    } else {
        "ready"
    };

    serde_json::json!({
        "ready": state == "ready",
        "state": state,
    })
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    runtime_control: Option<&RuntimeControlHandle>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Vanilla 1.7+ pattern: Status Request → Status Response → Ping → Pong.
    // It's tempting to skip the StatusRequest read and just wait for the
    // ping, but mcstatus-style clients always send the empty status
    // request first.
    let _ = read_packet_with_timeout::<StatusRequest, _>(
        reader,
        buf,
        Compression::Disabled,
        State::Status,
        PRE_PLAY_READ_TIMEOUT,
    )
    .await?;

    let response = StatusResponse {
        json: build_status_json(
            config,
            sessions.published_active_session_count(),
            runtime_control,
        ),
    };
    write_packet(writer, &response, Compression::Disabled).await?;

    let ping = read_packet_with_timeout::<PingRequest, _>(
        reader,
        buf,
        Compression::Disabled,
        State::Status,
        PRE_PLAY_READ_TIMEOUT,
    )
    .await?;
    write_packet(
        writer,
        &PongResponse {
            payload: ping.payload,
        },
        Compression::Disabled,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn test_config() -> ServerConfig {
        ServerConfig {
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            motd: "Solaris test".into(),
            max_players: 20,
            view_distance: crate::DEFAULT_VIEW_DISTANCE,
            data: std::sync::Arc::new(mc_data::testing::stub()),
            blocks: std::sync::Arc::new(
                mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
            ),
            world: None,
            tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
            recipes: std::sync::Arc::new(Vec::new()),
            loot: std::sync::Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
            biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: crate::ChunkPipelinePolicy::default(),
            random_tick: crate::RandomTickPolicy::default(),
            command_permissions: crate::CommandPermissionConfig::new(Vec::<String>::new(), true),
            shutdown: crate::ShutdownHandle::default(),
        }
    }

    fn playable_test_config() -> ServerConfig {
        let mut config = test_config();
        config.world = Some(std::sync::Arc::new(tokio::sync::Mutex::new(
            mc_world::WorldStorage::in_memory(std::sync::Arc::clone(&config.blocks)),
        )));
        config
    }

    #[test]
    fn status_json_contains_protocol_and_motd() {
        let cfg = test_config();
        let json = build_status_json(&cfg, 7, None);
        // Parse it back to make sure the value is a well-formed object.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"]["protocol"], PROTOCOL_VERSION);
        assert_eq!(value["version"]["name"], TARGET_RELEASE);
        assert_eq!(value["description"]["text"], "Solaris test");
        assert_eq!(value["players"]["max"], 20);
        assert_eq!(value["players"]["online"], 7);
    }

    #[test]
    fn status_json_reports_ready_health_without_runtime_control() {
        let cfg = playable_test_config();
        let json = build_status_json(&cfg, 0, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], true);
        assert_eq!(value["solaris"]["health"]["state"], "ready");
        assert!(value["solaris"]["health"].get("runtime_control").is_none());
        assert!(
            value["solaris"]["health"]
                .get("shutdown_requested")
                .is_none()
        );
    }

    #[test]
    fn status_json_reports_worldless_server_not_ready() {
        let cfg = test_config();
        let json = build_status_json(&cfg, 0, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "world_unavailable");
    }

    #[test]
    fn status_json_reports_online_auth_ready() {
        let mut cfg = playable_test_config();
        cfg.command_permissions = crate::CommandPermissionConfig::new(Vec::<String>::new(), false)
            .with_login_access(crate::LoginAccessConfig::normalized(
                true,
                false,
                Vec::<String>::new(),
                Vec::<String>::new(),
            ));
        let json = build_status_json(&cfg, 0, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], true);
        assert_eq!(value["solaris"]["health"]["state"], "ready");
    }

    #[test]
    fn status_json_reports_full_server_not_ready() {
        let mut cfg = playable_test_config();
        cfg.max_players = 1;
        let json = build_status_json(&cfg, 1, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "full");
    }

    #[test]
    fn status_json_reports_shutdown_not_ready() {
        let cfg = playable_test_config();
        cfg.shutdown.request();
        let json = build_status_json(&cfg, 0, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "shutting_down");
    }

    #[test]
    fn status_json_reports_runtime_drain_not_ready() {
        let cfg = playable_test_config();
        let runtime_control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy::default(),
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        });
        runtime_control.request_drain();

        let json = build_status_json(&cfg, 0, Some(&runtime_control));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "draining");
        assert!(value["solaris"]["health"].get("runtime_control").is_none());
    }
}
