use std::num::NonZeroUsize;

use mc_data::items::{ItemRegistry, ItemReport};
use mc_entity::EntityId;
use mc_protocol::packets::play::GameMode;
use mc_script::{
    ScriptCraftingSource, ScriptEvent, ScriptEventKind, ScriptInteractionHand,
    ScriptItemPickupSource, ScriptPlayerId, script_boundary_pair,
};
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

fn items() -> ItemRegistry {
    ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            protocol_id: 0,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:oak_planks").unwrap(),
            protocol_id: 5,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:arrow").unwrap(),
            protocol_id: 6,
        },
    ])
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

#[tokio::test]
async fn committed_craft_waits_for_capacity_and_publishes_exact_snapshot() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let publisher = publisher(ScriptEventSink::new(boundary));
    let items = items();

    let delivery = tokio::spawn(async move {
        publisher
            .publish_item_crafted(
                &items,
                5,
                12,
                3,
                ScriptCraftingSource::Inventory,
                PlayerPose::new(2.5, 66.0, -7.5),
                GameMode::Adventure,
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
        ScriptEventKind::PlayerItemCrafted {
            player_id,
            context,
            dimension,
            item_id,
            count: 12,
            craft_count: 3,
            source: ScriptCraftingSource::Inventory,
            game_mode: mc_script::ScriptGameMode::Adventure,
        } if *player_id == ScriptPlayerId::new(9)
            && (context.x(), context.y(), context.z()) == (2.5, 66.0, -7.5)
            && dimension == "minecraft:overworld"
            && item_id == "minecraft:oak_planks"
    ));
}

#[tokio::test]
async fn committed_creative_craft_publishes_creative_mode() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary));

    assert!(
        publisher
            .publish_item_crafted(
                &items(),
                5,
                4,
                1,
                ScriptCraftingSource::CraftingTable,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Creative,
            )
            .await
    );
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerItemCrafted {
            game_mode: mc_script::ScriptGameMode::Creative,
            ..
        }
    ));
}

#[tokio::test]
async fn closed_script_queue_does_not_claim_committed_craft_delivery() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    boundary.close_event_admission();

    assert!(
        !publisher
            .publish_item_crafted(
                &items(),
                5,
                4,
                1,
                ScriptCraftingSource::CraftingTable,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Survival,
            )
            .await
    );
}

#[tokio::test]
async fn invalid_or_spectator_craft_publishes_nothing_before_fifo_fence() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    let items = items();

    assert!(
        !publisher
            .publish_item_crafted(
                &items,
                99,
                1,
                1,
                ScriptCraftingSource::CraftingTable,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Survival,
            )
            .await
    );
    assert!(
        !publisher
            .publish_item_crafted(
                &items,
                5,
                1,
                1,
                ScriptCraftingSource::CraftingTable,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Spectator,
            )
            .await
    );
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(77))
        .unwrap();
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 77 }
    ));
}

#[tokio::test]
async fn committed_partial_item_pickup_waits_for_capacity_and_publishes_exact_credit() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let publisher = publisher(ScriptEventSink::new(boundary));
    let items = items();

    let delivery = tokio::spawn(async move {
        publisher
            .publish_item_picked_up(
                &items,
                5,
                2,
                ScriptItemPickupSource::ItemEntity,
                PlayerPose::new(7.5, 64.0, -3.5),
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
        ScriptEventKind::PlayerItemPickedUp {
            player_id,
            context,
            dimension,
            item_id,
            count: 2,
            source: ScriptItemPickupSource::ItemEntity,
            game_mode: mc_script::ScriptGameMode::Survival,
        } if *player_id == ScriptPlayerId::new(9)
            && (context.x(), context.y(), context.z()) == (7.5, 64.0, -3.5)
            && dimension == "minecraft:overworld"
            && item_id == "minecraft:oak_planks"
    ));
}

#[tokio::test]
async fn committed_arrow_pickup_publishes_arrow_source() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary));

    assert!(
        publisher
            .publish_item_picked_up(
                &items(),
                6,
                1,
                ScriptItemPickupSource::Arrow,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Adventure,
            )
            .await
    );
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerItemPickedUp {
            count: 1,
            source: ScriptItemPickupSource::Arrow,
            game_mode: mc_script::ScriptGameMode::Adventure,
            ..
        }
    ));
}

#[tokio::test]
async fn closed_script_queue_does_not_claim_committed_pickup_delivery() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    boundary.close_event_admission();

    assert!(
        !publisher
            .publish_item_picked_up(
                &items(),
                5,
                1,
                ScriptItemPickupSource::ItemEntity,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Survival,
            )
            .await
    );
}

#[tokio::test]
async fn invalid_or_spectator_pickup_publishes_nothing_before_fifo_fence() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    let items = items();

    assert!(
        !publisher
            .publish_item_picked_up(
                &items,
                99,
                1,
                ScriptItemPickupSource::ItemEntity,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Survival,
            )
            .await
    );
    assert!(
        !publisher
            .publish_item_picked_up(
                &items,
                5,
                1,
                ScriptItemPickupSource::ItemEntity,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Spectator,
            )
            .await
    );
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(78))
        .unwrap();
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 78 }
    ));
}

#[tokio::test]
async fn accepted_entity_interaction_publishes_exact_required_snapshot() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary));

    assert!(
        publisher
            .publish_entity_interacted(
                EntityId(77),
                "minecraft:villager",
                ScriptInteractionHand::OffHand,
                true,
                PlayerPose::new(1.5, 64.0, -2.5),
                GameMode::Adventure,
            )
            .await
    );
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerEntityInteracted {
            player_id,
            context,
            dimension,
            entity_id,
            entity_type,
            hand: ScriptInteractionHand::OffHand,
            secondary_action: true,
            game_mode: mc_script::ScriptGameMode::Adventure,
        } if *player_id == ScriptPlayerId::new(9)
            && (context.x(), context.y(), context.z()) == (1.5, 64.0, -2.5)
            && dimension == "minecraft:overworld"
            && entity_id.value() == 77
            && entity_type == "minecraft:villager"
    ));
}

#[tokio::test]
async fn closed_script_queue_rejects_entity_interaction_event_without_error() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(one, one);
    let publisher = publisher(ScriptEventSink::new(boundary.clone()));
    boundary.close_event_admission();

    assert!(
        !publisher
            .publish_entity_interacted(
                EntityId(77),
                "minecraft:villager",
                ScriptInteractionHand::MainHand,
                false,
                PlayerPose::new(0.5, 64.0, 0.5),
                GameMode::Survival,
            )
            .await
    );
}
