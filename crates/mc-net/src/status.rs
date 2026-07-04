//! Status state handler — the vanilla "server list ping" choreography.

use bytes::BytesMut;
use mc_protocol::frame::Compression;
use mc_protocol::packets::status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
use mc_protocol::{PROTOCOL_VERSION, State, TARGET_RELEASE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_packet_with_timeout, write_packet};
use crate::control_plane::{autoscale_action_label, autoscale_pressure_label};
use crate::error::ConnectionError;
use crate::server::ServerConfig;
use crate::{RuntimeControlHandle, RuntimeControlLimits};

/// Build the JSON payload the vanilla client renders in its server list.
///
/// See <https://minecraft.wiki/w/Java_Edition_protocol/Status>.
pub(crate) fn build_status_json(
    config: &ServerConfig,
    runtime_control: Option<&RuntimeControlHandle>,
) -> String {
    serde_json::json!({
        "version": {
            "name": TARGET_RELEASE,
            "protocol": PROTOCOL_VERSION,
        },
        "players": {
            "max": config.max_players,
            "online": 0,
            "sample": [],
        },
        "description": { "text": config.motd },
        "enforcesSecureChat": false,
        "solaris": {
            "health": status_health_json(config, runtime_control),
        },
    })
    .to_string()
}

fn status_health_json(
    config: &ServerConfig,
    runtime_control: Option<&RuntimeControlHandle>,
) -> serde_json::Value {
    let shutdown_requested = config.shutdown.is_requested();
    let runtime_snapshot = runtime_control.map(RuntimeControlHandle::snapshot);
    let runtime_draining = runtime_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.draining);
    let ready = !shutdown_requested && !runtime_draining;
    let state = if shutdown_requested {
        "shutting_down"
    } else if runtime_draining {
        "draining"
    } else {
        "ready"
    };
    let runtime_control_json = match runtime_snapshot {
        Some(snapshot) => serde_json::json!({
            "enabled": true,
            "draining": snapshot.draining,
            "action": autoscale_action_label(snapshot.last_decision.action),
            "pressure": autoscale_pressure_label(snapshot.last_decision.pressure),
            "limits": runtime_limits_json(snapshot.limits),
            "pressure_ticks": snapshot.pressure_ticks,
            "healthy_ticks": snapshot.healthy_ticks,
            "reason": snapshot.last_decision.reason,
        }),
        None => serde_json::json!({
            "enabled": false,
            "draining": false,
            "action": "disabled",
            "pressure": "none",
            "limits": null,
            "pressure_ticks": 0,
            "healthy_ticks": 0,
            "reason": "runtime control disabled",
        }),
    };

    serde_json::json!({
        "ready": ready,
        "state": state,
        "shutdown_requested": shutdown_requested,
        "runtime_control": runtime_control_json,
    })
}

fn runtime_limits_json(limits: RuntimeControlLimits) -> serde_json::Value {
    serde_json::json!({
        "view_distance": limits.view_distance,
        "chunk_send_rate": limits.chunk_send_rate,
        "chunk_load_rate": limits.chunk_load_rate,
        "chunk_generate_rate": limits.chunk_generate_rate,
    })
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    config: &ServerConfig,
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
        json: build_status_json(config, runtime_control),
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

    #[test]
    fn status_json_contains_protocol_and_motd() {
        let cfg = test_config();
        let json = build_status_json(&cfg, None);
        // Parse it back to make sure the value is a well-formed object.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"]["protocol"], PROTOCOL_VERSION);
        assert_eq!(value["version"]["name"], TARGET_RELEASE);
        assert_eq!(value["description"]["text"], "Solaris test");
        assert_eq!(value["players"]["max"], 20);
    }

    #[test]
    fn status_json_reports_ready_health_without_runtime_control() {
        let cfg = test_config();
        let json = build_status_json(&cfg, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], true);
        assert_eq!(value["solaris"]["health"]["state"], "ready");
        assert_eq!(value["solaris"]["health"]["shutdown_requested"], false);
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["enabled"],
            false
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["action"],
            "disabled"
        );
        assert!(value["solaris"]["health"]["runtime_control"]["limits"].is_null());
    }

    #[test]
    fn status_json_reports_shutdown_not_ready() {
        let cfg = test_config();
        cfg.shutdown.request();
        let json = build_status_json(&cfg, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "shutting_down");
        assert_eq!(value["solaris"]["health"]["shutdown_requested"], true);
    }

    #[test]
    fn status_json_reports_runtime_drain_not_ready() {
        let cfg = test_config();
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

        let json = build_status_json(&cfg, Some(&runtime_control));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["solaris"]["health"]["ready"], false);
        assert_eq!(value["solaris"]["health"]["state"], "draining");
        assert_eq!(value["solaris"]["health"]["shutdown_requested"], false);
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["enabled"],
            true
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["draining"],
            true
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["action"],
            "scale_down"
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["pressure"],
            "none"
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["limits"]["view_distance"],
            6
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["limits"]["chunk_send_rate"],
            8
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["limits"]["chunk_load_rate"],
            16
        );
        assert_eq!(
            value["solaris"]["health"]["runtime_control"]["limits"]["chunk_generate_rate"],
            8
        );
    }
}
