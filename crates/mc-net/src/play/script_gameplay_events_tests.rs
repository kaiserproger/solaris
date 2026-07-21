use std::num::NonZeroUsize;

use mc_protocol::packets::play::GameMode;
use mc_script::{ScriptEvent, ScriptEventKind, ScriptPlayerId, script_boundary_pair};
use mc_world::{BlockPos, BlockRegistry};

use super::PlayerPose;
use super::commands::CommandPermissions;
use super::script_gameplay_events::ScriptGameplayEventPublisher;
use crate::server::ScriptEventSink;

fn publisher(sink: ScriptEventSink) -> ScriptGameplayEventPublisher {
    ScriptGameplayEventPublisher::new(
        sink,
        ScriptPlayerId::new(9),
        "123e4567-e89b-12d3-a456-426614174000",
        "Builder",
        CommandPermissions::from_op(true),
        "minecraft:overworld",
    )
}

fn blocks() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}

#[tokio::test]
async fn required_block_break_waits_for_exact_queue_capacity_then_publishes_snapshot() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let publisher = publisher(ScriptEventSink::new(boundary));
    let blocks = blocks();
    let grass = blocks
        .block(&mc_data::Identifier::parse("minecraft:grass_block").unwrap())
        .unwrap()
        .default;

    let delivery = tokio::spawn(async move {
        publisher
            .publish_block_broken(
                &blocks,
                grass,
                BlockPos { x: 3, y: 64, z: -2 },
                PlayerPose::new(3.5, 65.0, -1.5),
                GameMode::Survival,
            )
            .await
    });
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
    assert!(delivery.await.unwrap());
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerBlockBroken {
            player_id,
            dimension,
            block_id,
            x: 3,
            y: 64,
            z: -2,
            game_mode: mc_script::ScriptGameMode::Survival,
            ..
        } if *player_id == ScriptPlayerId::new(9)
            && dimension == "minecraft:overworld"
            && block_id == "minecraft:grass_block"
    ));
}

#[tokio::test]
async fn closed_script_queue_does_not_claim_committed_break_delivery() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    boundary.close_event_admission();
    let blocks = blocks();
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;

    assert!(
        !publisher
            .publish_block_broken(
                &blocks,
                stone,
                BlockPos { x: 0, y: 12, z: 0 },
                PlayerPose::new(0.5, 13.0, 0.5),
                GameMode::Creative,
            )
            .await
    );
}

#[tokio::test]
async fn committed_block_placement_publishes_root_snapshot() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary));
    let blocks = blocks();
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;

    assert!(
        publisher
            .publish_block_placed(
                &blocks,
                stone,
                BlockPos { x: -4, y: 71, z: 8 },
                PlayerPose::new(-3.5, 72.0, 8.5),
                GameMode::Creative,
            )
            .await
    );
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerBlockPlaced {
            player_id,
            dimension,
            block_id,
            x: -4,
            y: 71,
            z: 8,
            game_mode: mc_script::ScriptGameMode::Creative,
            ..
        } if *player_id == ScriptPlayerId::new(9)
            && dimension == "minecraft:overworld"
            && block_id == "minecraft:stone"
    ));
}
