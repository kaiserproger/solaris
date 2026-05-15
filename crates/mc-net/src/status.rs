//! Status state handler — the vanilla "server list ping" choreography.

use bytes::BytesMut;
use mc_protocol::frame::Compression;
use mc_protocol::packets::status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
use mc_protocol::{PROTOCOL_VERSION, State, TARGET_RELEASE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::connection::{read_packet, write_packet};
use crate::error::ConnectionError;
use crate::server::ServerConfig;

/// Build the JSON payload the vanilla client renders in its server list.
///
/// See <https://minecraft.wiki/w/Java_Edition_protocol/Status>.
pub(crate) fn build_status_json(config: &ServerConfig) -> String {
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
    })
    .to_string()
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    config: &ServerConfig,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Vanilla 1.7+ pattern: Status Request → Status Response → Ping → Pong.
    // It's tempting to skip the StatusRequest read and just wait for the
    // ping, but mcstatus-style clients always send the empty status
    // request first.
    let _ =
        read_packet::<StatusRequest, _>(reader, buf, Compression::Disabled, State::Status).await?;

    let response = StatusResponse {
        json: build_status_json(config),
    };
    write_packet(writer, &response, Compression::Disabled).await?;

    let ping =
        read_packet::<PingRequest, _>(reader, buf, Compression::Disabled, State::Status).await?;
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

    #[test]
    fn status_json_contains_protocol_and_motd() {
        let cfg = ServerConfig {
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
            block_light: None,
            items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
            entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
            biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: crate::ChunkPipelinePolicy::default(),
        };
        let json = build_status_json(&cfg);
        // Parse it back to make sure the value is a well-formed object.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"]["protocol"], PROTOCOL_VERSION);
        assert_eq!(value["version"]["name"], TARGET_RELEASE);
        assert_eq!(value["description"]["text"], "Solaris test");
        assert_eq!(value["players"]["max"], 20);
    }
}
