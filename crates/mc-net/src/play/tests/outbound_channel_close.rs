use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use mc_extension::PlayerId;
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{ClientboundRespawn, GameMode};
use tokio::sync::mpsc;

use crate::chunk_pipeline::ChunkPipelineResources;

use super::super::commands::CommandPermissions;
use super::super::persistence::XpState;
use super::super::survival::SurvivalState;
use super::super::{DEFAULT_SEA_LEVEL, PlayerPose, SessionRegistry, play_loop, simulation_channel};
use super::play_loop_slow_client_test_config;

#[tokio::test]
async fn play_loop_exits_when_outbound_channel_closes() {
    let (_client, mut reader) = tokio::io::duplex(64);
    let mut writer = tokio::io::sink();
    let mut buf = BytesMut::new();
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _simulation_owner) = simulation_channel();
    let config = play_loop_slow_client_test_config();
    let (outbound_tx, outbound_rx) = mpsc::channel(1);
    drop(outbound_tx);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let respawn = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: Identifier::parse("minecraft:overworld").unwrap(),
        hashed_seed: 0,
        game_mode: GameMode::Survival.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        data_to_keep: 0,
    };

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        play_loop(
            &mut reader,
            &mut writer,
            &mut buf,
            Compression::Disabled,
            None,
            None,
            None,
            ChunkPipelineResources::with_limits(1, 1),
            sessions,
            simulation.for_session(1),
            &config,
            1,
            false,
            pose,
            pose,
            respawn,
            CommandPermissions::from_op(false),
            SurvivalState::FULL,
            XpState::default(),
            GameMode::Survival,
            outbound_rx,
            0,
            "ClosedOutbound".to_string(),
            "ClosedOutbound".to_string(),
            None,
            PlayerId::new(0),
            None,
            None,
        ),
    )
    .await
    .expect("closed outbound channel must wake and terminate play loop");

    result.expect("closed outbound channel should close session cleanly");
}
